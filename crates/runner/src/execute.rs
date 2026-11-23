//! The runner core `execute(JobSpec, EventSink) -> JobResult` (rule A4): prepares a fresh job directory with
//! the checkout at `job_dir/repo` (none for dashboard jobs, ADR-018) and a per-job `CODEX_HOME`, runs the
//! provider within the runtime cap, streams numbered and scrubbed events, maps the outcome to a `JobResult` and
//! removes both directories on every path (success, failure, and the job loop dropping the future).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use contracts::agent::{JobKind, verify_comments};
use contracts::ids::JobId;
use contracts::job::access::AgentAccess;
use contracts::job::event::{ActivityKind, JobEvent};
use contracts::job::result::{
    JobFailureKind, JobOutcome, JobResult, LlmCallOutcome, LlmCallStatus, SessionOutcome,
    SessionReport,
};
use contracts::job::spec::{JobLimits, JobSpec, JobTarget, LlmCredentialRef};
use contracts::time::Timestamp;
use tokio::sync::mpsc;

use crate::git;
use crate::phases::PhaseLog;
use crate::provider::{LlmProvider, ProviderCall, ProviderError, ProviderEvent, ProviderOutcome};
use crate::scrub::{SUMMARY_MAX_CHARS, Scrubber, clip};
use crate::sessions::SessionStore;

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

/// Activity line of the first turn of a new working session, which compacts the fork first and takes longer.
const PREPARING_FORK: &str = "Preparing the conversation (first question after an analysis)";
/// Events buffered between the provider and `execute`; small, as summaries are not clipped yet.
const EVENT_BUFFER: usize = 16;
/// Longest failure message in a result.
const MESSAGE_MAX_CHARS: usize = 300;

/// Runner directories, all on disk: `work` on a disposable volume emptied at start, the others on the runner's
/// persistent private volume (the agent's own `CODEX_HOME`s, under isolation, are on a disposable volume, ADR-013).
#[derive(Clone, Debug)]
pub struct RunnerDirs {
    /// Job directories (`<job>-<attempt>/`, checkout at `repo/`).
    pub work: PathBuf,
    /// Per-job `CODEX_HOME` directories (never tmpfs or `/tmp`, ADR-003).
    pub codex_homes: PathBuf,
    /// Bare mirrors by clone URL.
    pub mirrors: PathBuf,
    /// Agent-session store, one directory per pull request.
    pub sessions: PathBuf,
}

/// Runs claimed jobs with one provider and the runner's credential.
pub struct Executor {
    provider: Arc<dyn LlmProvider>,
    dirs: RunnerDirs,
    credential: LlmCredentialRef,
    /// Attempts run as their own task: dropping `execute` cancels the attempt instead of abandoning it.
    detached: bool,
    /// Held by a detached attempt until it has stopped, swept, copied back and cleaned up.
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Executor")
            .field("dirs", &self.dirs)
            .field("credential", &self.credential)
            .finish_non_exhaustive()
    }
}

