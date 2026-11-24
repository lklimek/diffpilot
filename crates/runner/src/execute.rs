//! The runner core `execute(JobSpec, EventSink) -> JobResult` (rule A4): prepares a bare
//! mirror, delegates checkout and persistent threads to the worker provider, streams scrubbed events and
//! maps the outcome to a `JobResult`. Cancellation settles before another claim.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use contracts::agent::{JobKind, verify_comments};
use contracts::ids::JobId;
use contracts::job::access::AgentAccess;
use contracts::job::result::{
    JobFailureKind, JobOutcome, JobResult, LlmCallOutcome, LlmCallStatus, SessionOutcome,
    SessionReport,
};
use contracts::job::spec::{JobSpec, JobTarget};
use contracts::time::Timestamp;
use tokio::sync::mpsc;

use crate::git;
use crate::phases::PhaseLog;
use crate::provider::{LlmProvider, ProviderCall, ProviderError, ProviderEvent, ProviderOutcome};
use crate::scrub::{SUMMARY_MAX_CHARS, Scrubber, clip};

/// Where `execute` sends a job's normalised events and its ready output, in order, together with the secrets
/// the receiver holds (job token, runner secret): `execute` scrubs every known secret before it clips text,
/// so no fragment of a secret survives a cut.
#[derive(Clone, Debug)]
pub struct EventSink {
    events: mpsc::Sender<ProviderEvent>,
    secrets: Scrubber,
}

impl EventSink {
    /// A sink sending to `events`; `secrets` are scrubbed from everything sent and from the result.
    pub fn new(events: mpsc::Sender<ProviderEvent>, secrets: Scrubber) -> Self {
        Self { events, secrets }
    }
}

/// Events buffered between the provider and `execute`; small, as summaries are not clipped yet.
const EVENT_BUFFER: usize = 16;
/// Longest failure message in a result.
const MESSAGE_MAX_CHARS: usize = 300;

/// Persistent repository mirrors, shared read-only with workers.
#[derive(Clone, Debug)]
pub struct RunnerDirs {
    /// Bare mirrors by clone URL.
    pub mirrors: PathBuf,
}

/// Runs claimed container jobs with one provider.
pub struct Executor {
    provider: Arc<dyn LlmProvider>,
    dirs: RunnerDirs,
    /// Held until the attempt has stopped and cleaned up, including cancellation.
    gate: Arc<tokio::sync::Mutex<()>>,
    active: Arc<std::sync::Mutex<Option<(JobId, tokio::task::AbortHandle)>>>,
}

impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Executor")
            .field("dirs", &self.dirs)
            .finish_non_exhaustive()
    }
}

impl Executor {
    /// An executor running container jobs with a persistent mirror cache.
    pub fn new(provider: Arc<dyn LlmProvider>, dirs: RunnerDirs) -> Self {
        Self {
            provider,
            dirs,
            gate: Arc::default(),
            active: Arc::default(),
        }
    }

    /// Waits until no detached attempt is still finishing (stop and cleanup); call before a
    /// terminal result is posted and before the next claim.
    pub async fn settled(&self) {
        drop(self.gate.lock().await);
    }

