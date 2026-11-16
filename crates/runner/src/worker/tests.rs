//! The job loop against a wiremock server with a scripted provider. Time runs at 20 ms per `LeaseTiming`
//! second: lease 1200 ms, heartbeat 300 ms, fence 700 ms, grace 200 ms, cleanup 100 ms.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use contracts::agent::JobKind;
use contracts::job::event::ActivityKind;
use contracts::job::protocol::ClaimResponse;
use contracts::job::result::Usage;
use contracts::job::spec::JobSpec;
use futures::future::BoxFuture;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Notify;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::*;
use crate::execute::Executor;
use crate::provider::{LlmProvider, ProviderCall, ProviderError, ProviderOutcome};
use crate::test_support::{credential, dirs, shared_repo, spec};

const UNIT: Duration = Duration::from_millis(20);
const RUNNER_SECRET: &str = "runner-secret-WORKER-CANARY";
const JOB_TOKEN: &str = "job-token-WORKER-CANARY";

fn timing() -> Timing {
    Timing::scaled(LeaseTiming::V1, UNIT)
}

/// Emits `events` activities `step` apart, then the output; honours cancel unless told otherwise.
struct Scripted {
    events: usize,
    step: Duration,
    summary: String,
    output: Value,
    honour_cancel: bool,
    cancel_called: AtomicBool,
    cancelled: Notify,
    finished: AtomicBool,
    job_dir: Mutex<Option<PathBuf>>,
}

impl Scripted {
    fn new(events: usize, step: Duration) -> Self {
        Self {
            events,
            step,
            summary: "$ ls (exit 0)".to_owned(),
            output: json!({"answer": 42}),
            honour_cancel: true,
            cancel_called: AtomicBool::new(false),
            cancelled: Notify::new(),
            finished: AtomicBool::new(false),
            job_dir: Mutex::new(None),
        }
    }

    fn job_dir(&self) -> PathBuf {
        self.job_dir
            .lock()
            .unwrap()
            .clone()
            .expect("the provider ran")
    }
}

struct SetOnDrop<'a>(&'a AtomicBool);

impl Drop for SetOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl LlmProvider for Scripted {
    fn run<'a>(
        &'a self,
        call: &'a ProviderCall,
        events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        Box::pin(async move {
            let _finished = SetOnDrop(&self.finished);
            *self.job_dir.lock().unwrap() = Some(call.job_dir.clone());
            for _ in 0..self.events {
                if self.honour_cancel {
                    tokio::select! {
                        () = sleep(self.step) => {}
                        () = self.cancelled.notified() => return Err(ProviderError::Cancelled),
                    }
                } else {
                    sleep(self.step).await;
                }
                let event = JobEvent {
                    seq: 0,
                    at: Utc::now(),
                    activity: ActivityKind::Command,
                    summary: self.summary.clone(),
                };
                let _ = events.send(ProviderEvent::Activity(event)).await;
            }
            let _ = events
                .send(ProviderEvent::OutputReady(self.output.clone()))
                .await;
            Ok(ProviderOutcome {
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
        lease_expires_at: Utc::now(),
    })
}

async fn setup(provider: Scripted, claim_delay: Duration) -> Setup {
    let server = MockServer::start().await;
    let root = TempDir::new().unwrap();
    let spec = spec(JobKind::Plan, shared_repo());
    let claim = ClaimResponse {
        claim: Some(Claim {
            job_token: Secret::new(JOB_TOKEN),
            lease_expires_at: Utc::now(),
            spec: spec.clone(),
        }),
        drop_sessions: Vec::new(),
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
    let executor = Arc::new(Executor::new(
        provider.clone(),
        dirs(root.path()),
        credential(),
    ));
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
    assert_eq!(
        claim.credentials,
        [CredentialStatus {
            credential: credential(),
            usable: true
        }]
    );

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
    assert!(!setup.provider.job_dir().exists(), "work dir removed");
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
    assert!(!setup.provider.job_dir().exists(), "work dir removed");
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
    assert!(!setup.provider.job_dir().exists(), "work dir removed");
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
    assert!(!setup.provider.job_dir().exists(), "work dir removed");
}

/// The claim reply is delayed by 300 ms and no later request is ever acked: the deadline runs from the claim's
/// SEND time (700 ms), so the agent is stopped and cleaned up well before the lease (1200 ms from receipt) and
/// before a deadline wrongly measured from the delayed ack (1000 ms).
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
    assert!(!setup.provider.job_dir().exists(), "cleanup done");
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
async fn dropped_sessions_are_acknowledged_with_the_next_claim() {
    let server = MockServer::start().await;
    let root = TempDir::new().unwrap();
    let analysis = AnalysisId::new_random();
    Mock::given(method("POST"))
        .and(path("/runner/v1/claim"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ClaimResponse {
            claim: None,
            drop_sessions: vec![analysis],
        }))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/runner/v1/claim"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ClaimResponse {
            claim: None,
            drop_sessions: Vec::new(),
        }))
        .mount(&server)
        .await;
    let executor = Arc::new(Executor::new(
        Arc::new(Scripted::new(0, UNIT)),
        dirs(root.path()),
        credential(),
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
    assert!(body::<ClaimRequest>(&claims[0]).dropped_sessions.is_empty());
    assert_eq!(
        body::<ClaimRequest>(&claims[1]).dropped_sessions,
        [analysis]
    );
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
        credential(),
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
    assert_eq!(t.idle_poll, Duration::from_secs(5));
}
