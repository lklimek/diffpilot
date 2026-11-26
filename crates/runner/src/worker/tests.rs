//! The job loop against a wiremock server with a scripted provider. Time runs at 40 ms per `LeaseTiming`
//! second: lease 2400 ms, heartbeat 600 ms, fence 1400 ms, grace 400 ms, cleanup 200 ms.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use contracts::agent::JobKind;
use contracts::job::event::ActivityKind;
use contracts::job::protocol::ClaimResponse;
use contracts::job::result::Usage;
use contracts::job::spec::JobSpec;
use contracts::time::Timestamp;
use futures::future::BoxFuture;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Notify;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::*;
use crate::execute::Executor;
use crate::provider::{LlmProvider, ProviderCall, ProviderError, ProviderOutcome};
use crate::test_support::{dirs, shared_repo, spec};

const UNIT: Duration = Duration::from_millis(40);
const RUNNER_SECRET: &str = "runner-secret-WORKER-CANARY";
const JOB_TOKEN: &str = "job-token-WORKER-CANARY";

fn timing() -> Timing {
    Timing::scaled(LeaseTiming::V1, UNIT)
}

/// Emits `events` activities `step` apart, then the output; honours cancel unless told otherwise.
struct Scripted {
    retire_fails: bool,
    retire_calls: AtomicUsize,
    events: usize,
    step: Duration,
    summary: String,
    output: Value,
    honour_cancel: bool,
    cancel_called: AtomicBool,
    cancelled: Notify,
    finished: AtomicBool,
    /// Events the job loop accepted.
    sent: AtomicUsize,
    /// `sent` when the first cancel arrived (the start of the stop; later events are discarded).
    sent_at_cancel: Mutex<Option<usize>>,
    /// Simulated worker cleanup completes this long after cancellation.
    ends_after_cancel: Option<Duration>,
    cancel_at: Mutex<Option<Instant>>,
}

impl Scripted {
    fn new(events: usize, step: Duration) -> Self {
        Self {
            retire_fails: false,
            retire_calls: AtomicUsize::new(0),
            events,
            step,
            summary: "$ ls (exit 0)".to_owned(),
            output: json!({"answer": 42}),
            honour_cancel: true,
            cancel_called: AtomicBool::new(false),
            cancelled: Notify::new(),
            finished: AtomicBool::new(false),
            sent: AtomicUsize::new(0),
            sent_at_cancel: Mutex::new(None),
            ends_after_cancel: None,
            cancel_at: Mutex::new(None),
        }
    }
}

struct SetOnDrop<'a>(&'a AtomicBool);

impl Drop for SetOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl LlmProvider for Scripted {
    fn retire<'a>(&'a self, _prs: &'a [i64]) -> BoxFuture<'a, Result<(), ProviderError>> {
        self.retire_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if self.retire_fails {
                Err(ProviderError::Failed {
                    message: "fake retirement failure".into(),
                })
            } else {
                Ok(())
            }
        })
    }

    fn run<'a>(
        &'a self,
        _call: &'a ProviderCall,
        events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        Box::pin(async move {
            let _finished = SetOnDrop(&self.finished);
            for _ in 0..self.events {
                if self.step.is_zero() {
                    // A flood: no timer between events.
                } else if self.honour_cancel {
                    tokio::select! {
                        () = sleep(self.step) => {}
                        () = self.cancelled.notified() => return Err(ProviderError::Cancelled),
                    }
                } else {
                    sleep(self.step).await;
                }
                let event = JobEvent {
                    seq: 0,
                    at: Timestamp::now(),
                    activity: ActivityKind::Command,
                    summary: self.summary.clone(),
                };
                if events.send(ProviderEvent::Activity(event)).await.is_ok() {
                    self.sent.fetch_add(1, Ordering::SeqCst);
                } else {
                    // Nobody reads any more; let the runtime breathe.
                    tokio::task::yield_now().await;
                }
                let cancelled_at = *self.cancel_at.lock().unwrap();
                if let (Some(linger), Some(at)) = (self.ends_after_cancel, cancelled_at)
                    && at.elapsed() >= linger
                {
                    return Err(ProviderError::Cancelled);
                }
            }
            let _ = events
                .send(ProviderEvent::OutputReady(self.output.clone()))
                .await;
            Ok(ProviderOutcome {
                runtime_session: None,
                session_turns: None,
                output: self.output.clone(),
                session_id: Some("thread-1".to_owned()),
                usage: Usage::default(),
                skills_commit: None,
            })
        })
    }

    fn resume<'a>(
        &'a self,
        _session_id: &'a str,
        _call: &'a ProviderCall,
        _events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        Box::pin(async { Err(ProviderError::SessionUnavailable) })
    }

    fn cancel(&self, _job: JobId) -> BoxFuture<'_, Result<(), ProviderError>> {
        self.cancel_called.store(true, Ordering::SeqCst);
        self.cancel_at
            .lock()
            .unwrap()
            .get_or_insert_with(Instant::now);
        self.sent_at_cancel
            .lock()
            .unwrap()
            .get_or_insert_with(|| self.sent.load(Ordering::SeqCst));
        if self.honour_cancel {
            self.cancelled.notify_one();
        }
        Box::pin(async { Ok(()) })
    }
}

