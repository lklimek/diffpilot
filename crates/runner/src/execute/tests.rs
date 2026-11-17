//! `execute` against the fixture repository (`fixtures/pr-482/repo`, built with `build-repo.sh`) with a stub
//! provider, and with the fake provider when the feature is on.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

use contracts::agent::JobKind;
use contracts::common::RepoRef;
use contracts::job::access::{InstallationToken, Secret};
use contracts::job::event::{ActivityKind, JobEvent};
use contracts::job::result::Usage;
use contracts::job::spec::SessionRef;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;
use crate::test_support::{build_repo, commits, credential, dirs, shared_repo, spec};

const TOKEN_CANARY: &str = "ghs_EXECUTE_CANARY_000000000000001";
/// A secret only the receiver of the sink knows (job token, runner secret).
const SINK_SECRET: &str = "sink-SECRET-job-token-000000000002";

/// What the stub saw while it ran.
#[derive(Debug, Default, Clone)]
struct Seen {
    job_dir: PathBuf,
    codex_home: PathBuf,
    head: String,
    base_present: bool,
    prompt: String,
}

enum Behaviour {
    Succeed,
    Fail(fn() -> ProviderError),
    Hang,
    Leak,
    Straddle,
    /// Succeeds and leaves a session file holding the sink's secret.
    WriteSession,
    /// Runs until cancelled, then needs a while to stop (sweep, copy-back) before it ends as cancelled.
    StopsOnCancel,
}

struct Stub {
    behaviour: Behaviour,
    seen: Mutex<Option<Seen>>,
    /// Whether `resume` succeeds; the (thread, prompt) of every resume.
    resumable: bool,
    resumed: Mutex<Vec<(String, String)>>,
    cancelled: tokio::sync::Notify,
    stopped: std::sync::atomic::AtomicBool,
}

impl Stub {
    fn new(behaviour: Behaviour) -> Arc<Self> {
        Arc::new(Self {
            behaviour,
            seen: Mutex::new(None),
            resumable: false,
            resumed: Mutex::new(Vec::new()),
            cancelled: tokio::sync::Notify::new(),
            stopped: std::sync::atomic::AtomicBool::new(false),
        })
    }

    fn resumable() -> Arc<Self> {
        Arc::new(Self {
            resumable: true,
            ..Arc::into_inner(Self::new(Behaviour::Succeed)).unwrap()
        })
    }

    fn seen(&self) -> Seen {
        self.seen.lock().unwrap().clone().expect("the provider ran")
    }
}