    /// Drops an unresponsive call after provider cancellation; app-server fences any orphan before reuse.
    pub(crate) fn abort_attempt(&self) {
        if let Some((_, active)) = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            active.abort();
        }
    }

    /// The provider, for cancelling a running call.
    pub fn provider(&self) -> &Arc<dyn LlmProvider> {
        &self.provider
    }

    /// Runs `spec` to its result. Events and the ready output go to `sink`; a closed sink does not stop the
    /// job. Worker cleanup settles before the next claim.
    pub async fn execute(&self, spec: JobSpec, sink: EventSink) -> JobResult {
        let attempt = Self {
            provider: Arc::clone(&self.provider),
            dirs: self.dirs.clone(),
            gate: Arc::clone(&self.gate),
            active: Arc::clone(&self.active),
        };
        // Taken before the task starts, so `settled` never passes an attempt that has not begun.
        let held = Arc::clone(&self.gate).lock_owned().await;
        let mut cancel = CancelOnDrop {
            provider: Arc::clone(&self.provider),
            job: Some(spec.job_id),
        };
        let job = spec.job_id;
        let task = tokio::spawn(async move {
            let result = attempt.execute_here(spec, sink).await;
            drop(held);
            result
        });
        *self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((job, task.abort_handle()));
        let joined = task.await;
        {
            let mut active = self
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if active.as_ref().is_some_and(|(id, _)| *id == job) {
                active.take();
            }
        }
        cancel.job = None;
        match joined {
            Ok(result) => result,
            Err(error) => std::panic::resume_unwind(error.into_panic()),
        }
    }

    async fn execute_here(&self, spec: JobSpec, sink: EventSink) -> JobResult {
        let phases = PhaseLog::default();
        let started = tokio::time::Instant::now();
        let result = self.attempt(&spec, &sink, &phases).await;
        phases.since("total", started);
        tracing::info!(job = %spec.job_id, attempt = spec.attempt, kind = ?spec.kind,
            phases = %phases.summary(), "job phase timings");
        tracing::info!(job = %spec.job_id, attempt = spec.attempt, outcome = outcome_name(&result.outcome), "job attempt finished");
        result
    }

    async fn attempt(&self, spec: &JobSpec, sink: &EventSink, phases: &PhaseLog) -> JobResult {
        // The runtime limit counts from the start of the attempt: a hanging fetch must not hold the lease.
        let deadline = tokio::time::Instant::now()
            + Duration::from_secs(u64::from(spec.limits.max_runtime_secs));
        if let Some(problem) = misfit(spec) {
            tracing::error!(job = %spec.job_id, problem, "refusing a job whose kind, target and access do not fit");
            return failed(spec, JobFailureKind::Internal, problem);
        }
        match tokio::time::timeout_at(deadline, self.prepare(spec, phases)).await {
            Ok(Ok(())) => {}
            Ok(Err((kind, message))) => return failed(spec, kind, message),
            Err(_) => {
                tracing::warn!(job = %spec.job_id, "repository fetch or checkout exceeded the job's runtime limit");
                return failed(
                    spec,
                    JobFailureKind::Timeout,
                    "the repository fetch or checkout exceeded the job's runtime limit",
                );
            }
        }

        let scrubber = sink.secrets.with(&Scrubber::new(
            spec.access
                .github
                .iter()
                .map(|token| token.token.expose())
                .chain(
                    spec.access
                        .memcan
                        .iter()
                        .map(|memcan| memcan.token.expose()),
                ),
        ));
        let call = ProviderCall {
            job_spec: Box::new(spec.clone()),
        };
        let (tx, rx) = mpsc::channel(EVENT_BUFFER);
        let run = async {
            let session = self.provider.run(&call, tx);
            tokio::pin!(session);
            match tokio::time::timeout_at(deadline, &mut session).await {
                Ok(done) => done,
                Err(_) => {
                    let (cancelled, settled) =
                        tokio::join!(self.provider.cancel(spec.job_id), &mut session);
                    if let Err(error) = cancelled {
                        tracing::warn!(job = %spec.job_id, %error, "cannot stop the worker at the runtime cap");
                    }
                    let (usage, checkpoint) = match settled {
                        Ok(done) => (Some(done.usage), done.runtime_session),
                        Err(error) => {
                            let (_, usage, checkpoint) = error.into_details();
                            (usage, checkpoint)
                        }
                    };
                    let mut error = ProviderError::Timeout;
                    if let Some(usage) = usage {
                        error = ProviderError::Metered {
                            error: Box::new(error),
                            usage,
                        };
                    }
                    if let Some(checkpoint) = checkpoint {
                        error = ProviderError::Checkpoint {
                            error: Box::new(error),
                            session: Box::new(checkpoint),
                        };
                    }
                    Err(error)
                }
            }
        };
        let (outcome, ()) = tokio::join!(run, pump(rx, sink, &scrubber));
        let session_outcome = outcome
            .as_ref()
            .ok()
            .and_then(|done| done.runtime_session.as_ref())
            .map_or(SessionOutcome::Started, |session| session.outcome);
        let (usage, turns) = match &outcome {
            Ok(done) => (Some(done.usage), done.session_turns),
            Err(_) => (None, None),
        };
        let origin = match session_outcome {
            SessionOutcome::Started => "new session",
            SessionOutcome::Resumed => "resumed session",
            SessionOutcome::StartedAfterLoss => "context pack after a lost session",
        };
        tracing::info!(job = %spec.job_id, kind = ?spec.kind, session = origin,
            model = %spec.model.model, effort = ?spec.model.reasoning_effort,
            session_turns = %turns.map_or_else(|| "unknown".to_owned(), |n| n.to_string()),
            input_tokens = usage.map(|u| u.input_tokens), cached_input_tokens = usage.map(|u| u.cached_input_tokens),
            output_tokens = usage.map(|u| u.output_tokens), requests = usage.and_then(|u| u.requests),
            items = verify_comments::item_count(spec.kind, &spec.prompt),
            "job agent session summary");
        to_result(spec, outcome, session_outcome, &scrubber)
    }

    async fn prepare(
        &self,
        spec: &JobSpec,
        phases: &PhaseLog,
    ) -> Result<(), (JobFailureKind, &'static str)> {
        let Some(repo) = spec.target.checkout() else {
            return Ok(());
        };
        let mirror = git::mirror_path(&self.dirs.mirrors, &repo.clone_url);
        let commits = [repo.head_sha.0.as_str(), repo.base_sha.0.as_str()];
        if let Err(error) = git::prepare_mirror(&mirror, &repo.clone_url, &commits, phases).await {
            tracing::warn!(job = %spec.job_id, %error, "repository mirror fetch failed");
            return Err((
                JobFailureKind::CheckoutFailed,
                "the repository mirror could not be fetched",
            ));
        }
        Ok(())
    }
}