struct Setup {
    server: MockServer,
    worker: Worker,
    provider: Arc<Scripted>,
    spec: JobSpec,
    _root: TempDir,
}

fn ack(cancel_requested: bool) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(EventsAck {
        cancel_requested,
        lease_expires_at: Timestamp::now(),
    })
}

async fn setup(provider: Scripted, claim_delay: Duration) -> Setup {
    setup_with(provider, claim_delay, false).await
}

async fn setup_with(provider: Scripted, claim_delay: Duration, _detached: bool) -> Setup {
    setup_retire(provider, claim_delay, Vec::new()).await
}

async fn setup_retire(provider: Scripted, claim_delay: Duration, retire: Vec<i64>) -> Setup {
    let server = MockServer::start().await;
    let root = TempDir::new().unwrap();
    let spec = spec(JobKind::Plan, shared_repo());
    let claim = ClaimResponse {
        retire,
        claim: Some(Claim {
            job_token: Secret::new(JOB_TOKEN),
            lease_expires_at: Timestamp::now(),
            spec: spec.clone(),
        }),
    };
    Mock::given(method("POST"))
        .and(path("/runner/v1/claim"))
        .and(header(
            "authorization",
            format!("Bearer {RUNNER_SECRET}").as_str(),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(&claim)
                .set_delay(claim_delay),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let provider = Arc::new(provider);
    let executor = Executor::new(provider.clone(), dirs(root.path()));
    let executor = Arc::new(executor);
    let client = ServerClient::new(
        &server.uri(),
        Secret::new(RUNNER_SECRET),
        Duration::from_secs(2),
    )
    .unwrap();
    let worker = Worker::new(
        client,
        executor,
        "runner-test".to_owned(),
        Secret::new(RUNNER_SECRET),
        timing(),
    );
    Setup {
        server,
        worker,
        provider,
        spec,
        _root: root,
    }
}

fn job_path(spec: &JobSpec, kind: &str) -> String {
    format!("/runner/v1/jobs/{}/{kind}", spec.job_id)
}

async fn mount(setup: &Setup, kind: &str, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(job_path(&setup.spec, kind)))
        .and(header(
            "authorization",
            format!("Bearer {JOB_TOKEN}").as_str(),
        ))
        .respond_with(response)
        .mount(&setup.server)
        .await;
}

async fn requests(setup: &Setup, kind: &str) -> Vec<Request> {
    let wanted = job_path(&setup.spec, kind);
    setup
        .server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.url.path() == wanted)
        .collect()
}

fn body<T: serde::de::DeserializeOwned>(request: &Request) -> T {
    serde_json::from_slice(&request.body).unwrap()
}

#[tokio::test]
async fn claim_events_output_and_result_in_order() {
    let setup = setup(Scripted::new(3, UNIT), Duration::ZERO).await;
    mount(&setup, "events", ack(false)).await;
    mount(&setup, "output", ack(false)).await;
    mount(&setup, "result", ResponseTemplate::new(204)).await;
    let mut setup = setup;
    let end = setup.worker.poll_once().await.unwrap();
    assert_eq!(
        end,
        Some(JobEnd::ResultPosted(JobOutcome::Succeeded {
            output: json!({"answer": 42})
        }))
    );
    let claim: ClaimRequest = body(&setup.server.received_requests().await.unwrap()[0]);
    assert_eq!(claim.runner_id, "runner-test");
    assert!(claim.retired.is_empty());

    let all = setup.server.received_requests().await.unwrap();
    let position = |kind: &str| {
        let wanted = job_path(&setup.spec, kind);
        all.iter().position(|r| r.url.path() == wanted).unwrap()
    };
    let last_events = all
        .iter()
        .rposition(|r| {
            r.url.path() == job_path(&setup.spec, "events")
                && !body::<EventBatch>(r).events.is_empty()
        })
        .unwrap();
    assert!(
        last_events < position("output"),
        "every event before the output"
    );
    assert!(
        position("output") < position("result"),
        "output before the result"
    );
    let seqs: Vec<u32> = requests(&setup, "events")
        .await
        .iter()
        .flat_map(|r| body::<EventBatch>(r).events)
        .map(|event| event.seq)
        .collect();
    assert_eq!(seqs, [1, 2, 3], "each event posted once, in order");
    let outputs = requests(&setup, "output").await;
    assert_eq!(outputs.len(), 1);
    let output: OutputReady = body(&outputs[0]);
    assert_eq!(output.attempt, 1);
    assert_eq!(output.output, json!({"answer": 42}));
    let result: JobResult = body(&requests(&setup, "result").await[0]);
    assert_eq!(result.job_id, setup.spec.job_id);
    assert!(setup.provider.finished.load(Ordering::SeqCst));
}