impl LlmProvider for Stub {
    fn run<'a>(
        &'a self,
        call: &'a ProviderCall,
        events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        Box::pin(async move {
            let repo = call.job_dir.join("repo");
            let base = commits().base;
            let base_present = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["cat-file", "-e", &format!("{base}^{{commit}}")])
                .status()
                .is_ok_and(|s| s.success());
            *self.seen.lock().unwrap() = Some(Seen {
                job_dir: call.job_dir.clone(),
                codex_home: call.codex_home.clone(),
                head: fs::read_to_string(repo.join(".git/HEAD")).unwrap_or_default(),
                base_present,
                prompt: call.prompt.clone(),
            });
            let activity = |summary: String| {
                ProviderEvent::Activity(JobEvent {
                    seq: 0,
                    at: Utc::now(),
                    activity: ActivityKind::Command,
                    summary,
                })
            };
            match &self.behaviour {
                Behaviour::Succeed => {
                    events
                        .send(activity("$ ls (exit 0)".to_owned()))
                        .await
                        .unwrap();
                    events
                        .send(ProviderEvent::OutputReady(json!({"ok": true})))
                        .await
                        .unwrap();
                    Ok(outcome(json!({"ok": true})))
                }
                Behaviour::Fail(error) => Err(error()),
                Behaviour::WriteSession => {
                    fs::create_dir_all(&call.session_dir).unwrap();
                    fs::write(
                        call.session_dir.join("rollout.jsonl"),
                        format!("{{\"leak\":\"{SINK_SECRET}\"}}\n"),
                    )
                    .unwrap();
                    Ok(outcome(json!({"ok": true})))
                }
                Behaviour::Hang => std::future::pending().await,
                Behaviour::StopsOnCancel => {
                    self.cancelled.notified().await;
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    self.stopped
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    Err(ProviderError::Cancelled)
                }
                Behaviour::Straddle => {
                    for secret in [TOKEN_CANARY, SINK_SECRET] {
                        events
                            .send(activity(format!("{}{secret}", "x".repeat(185))))
                            .await
                            .unwrap();
                    }
                    Err(ProviderError::Failed {
                        message: format!("{}{TOKEN_CANARY} {SINK_SECRET}", "x".repeat(270)),
                    })
                }
                Behaviour::Leak => {
                    events
                        .send(activity(format!("$ echo {TOKEN_CANARY}")))
                        .await
                        .unwrap();
                    events.send(activity("x".repeat(500))).await.unwrap();
                    let leaked = json!({"text": format!("token {TOKEN_CANARY}")});
                    events
                        .send(ProviderEvent::OutputReady(leaked.clone()))
                        .await
                        .unwrap();
                    events
                        .send(ProviderEvent::OutputReady(json!({"second": 1})))
                        .await
                        .unwrap();
                    Ok(outcome(leaked))
                }
            }
        })
    }

    fn resume<'a>(
        &'a self,
        session_id: &'a str,
        call: &'a ProviderCall,
        _events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        Box::pin(async move {
            self.resumed
                .lock()
                .unwrap()
                .push((session_id.to_owned(), call.prompt.clone()));
            if self.resumable {
                Ok(outcome(json!({"resumed": true})))
            } else {
                Err(ProviderError::SessionUnavailable)
            }
        })
    }

    fn cancel(&self, _job: contracts::ids::JobId) -> BoxFuture<'_, Result<(), ProviderError>> {
        self.cancelled.notify_one();
        Box::pin(async { Ok(()) })
    }
}

fn outcome(output: Value) -> ProviderOutcome {
    ProviderOutcome {
        output,
        session_id: Some("thread-1".to_owned()),
        usage: Usage {
            input_tokens: 10,
            cached_input_tokens: 5,
            output_tokens: 2,
        },
        skills_commit: None,
    }
}

async fn execute_collect(executor: &Executor, spec: JobSpec) -> (JobResult, Vec<ProviderEvent>) {
    let (tx, mut rx) = mpsc::channel(1024);
    let result = executor
        .execute(spec, EventSink::new(tx, Scrubber::new([SINK_SECRET])))
        .await;
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    (result, events)
}

fn assert_removed(seen: &Seen) {
    assert!(!seen.job_dir.exists(), "job dir left behind");
    assert!(!seen.codex_home.exists(), "per-job CODEX_HOME left behind");
}

#[tokio::test]
async fn work_dir_removed_on_success() {
    let root = TempDir::new().unwrap();
    let stub = Stub::new(Behaviour::Succeed);
    let executor = Executor::new(stub.clone(), dirs(root.path()), credential());
    let (result, _) = execute_collect(&executor, spec(JobKind::Plan, shared_repo())).await;
    assert!(
        matches!(result.outcome, JobOutcome::Succeeded { .. }),
        "{result:?}"
    );
    let seen = stub.seen();
    assert_eq!(
        seen.head.trim(),
        commits().head,
        "head checked out at job_dir/repo"
    );
    assert!(seen.base_present, "base commit fetched for diffs");
    assert!(
        seen.codex_home
            .starts_with(root.path().join("state/codex-homes"))
    );
    assert_removed(&seen);
}

#[tokio::test]
async fn work_dir_removed_on_failure() {
    let root = TempDir::new().unwrap();
    let stub = Stub::new(Behaviour::Fail(|| ProviderError::Failed {
        message: "boom".to_owned(),
    }));
    let executor = Executor::new(stub.clone(), dirs(root.path()), credential());
    let (result, _) = execute_collect(&executor, spec(JobKind::Plan, shared_repo())).await;
    assert!(
        matches!(
            result.outcome,
            JobOutcome::Failed {
                kind: JobFailureKind::ProviderFailed,
                ..
            }
        ),
        "{result:?}"
    );
    assert!(matches!(
        result.llm_call,
        Some(LlmCallOutcome {
            status: LlmCallStatus::NotWorking { .. },
            ..
        })
    ));
    assert_removed(&stub.seen());
}

