//! The runner core `execute(JobSpec, EventSink) -> JobResult` (rule A4): prepares a bare
//! mirror, delegates checkout and persistent threads to the worker provider, streams scrubbed events and
//! maps the outcome to a `JobResult`. Cancellation settles before another claim.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
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
use tokio::sync::{mpsc, watch};
use tokio::task::AbortHandle;

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
#[derive(Clone)]
pub struct Executor {
    provider: Arc<dyn LlmProvider>,
    dirs: RunnerDirs,
    mirrors: Arc<Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>>,
    active: Arc<Mutex<HashMap<JobId, ActiveAttempt>>>,
}

struct ActiveAttempt {
    abort: AbortHandle,
    cancel: watch::Sender<bool>,
    finished: watch::Receiver<()>,
}

struct AttemptRegistration {
    executor: Executor,
    job: JobId,
    _finished: watch::Sender<()>,
}

impl Drop for AttemptRegistration {
    fn drop(&mut self) {
        self.executor.active().remove(&self.job);
    }
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
            mirrors: Arc::default(),
            active: Arc::default(),
        }
    }

    fn active(&self) -> MutexGuard<'_, HashMap<JobId, ActiveAttempt>> {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Waits for the retained executions after the job monitors have stopped.
    pub async fn settled(&self) {
        let completions: Vec<_> = self
            .active()
            .values()
            .map(|active| active.finished.clone())
            .collect();
        for mut done in completions {
            let _ = done.changed().await;
        }
    }

    pub(crate) async fn settled_attempt(&self, job: JobId) {
        let completion = self
            .active()
            .get(&job)
            .map(|active| active.finished.clone());
        if let Some(mut done) = completion {
            let _ = done.changed().await;
        }
    }

    pub(crate) fn cancel_attempt(&self, job: JobId) {
        if let Some(active) = self.active().get(&job) {
            active.cancel.send_replace(true);
        }
    }

    pub(crate) fn abort_attempt(&self, job: JobId) {
        if let Some(active) = self.active().get(&job) {
            active.abort.abort();
        }
    }

    pub(crate) fn abort_all(&self) -> Vec<JobId> {
        self.active()
            .iter()
            .map(|(job, active)| {
                active.abort.abort();
                *job
            })
            .collect()
    }

    /// The provider, for runner housekeeping and fencing stopped workers.
    pub fn provider(&self) -> &Arc<dyn LlmProvider> {
        &self.provider
    }

    /// Runs a job to a result, with ordered events and ready output on `sink`.
    pub async fn execute(&self, spec: JobSpec, sink: EventSink) -> JobResult {
        self.start(spec, sink).await
    }

    pub(crate) fn start(
        &self,
        spec: JobSpec,
        sink: EventSink,
    ) -> impl Future<Output = JobResult> + use<> {
        let job = spec.job_id;
        let mut fallback = failed(
            &spec,
            JobFailureKind::Internal,
            "the runner failed while running the job",
        );
        let mut active = self.active();
        let execution = match active.entry(job) {
            std::collections::hash_map::Entry::Occupied(_) => {
                fallback.outcome = failure(
                    JobFailureKind::WorkerUnavailable,
                    "the job already has a local attempt",
                );
                None
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let (cancel, cancelled) = watch::channel(false);
                let (finish, finished) = watch::channel(());
                let registration = AttemptRegistration {
                    executor: self.clone(),
                    job,
                    _finished: finish,
                };
                let task = tokio::spawn(async move {
                    let held = registration;
                    held.executor.execute_here(spec, sink, cancelled).await
                });
                entry.insert(ActiveAttempt {
                    abort: task.abort_handle(),
                    cancel: cancel.clone(),
                    finished,
                });
                Some((task, CancelOnDrop(cancel)))
            }
        };
        async move {
            let Some((task, _cancel)) = execution else {
                return fallback;
            };
            match task.await {
                Ok(result) => result,
                Err(error) if error.is_cancelled() => {
                    fallback.outcome = JobOutcome::Cancelled;
                    fallback
                }
                Err(error) => {
                    tracing::error!(%job, %error, "the job task panicked");
                    fallback
                }
            }
        }
    }

    async fn execute_here(
        &self,
        spec: JobSpec,
        sink: EventSink,
        mut cancelled: watch::Receiver<bool>,
    ) -> JobResult {
        let phases = PhaseLog::default();
        let started = tokio::time::Instant::now();
        let result = self.attempt(&spec, &sink, &phases, &mut cancelled).await;
        phases.since("total", started);
        tracing::info!(job = %spec.job_id, attempt = spec.attempt, kind = ?spec.kind,
            phases = %phases.summary(), "job phase timings");
        tracing::info!(job = %spec.job_id, attempt = spec.attempt, outcome = outcome_name(&result.outcome), "job attempt finished");
        result
    }

    async fn attempt(
        &self,
        spec: &JobSpec,
        sink: &EventSink,
        phases: &PhaseLog,
        cancelled: &mut watch::Receiver<bool>,
    ) -> JobResult {
        // The runtime limit counts from the start of the attempt: a hanging fetch must not hold the lease.
        let deadline = tokio::time::Instant::now()
            + Duration::from_secs(u64::from(spec.limits.max_runtime_secs));
        if let Some(problem) = misfit(spec) {
            tracing::error!(job = %spec.job_id, problem, "refusing a job whose kind, target and access do not fit");
            return failed(spec, JobFailureKind::Internal, problem);
        }
        let prepared = tokio::select! {
            biased;
            () = wait_for_cancel(cancelled) => {
                return to_result(spec, Err(ProviderError::Cancelled), SessionOutcome::Started, &sink.secrets);
            }
            prepared = tokio::time::timeout_at(deadline, self.prepare(spec, phases)) => prepared,
        };
        match prepared {
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
            let mut error = tokio::select! {
                biased;
                done = &mut session => return done,
                () = wait_for_cancel(cancelled) => ProviderError::Cancelled,
                () = tokio::time::sleep_until(deadline) => ProviderError::Timeout,
            };
            let (cancelled, settled) =
                tokio::join!(self.provider.cancel(spec.job_id), &mut session);
            if let Err(error) = cancelled {
                tracing::warn!(job = %spec.job_id, %error, "cannot stop the worker after cancellation");
            }
            let (usage, checkpoint) = match settled {
                Ok(done) => (Some(done.usage), done.runtime_session),
                Err(error) => {
                    let (_, usage, checkpoint) = error.into_details();
                    (usage, checkpoint)
                }
            };
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
        let lock = self.mirror_lock(&mirror);
        let _guard = lock.lock().await;
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

    fn mirror_lock(&self, mirror: &Path) -> Arc<tokio::sync::Mutex<()>> {
        let mut mirrors = self
            .mirrors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        mirrors.retain(|_, lock| lock.strong_count() != 0);
        if let Some(lock) = mirrors.get(mirror).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::default();
        mirrors.insert(mirror.to_owned(), Arc::downgrade(&lock));
        lock
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

async fn wait_for_cancel(cancelled: &mut watch::Receiver<bool>) {
    let _ = cancelled.wait_for(|value| *value).await;
}

/// Retained execution cleans up when its monitor is dropped.
struct CancelOnDrop(watch::Sender<bool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

#[cfg(test)]
mod tests;
