//! `JobSpec`: everything one job needs; nothing travels through shared volumes except the session store.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::JobKind;
use crate::ids::{AnalysisId, CommitSha, JobId};
use crate::job::access::AgentAccess;

/// One agent job, self-contained (brief §5 item 1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobSpec {
    /// Job id.
    pub job_id: JobId,
    /// Attempt number, 1-based; incremented when a lease expires.
    pub attempt: u32,
    /// Job kind.
    pub kind: JobKind,
    /// The analysis the job belongs to.
    pub analysis_id: AnalysisId,
    /// Stable database key of the pull request; keys the shared agent-session archive.
    pub pull_request_id: i64,
    /// The repository to check out.
    pub repo: RepoCheckout,
    /// The full prompt, built by the server.
    pub prompt: String,
    /// The output JSON Schema (Codex strict subset).
    pub output_schema: Value,
    /// Model and reasoning effort from `select_model`.
    pub model: ModelChoice,
    /// Limits.
    pub limits: JobLimits,
    /// The LLM credential to run with; jobs are serialised per credential (ADR-003).
    pub credential: LlmCredentialRef,
    /// The pull request's agent session to resume, or none for the first job.
    pub session: Option<SessionRef>,
    /// Context to seed a new session when `session` is missing or unusable.
    pub context_pack: Option<String>,
    /// Names of the claudius skills exposed read-only to the agent.
    pub skills: Vec<String>,
    /// Agent credentials, filled at claim.
    pub access: AgentAccess,
}

/// Where the runner fetches the repository from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepoCheckout {
    /// Clone URL (any scheme; `file://` in development).
    pub clone_url: String,
    /// Base commit, fetched for diffs.
    pub base_sha: CommitSha,
    /// Head commit, checked out.
    pub head_sha: CommitSha,
}

/// Model and reasoning effort (plan §5.2, ADR-002).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelChoice {
    /// Provider model id, e.g. `gpt-6.1-sol`.
    pub model: String,
    /// Reasoning effort.
    pub reasoning_effort: ReasoningEffort,
}

/// Reasoning effort of the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    /// Low effort.
    Low,
    /// Medium effort.
    Medium,
    /// High effort (v1 default for every job kind).
    High,
}

/// Limits of one job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobLimits {
    /// Wall-clock cap in seconds; below the 1-hour installation token lifetime.
    pub max_runtime_secs: u32,
}

impl JobLimits {
    /// The v1 runtime cap: 45 minutes (ADR-005).
    pub const MAX_RUNTIME_SECS: u32 = 45 * 60;
}

impl Default for JobLimits {
    fn default() -> Self {
        Self {
            max_runtime_secs: Self::MAX_RUNTIME_SECS,
        }
    }
}

/// Reference to an LLM credential held by the runner; never the credential itself.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct LlmCredentialRef {
    /// Credential id in the runner's store.
    pub id: String,
    /// Credential kind.
    pub kind: CredentialKind,
}

/// Kind of LLM credential. v1: the owner's subscription only, no paid API (ADR-003).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    /// Codex CLI signed in with a ChatGPT subscription.
    CodexChatgpt,
}

/// The agent session of a pull request (ADR-004).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionRef {
    /// The pull request that owns the session.
    pub pull_request_id: i64,
    /// Provider session (Codex thread) id.
    pub thread_id: String,
}

/// Claim priority: interactive jobs before batch jobs, no pre-emption.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum JobPriority {
    /// Ask jobs.
    Interactive,
    /// Pipeline jobs.
    Batch,
}