#[tokio::test]
async fn work_dir_removed_when_the_job_is_dropped() {
    let root = TempDir::new().unwrap();
    let stub = Stub::new(Behaviour::Hang);
    let executor = Executor::new(stub.clone(), dirs(root.path()), credential());
    let (tx, _rx) = mpsc::channel(16);
    let running = executor.execute(
        spec(JobKind::Plan, shared_repo()),
        EventSink::new(tx, Scrubber::default()),
    );
    let dropped = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::pin!(running);
        loop {
            tokio::select! {
                _ = &mut running => panic!("a hanging provider cannot finish"),
                () = tokio::time::sleep(Duration::from_millis(20)) => {
                    if stub.seen.lock().unwrap().is_some() { break; }
                }
            }
        }
    })
    .await;
    assert!(dropped.is_ok(), "the provider never started");
    assert_removed(&stub.seen());
}

#[tokio::test]
async fn empty_cache_works() {
    let root = TempDir::new().unwrap();
    let source = TempDir::new().unwrap();
    let repo = source.path().join("node-client");
    build_repo(&repo);
    assert!(
        !root.path().join("state").exists(),
        "no cache, no session store"
    );
    let stub = Stub::new(Behaviour::Succeed);
    let executor = Executor::new(stub.clone(), dirs(root.path()), credential());
    let (result, _) = execute_collect(&executor, spec(JobKind::Plan, &repo)).await;
    assert!(
        matches!(result.outcome, JobOutcome::Succeeded { .. }),
        "{result:?}"
    );
    // The mirror now serves the commits even without the source.
    fs::remove_dir_all(&repo).unwrap();
    let (again, _) = execute_collect(&executor, spec(JobKind::Plan, &repo)).await;
    assert!(
        matches!(again.outcome, JobOutcome::Succeeded { .. }),
        "cached mirror: {again:?}"
    );
}

#[tokio::test]
async fn checkout_failure_is_reported_and_cleaned_up() {
    let root = TempDir::new().unwrap();
    let stub = Stub::new(Behaviour::Succeed);
    let executor = Executor::new(stub.clone(), dirs(root.path()), credential());
    let spec = spec(JobKind::Plan, &root.path().join("missing-repo"));
    let key = format!("{}-{}", spec.job_id, spec.attempt);
    let (result, _) = execute_collect(&executor, spec).await;
    assert!(
        matches!(
            result.outcome,
            JobOutcome::Failed {
                kind: JobFailureKind::CheckoutFailed,
                ..
            }
        ),
        "{result:?}"
    );
    assert!(result.llm_call.is_none(), "no LLM call was made");
    assert!(stub.seen.lock().unwrap().is_none());
    assert!(!root.path().join("work").join(&key).exists());
    assert!(!root.path().join("state/codex-homes").join(&key).exists());
}

#[tokio::test]
async fn events_are_numbered_scrubbed_and_clipped() {
    let root = TempDir::new().unwrap();
    let stub = Stub::new(Behaviour::Leak);
    let executor = Executor::new(stub, dirs(root.path()), credential());
    let mut spec = spec(JobKind::Plan, shared_repo());
    spec.access.github = Some(InstallationToken {
        token: Secret::new(TOKEN_CANARY),
        expires_at: Utc::now(),
        repository: RepoRef {
            owner: "acme".to_owned(),
            name: "node-client".to_owned(),
        },
    });
    let (result, events) = execute_collect(&executor, spec).await;
    let mut seqs = Vec::new();
    let mut outputs = Vec::new();
    for event in &events {
        match event {
            ProviderEvent::Activity(event) => {
                seqs.push(event.seq);
                assert!(event.summary.chars().count() <= SUMMARY_MAX_CHARS);
            }
            ProviderEvent::OutputReady(output) => outputs.push(output.clone()),
        }
    }
    assert_eq!(seqs, [1, 2]);
    assert_eq!(
        outputs,
        [json!({"text": "token [redacted]"})],
        "one ready output, scrubbed"
    );
    let all = format!("{events:?} {result:?}");
    assert!(!all.contains(TOKEN_CANARY), "{all}");
}

