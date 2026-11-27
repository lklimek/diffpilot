//! LLM providers behind the `LlmProvider` seam; only this module knows Codex (rule A5). A provider runs one
//! container job and streams normalised events; the worker volume retains its threads.

pub mod appserver;
#[cfg(feature = "fake-provider")]
pub mod fake;

use contracts::ids::JobId;
use contracts::job::event::JobEvent;
use contracts::job::result::Usage;
use contracts::job::spec::JobSpec;
use contracts::time::Timestamp;
use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::mpsc;

/// Identity of a local provider attempt, derived from its claimed job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AttemptId {
    /// Job whose lease was granted.
    pub job: JobId,
    /// Lease attempt number assigned by the server.
    pub number: u32,
}

impl From<&JobSpec> for AttemptId {
    fn from(spec: &JobSpec) -> Self {
        Self {
            job: spec.job_id,
            number: spec.attempt,
        }
    }
}

/// Runs agent calls. `run` starts a new session, `resume` continues one; both send
/// [`ProviderEvent::OutputReady`] before reporting the persistent-thread checkpoint.
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

    /// Removes retired PR containers and their local keys before acknowledging retirement.
    fn retire<'a>(&'a self, _pull_requests: &'a [i64]) -> BoxFuture<'a, Result<(), ProviderError>> {
        Box::pin(async { Ok(()) })
    }

    /// Stops the running call of `attempt` (stop the worker turn); the call then ends with
    /// [`ProviderError::Cancelled`].
    fn cancel(&self, attempt: AttemptId) -> BoxFuture<'_, Result<(), ProviderError>>;

    /// Fences a dropped call's resources if they are still abandoned.
    fn reap_abandoned(&self, _attempt: AttemptId) -> BoxFuture<'_, Result<(), ProviderError>> {
        Box::pin(async { Ok(()) })
    }
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
    /// Self-contained container job; the provider owns checkout and thread routing.
    pub job_spec: Box<contracts::job::spec::JobSpec>,
}

/// A finished container turn and its usage/checkpoint metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderOutcome {
    /// Container checkpoint metadata, including acknowledged first-Ask preflight.
    pub runtime_session: Option<contracts::job::result::SessionReport>,
    /// Final JSON object or tagged flow file string (identical to [`ProviderEvent::OutputReady`]).
    pub output: Value,
    /// Provider session (Codex thread) id after the call; none for ephemeral jobs.
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
    /// Failed call with its latest acknowledged persistent-thread checkpoint.
    #[error("{error}")]
    Checkpoint {
        /// Safe underlying failure.
        #[source]
        error: Box<ProviderError>,
        /// Persist under the job attempt and generation fence, even when the answer failed.
        session: Box<contracts::job::result::SessionReport>,
    },
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
    /// The session is missing or unusable; the caller starts a new one with the context pack.
    #[error("the agent session could not be resumed")]
    SessionUnavailable,
    /// The provider's usage limit was reached.
    #[error("the Codex usage limit was reached")]
    UsageLimit {
        /// When the limit resets, if known.
        resets_at: Option<Timestamp>,
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
        let (error, usage, _) = self.into_details();
        (error, usage)
    }

    /// Separate failure, authoritative usage and acknowledged checkpoint for fenced persistence.
    pub fn into_details(
        self,
    ) -> (
        Self,
        Option<Usage>,
        Option<contracts::job::result::SessionReport>,
    ) {
        let mut error = self;
        let mut metered = None;
        let mut checkpoint = None;
        loop {
            match error {
                Self::Metered {
                    error: source,
                    usage,
                } => {
                    metered.get_or_insert(usage);
                    error = *source;
                }
                Self::Checkpoint {
                    error: source,
                    session,
                } => {
                    checkpoint.get_or_insert(*session);
                    error = *source;
                }
                error => return (error, metered, checkpoint),
            }
        }
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