#[tokio::test]
async fn result_is_retried_until_204_with_the_same_body() {
    let mut setup = setup(Scripted::new(1, UNIT), Duration::ZERO).await;
    mount(&setup, "events", ack(false)).await;
    mount(&setup, "output", ack(false)).await;
    Mock::given(method("POST"))
        .and(path(job_path(&setup.spec, "result")))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&setup.server)
        .await;
    mount(&setup, "result", ResponseTemplate::new(204)).await;
    let end = setup.worker.poll_once().await.unwrap();
    assert!(matches!(end, Some(JobEnd::ResultPosted(_))), "{end:?}");
    let results = requests(&setup, "result").await;
    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|r| r.body == results[0].body));
}

#[tokio::test]
async fn result_rejected_with_4xx_is_dropped() {
    let mut setup = setup(Scripted::new(1, UNIT), Duration::ZERO).await;
    mount(&setup, "events", ack(false)).await;
    mount(&setup, "output", ack(false)).await;
    mount(&setup, "result", ResponseTemplate::new(409)).await;
    let end = setup.worker.poll_once().await.unwrap();
    assert_eq!(end, Some(JobEnd::ResultRejected(StatusCode::CONFLICT)));
    assert_eq!(requests(&setup, "result").await.len(), 1);
}

#[tokio::test]
async fn output_is_retried_on_5xx_with_the_same_body() {
    let mut setup = setup(Scripted::new(1, UNIT), Duration::ZERO).await;
    mount(&setup, "events", ack(false)).await;
    Mock::given(method("POST"))
        .and(path(job_path(&setup.spec, "output")))
        .respond_with(ResponseTemplate::new(502))
        .up_to_n_times(1)
        .mount(&setup.server)
        .await;
    mount(&setup, "output", ack(false)).await;
    mount(&setup, "result", ResponseTemplate::new(204)).await;
    let end = setup.worker.poll_once().await.unwrap();
    assert!(matches!(end, Some(JobEnd::ResultPosted(_))), "{end:?}");
    let outputs = requests(&setup, "output").await;
    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0].body, outputs[1].body);
}

#[tokio::test]
async fn stale_attempt_409_stops_the_agent_without_a_result() {
    let mut setup = setup(Scripted::new(50, UNIT * 5), Duration::ZERO).await;
    mount(&setup, "events", ResponseTemplate::new(409)).await;
    mount(&setup, "result", ResponseTemplate::new(204)).await;
    let end = setup.worker.poll_once().await.unwrap();
    assert_eq!(end, Some(JobEnd::Stale(StatusCode::CONFLICT)));
    assert!(setup.provider.cancel_called.load(Ordering::SeqCst));
    assert!(
        setup.provider.finished.load(Ordering::SeqCst),
        "agent call ended"
    );
    assert!(setup.provider.finished.load(Ordering::SeqCst));
    assert!(
        requests(&setup, "result").await.is_empty(),
        "no result for a stale attempt"
    );
}

#[tokio::test]
async fn cancel_flag_cancels_the_provider_and_posts_cancelled() {
    let mut setup = setup(Scripted::new(50, UNIT * 5), Duration::ZERO).await;
    mount(&setup, "events", ack(true)).await;
    mount(&setup, "result", ResponseTemplate::new(204)).await;
    let end = setup.worker.poll_once().await.unwrap();
    assert_eq!(end, Some(JobEnd::ResultPosted(JobOutcome::Cancelled)));
    assert!(setup.provider.cancel_called.load(Ordering::SeqCst));
    assert!(setup.provider.finished.load(Ordering::SeqCst));
}