#[tokio::test]
async fn runtime_cap_ends_with_timeout() {
    let root = TempDir::new().unwrap();
    let stub = Stub::new(Behaviour::Hang);
    let executor = Executor::new(stub.clone(), dirs(root.path()), credential());
    let mut spec = spec(JobKind::Plan, shared_repo());
    spec.limits.max_runtime_secs = 1;
    let (result, _) = execute_collect(&executor, spec).await;
    assert!(
        matches!(
            result.outcome,
            JobOutcome::Failed {
                kind: JobFailureKind::Timeout,
                ..
            }
        ),
        "{result:?}"
    );
    assert_removed(&stub.seen());
}

#[tokio::test]
async fn requested_session_counts_as_lost_and_the_context_pack_seeds_the_new_one() {
    let root = TempDir::new().unwrap();
    let stub = Stub::new(Behaviour::Succeed);
    let executor = Executor::new(stub.clone(), dirs(root.path()), credential());
    let mut spec = spec(JobKind::Plan, shared_repo());
    spec.session = Some(SessionRef {
        analysis_id: spec.analysis_id,
        thread_id: "old-thread".to_owned(),
    });
    spec.context_pack = Some("CONTEXT PACK".to_owned());
    let (result, _) = execute_collect(&executor, spec).await;
    assert!(stub.seen().prompt.starts_with("CONTEXT PACK\n\n"));
    assert_eq!(
        result.session,
        Some(SessionReport {
            thread_id: "thread-1".to_owned(),
            outcome: SessionOutcome::StartedAfterLoss
        })
    );
}

#[tokio::test]
async fn own_session_is_resumed_without_the_context_pack() {
    let root = TempDir::new().unwrap();
    let stub = Stub::resumable();
    let executor = Executor::new(stub.clone(), dirs(root.path()), credential());
    let mut spec = spec(JobKind::Plan, shared_repo());
    spec.session = Some(SessionRef {
        analysis_id: spec.analysis_id,
        thread_id: "thread-1".to_owned(),
    });
    spec.context_pack = Some("CONTEXT PACK".to_owned());
    let prompt = spec.prompt.clone();
    let (result, _) = execute_collect(&executor, spec).await;
    assert_eq!(
        *stub.resumed.lock().unwrap(),
        [("thread-1".to_owned(), prompt)]
    );
    assert!(stub.seen.lock().unwrap().is_none(), "no new session");
    assert_eq!(
        result.session,
        Some(SessionReport {
            thread_id: "thread-1".to_owned(),
            outcome: SessionOutcome::Resumed
        })
    );
}

#[tokio::test]
async fn session_of_another_analysis_is_never_resumed() {
    let root = TempDir::new().unwrap();
    let stub = Stub::resumable();
    let executor = Executor::new(stub.clone(), dirs(root.path()), credential());
    let mut spec = spec(JobKind::Plan, shared_repo());
    spec.session = Some(SessionRef {
        analysis_id: contracts::ids::AnalysisId::new_random(),
        thread_id: "thread-of-another-analysis".to_owned(),
    });
    spec.context_pack = Some("CONTEXT PACK".to_owned());
    let (result, _) = execute_collect(&executor, spec).await;
    assert!(stub.resumed.lock().unwrap().is_empty());
    assert!(stub.seen().prompt.starts_with("CONTEXT PACK\n\n"));
    assert_eq!(
        result.session.map(|s| s.outcome),
        Some(SessionOutcome::StartedAfterLoss)
    );
}

#[tokio::test]
async fn stored_session_is_scrubbed_of_the_sink_secrets() {
    let root = TempDir::new().unwrap();
    let executor = Executor::new(
        Stub::new(Behaviour::WriteSession),
        dirs(root.path()),
        credential(),
    );
    let spec = spec(JobKind::Plan, shared_repo());
    let stored = executor
        .sessions()
        .dir(spec.analysis_id)
        .join("rollout.jsonl");
    let (result, _) = execute_collect(&executor, spec).await;
    assert!(result.session.is_some());
    assert_eq!(
        fs::read_to_string(stored).unwrap(),
        "{\"leak\":\"[redacted]\"}\n"
    );
}