/// Why a job's kind, target and access do not fit together (ADR-018, ADR-023, threat model T17, T23): a judge job
/// is a dashboard job and a `verify_comments` job a comments job, each the other way round too; a dashboard job
/// carries no agent access, skills, flow file or session to resume, a `verify_comments` job no MemCan endpoint,
/// flow file, session or skill (only the read-only GitHub token; ADR-002 amendment 2026-11-23).
fn misfit(spec: &JobSpec) -> Option<&'static str> {
    let dashboard = matches!(spec.target, JobTarget::Dashboard { .. });
    let comments = matches!(spec.target, JobTarget::PullRequestComments(_));
    if dashboard != (spec.kind == JobKind::JudgeWork)
        || comments != (spec.kind == JobKind::VerifyComments)
    {
        return Some("the job kind does not match its target");
    }
    let holds_session =
        spec.flow_file.is_some() || spec.session.is_some() || spec.working_session.is_some();
    if dashboard
        && (holds_session || !spec.skills.is_empty() || spec.access != AgentAccess::default())
    {
        return Some("a dashboard job must not carry agent access, skills or a session");
    }
    let host = spec
        .assessment
        .as_ref()
        .is_some_and(|run| run.method == contracts::assessment::AssessmentMethod::ClaudiusHost);
    let skills_fit = if host {
        spec.skills.iter().all(|skill| skill == "check-pr-comments")
    } else {
        spec.skills.is_empty()
    };
    (comments && (holds_session || !skills_fit || spec.access.memcan.is_some())).then_some(
        "a verify_comments job must use its fixed method skills without MemCan access or a session",
    )
}

/// Numbers activities from 1, scrubs every known secret and then clips them, forwards at most one ready
/// output.
async fn pump(mut events: mpsc::Receiver<ProviderEvent>, sink: &EventSink, scrubber: &Scrubber) {
    let mut seq = 0_u32;
    let mut output_sent = false;
    while let Some(event) = events.recv().await {
        let event = match event {
            ProviderEvent::Activity(mut event) => {
                seq += 1;
                event.seq = seq;
                event.summary = clip(&scrubber.text(&event.summary), SUMMARY_MAX_CHARS);
                ProviderEvent::Activity(event)
            }
            ProviderEvent::OutputReady(_) if output_sent => continue,
            ProviderEvent::OutputReady(output) => {
                output_sent = true;
                ProviderEvent::OutputReady(scrubber.value(output))
            }
        };
        // The job loop may be gone (fenced or stale); the job still runs to its end and cleans up.
        let _ = sink.events.send(event).await;
    }
}