#[tokio::test]
async fn provider_ignoring_cancel_is_killed_after_the_grace() {
    let mut provider = Scripted::new(50, UNIT * 5);
    provider.honour_cancel = false;
    let mut setup = setup(provider, Duration::ZERO).await;
    mount(&setup, "events", ack(true)).await;
    mount(&setup, "result", ResponseTemplate::new(204)).await;
    let started = Instant::now();
    let end = setup.worker.poll_once().await.unwrap();
    assert_eq!(end, Some(JobEnd::ResultPosted(JobOutcome::Cancelled)));
    assert!(
        started.elapsed() < UNIT * 50,
        "killed after the grace, not after the script"
    );
    assert!(
        setup.provider.finished.load(Ordering::SeqCst),
        "agent call dropped"
    );
    assert!(setup.provider.finished.load(Ordering::SeqCst));
}

/// A detached attempt keeps producing events after the cancel grace (its agent ignores SIGTERM) while the job
/// loop no longer receives them: back-pressure must not keep the attempt, and so the worker, from finishing.
#[tokio::test]
async fn cancel_with_full_event_channels_settles_a_detached_attempt() {
    let mut provider = Scripted::new(usize::MAX, Duration::ZERO);
    provider.honour_cancel = false;
    provider.ends_after_cancel = Some(UNIT * 25);
    let mut setup = setup_with(provider, Duration::ZERO, true).await;
    mount(&setup, "events", ack(true)).await;
    mount(&setup, "output", ResponseTemplate::new(204)).await;
    mount(&setup, "result", ResponseTemplate::new(204)).await;
    let end = tokio::time::timeout(UNIT * 200, setup.worker.poll_once())
        .await
        .expect("the worker settles instead of deadlocking")
        .unwrap();
    assert_eq!(end, Some(JobEnd::ResultPosted(JobOutcome::Cancelled)));
    assert!(
        setup.provider.finished.load(Ordering::SeqCst),
        "the attempt ran to its end"
    );
}

/// A stop signal cancels the attempt; the worker returns after its cleanup settles.
#[tokio::test]
async fn shutdown_cancels_the_running_attempt_and_waits_for_it_to_end() {
    let mut provider = Scripted::new(usize::MAX, UNIT / 4);
    provider.honour_cancel = false;
    provider.ends_after_cancel = Some(UNIT * 5);
    let setup = setup_with(provider, Duration::ZERO, true).await;
    mount(&setup, "events", ack(false)).await;
    let provider = Arc::clone(&setup.provider);
    let running = Arc::clone(&setup.provider);
    let shutdown = async move {
        while running.sent.load(Ordering::SeqCst) == 0 {
            sleep(UNIT / 4).await;
        }
    };
    tokio::time::timeout(UNIT * 200, setup.worker.run(shutdown))
        .await
        .expect("the wait for the attempt is bounded");
    assert!(
        provider.cancel_called.load(Ordering::SeqCst),
        "the running attempt is cancelled"
    );
    assert!(
        provider.finished.load(Ordering::SeqCst),
        "the attempt ended before the worker returned"
    );
}

/// An attempt that does not end is not waited for beyond the bound.
#[tokio::test]
async fn shutdown_does_not_wait_for_an_attempt_that_never_ends() {
    let mut provider = Scripted::new(usize::MAX, UNIT / 4);
    provider.honour_cancel = false;
    let setup = setup_with(provider, Duration::ZERO, true).await;
    mount(&setup, "events", ack(false)).await;
    let running = Arc::clone(&setup.provider);
    let shutdown = async move {
        while running.sent.load(Ordering::SeqCst) == 0 {
            sleep(UNIT / 4).await;
        }
    };
    let started = Instant::now();
    setup.worker.run(shutdown).await;
    let waited = started.elapsed();
    assert!(
        waited >= timing().term_grace + timing().kill_and_cleanup && waited < UNIT * 100,
        "{waited:?}"
    );
}