#[tokio::test]
async fn first_job_reports_a_started_session_and_usage() {
    let root = TempDir::new().unwrap();
    let executor = Executor::new(
        Stub::new(Behaviour::Succeed),
        dirs(root.path()),
        credential(),
    );
    let (result, _) = execute_collect(&executor, spec(JobKind::Plan, shared_repo())).await;
    assert_eq!(
        result.session,
        Some(SessionReport {
            thread_id: "thread-1".to_owned(),
            outcome: SessionOutcome::Started
        })
    );
    assert_eq!(result.usage.map(|u| u.input_tokens), Some(10));
    assert!(matches!(
        result.llm_call,
        Some(LlmCallOutcome {
            status: LlmCallStatus::Working,
            ..
        })
    ));
}

#[tokio::test]
async fn foreign_credential_is_rejected_before_any_work() {
    let root = TempDir::new().unwrap();
    let stub = Stub::new(Behaviour::Succeed);
    let executor = Executor::new(stub.clone(), dirs(root.path()), credential());
    let mut spec = spec(JobKind::Plan, shared_repo());
    spec.credential.id = "someone-else".to_owned();
    let (result, _) = execute_collect(&executor, spec).await;
    assert!(
        matches!(
            result.outcome,
            JobOutcome::Failed {
                kind: JobFailureKind::Internal,
                ..
            }
        ),
        "{result:?}"
    );
    assert!(stub.seen.lock().unwrap().is_none());
}

type ResultCheck = fn(&JobResult) -> bool;

#[tokio::test]
async fn provider_errors_map_to_typed_outcomes() {
    let cases: [(fn() -> ProviderError, ResultCheck); 4] = [
        (
            || ProviderError::UsageLimit { resets_at: None },
            |r| matches!(r.outcome, JobOutcome::UsageLimited { resets_at: None }),
        ),
        (
            || ProviderError::Cancelled,
            |r| matches!(r.outcome, JobOutcome::Cancelled) && r.llm_call.is_none(),
        ),
        (
            || ProviderError::CredentialUnusable,
            |r| {
                matches!(
                    r.outcome,
                    JobOutcome::Failed {
                        kind: JobFailureKind::CredentialUnusable,
                        ..
                    }
                )
            },
        ),
        (
            || ProviderError::OutputNotJson,
            |r| {
                matches!(
                    r.outcome,
                    JobOutcome::Failed {
                        kind: JobFailureKind::OutputNotJson,
                        ..
                    }
                )
            },
        ),
    ];
    for (error, check) in cases {
        let root = TempDir::new().unwrap();
        let executor = Executor::new(
            Stub::new(Behaviour::Fail(error)),
            dirs(root.path()),
            credential(),
        );
        let (result, _) = execute_collect(&executor, spec(JobKind::Plan, shared_repo())).await;
        assert!(check(&result), "{result:?}");
    }
}

#[cfg(feature = "fake-provider")]
mod fake {
    use std::time::Duration;

    use super::*;
    use crate::provider::fake::FakeProvider;
    use crate::test_support::fixture;

    /// Activities the recording yields through the Codex stream parser.
    fn recorded_activities(file: &str) -> usize {
        let text = fs::read_to_string(fixture(&format!("codex/{file}"))).unwrap();
        let mut stream = crate::provider::codex::CodexStream::default();
        text.lines()
            .filter(|line| stream.line(line).is_some())
            .count()
    }

