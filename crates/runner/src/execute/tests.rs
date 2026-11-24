//! Container execution boundary: mirrors only, scrubbed output, routing and cancellation.
use super::*;
use crate::test_support::{
    dirs, judge_input, judge_spec, shared_repo, spec, verify_input, verify_spec,
};
use contracts::agent::JobKind;
use contracts::job::event::{ActivityKind, JobEvent};
use contracts::job::result::Usage;
use contracts::job::spec::FlowFile;
use futures::future::BoxFuture;
use serde_json::json;
use std::path::Path;
use std::sync::Mutex;

const SINK_SECRET: &str = "sink-secret-canary";

#[derive(Default)]
struct Stub {
    seen: Mutex<Option<JobSpec>>,
    hang: bool,
    stopped: tokio::sync::Notify,
    cancelled: std::sync::atomic::AtomicBool,
}
impl LlmProvider for Stub {
    fn run<'a>(
        &'a self,
        call: &'a ProviderCall,
        events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        Box::pin(async move {
            *self.seen.lock().unwrap() = Some((*call.job_spec).clone());
            if self.hang {
                self.stopped.notified().await;
                return Err(ProviderError::Checkpoint {
                    error: Box::new(ProviderError::Metered {
                        error: Box::new(ProviderError::Cancelled),
                        usage: Usage {
                            input_tokens: 7,
                            ..Usage::default()
                        },
                    }),
                    session: Box::new(SessionReport {
                        thread_id: "worker-checkpoint".into(),
                        outcome: SessionOutcome::Started,
                        last_context: None,
                        preflight: None,
                    }),
                });
            }
            events
                .send(ProviderEvent::Activity(JobEvent {
                    seq: 99,
                    at: Timestamp::now(),
                    activity: ActivityKind::Command,
                    summary: format!("{}{}", "x".repeat(195), SINK_SECRET),
                }))
                .await
                .unwrap();
            let output = json!({"secret": SINK_SECRET});
            events
                .send(ProviderEvent::OutputReady(output.clone()))
                .await
                .unwrap();
            Ok(ProviderOutcome {
                runtime_session: None,
                output,
                session_id: None,
                session_turns: None,
                usage: Usage::default(),
                skills_commit: None,
            })
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
    fn cancel(&self, _: JobId) -> BoxFuture<'_, Result<(), ProviderError>> {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.stopped.notify_one();
        Box::pin(async { Ok(()) })
    }
}
async fn collect(executor: &Executor, spec: JobSpec) -> (JobResult, Vec<ProviderEvent>) {
    let (send, mut receive) = mpsc::channel(64);
    let result = executor
        .execute(spec, EventSink::new(send, Scrubber::new([SINK_SECRET])))
        .await;
    let mut events = vec![];
    while let Some(event) = receive.recv().await {
        events.push(event);
    }
    (result, events)
}

#[tokio::test]
async fn container_execution_does_not_create_a_host_checkout() {
    let root = tempfile::tempdir().unwrap();
    let provider = Arc::new(Stub::default());
    let executor = Executor::new(provider.clone(), dirs(root.path()));
    let mut job = spec(JobKind::Plan, shared_repo());
    job.payer = contracts::ids::UserId("any-linked-payer".into());
    let (result, _) = collect(&executor, job.clone()).await;
    assert!(matches!(result.outcome, JobOutcome::Succeeded { .. }));
    assert_eq!(*provider.seen.lock().unwrap(), Some(job));
    assert!(!root.path().join("work").exists());
    assert!(!root.path().join("state/codex-homes").exists());
    assert!(!root.path().join("state/sessions").exists());
    assert!(root.path().join("state/mirrors").exists());
}

#[tokio::test]
async fn events_and_output_are_numbered_scrubbed_and_clipped() {
    let root = tempfile::tempdir().unwrap();
    let executor = Executor::new(Arc::new(Stub::default()), dirs(root.path()));
    let (result, events) = collect(&executor, spec(JobKind::Plan, shared_repo())).await;
    assert_eq!(
        result.outcome,
        JobOutcome::Succeeded {
            output: json!({"secret":"[redacted]"})
        }
    );
    assert!(
        matches!(&events[0], ProviderEvent::Activity(event) if event.seq == 1 && event.summary.chars().count() <= SUMMARY_MAX_CHARS)
    );
    for event in events {
        assert!(!format!("{event:?}").contains(SINK_SECRET));
    }
}

#[tokio::test]
async fn runtime_cap_cancels_and_settles_the_provider() {
    let root = tempfile::tempdir().unwrap();
    let provider = Arc::new(Stub {
        hang: true,
        ..Stub::default()
    });
    let executor = Executor::new(provider.clone(), dirs(root.path()));
    let mut job = spec(JobKind::Plan, shared_repo());
    job.limits.max_runtime_secs = 1;
    let (result, _) = collect(&executor, job).await;
    executor.settled().await;
    assert!(matches!(
        result.outcome,
        JobOutcome::Failed {
            kind: JobFailureKind::Timeout,
            ..
        }
    ));
    assert!(provider.cancelled.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        result.usage.unwrap().input_tokens,
        7,
        "settled timeout usage is retained"
    );
    assert_eq!(result.session.unwrap().thread_id, "worker-checkpoint");
}

#[tokio::test]
async fn dashboard_and_verify_jobs_enforce_their_capabilities() {
    let root = tempfile::tempdir().unwrap();
    let provider = Arc::new(Stub::default());
    let executor = Executor::new(provider.clone(), dirs(root.path()));
    let mut dashboard = judge_spec(&judge_input());
    dashboard.skills.push("severity".into());
    let mut verify = verify_spec(shared_repo(), &verify_input(None));
    verify.flow_file = Some(FlowFile {
        context: json!({}),
        previous: None,
    });
    for job in [dashboard, verify] {
        let (result, _) = collect(&executor, job).await;
        assert!(matches!(
            result.outcome,
            JobOutcome::Failed {
                kind: JobFailureKind::Internal,
                ..
            }
        ));
    }
    assert!(provider.seen.lock().unwrap().is_none());
}

#[cfg(feature = "fake-provider")]
#[tokio::test]
async fn fake_provider_replays_json_and_tagged_flows() {
    let root = tempfile::tempdir().unwrap();
    let executor = Executor::new(
        Arc::new(crate::provider::fake::FakeProvider::default()),
        dirs(root.path()),
    );
    for kind in [JobKind::Plan, JobKind::PrContext, JobKind::AnalyseFlow] {
        let mut job = spec(kind, shared_repo());
        if kind == JobKind::AnalyseFlow {
            job.flow_file = Some(FlowFile {
                context: json!({}),
                previous: None,
            });
        }
        let (result, _) = collect(&executor, job).await;
        assert!(
            matches!(result.outcome, JobOutcome::Succeeded { .. }),
            "{result:?}"
        );
        assert!(result.usage.is_some());
    }
}

#[test]
fn metered_runtime_failure_preserves_usage_and_non_retryable_kind() {
    let spec = crate::test_support::spec(JobKind::Plan, Path::new("/unused"));
    let usage = Usage {
        input_tokens: 17,
        cached_input_tokens: 5,
        output_tokens: 2,
        requests: Some(1),
    };
    let result = super::to_result(
        &spec,
        Err(ProviderError::Metered {
            error: Box::new(ProviderError::Runtime {
                kind: JobFailureKind::InvalidJsonSchema,
            }),
            usage,
        }),
        SessionOutcome::Started,
        &Scrubber::default(),
    );
    assert_eq!(result.usage, Some(usage));
    assert!(matches!(
        result.outcome,
        JobOutcome::Failed {
            kind: JobFailureKind::InvalidJsonSchema,
            ..
        }
    ));
}

#[test]
fn failed_answer_checkpoint_reaches_job_result_with_its_usage_and_failure() {
    let spec = crate::test_support::spec(JobKind::Ask, Path::new("/unused"));
    let session = contracts::job::result::SessionReport {
        thread_id: "working-fork".into(),
        outcome: SessionOutcome::Resumed,
        last_context: Some(Box::new(contracts::runtime::LastContext {
            tokens: 30000,
            generation: 7,
        })),
        preflight: Some(Box::new(contracts::runtime::AskPreflight {
            seed: contracts::runtime::SessionSeed::Fork,
            generation: 7,
            state: contracts::runtime::PreflightState::Done,
        })),
    };
    let usage = Usage {
        requests: Some(2),
        ..Usage::default()
    };
    let result = super::to_result(
        &spec,
        Err(ProviderError::Metered {
            error: Box::new(ProviderError::Checkpoint {
                error: Box::new(ProviderError::Timeout),
                session: Box::new(session.clone()),
            }),
            usage,
        }),
        SessionOutcome::Resumed,
        &Scrubber::default(),
    );
    assert_eq!(result.session, Some(session));
    assert_eq!(result.usage, Some(usage));
    assert!(matches!(
        result.outcome,
        JobOutcome::Failed {
            kind: JobFailureKind::Timeout,
            ..
        }
    ));
}

#[tokio::test]
async fn host_assessment_skill_is_allowed_and_the_frozen_method_is_returned() {
    let root = tempfile::tempdir().unwrap();
    let provider = Arc::new(Stub::default());
    let executor = Executor::new(provider, dirs(root.path()));
    let mut job = verify_spec(shared_repo(), &verify_input(None));
    let run = contracts::assessment::AssessmentRunRef {
        pass_id: contracts::ids::PassId::new_random(),
        method: contracts::assessment::AssessmentMethod::ClaudiusHost,
    };
    job.assessment = Some(run.clone());
    job.skills = vec!["check-pr-comments".into()];
    let (result, _) = collect(&executor, job).await;
    assert!(
        matches!(result.outcome, JobOutcome::Succeeded { .. }),
        "{result:?}"
    );
    assert_eq!(result.assessment.as_deref(), Some(&run));
}

#[tokio::test]
async fn native_assessment_returns_its_frozen_method_without_host_skills() {
    let root = tempfile::tempdir().unwrap();
    let executor = Executor::new(Arc::new(Stub::default()), dirs(root.path()));
    let mut job = verify_spec(shared_repo(), &verify_input(None));
    let run = contracts::assessment::AssessmentRunRef {
        pass_id: contracts::ids::PassId::new_random(),
        method: contracts::assessment::AssessmentMethod::PromptV4,
    };
    job.assessment = Some(run.clone());
    let (result, _) = collect(&executor, job).await;
    assert!(matches!(result.outcome, JobOutcome::Succeeded { .. }));
    assert_eq!(result.assessment.as_deref(), Some(&run));
}