/// The claim reply is delayed by 600 ms and no later request is ever acked: the deadline runs from the claim's
/// SEND time (1400 ms), so the agent is stopped and cleaned up well before the lease (2400 ms from receipt) and
/// before a deadline wrongly measured from the delayed ack (2000 ms).
#[tokio::test]
async fn self_fence_completes_before_lease() {
    let claim_delay = UNIT * 15;
    let setup = setup(Scripted::new(1000, UNIT * 5), claim_delay).await;
    mount(
        &setup,
        "events",
        ack(false).set_delay(Duration::from_secs(30)),
    )
    .await;
    mount(
        &setup,
        "output",
        ack(false).set_delay(Duration::from_secs(30)),
    )
    .await;
    mount(&setup, "result", ResponseTemplate::new(204)).await;
    let mut setup = setup;
    let t = timing();
    let claim_sent = Instant::now();
    let end = setup.worker.poll_once().await.unwrap();
    let stopped = claim_sent.elapsed();
    assert_eq!(end, Some(JobEnd::Fenced));
    assert!(stopped >= t.fence_after_send, "fenced early: {stopped:?}");
    assert!(
        stopped < t.fence_after_send + t.term_grace,
        "stop must follow the deadline from the send time, not the ack: {stopped:?}"
    );
    assert!(stopped + t.kill_and_cleanup < t.lease, "{stopped:?}");
    assert!(
        stopped < claim_delay + t.fence_after_send,
        "a delayed ack extended the deadline"
    );
    assert!(setup.provider.cancel_called.load(Ordering::SeqCst));
    assert!(setup.provider.finished.load(Ordering::SeqCst), "agent gone");
    assert!(setup.provider.finished.load(Ordering::SeqCst));
    assert!(
        requests(&setup, "result").await.is_empty(),
        "no result after fencing"
    );
}

#[tokio::test]
async fn heartbeats_keep_a_quiet_job_alive_past_the_fence() {
    // One event after 1.5 fence periods; only heartbeats are acked before it.
    let mut setup = setup(Scripted::new(1, UNIT * 52), Duration::ZERO).await;
    mount(&setup, "events", ack(false)).await;
    mount(&setup, "output", ack(false)).await;
    mount(&setup, "result", ResponseTemplate::new(204)).await;
    let end = setup.worker.poll_once().await.unwrap();
    assert!(
        matches!(
            end,
            Some(JobEnd::ResultPosted(JobOutcome::Succeeded { .. }))
        ),
        "{end:?}"
    );
    let heartbeats = requests(&setup, "events")
        .await
        .iter()
        .filter(|r| body::<EventBatch>(r).events.is_empty())
        .count();
    assert!(heartbeats >= 2, "heartbeats sent: {heartbeats}");
}

#[tokio::test]
async fn runner_secrets_never_leave_in_bodies() {
    let mut provider = Scripted::new(1, UNIT);
    provider.summary = format!("$ echo {JOB_TOKEN} {RUNNER_SECRET}");
    provider.output = json!({"leak": format!("{JOB_TOKEN}/{RUNNER_SECRET}")});
    let mut setup = setup(provider, Duration::ZERO).await;
    mount(&setup, "events", ack(false)).await;
    mount(&setup, "output", ack(false)).await;
    mount(&setup, "result", ResponseTemplate::new(204)).await;
    setup.worker.poll_once().await.unwrap();
    for request in setup.server.received_requests().await.unwrap() {
        let text = String::from_utf8_lossy(&request.body);
        assert!(
            !text.contains(JOB_TOKEN),
            "{} leaks the job token",
            request.url.path()
        );
        assert!(
            !text.contains(RUNNER_SECRET),
            "{} leaks the runner secret",
            request.url.path()
        );
    }
    let result: JobResult = body(&requests(&setup, "result").await[0]);
    assert_eq!(
        result.outcome,
        JobOutcome::Succeeded {
            output: json!({"leak": "[redacted]/[redacted]"})
        }
    );
}

#[tokio::test]
async fn retired_are_acknowledged_with_the_next_claim() {
    let server = MockServer::start().await;
    let root = TempDir::new().unwrap();
    let analysis = 1;
    Mock::given(method("POST"))
        .and(path("/runner/v1/claim"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ClaimResponse {
            retire: vec![analysis],
            claim: None,
        }))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/runner/v1/claim"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ClaimResponse {
            retire: Vec::new(),
            claim: None,
        }))
        .mount(&server)
        .await;
    let executor = Arc::new(Executor::new(
        Arc::new(Scripted::new(0, UNIT)),
        dirs(root.path()),
    ));
    let client = ServerClient::new(
        &server.uri(),
        Secret::new(RUNNER_SECRET),
        Duration::from_secs(2),
    )
    .unwrap();
    let mut worker = Worker::new(
        client,
        executor,
        "r".to_owned(),
        Secret::new(RUNNER_SECRET),
        timing(),
    );
    assert_eq!(worker.poll_once().await.unwrap(), None);
    assert_eq!(worker.poll_once().await.unwrap(), None);
    let claims = server.received_requests().await.unwrap();
    assert!(body::<ClaimRequest>(&claims[0]).retired.is_empty());
    assert_eq!(body::<ClaimRequest>(&claims[1]).retired, [analysis]);
}