    #[tokio::test]
    async fn fake_provider_reproduces_fixture_events_and_result() {
        let mut cases = vec![
            (
                JobKind::Plan,
                "Job: plan.".to_owned(),
                "plan.jsonl".to_owned(),
                "agent/plan.json".to_owned(),
            ),
            (
                JobKind::PrContext,
                "Job: pr_context.".to_owned(),
                "pr-context.jsonl".to_owned(),
                "agent/pr_context.json".to_owned(),
            ),
        ];
        for flow in 1..=5 {
            cases.push((
                JobKind::AnalyseFlow,
                format!("Job: analyse_flow f{flow}. Plan: f1 f2 f3 f4 f5"),
                format!("analyse-flow-{flow}.jsonl"),
                format!("agent/flow-{flow}.json"),
            ));
        }
        for (kind, prompt, recording, output) in cases {
            let root = TempDir::new().unwrap();
            let executor = Executor::new(
                Arc::new(FakeProvider::new(Duration::ZERO)),
                dirs(root.path()),
                credential(),
            );
            let mut spec = spec(kind, shared_repo());
            spec.prompt = prompt;
            let (result, events) = execute_collect(&executor, spec).await;
            let expected: Value =
                serde_json::from_str(&fs::read_to_string(fixture(&output)).unwrap()).unwrap();
            assert_eq!(
                result.outcome,
                JobOutcome::Succeeded {
                    output: expected.clone()
                },
                "{recording}"
            );
            assert_eq!(
                result.session.as_ref().map(|s| s.thread_id.as_str()),
                Some("019a8f2c-4820-7000-8000-000000000482")
            );
            assert!(
                result.usage.is_some_and(|u| u.input_tokens > 0),
                "{recording}"
            );
            let activities = recorded_activities(&recording);
            assert!(activities > 0);
            let seqs: Vec<u32> = events
                .iter()
                .filter_map(|e| match e {
                    ProviderEvent::Activity(event) => Some(event.seq),
                    ProviderEvent::OutputReady(_) => None,
                })
                .collect();
            assert_eq!(
                seqs,
                (1..=u32::try_from(activities).unwrap()).collect::<Vec<_>>(),
                "{recording}"
            );
            assert_eq!(
                events.last(),
                Some(&ProviderEvent::OutputReady(expected)),
                "{recording}"
            );
        }
    }

    #[tokio::test]
    async fn fake_provider_has_no_ask_recording() {
        let root = TempDir::new().unwrap();
        let executor = Executor::new(
            Arc::new(FakeProvider::new(Duration::ZERO)),
            dirs(root.path()),
            credential(),
        );
        let (result, _) = execute_collect(&executor, spec(JobKind::Ask, shared_repo())).await;
        assert!(
            matches!(
                result.outcome,
                JobOutcome::Failed {
                    kind: JobFailureKind::ProviderFailed,
                    ..
                }
            ),
            "{result:?}"
        );
    }
}

#[tokio::test]
async fn secrets_straddling_the_clip_limits_are_scrubbed_whole() {
    let root = TempDir::new().unwrap();
    let executor = Executor::new(
        Stub::new(Behaviour::Straddle),
        dirs(root.path()),
        credential(),
    );
    let mut spec = spec(JobKind::Plan, shared_repo());
    spec.access.github = Some(InstallationToken {
        token: Secret::new(TOKEN_CANARY),
        expires_at: Utc::now(),
        repository: RepoRef {
            owner: "acme".to_owned(),
            name: "node-client".to_owned(),
        },
    });
    let (result, events) = execute_collect(&executor, spec).await;
    let all = format!("{events:?} {result:?}");
    for secret in [TOKEN_CANARY, SINK_SECRET] {
        assert!(
            !all.contains(&secret[..8]),
            "a fragment of {secret} survived: {all}"
        );
    }
    assert!(all.contains("[redacted]"), "{all}");
}

#[tokio::test]
async fn context_pack_seeds_a_new_session_without_a_requested_session() {
    let root = TempDir::new().unwrap();
    let stub = Stub::new(Behaviour::Succeed);
    let executor = Executor::new(stub.clone(), dirs(root.path()), credential());
    let mut spec = spec(JobKind::Plan, shared_repo());
    spec.context_pack = Some("CONTEXT PACK".to_owned());
    let (result, _) = execute_collect(&executor, spec).await;
    assert!(
        stub.seen().prompt.starts_with("CONTEXT PACK\n\n"),
        "{}",
        stub.seen().prompt
    );
    assert_eq!(
        result.session.map(|s| s.outcome),
        Some(SessionOutcome::Started)
    );
}

