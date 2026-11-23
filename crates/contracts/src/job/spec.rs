//! Self-contained job input; container identity and runtime paths follow the shared contracts.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::JobKind;
use crate::ids::{AnalysisId, CommitSha, JobId, PassId, RefreshId};
use crate::job::access::AgentAccess;

/// One agent job, self-contained (brief §5 item 1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobSpec {
    /// Frozen logical pass and independent method; absent outside assessment jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment: Option<crate::assessment::AssessmentRunRef>,
    /// Job id.
    pub job_id: JobId,
    /// Attempt number, 1-based; incremented when a lease expires.
    pub attempt: u32,
    /// Job kind.
    pub kind: JobKind,
    /// What the job works on (ADR-018).
    pub target: JobTarget,
    /// The full prompt, built by the server.
    pub prompt: String,
    /// The output JSON Schema (Codex strict subset), or null when `flow_file` is present.
    pub output_schema: Value,
    /// Flow-only file output and no-secret checker context; absent for JSON jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_file: Option<FlowFile>,
    /// Host review files for a container verify pass; absent for other jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_data: Option<crate::agent::pr_data::PrData>,
    /// Model and reasoning effort from `select_model`.
    pub model: ModelChoice,
    /// Limits.
    pub limits: JobLimits,
    /// User paying for the turn; container identity follows from kind, PR and payer.
    pub payer: crate::ids::UserId,
    /// The pull request's analytic session to resume or fork, or none before its first checkpoint.
    pub session: Option<SessionRef>,
    /// Ask-only archive routing. Absent for analytic jobs; required for Ask.
    #[serde(default)]
    pub working_session: Option<WorkingSession>,
    /// Context to seed a new session when `session` is missing or unusable.
    pub context_pack: Option<String>,
    /// Names of the claudius skills exposed read-only to the agent.
    pub skills: Vec<String>,
    /// Agent credentials, filled at claim.
    pub access: AgentAccess,
}

/// What a job works on (ADR-018); exhaustive matches mark every pull-request-only site.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ts_rs::TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobTarget {
    /// A job of an analysis: checkout, the pull request's sessions, GitHub and MemCan access.
    PullRequest(PullRequestTarget),
    /// A dashboard ranking job: no checkout, no session, no GitHub or MemCan access.
    Dashboard {
        /// The refresh whose items are judged.
        refresh_id: RefreshId,
    },
    /// A `verify_comments` job (ADR-023): checkout and the read-only GitHub token, no session, no MemCan.
    PullRequestComments(PullRequestCommentsTarget),
    /// Private comment Ask without an analysis or assessment pass; payer owns its Verify thread.
    PullRequestAsk(PullRequestAskTarget),
}

impl JobTarget {
    /// The pull request of an analysis job; `None` for other jobs.
    pub fn pull_request(&self) -> Option<&PullRequestTarget> {
        match self {
            Self::PullRequest(target) => Some(target),
            Self::Dashboard { .. } | Self::PullRequestComments(_) | Self::PullRequestAsk(_) => None,
        }
    }

    /// The repository a job checks out; `None` for dashboard jobs.
    pub fn checkout(&self) -> Option<&RepoCheckout> {
        match self {
            Self::PullRequest(target) => Some(&target.repo),
            Self::PullRequestComments(target) => Some(&target.repo),
            Self::PullRequestAsk(target) => Some(&target.repo),
            Self::Dashboard { .. } => None,
        }
    }

    /// The analysis of an analysis job; `None` for dashboard jobs.
    pub fn analysis_id(&self) -> Option<AnalysisId> {
        self.pull_request().map(|target| target.analysis_id)
    }
}

/// The pull request an analysis job works on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ts_rs::TS)]
pub struct PullRequestTarget {
    /// The analysis the job belongs to.
    pub analysis_id: AnalysisId,
    /// Stable database key of the pull request; root of its analytic and private working archives.
    #[ts(type = "number")]
    pub pull_request_id: i64,
    /// The repository to check out.
    pub repo: RepoCheckout,
}

/// What a `verify_comments` job works on (ADR-023, plan §4.2): the head checkout and the read-only PR-scoped token,
/// no session, no MemCan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ts_rs::TS)]
pub struct PullRequestCommentsTarget {
    /// Stable database key of the pull request.
    #[ts(type = "number")]
    pub pull_request_id: i64,
    /// The repository to check out (head).
    pub repo: RepoCheckout,
    /// The assessment pass.
    pub pass_id: PassId,
}

/// Private comment Ask, scoped by `JobSpec::payer`; no assessment quota or analytic archive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct PullRequestAskTarget {
    /// Stable database key of the pull request.
    #[ts(type = "number")]
    pub pull_request_id: i64,
    /// Frozen checkout for the question and its evidence.
    pub repo: RepoCheckout,
}

/// Where the runner fetches the repository from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ts_rs::TS)]
pub struct RepoCheckout {
    /// Clone URL (any scheme; `file://` in development).
    pub clone_url: String,
    /// Base commit, fetched for diffs.
    pub base_sha: CommitSha,
    /// Head commit, checked out.
    pub head_sha: CommitSha,
}

/// Model and reasoning effort (plan §5.2, ADR-002).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ts_rs::TS)]
pub struct ModelChoice {
    /// Provider model id, e.g. `gpt-6.1-sol`.
    pub model: String,
    /// Reasoning effort.
    pub reasoning_effort: ReasoningEffort,
}

/// Reasoning effort of the model.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, ts_rs::TS,
)]
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
    /// Ask and dashboard ranking jobs.
    Interactive,
    /// Pipeline jobs.
    Batch,
}

/// Private Ask archive: restore this thread first, then the analytic `session`; save only here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkingSession {
    /// Immutable working-thread policy; verify Ask never gains MemCan access.
    #[serde(default)]
    pub kind: Option<crate::runtime::ThreadKind>,
    /// Bound first-Ask preflight; absence requires recovery before an answer.
    #[serde(default)]
    pub preflight: Option<crate::runtime::AskPreflight>,
    /// Parent last-context estimate; absence or stale generation requires recovery.
    #[serde(default)]
    pub parent_context: Option<crate::runtime::LastContext>,
    /// SHA-256 of the authenticated user's stable id (64 lowercase hex digits).
    pub user_key: String,
    /// Analytic generation this fork is based on. Stale forks are never offered for restore.
    pub analytic_generation: i64,
    /// Working thread to restore, or none for a lazy fork from the analytic session.
    pub thread_id: Option<String>,
}

/// Tagged flow output. Fixed paths are relative to the job directory, never model-controlled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FlowFile {
    /// Versioned analysis checker context, written to `flow-context.json`.
    pub context: Value,
    /// Rejected document restored to `out/flow.md` for an edit-only repair.
    pub previous: Option<String>,
}