impl Executor {
    /// An executor running `provider` in `dirs` with `credential`, the one LLM credential the runner holds.
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        dirs: RunnerDirs,
        credential: LlmCredentialRef,
    ) -> Self {
        Self {
            provider,
            dirs,
            credential,
            detached: false,
            gate: Arc::default(),
        }
    }

    /// Waits until no detached attempt is still finishing (stop, sweep, copy-back, cleanup); call before a
    /// terminal result is posted and before the next claim.
    pub async fn settled(&self) {
        drop(self.gate.lock().await);
    }

    /// Runs every attempt as its own task (process isolation, ADR-013): agent processes of another uid can
    /// only be stopped through the supervisor, so a dropped `execute` cancels the provider call and the
    /// attempt still stops, sweeps, copies back and cleans up before its directories are removed.
    pub fn with_detached_attempts(mut self) -> Self {
        self.detached = true;
        self
    }

    /// The LLM credential the runner holds.
    pub fn credential(&self) -> &LlmCredentialRef {
        &self.credential
    }

    /// The agent-session store.
    pub fn sessions(&self) -> SessionStore {
        SessionStore::new(self.dirs.sessions.clone())
    }

    /// The provider, for cancelling a running call.
    pub fn provider(&self) -> &Arc<dyn LlmProvider> {
        &self.provider
    }

    /// Runs `spec` to its result. Events and the ready output go to `sink`; a closed sink does not stop the
    /// job. Both job directories are removed when this returns or is dropped.
    pub async fn execute(&self, spec: JobSpec, sink: EventSink) -> JobResult {
        if !self.detached {
            return self.execute_here(spec, sink).await;
        }
        let attempt = Self {
            provider: Arc::clone(&self.provider),
            dirs: self.dirs.clone(),
            credential: self.credential.clone(),
            detached: true,
            gate: Arc::clone(&self.gate),
        };
        // Taken before the task starts, so `settled` never passes an attempt that has not begun.
        let held = Arc::clone(&self.gate).lock_owned().await;
        let mut cancel = CancelOnDrop {
            provider: Arc::clone(&self.provider),
            job: Some(spec.job_id),
        };
        let joined = tokio::spawn(async move {
            let result = attempt.execute_here(spec, sink).await;
            drop(held);
            result
        })
        .await;
        cancel.job = None;
        match joined {
            Ok(result) => result,
            Err(error) => std::panic::resume_unwind(error.into_panic()),
        }
    }

    async fn execute_here(&self, spec: JobSpec, sink: EventSink) -> JobResult {
        let key = format!("{}-{}", spec.job_id, spec.attempt);
        let job_dir = RemoveOnDrop(self.dirs.work.join(&key));
        let codex_home = RemoveOnDrop(self.dirs.codex_homes.join(&key));
        let phases = PhaseLog::default();
        let started = tokio::time::Instant::now();
        let result = self
            .attempt(&spec, &job_dir.0, &codex_home.0, &sink, &phases)
            .await;
        let cleanup = tokio::time::Instant::now();
        drop(job_dir);
        drop(codex_home);
        phases.since("cleanup", cleanup);
        phases.since("total", started);
        tracing::info!(job = %spec.job_id, attempt = spec.attempt, kind = ?spec.kind,
            phases = %phases.summary(), "job phase timings");
        tracing::info!(job = %spec.job_id, attempt = spec.attempt, outcome = outcome_name(&result.outcome), "job attempt finished");
        result
    }

    async fn attempt(
        &self,
        spec: &JobSpec,
        job_dir: &Path,
        codex_home: &Path,
        sink: &EventSink,
        phases: &PhaseLog,
    ) -> JobResult {
        // The runtime limit counts from the start of the attempt: a hanging fetch must not hold the lease.
        let deadline = tokio::time::Instant::now()
            + Duration::from_secs(u64::from(spec.limits.max_runtime_secs));
        if spec.credential != self.credential {
            return failed(
                spec,
                JobFailureKind::Internal,
                "the runner does not hold the LLM credential this job needs",
            );
        }
        if let Some(problem) = misfit(spec) {
            tracing::error!(job = %spec.job_id, problem, "refusing a job whose kind, target and access do not fit");
            return failed(spec, JobFailureKind::Internal, problem);
        }
        match tokio::time::timeout_at(deadline, self.prepare(spec, job_dir, codex_home, phases))
            .await
        {
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

        // Fail closed: user text must never run without a private save target. Keys are opaque,
        // server-derived hex, never path fragments supplied by a user.
        let valid_route = match (&spec.working_session, spec.kind) {
            (Some(route), contracts::agent::JobKind::Ask) => {
                route.user_key.len() == 64 && route.user_key.bytes().all(|b| b.is_ascii_hexdigit())
            }
            (None, kind) => kind != contracts::agent::JobKind::Ask,
            _ => false,
        };
        if !valid_route {
            return failed(
                spec,
                JobFailureKind::Internal,
                "invalid private session routing",
            );
        }
        let (analytic_dir, save_dir) = match spec.target.pull_request() {
            Some(target) => {
                let analytic_dir = self.sessions().dir(target.pull_request_id);
                let save_dir = spec.working_session.as_ref().map_or_else(
                    || analytic_dir.clone(),
                    |route| {
                        self.sessions()
                            .working_dir(target.pull_request_id, &route.user_key)
                    },
                );
                (analytic_dir, save_dir)
            }
            // Fresh and non-persistent: the provider's session files stay in the job directory, removed with it.
            None => (job_dir.join("session"), job_dir.join("session")),
        };
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
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let call = ProviderCall {
            job_id: spec.job_id,
            job_dir: job_dir.to_path_buf(),
            codex_home: codex_home.to_path_buf(),
            session_dir: save_dir.clone(),
            restore_dir: analytic_dir,
            prompt: spec.prompt.clone(),
            output_schema: spec.output_schema.clone(),
            flow_file: spec.flow_file.clone(),
            model: spec.model.clone(),
            limits: JobLimits {
                max_runtime_secs: u32::try_from(remaining.as_secs()).unwrap_or(u32::MAX),
            },
            access: spec.access.clone(),
            skills: spec.skills.clone(),
            answer_only: matches!(spec.target, JobTarget::Dashboard { .. }),
            compact_on_start: false,
            phases: phases.clone(),
        };
        let (tx, rx) = mpsc::channel(EVENT_BUFFER);
        let run = async {
            let session = self.run_session(spec, call, tx);
            tokio::pin!(session);
            match tokio::time::timeout_at(deadline, &mut session).await {
                Ok(done) => done,
                // Supervised agent processes are stopped through the provider, never by dropping the call
                // that owns them: the cap starts the stop sequence and the call still finishes it.
                Err(_) if self.detached => {
                    if let Err(error) = self.provider.cancel(spec.job_id).await {
                        tracing::warn!(job = %spec.job_id, %error, "cannot stop the agent at the runtime cap");
                    }
                    let (_, session_outcome) = session.await;
                    (Err(ProviderError::Timeout), session_outcome)
                }
                Err(_) => (Err(ProviderError::Timeout), SessionOutcome::Started),
            }
        };
        let ((outcome, session_outcome), ()) = tokio::join!(run, pump(rx, sink, &scrubber));
        let outcome = match outcome {
            Ok(mut done) if spec.target.pull_request().is_none() => {
                done.session_id = None;
                Ok(done)
            }
            Ok(mut done) if done.session_id.is_some() && !sink.secrets.is_empty() => {
                let scrub = tokio::time::Instant::now();
                let scrubbed =
                    crate::sessions::SessionStore::scrub_archive(&save_dir, &sink.secrets);
                phases.since("archive_scrub", scrub);
                if let Err(error) = scrubbed {
                    tracing::warn!(job = %spec.job_id, %error, "cannot scrub the stored agent session; it is discarded");
                    if let Err(error) = std::fs::remove_dir_all(&save_dir) {
                        tracing::warn!(job = %spec.job_id, %error, "cannot remove an unscrubbed agent session");
                    }
                    done.session_id = None;
                }
                Ok(done)
            }
            other => other,
        };
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

    /// Resumes the pull request's own session (ADR-004) or starts a new one; a lost session, or one of another
    /// pull request, starts a new session seeded with the context pack.
    async fn run_session(
        &self,
        spec: &JobSpec,
        mut call: ProviderCall,
        events: mpsc::Sender<ProviderEvent>,
    ) -> (Result<ProviderOutcome, ProviderError>, SessionOutcome) {
        let Some(target) = spec.target.pull_request() else {
            // A dashboard or `verify_comments` job always starts a fresh session: nothing to resume, seed or keep
            // (ADR-018, ADR-023).
            return (
                self.provider.run(&call, events).await,
                SessionOutcome::Started,
            );
        };
        if let Some(route) = &spec.working_session {
            if let Some(thread) = &route.thread_id {
                call.restore_dir = call.session_dir.clone();
                match self.provider.resume(thread, &call, events.clone()).await {
                    Err(ProviderError::SessionUnavailable) => {}
                    other => return (other, SessionOutcome::Resumed),
                }
            }
            // Lazy fork, stale fork, or lost working archive: seed the fork with only the
            // asker's earlier answers, fenced as data, even when the analytic restore succeeds.
            call.restore_dir = self.sessions().dir(target.pull_request_id);
            if let Some(pack) = &spec.context_pack {
                call.prompt = format!("{pack}\n\n{}", spec.prompt);
            }
        }
        if let Some(session) = &spec.session {
            if session.pull_request_id == target.pull_request_id {
                // A new working fork is compacted once, in its first turn (ADR-004 addendum 2026-11-18).
                call.compact_on_start = spec.working_session.is_some();
                if call.compact_on_start {
                    let _ = events
                        .send(ProviderEvent::Activity(JobEvent {
                            seq: 0,
                            at: Timestamp::now(),
                            activity: ActivityKind::Reasoning,
                            summary: PREPARING_FORK.to_owned(),
                        }))
                        .await;
                }
                let mut resumed = self
                    .provider
                    .resume(&session.thread_id, &call, events.clone())
                    .await;
                if call.compact_on_start && matches!(resumed, Err(ProviderError::Failed { .. })) {
                    tracing::warn!(job = %spec.job_id, "the turn compacting the forked session failed; answering from the uncompacted fork; the failed turn's tokens are not counted (no usage is reported for a failed turn)");
                    call.compact_on_start = false;
                    resumed = self
                        .provider
                        .resume(&session.thread_id, &call, events.clone())
                        .await;
                }
                match resumed {
                    Err(ProviderError::SessionUnavailable) => {
                        tracing::info!(job = %spec.job_id, "the pull request's agent session is unavailable; starting a new one with the context pack");
                    }
                    other => return (other, SessionOutcome::Resumed),
                }
            } else {
                tracing::warn!(job = %spec.job_id, "the job names another pull request's session; starting a new one");
            }
        }
        if let Some(pack) = &spec.context_pack {
            call.prompt = format!("{pack}\n\n{}", spec.prompt);
        }
        let outcome = match spec.session {
            Some(_) => SessionOutcome::StartedAfterLoss,
            None => SessionOutcome::Started,
        };
        // A new session holds only the context pack: nothing to compact.
        call.compact_on_start = false;
        (self.provider.run(&call, events).await, outcome)
    }

    /// Creates the job directories and checks the repository out (dashboard jobs have none); the error is the job's
    /// failure.
    async fn prepare(
        &self,
        spec: &JobSpec,
        job_dir: &Path,
        codex_home: &Path,
        phases: &PhaseLog,
    ) -> Result<(), (JobFailureKind, &'static str)> {
        let dirs = tokio::time::Instant::now();
        let prepared = prepare_dirs(&[job_dir, codex_home]).await;
        phases.since("job_dirs", dirs);
        if let Err(error) = prepared {
            tracing::error!(job = %spec.job_id, %error, "cannot create the job directories; check SR_RUNNER_STATE_DIR and SR_RUNNER_WORK_DIR");
            return Err((
                JobFailureKind::Internal,
                "the runner could not prepare the job directory",
            ));
        }
        if let Some(file) = &spec.flow_file
            && (spec.kind != contracts::agent::JobKind::AnalyseFlow
                || crate::provider::output_file::prepare(job_dir, file).is_err())
        {
            return Err((
                JobFailureKind::Internal,
                "the flow output directory could not be prepared",
            ));
        }
        let Some(repo) = spec.target.checkout() else {
            return Ok(());
        };
        let mirror = git::mirror_path(&self.dirs.mirrors, &repo.clone_url);
        let commits = [repo.head_sha.0.as_str(), repo.base_sha.0.as_str()];
        if let Err(error) = git::checkout(
            &mirror,
            &repo.clone_url,
            &commits,
            &job_dir.join("repo"),
            phases,
        )
        .await
        {
            tracing::warn!(job = %spec.job_id, %error, "repository fetch or checkout failed");
            return Err((
                JobFailureKind::CheckoutFailed,
                "the repository could not be fetched or checked out",
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
    (comments && (holds_session || !spec.skills.is_empty() || spec.access.memcan.is_some()))
        .then_some("a verify_comments job must not carry MemCan access, a session or skills")
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
            job_id: spec.job_id,
            attempt: spec.attempt,
            outcome: JobOutcome::Succeeded {
                output: scrubber.value(outcome.output),
            },
            session: outcome.session_id.map(|thread_id| SessionReport {
                thread_id,
                outcome: session_outcome,
            }),
            usage: Some(outcome.usage),
            skills_commit: outcome.skills_commit,
            llm_call: Some(LlmCallOutcome {
                at,
                status: LlmCallStatus::Working,
            }),
        },
        Err(error) => {
            let message = clip(&scrubber.text(&error.to_string()), MESSAGE_MAX_CHARS);
            let outcome = match error {
                ProviderError::Cancelled => JobOutcome::Cancelled,
                ProviderError::UsageLimit { resets_at } => JobOutcome::UsageLimited { resets_at },
                ProviderError::CredentialUnusable => {
                    failure(JobFailureKind::CredentialUnusable, &message)
                }
                ProviderError::OutputNotJson => failure(JobFailureKind::OutputNotJson, &message),
                ProviderError::Timeout => failure(JobFailureKind::Timeout, &message),
                ProviderError::Configuration(_) => failure(
                    JobFailureKind::Configuration,
                    &format!("The runner is misconfigured: {message}."),
                ),
                ProviderError::Io(_) => failure(JobFailureKind::Internal, &message),
                ProviderError::SessionUnavailable
                | ProviderError::SchemaRejected { .. }
                | ProviderError::Failed { .. } => failure(JobFailureKind::ProviderFailed, &message),
            };
            // An interrupted call says nothing about whether the LLM works.
            let llm_call = (!matches!(outcome, JobOutcome::Cancelled)).then_some(LlmCallOutcome {
                at,
                status: LlmCallStatus::NotWorking { message },
            });
            JobResult {
                job_id: spec.job_id,
                attempt: spec.attempt,
                outcome,
                session: None,
                usage: None,
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

/// Fresh private (0700) directories; leftovers of an earlier run of the same attempt are removed first.
async fn prepare_dirs(dirs: &[&Path]) -> io::Result<()> {
    for dir in dirs {
        match tokio::fs::remove_dir_all(dir).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if let Some(parent) = dir.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::DirBuilder::new().mode(0o700).create(dir).await?;
    }
    Ok(())
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

/// Removes a directory tree when dropped (also when the job future is dropped by the job loop).
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        // std's remove_dir_all does not follow symlinks (GHSA-r9cc-f5pr-p3j2 fixed in 1.58.1).
        match std::fs::remove_dir_all(&self.0) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(dir = %self.0.display(), %error, "cannot remove a job directory; remove it by hand")
            }
        }
    }
}

#[cfg(test)]
mod tests;
