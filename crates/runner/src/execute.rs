//! The runner core `execute(JobSpec, EventSink) -> JobResult` (rule A4): prepares a fresh job directory with
//! the checkout at `job_dir/repo` and a per-job `CODEX_HOME`, runs the provider within the runtime cap,
//! streams numbered and scrubbed events, maps the outcome to a `JobResult` and removes both directories on
//! every path (success, failure, and the job loop dropping the future).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use contracts::job::result::{
    JobFailureKind, JobOutcome, JobResult, LlmCallOutcome, LlmCallStatus, SessionOutcome,
    SessionReport,
};
use contracts::job::spec::{JobSpec, LlmCredentialRef};
use tokio::sync::mpsc;

use crate::git;
use crate::provider::{LlmProvider, ProviderCall, ProviderError, ProviderEvent, ProviderOutcome};
use crate::scrub::{SUMMARY_MAX_CHARS, Scrubber, clip};

/// Where `execute` sends a job's normalised events and its ready output, in order.
pub type EventSink = mpsc::Sender<ProviderEvent>;

/// Events buffered between the provider and the sink.
const EVENT_BUFFER: usize = 256;
/// Longest failure message in a result.
const MESSAGE_MAX_CHARS: usize = 300;

/// Runner directories. `work` may be tmpfs; the others live on the runner's persistent private volume.
#[derive(Clone, Debug)]
pub struct RunnerDirs {
    /// Job directories (`<job>-<attempt>/`, checkout at `repo/`).
    pub work: PathBuf,
    /// Per-job `CODEX_HOME` directories (never tmpfs or `/tmp`, ADR-003).
    pub codex_homes: PathBuf,
    /// Bare mirrors by clone URL.
    pub mirrors: PathBuf,
    /// Agent-session store per analysis (used from WP3-RUN).
    pub sessions: PathBuf,
}

/// Runs claimed jobs with one provider and the runner's credential.
pub struct Executor {
    provider: Arc<dyn LlmProvider>,
    dirs: RunnerDirs,
    credential: LlmCredentialRef,
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
        }
    }

    /// The LLM credential the runner holds.
    pub fn credential(&self) -> &LlmCredentialRef {
        &self.credential
    }

    /// The provider, for cancelling a running call.
    pub fn provider(&self) -> &Arc<dyn LlmProvider> {
        &self.provider
    }

    /// Runs `spec` to its result. Events and the ready output go to `sink`; a closed sink does not stop the
    /// job. Both job directories are removed when this returns or is dropped.
    pub async fn execute(&self, spec: JobSpec, sink: EventSink) -> JobResult {
        let key = format!("{}-{}", spec.job_id, spec.attempt);
        let job_dir = RemoveOnDrop(self.dirs.work.join(&key));
        let codex_home = RemoveOnDrop(self.dirs.codex_homes.join(&key));
        let result = self.attempt(&spec, &job_dir.0, &codex_home.0, &sink).await;
        drop(job_dir);
        drop(codex_home);
        tracing::info!(job = %spec.job_id, attempt = spec.attempt, outcome = outcome_name(&result.outcome), "job attempt finished");
        result
    }

    async fn attempt(
        &self,
        spec: &JobSpec,
        job_dir: &Path,
        codex_home: &Path,
        sink: &EventSink,
    ) -> JobResult {
        if spec.credential != self.credential {
            return failed(
                spec,
                JobFailureKind::Internal,
                "the runner does not hold the LLM credential this job needs",
            );
        }
        if let Err(error) = prepare_dirs(&[job_dir, codex_home]).await {
            tracing::error!(job = %spec.job_id, %error, "cannot create the job directories; check SR_RUNNER_STATE_DIR and SR_RUNNER_WORK_DIR");
            return failed(
                spec,
                JobFailureKind::Internal,
                "the runner could not prepare the job directory",
            );
        }
        let mirror = git::mirror_path(&self.dirs.mirrors, &spec.repo.clone_url);
        let commits = [spec.repo.head_sha.0.as_str(), spec.repo.base_sha.0.as_str()];
        if let Err(error) = git::checkout(
            &mirror,
            &spec.repo.clone_url,
            &commits,
            &job_dir.join("repo"),
        )
        .await
        {
            tracing::warn!(job = %spec.job_id, %error, "repository fetch or checkout failed");
            return failed(
                spec,
                JobFailureKind::CheckoutFailed,
                "the repository could not be fetched or checked out",
            );
        }

        let scrubber = Scrubber::new(
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
        );
        // Stage 2 starts a session for every job: a requested session counts as lost and the context pack
        // seeds the new one (ADR-004). Resume arrives with WP3-RUN.
        let (prompt, session_outcome) = match (&spec.session, &spec.context_pack) {
            (Some(_), Some(pack)) => (
                format!("{pack}\n\n{}", spec.prompt),
                SessionOutcome::StartedAfterLoss,
            ),
            (Some(_), None) => (spec.prompt.clone(), SessionOutcome::StartedAfterLoss),
            (None, _) => (spec.prompt.clone(), SessionOutcome::Started),
        };
        let call = ProviderCall {
            job_id: spec.job_id,
            job_dir: job_dir.to_path_buf(),
            codex_home: codex_home.to_path_buf(),
            session_dir: self.dirs.sessions.join(spec.analysis_id.to_string()),
            prompt,
            output_schema: spec.output_schema.clone(),
            model: spec.model.clone(),
            limits: spec.limits,
            access: spec.access.clone(),
            skills: spec.skills.clone(),
        };
        let cap = Duration::from_secs(u64::from(spec.limits.max_runtime_secs));
        let (tx, rx) = mpsc::channel(EVENT_BUFFER);
        let run = async {
            tokio::time::timeout(cap, self.provider.run(&call, tx))
                .await
                .unwrap_or(Err(ProviderError::Timeout))
        };
        let (outcome, ()) = tokio::join!(run, pump(rx, sink, &scrubber));
        to_result(spec, outcome, session_outcome, &scrubber)
    }
}

/// Numbers activities from 1, scrubs and clips them, forwards at most one ready output.
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
        let _ = sink.send(event).await;
    }
}

fn to_result(
    spec: &JobSpec,
    outcome: Result<ProviderOutcome, ProviderError>,
    session_outcome: SessionOutcome,
    scrubber: &Scrubber,
) -> JobResult {
    let at = Utc::now();
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
