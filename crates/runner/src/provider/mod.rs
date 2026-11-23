//! LLM providers behind the `LlmProvider` seam; only this module knows Codex (rule A5). A provider runs one
//! agent call in a prepared job directory, streams normalised events, restores and saves the pull request's
//! session files after every successful useful turn (ADR-004).

pub mod appserver;
pub mod codex;
#[cfg(feature = "fake-provider")]
pub mod fake;

use std::path::PathBuf;

use contracts::ids::JobId;
use contracts::job::access::AgentAccess;
use contracts::job::event::JobEvent;
use contracts::job::result::Usage;
use contracts::job::spec::{JobLimits, ModelChoice};
use contracts::time::Timestamp;
use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::mpsc;

/// Runs agent calls. `run` starts a new session, `resume` continues one; both send
/// [`ProviderEvent::OutputReady`] before archiving the session.
pub trait LlmProvider: Send + Sync {
    /// Starts a new session for `call` (first job, or a lost session seeded with the context pack in the prompt).
    fn run<'a>(
        &'a self,
        call: &'a ProviderCall,
        events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>>;

    /// Resumes the session `session_id` for `call`; [`ProviderError::SessionUnavailable`] if it cannot.
    fn resume<'a>(
        &'a self,
        session_id: &'a str,
        call: &'a ProviderCall,
        events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>>;

    /// Stops the running call of `job` (SIGTERM to the agent process); the call then ends with
    /// [`ProviderError::Cancelled`].
    fn cancel(&self, job: JobId) -> BoxFuture<'_, Result<(), ProviderError>>;
}

/// What a provider reports while a call runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderEvent {
    /// Normalised agent activity.
    Activity(JobEvent),
    /// Final JSON object or tagged flow file string; archival follows. Sent at most once per call.
    OutputReady(Value),
}

/// One agent call, prepared by `execute`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderCall {
    /// Self-contained container runtime input; absent for the legacy provider.
    pub job_spec: Option<Box<contracts::job::spec::JobSpec>>,
    /// The job.
    pub job_id: JobId,
    /// Fresh job directory; the agent's working directory, with the checkout at `job_dir/repo`.
    pub job_dir: PathBuf,
    /// Per-job `CODEX_HOME` (credential copy, config, session files) on the runner's persistent private
    /// volume (0700, never tmpfs or `/tmp`), so a rotated credential survives a crash before copy-back.
    pub codex_home: PathBuf,
    /// Destination archive (runner uid): analytic or the asking user's working archive.
    pub session_dir: PathBuf,
    /// Archive to restore; may be the analytic archive when saving a private Ask fork.
    pub restore_dir: PathBuf,
    /// The full prompt.
    pub prompt: String,
    /// Output JSON Schema for JSON jobs; ignored when `flow_file` is present.
    pub output_schema: Value,
    /// File output routing and checker context (flow jobs only).
    pub flow_file: Option<contracts::job::spec::FlowFile>,
    /// Model and reasoning effort.
    pub model: ModelChoice,
    /// Limits of the useful turn.
    pub limits: JobLimits,
    /// Credentials the agent may hold (GitHub MCP, MemCan MCP).
    pub access: AgentAccess,
    /// claudius skills exposed read-only.
    pub skills: Vec<String>,
    /// The agent only reads its prompt and answers: no command, web or image tool, read-only sandbox (dashboard
    /// jobs, whose prompt holds private titles and untrusted text; ADR-018).
    pub answer_only: bool,
    /// Compact the restored session before this turn's answer (the first turn of an Ask working session forked
    /// from the analytic one; ADR-004 addendum 2026-11-18).
    pub compact_on_start: bool,
    /// The job attempt's phase log, written by the provider too.
    pub phases: crate::phases::PhaseLog,
}

/// A finished agent call. Failed archival after the output does not fail the call: the outcome then has
/// `session_id: None` and the next job starts with a context pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderOutcome {
    /// Container checkpoint metadata, including acknowledged first-Ask preflight.
    pub runtime_session: Option<contracts::job::result::SessionReport>,
    /// Final JSON object or tagged flow file string (identical to [`ProviderEvent::OutputReady`]).
    pub output: Value,
    /// Provider session (Codex thread) id after the call; none when archival failed.
    pub session_id: Option<String>,
    /// Completed session turns, when the provider can determine them.
    pub session_turns: Option<u64>,
    /// Usage of this call alone.
    pub usage: Usage,
    /// Commit of the claudius checkout whose skills were exposed.
    pub skills_commit: Option<String>,
}

/// An agent call failed; `Display` is a plain message safe to report as the LLM status.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// Failed call with the gateway's response-ID usage, including any completed compaction.
    #[error("{error}")]
    Metered {
        /// Safe provider failure, without upstream payloads.
        #[source]
        error: Box<ProviderError>,
        /// Actual usage of this key, never a payer-wide or time-window sum.
        usage: Usage,
    },
    /// Safe container/gateway failure with a shared retry policy.
    #[error("container agent failed: {kind:?}")]
    Runtime {
        /// Provider-neutral failure category; no raw worker error leaves the boundary.
        kind: contracts::job::result::JobFailureKind,
    },
    /// A skill required by the prompt cannot be provided.
    #[error("{0}")]
    Configuration(#[from] crate::skills::SkillError),
    /// The session is missing or unusable; the caller starts a new one with the context pack.
    #[error("the agent session could not be resumed")]
    SessionUnavailable,
    /// The provider's usage limit was reached.
    #[error("the Codex usage limit was reached")]
    UsageLimit {
        /// When the limit resets, if known.
        resets_at: Option<Timestamp>,
    },
    /// The credential can no longer be refreshed; the owner must sign in again.
    #[error("the Codex login is no longer valid; sign in again in the runner container")]
    CredentialUnusable,
    /// The provider rejected the output schema.
    #[error("the output schema was rejected: {message}")]
    SchemaRejected {
        /// Plain provider message.
        message: String,
    },
    /// The final message was not JSON.
    #[error("the agent's final answer was not JSON")]
    OutputNotJson,
    /// Cancelled on request.
    #[error("the agent call was cancelled")]
    Cancelled,
    /// The runtime cap was reached.
    #[error("the agent call took longer than the job limit")]
    Timeout,
    /// The provider failed otherwise.
    #[error("the agent call failed: {message}")]
    Failed {
        /// Plain provider message, secrets scrubbed.
        message: String,
    },
    /// Local I/O (job directory, session store) failed.
    #[error("the runner could not prepare or clean up the agent call")]
    Io(#[from] std::io::Error),
}

impl ProviderError {
    /// Separate a call's failure from its optional authoritative usage for JobResult persistence.
    pub fn into_parts(self) -> (Self, Option<Usage>) {
        let mut error = self;
        let mut metered = None;
        while let Self::Metered {
            error: source,
            usage,
        } = error
        {
            metered.get_or_insert(usage);
            error = *source;
        }
        (error, metered)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn provider_is_usable_as_a_trait_object() {
        let provider: Option<Arc<dyn LlmProvider>> = None;
        assert!(provider.is_none());
    }

    #[test]
    fn usage_limit_message_is_plain() {
        let error = ProviderError::UsageLimit { resets_at: None };
        assert_eq!(error.to_string(), "the Codex usage limit was reached");
    }
}

pub(crate) mod output_file;
