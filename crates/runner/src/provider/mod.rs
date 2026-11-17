//! LLM providers behind the `LlmProvider` seam; only this module knows Codex (rule A5). A provider runs one
//! agent call in a prepared job directory, streams normalised events, restores and saves the pull request's
//! session files after every successful useful turn (ADR-004).

pub mod codex;
#[cfg(feature = "fake-provider")]
pub mod fake;

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use contracts::ids::JobId;
use contracts::job::access::AgentAccess;
use contracts::job::event::JobEvent;
use contracts::job::result::Usage;
use contracts::job::spec::{JobLimits, ModelChoice};
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
    /// The model turn ended with this final output (parsed JSON); archival follows. Sent at most once per call.
    OutputReady(Value),
}

/// One agent call, prepared by `execute`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderCall {
    /// The job.
    pub job_id: JobId,
    /// Fresh job directory; the agent's working directory, with the checkout at `job_dir/repo`. May be tmpfs.
    pub job_dir: PathBuf,
    /// Per-job `CODEX_HOME` (credential copy, config, session files) on the runner's persistent private
    /// volume (0700, never tmpfs or `/tmp`), so a rotated credential survives a crash before copy-back.
    pub codex_home: PathBuf,
    /// This pull request's session store (runner uid); only its files are copied into the call.
    pub session_dir: PathBuf,
    /// The full prompt.
    pub prompt: String,
    /// Output JSON Schema (Codex strict subset).
    pub output_schema: Value,
    /// Model and reasoning effort.
    pub model: ModelChoice,
    /// Limits of the useful turn.
    pub limits: JobLimits,
    /// Credentials the agent may hold (GitHub MCP, MemCan MCP).
    pub access: AgentAccess,
    /// claudius skills exposed read-only.
    pub skills: Vec<String>,
}

/// A finished agent call. Failed archival after the output does not fail the call: the outcome then has
/// `session_id: None` and the next job starts with a context pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderOutcome {
    /// The final structured output (identical to the one sent as [`ProviderEvent::OutputReady`]).
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
        resets_at: Option<DateTime<Utc>>,
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