/// An HTTP remote that accepts the fetch and never answers. Reports whether the connection was closed (every
/// process of the fetch gone) within `closed_within` of the request.
fn hanging_http_remote(closed_within: Duration) -> (String, std::sync::mpsc::Receiver<bool>) {
    use std::io::{ErrorKind, Read};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/repo.git", listener.local_addr().unwrap());
    let (report, closed) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = socket.read(&mut request);
        socket.set_read_timeout(Some(closed_within)).unwrap();
        let was_closed = match socket.read(&mut request) {
            Ok(0) => true,
            Ok(_) => false,
            Err(error) => !matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
        };
        let _ = report.send(was_closed);
        // Never answers or closes: only the fetch going away ends the connection.
        std::thread::sleep(Duration::from_secs(60));
        drop(socket);
    });
    (url, closed)
}

#[tokio::test]
async fn hanging_fetch_ends_with_timeout_and_kills_the_whole_git_group() {
    let root = TempDir::new().unwrap();
    let stub = Stub::new(Behaviour::Succeed);
    let executor = Executor::new(stub.clone(), dirs(root.path()), credential());
    let (url, closed) = hanging_http_remote(Duration::from_secs(5));
    let mut spec = spec(JobKind::Plan, shared_repo());
    spec.repo.clone_url = url;
    spec.limits.max_runtime_secs = 1;
    let key = format!("{}-{}", spec.job_id, spec.attempt);
    let (result, _) =
        tokio::time::timeout(Duration::from_secs(20), execute_collect(&executor, spec))
            .await
            .expect("the runtime limit covers the fetch");
    assert!(
        matches!(
            result.outcome,
            JobOutcome::Failed {
                kind: JobFailureKind::Timeout,
                ..
            }
        ),
        "{result:?}"
    );
    assert!(
        stub.seen.lock().unwrap().is_none(),
        "the provider never ran"
    );
    assert!(
        !root.path().join("work").join(&key).exists(),
        "the job dir is removed"
    );
    assert!(
        tokio::task::spawn_blocking(move || closed.recv().unwrap())
            .await
            .unwrap(),
        "git-remote-http (in git's process group) still holds the connection"
    );
}

#[tokio::test]
async fn runtime_cap_stops_a_detached_attempt_through_the_provider_and_awaits_it() {
    let root = TempDir::new().unwrap();
    let stub = Stub::new(Behaviour::StopsOnCancel);
    let executor =
        Executor::new(stub.clone(), dirs(root.path()), credential()).with_detached_attempts();
    let mut spec = spec(JobKind::Plan, shared_repo());
    spec.limits.max_runtime_secs = 1;
    let (result, _) = execute_collect(&executor, spec).await;
    assert!(
        matches!(
            result.outcome,
            JobOutcome::Failed {
                kind: JobFailureKind::Timeout,
                ..
            }
        ),
        "{result:?}"
    );
    assert!(
        stub.stopped.load(std::sync::atomic::Ordering::SeqCst),
        "the result waited for the provider's stop sequence"
    );
    assert_removed(&stub.seen());
}

#[tokio::test]
async fn dropped_detached_attempt_holds_settled_until_it_has_stopped() {
    let root = TempDir::new().unwrap();
    let stub = Stub::new(Behaviour::StopsOnCancel);
    let executor =
        Executor::new(stub.clone(), dirs(root.path()), credential()).with_detached_attempts();
    let spec = spec(JobKind::Plan, shared_repo());
    let (tx, _rx) = mpsc::channel(64);
    let running = executor.execute(spec, EventSink::new(tx, Scrubber::default()));
    tokio::select! {
        _ = running => panic!("the attempt ended without a cancel"),
        () = async {
            while stub.seen.lock().unwrap().is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        } => {}
    }
    // `execute` was dropped: the attempt is cancelled, and the runner may go on only once it stopped.
    executor.settled().await;
    assert!(stub.stopped.load(std::sync::atomic::Ordering::SeqCst));
    assert_removed(&stub.seen());
}