#[tokio::test]
async fn below_the_reserve_the_runner_claims_cleanup_without_taking_a_job() {
    let mut setup = setup(Scripted::new(0, UNIT), Duration::ZERO).await;
    setup.worker.disk_reserve = Some(crate::scratch::Reserve::new(
        setup._root.path().to_owned(),
        u64::MAX,
    ));
    setup.server.reset().await;
    Mock::given(method("POST"))
        .and(path("/runner/v1/claim"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ClaimResponse {
            retire: vec![1],
            claim: None,
        }))
        .mount(&setup.server)
        .await;
    assert_eq!(setup.worker.poll_once().await.unwrap(), None);
    assert_eq!(setup.worker.poll_once().await.unwrap(), None);
    let claims = setup.server.received_requests().await.unwrap();
    assert_eq!(claims.len(), 2);
    assert_eq!(body::<Value>(&claims[0])["accept_job"], false);
    assert_eq!(body::<Value>(&claims[1])["retired"], json!([1]));
    assert!(!setup.provider.finished.load(Ordering::SeqCst));
}

#[tokio::test]
async fn failed_retirement_still_runs_the_unrelated_claim_without_acknowledging() {
    let mut provider = Scripted::new(0, Duration::ZERO);
    provider.retire_fails = true;
    let mut setup = setup_retire(provider, Duration::ZERO, vec![1]).await;
    mount(&setup, "events", ack(false)).await;
    mount(&setup, "output", ack(false)).await;
    mount(&setup, "result", ResponseTemplate::new(204)).await;
    Mock::given(method("POST"))
        .and(path("/runner/v1/claim"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ClaimResponse {
            retire: vec![1],
            claim: None,
        }))
        .mount(&setup.server)
        .await;
    assert!(matches!(
        setup.worker.poll_once().await.unwrap(),
        Some(JobEnd::ResultPosted(JobOutcome::Succeeded { .. }))
    ));
    assert_eq!(setup.worker.poll_once().await.unwrap(), None);
    let claims: Vec<_> = setup
        .server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/runner/v1/claim")
        .collect();
    assert_eq!(claims.len(), 2);
    assert!(body::<ClaimRequest>(&claims[1]).retired.is_empty());
    assert_eq!(setup.provider.retire_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn wrong_runner_secret_is_a_rejected_claim() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/runner/v1/claim"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let root = TempDir::new().unwrap();
    let executor = Arc::new(Executor::new(
        Arc::new(Scripted::new(0, UNIT)),
        dirs(root.path()),
    ));
    let client =
        ServerClient::new(&server.uri(), Secret::new("wrong"), Duration::from_secs(2)).unwrap();
    let mut worker = Worker::new(
        client,
        executor,
        "r".to_owned(),
        Secret::new("wrong"),
        timing(),
    );
    assert!(matches!(
        worker.poll_once().await,
        Err(ClientError::Rejected(StatusCode::UNAUTHORIZED))
    ));
}

#[test]
fn delayed_ack_does_not_extend_the_deadline() {
    let fence_after = Duration::from_secs(35);
    let claim_sent = Instant::now();
    let mut fence = Fence::new(claim_sent, fence_after);
    assert_eq!(fence.deadline(), claim_sent + fence_after);
    // A heartbeat sent at +10 s whose ack arrives at +30 s counts from +10 s.
    let heartbeat_sent = claim_sent + Duration::from_secs(10);
    fence.acked(heartbeat_sent);
    assert_eq!(fence.deadline(), heartbeat_sent + fence_after);
    // An ack of an older request never moves the deadline back.
    fence.acked(claim_sent);
    assert_eq!(fence.deadline(), heartbeat_sent + fence_after);
}

#[test]
fn v1_stop_fits_inside_the_lease() {
    let t = Timing::v1();
    let v1 = LeaseTiming::V1;
    assert!(
        t.fence_after_send
            + t.term_grace
            + t.kill_and_cleanup
            + Duration::from_secs(u64::from(v1.margin_secs))
            <= t.lease
    );
    assert!(
        t.heartbeat * 2 < t.fence_after_send,
        "two lost heartbeats stay inside the deadline"
    );
    assert_eq!(
        t.idle_poll,
        Duration::from_secs(1),
        "an Ask waits at most 1 s"
    );
    assert_eq!(
        t.error_poll,
        Duration::from_secs(5),
        "failed claims retry slower"
    );
}

#[tokio::test]
async fn pending_events_are_bounded_while_posts_fail() {
    let mut setup = setup(Scripted::new(5000, Duration::ZERO), Duration::ZERO).await;
    mount(&setup, "events", ResponseTemplate::new(503)).await;
    mount(&setup, "result", ResponseTemplate::new(204)).await;
    let end = setup.worker.poll_once().await.unwrap();
    assert_eq!(
        end,
        Some(JobEnd::Fenced),
        "the fence still works under backpressure"
    );
    // Until the stop: afterwards the job loop no longer reads, so the agent's last events are discarded.
    let sent = setup
        .provider
        .sent_at_cancel
        .lock()
        .unwrap()
        .expect("the fence stopped the agent");
    // Pending limit plus the two channel buffers and the events in hand.
    assert!(
        sent <= MAX_PENDING_EVENTS + 16 + EVENT_BUFFER + 4,
        "the agent was not held back: {sent}"
    );
    assert!(setup.provider.finished.load(Ordering::SeqCst));
}

/// Event batches carrying events (heartbeats carry none).
async fn mount_event_failures(setup: &Setup, status: u16, times: u64) {
    Mock::given(method("POST"))
        .and(path(job_path(&setup.spec, "events")))
        .and(body_string_contains("\"seq\":"))
        .respond_with(ResponseTemplate::new(status))
        .up_to_n_times(times)
        .mount(&setup.server)
        .await;
}

#[tokio::test]
async fn output_is_skipped_when_earlier_events_are_not_acked() {
    let mut setup = setup(Scripted::new(1, UNIT), Duration::ZERO).await;
    mount_event_failures(&setup, 503, 2).await;
    mount(&setup, "events", ack(false)).await;
    mount(&setup, "output", ack(false)).await;
    mount(&setup, "result", ResponseTemplate::new(204)).await;
    let end = setup.worker.poll_once().await.unwrap();
    assert!(
        matches!(
            end,
            Some(JobEnd::ResultPosted(JobOutcome::Succeeded { .. }))
        ),
        "{end:?}"
    );
    assert!(
        requests(&setup, "output").await.is_empty(),
        "the output must not overtake the events"
    );
    let all = setup.server.received_requests().await.unwrap();
    let events_path = job_path(&setup.spec, "events");
    let last_events = all
        .iter()
        .rposition(|r| r.url.path() == events_path && !body::<EventBatch>(r).events.is_empty())
        .unwrap();
    let result = all
        .iter()
        .position(|r| r.url.path() == job_path(&setup.spec, "result"))
        .unwrap();
    assert!(last_events < result, "events are acked before the result");
}

#[tokio::test]
async fn stale_attempt_on_the_final_flush_suppresses_the_result() {
    let mut setup = setup(Scripted::new(1, UNIT), Duration::ZERO).await;
    mount_event_failures(&setup, 503, 2).await;
    mount_event_failures(&setup, 409, 100).await;
    mount(&setup, "events", ack(false)).await;
    mount(&setup, "output", ack(false)).await;
    mount(&setup, "result", ResponseTemplate::new(204)).await;
    let end = setup.worker.poll_once().await.unwrap();
    assert_eq!(end, Some(JobEnd::Stale(StatusCode::CONFLICT)));
    assert!(requests(&setup, "output").await.is_empty());
    assert!(
        requests(&setup, "result").await.is_empty(),
        "no result after a 409"
    );
}

#[tokio::test]
async fn job_token_straddling_the_summary_limit_never_leaves_in_part() {
    let mut provider = Scripted::new(1, UNIT);
    provider.summary = format!("{}{JOB_TOKEN}", "x".repeat(190));
    let mut setup = setup(provider, Duration::ZERO).await;
    mount(&setup, "events", ack(false)).await;
    mount(&setup, "output", ack(false)).await;
    mount(&setup, "result", ResponseTemplate::new(204)).await;
    setup.worker.poll_once().await.unwrap();
    for request in setup.server.received_requests().await.unwrap() {
        let text = String::from_utf8_lossy(&request.body);
        assert!(
            !text.contains(&JOB_TOKEN[..8]),
            "{} leaks part of the job token",
            request.url.path()
        );
    }
}

#[tokio::test]
async fn claim_failing_with_github_unavailable_is_polled_again() {
    let mut setup = setup(Scripted::new(1, UNIT), Duration::ZERO).await;
    Mock::given(method("POST"))
        .and(path("/runner/v1/claim"))
        .respond_with(
            ResponseTemplate::new(502).set_body_json(json!({"code": "github_unavailable"})),
        )
        .with_priority(1)
        .up_to_n_times(1)
        .mount(&setup.server)
        .await;
    mount(&setup, "events", ack(false)).await;
    mount(&setup, "output", ack(false)).await;
    mount(&setup, "result", ResponseTemplate::new(204)).await;
    let error = setup.worker.poll_once().await.unwrap_err();
    assert!(error.is_retryable(), "{error}");
    let end = setup.worker.poll_once().await.unwrap();
    assert!(matches!(end, Some(JobEnd::ResultPosted(_))), "{end:?}");
}

struct ParallelProvider {
    started: mpsc::UnboundedSender<JobId>,
    cancel: tokio::sync::watch::Sender<Vec<JobId>>,
}

impl LlmProvider for ParallelProvider {
    fn run<'a>(
        &'a self,
        call: &'a ProviderCall,
        _events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        Box::pin(async move {
            let job = call.job_spec.job_id;
            let mut cancelled = self.cancel.subscribe();
            self.started.send(job).unwrap();
            cancelled
                .wait_for(|jobs| jobs.contains(&job))
                .await
                .unwrap();
            Err(ProviderError::Cancelled)
        })
    }

    fn resume<'a>(
        &'a self,
        _: &'a str,
        call: &'a ProviderCall,
        events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        self.run(call, events)
    }

    fn cancel(&self, job: JobId) -> BoxFuture<'_, Result<(), ProviderError>> {
        self.cancel.send_modify(|jobs| jobs.push(job));
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn three_pr_jobs_run_before_any_finishes_and_a_fourth_waits_for_a_slot() {
    use contracts::job::spec::JobTarget;

    let server = MockServer::start().await;
    let root = TempDir::new().unwrap();
    let (started, mut active) = mpsc::unbounded_channel();
    let (cancel, cancelled) = tokio::sync::watch::channel(Vec::new());
    let provider = Arc::new(ParallelProvider { started, cancel });
    let mut specs = Vec::new();
    let stop_first = Arc::new(AtomicBool::new(false));
    for index in 1..=4 {
        let mut spec = spec(JobKind::Plan, shared_repo());
        spec.job_id = JobId::new_random();
        let JobTarget::PullRequest(target) = &mut spec.target else {
            panic!("expected a pull request");
        };
        target.pull_request_id = index;
        Mock::given(method("POST"))
            .and(path("/runner/v1/claim"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ClaimResponse {
                retire: vec![],
                claim: Some(Claim {
                    job_token: Secret::new(JOB_TOKEN),
                    lease_expires_at: Timestamp::now(),
                    spec: spec.clone(),
                }),
            }))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let flag = Arc::clone(&stop_first);
        Mock::given(method("POST"))
            .and(path(job_path(&spec, "events")))
            .respond_with(move |_: &Request| ack(index == 1 && flag.load(Ordering::SeqCst)))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(job_path(&spec, "result")))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        specs.push(spec);
    }
    let worker = Worker::new(
        ServerClient::new(
            &server.uri(),
            Secret::new(RUNNER_SECRET),
            Duration::from_secs(2),
        )
        .unwrap(),
        Arc::new(Executor::new(provider, dirs(root.path()))),
        "runner-test".into(),
        Secret::new(RUNNER_SECRET),
        timing(),
    );
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let runner = tokio::spawn(worker.run(async {
        let _ = shutdown.await;
    }));

    let mut first = Vec::new();
    for _ in 0..3 {
        first.push(
            tokio::time::timeout(Duration::from_secs(2), active.recv())
                .await
                .expect("all three PR jobs start before any is cancelled or finished")
                .unwrap(),
        );
    }
    let expected: std::collections::HashSet<_> =
        specs.iter().take(3).map(|spec| spec.job_id).collect();
    assert_eq!(
        first.into_iter().collect::<std::collections::HashSet<_>>(),
        expected
    );
    assert!(
        tokio::time::timeout(timing().idle_poll * 3, active.recv())
            .await
            .is_err()
    );
    let initial = server.received_requests().await.unwrap();
    assert_eq!(
        initial
            .iter()
            .filter(|r| r.url.path() == "/runner/v1/claim")
            .count(),
        3
    );
    assert!(cancelled.borrow().is_empty());

    stop_first.store(true, Ordering::SeqCst);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), active.recv())
            .await
            .unwrap(),
        Some(specs[3].job_id),
    );
    assert_eq!(&*cancelled.borrow(), &[specs[0].job_id]);
    let requests = server.received_requests().await.unwrap();
    let result: JobResult = body(
        requests
            .iter()
            .find(|r| r.url.path() == job_path(&specs[0], "result"))
            .unwrap(),
    );
    assert_eq!(result.outcome, JobOutcome::Cancelled);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner)
        .await
        .unwrap()
        .unwrap();
    for spec in specs.iter().skip(1) {
        assert!(cancelled.borrow().contains(&spec.job_id));
    }
}