fn to_result(
    spec: &JobSpec,
    outcome: Result<ProviderOutcome, ProviderError>,
    session_outcome: SessionOutcome,
    scrubber: &Scrubber,
) -> JobResult {
    let at = Timestamp::now();
    match outcome {
        Ok(outcome) => JobResult {
            assessment: spec.assessment.clone().map(Box::new),
            report: None,
            job_id: spec.job_id,
            attempt: spec.attempt,
            outcome: JobOutcome::Succeeded {
                output: scrubber.value(outcome.output),
            },
            session: outcome.runtime_session.or_else(|| {
                outcome.session_id.map(|thread_id| SessionReport {
                    last_context: None,
                    preflight: None,
                    thread_id,
                    outcome: session_outcome,
                })
            }),
            usage: Some(outcome.usage),
            skills_commit: outcome.skills_commit,
            llm_call: Some(LlmCallOutcome {
                at,
                status: LlmCallStatus::Working,
            }),
        },
        Err(error) => {
            let (error, usage, session) = error.into_details();
            let message = clip(&scrubber.text(&error.to_string()), MESSAGE_MAX_CHARS);
            let outcome = match error {
                ProviderError::Metered { .. } | ProviderError::Checkpoint { .. } => {
                    failure(JobFailureKind::Internal, &message)
                }
                ProviderError::Runtime { kind } => failure(kind, &message),
                ProviderError::Cancelled => JobOutcome::Cancelled,
                ProviderError::UsageLimit { resets_at } => JobOutcome::UsageLimited { resets_at },
                ProviderError::OutputNotJson => failure(JobFailureKind::OutputNotJson, &message),
                ProviderError::Timeout => failure(JobFailureKind::Timeout, &message),
                ProviderError::Io(_) => failure(JobFailureKind::Internal, &message),
                ProviderError::SessionUnavailable | ProviderError::Failed { .. } => {
                    failure(JobFailureKind::ProviderFailed, &message)
                }
            };
            // An interrupted call says nothing about whether the LLM works.
            let llm_call = (!matches!(outcome, JobOutcome::Cancelled)).then_some(LlmCallOutcome {
                at,
                status: LlmCallStatus::NotWorking { message },
            });
            JobResult {
                assessment: spec.assessment.clone().map(Box::new),
                report: None,
                job_id: spec.job_id,
                attempt: spec.attempt,
                outcome,
                session,
                usage,
                skills_commit: None,
                llm_call,
            }
        }
    }
}

fn failure(kind: JobFailureKind, message: &str) -> JobOutcome {
    JobOutcome::Failed {
        kind,
        message: message.to_owned(),
    }
}

/// A failed result before any provider call.
fn failed(spec: &JobSpec, kind: JobFailureKind, message: &str) -> JobResult {
    JobResult {
        assessment: spec.assessment.clone().map(Box::new),
        report: None,
        job_id: spec.job_id,
        attempt: spec.attempt,
        outcome: failure(kind, message),
        session: None,
        usage: None,
        skills_commit: None,
        llm_call: None,
    }
}

fn outcome_name(outcome: &JobOutcome) -> &'static str {
    match outcome {
        JobOutcome::Succeeded { .. } => "succeeded",
        JobOutcome::Failed { .. } => "failed",
        JobOutcome::Cancelled => "cancelled",
        JobOutcome::UsageLimited { .. } => "usage_limited",
    }
}

/// Cancels a detached attempt's provider call when `execute` is dropped before it finished.
struct CancelOnDrop {
    provider: Arc<dyn LlmProvider>,
    job: Option<JobId>,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(job) = self.job
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let provider = Arc::clone(&self.provider);
            runtime.spawn(async move {
                if let Err(error) = provider.cancel(job).await {
                    tracing::warn!(%job, %error, "cannot cancel a dropped attempt");
                }
            });
        }
    }
}

#[cfg(test)]
mod tests;
