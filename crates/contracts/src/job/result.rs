//! `JobResult`: what the runner reports when a job attempt ends.

use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::ids::JobId;

/// The end of one job attempt; posting it twice for one attempt is idempotent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobResult {
    /// Job id.
    pub job_id: JobId,
    /// Attempt the result belongs to.
    pub attempt: u32,
    /// Outcome.
    pub outcome: JobOutcome,
    /// The agent session after the job, if a provider ran.
    pub session: Option<SessionReport>,
    /// Token usage of this job alone (delta of cumulative totals), if known.
    pub usage: Option<Usage>,
    /// Commit of the claudius checkout whose skills were exposed.
    pub skills_commit: Option<String>,
    /// Outcome of the job's last LLM call, for the LLM status in Settings; none if no call was made.
    pub llm_call: Option<LlmCallOutcome>,
}

/// Outcome of a job attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum JobOutcome {
    /// The agent produced its final output (not yet validated; the server validates it).
    Succeeded {
        /// JSON object for JSON jobs; a string containing the collected tagged file for flow jobs (ADR-014).
        output: Value,
    },
    /// The attempt failed.
    Failed {
        /// Failure kind.
        kind: JobFailureKind,
        /// Plain description without internal or secret details.
        message: String,
    },
    /// Cancelled on request.
    Cancelled,
    /// The provider's usage limit was reached; never paid around (ADR-003).
    UsageLimited {
        /// When the limit resets, if known.
        resets_at: Option<Timestamp>,
    },
}

/// Kind of a failed attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobFailureKind {
    /// A required agent skill is unavailable; administrator action is needed.
    Configuration,
    /// The provider failed (non-zero exit, `turn.failed`, rejected schema).
    ProviderFailed,
    /// The final message was not JSON.
    OutputNotJson,
    /// The LLM credential cannot be refreshed; the owner must sign in again.
    CredentialUnusable,
    /// Fetching or checking out the repository failed.
    CheckoutFailed,
    /// The runtime cap was reached.
    Timeout,
    /// A runner-side error.
    Internal,
}

/// The agent session after a job (ADR-004).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionReport {
    /// Provider session (Codex thread) id.
    pub thread_id: String,
    /// How the session was obtained.
    pub outcome: SessionOutcome,
}

/// How the job's agent session was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionOutcome {
    /// The analysis's session was resumed.
    Resumed,
    /// A new session was started (first job).
    Started,
    /// The session was missing or unusable; a new one was seeded with the context pack.
    StartedAfterLoss,
}

/// Token usage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Usage {
    /// Input tokens, cached included.
    pub input_tokens: u64,
    /// Cached input tokens.
    pub cached_input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Model requests of the call, counted from the provider's events (one per batch of tool calls, and the
    /// answer); null when not counted.
    #[serde(default)]
    pub requests: Option<u64>,
}

/// Outcome and time of an LLM call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LlmCallOutcome {
    /// When the call ended.
    pub at: Timestamp,
    /// Whether it worked.
    pub status: LlmCallStatus,
}

/// Whether the LLM worked on its most recent call (brief §4.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LlmCallStatus {
    /// The call succeeded.
    Working,
    /// The call failed.
    NotWorking {
        /// Plain error message.
        message: String,
    },
}
