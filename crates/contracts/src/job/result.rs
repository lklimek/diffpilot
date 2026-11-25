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
    /// Frozen logical pass and independent method; absent outside assessment jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment: Option<Box<crate::assessment::AssessmentRunRef>>,
    /// Host-supplied review report, validated by server ingest; absent for other jobs.
    #[serde(default)]
    pub report: Option<Box<crate::agent::comment_report::CommentReport>>,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
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
    /// The worker could not be started or attached.
    WorkerUnavailable,
    /// Relay or frame loss; stop the container before any subsequent turn.
    WorkerLost,
    /// No usable credential exists for the payer.
    CredentialMissing,
    /// Terminal or quarantined credentials require explicit relink (gateway HTTP 403).
    CredentialRelink,
    /// Model metadata or the selected model is unavailable.
    ModelUnavailable,
    /// The gateway rejected a request's body or tool type.
    UpstreamRejected,
    /// The report is not a supported host-supplied 4.2 report.
    ReportVersionUnsupported,
    /// The provider rejected the strict output schema; no model repair or retry.
    InvalidJsonSchema,
    /// Required first-Ask compaction was skipped or not confirmed; the answer is discarded.
    AskCompactionUnconfirmed,
}

impl JobFailureKind {
    /// Whether another attempt may recover without changing user configuration or output contracts.
    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::WorkerUnavailable
                | Self::WorkerLost
                | Self::ProviderFailed
                | Self::Timeout
                | Self::Internal
        )
    }
}

/// The agent session after a job (ADR-004).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionReport {
    /// Last-context estimate, never a billable cumulative total.
    #[serde(default)]
    pub last_context: Option<Box<crate::runtime::LastContext>>,
    /// Confirmed first-Ask preflight for this seed/generation, when applicable.
    #[serde(default)]
    pub preflight: Option<Box<crate::runtime::AskPreflight>>,
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Usage {
    /// Input tokens, cached included.
    #[ts(type = "number")]
    pub input_tokens: u64,
    /// Cached input tokens.
    #[ts(type = "number")]
    pub cached_input_tokens: u64,
    /// Output tokens.
    #[ts(type = "number")]
    pub output_tokens: u64,
    /// Model requests of the call, counted from the provider's events (one per batch of tool calls, and the
    /// answer); null when not counted.
    #[serde(default)]
    #[ts(type = "number | null")]
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

/// Usage counters regressed or overflowed; do not substitute cumulative totals or silently undercharge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("provider usage counters are inconsistent")]
pub struct InvalidUsage;

impl Usage {
    /// Billable delta of one turn, excluding the inherited baseline of a fork.
    pub fn delta_from(self, baseline: Self) -> Result<Self, InvalidUsage> {
        let delta = Self {
            input_tokens: self
                .input_tokens
                .checked_sub(baseline.input_tokens)
                .ok_or(InvalidUsage)?,
            cached_input_tokens: self
                .cached_input_tokens
                .checked_sub(baseline.cached_input_tokens)
                .ok_or(InvalidUsage)?,
            output_tokens: self
                .output_tokens
                .checked_sub(baseline.output_tokens)
                .ok_or(InvalidUsage)?,
            requests: match (self.requests, baseline.requests) {
                (Some(end), Some(start)) => Some(end.checked_sub(start).ok_or(InvalidUsage)?),
                _ => None,
            },
        };
        if delta.cached_input_tokens > delta.input_tokens {
            return Err(InvalidUsage);
        }
        Ok(delta)
    }

    /// Sums independent turn deltas, including standalone compaction before an answer.
    pub fn checked_add(self, other: Self) -> Result<Self, InvalidUsage> {
        if self.cached_input_tokens > self.input_tokens
            || other.cached_input_tokens > other.input_tokens
        {
            return Err(InvalidUsage);
        }
        Ok(Self {
            input_tokens: self
                .input_tokens
                .checked_add(other.input_tokens)
                .ok_or(InvalidUsage)?,
            cached_input_tokens: self
                .cached_input_tokens
                .checked_add(other.cached_input_tokens)
                .ok_or(InvalidUsage)?,
            output_tokens: self
                .output_tokens
                .checked_add(other.output_tokens)
                .ok_or(InvalidUsage)?,
            requests: match (self.requests, other.requests) {
                (Some(a), Some(b)) => Some(a.checked_add(b).ok_or(InvalidUsage)?),
                _ => None,
            },
        })
    }
}
