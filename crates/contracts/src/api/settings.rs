//! Settings (brief §4.4).

use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::api::tour::ProjectProfileStatus;
use crate::common::{LlmLanguage, NotAnalysedCategory, RepoRef};
use crate::job::result::LlmCallOutcome;

/// Response of `GET /api/v1/settings`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Settings {
    /// Installation-wide model selection; absent on older servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub llm_selection: Option<LlmSelection>,
    /// Tenant usage in the server's local calendar; absent on older responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub llm_usage: Option<crate::api::usage::UsageWindows>,
    /// Outcome of the most recent LLM call; null before the first call. No on-demand check.
    pub llm_status: Option<LlmCallOutcome>,
    /// Observed MemCan connection state, without an on-demand probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub memcan_status: Option<MemcanStatus>,
    /// Language of LLM texts.
    pub language: LlmLanguage,
    /// GitHub App installations the user can access; null = not checked (GitHub could not be reached or the
    /// user's GitHub sign-in is not usable), never a stand-in for "none installed".
    pub app_installations: Option<Vec<AppInstallation>>,
    /// The tenant's known repositories (any pull request opened); each one's settings can be opened.
    pub repos: Vec<RepoRef>,
    /// Installation-wide controlled GitHub owners (Dependencies view, ADR-021); absent on older servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub controlled_owners: Option<ControlledOwners>,
}

/// The installation-wide list of controlled GitHub owners: their repositories are "yours" in the Dependencies
/// view, besides the pull request's owner, which always is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ControlledOwners {
    /// Owner logins, sorted case-insensitively, unique.
    pub owners: Vec<String>,
    /// Who saved the list last; null before the first save.
    pub changed_by: Option<String>,
    /// When it was saved last.
    pub changed_at: Option<Timestamp>,
}

/// Body of `PUT /api/v1/settings/controlled-owners`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SetControlledOwnersRequest {
    /// GitHub owner logins (users or organisations), at most [`SetControlledOwnersRequest::MAX_OWNERS`].
    pub owners: Vec<String>,
}

impl SetControlledOwnersRequest {
    /// Most owners in the list.
    pub const MAX_OWNERS: usize = 50;
}

/// A GitHub App installation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AppInstallation {
    /// Account (user or organisation) login.
    pub account: String,
    /// Installed on all repositories of the account, or on selected ones.
    pub all_repositories: bool,
}

/// Body of `PUT /api/v1/settings/language`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SetLanguageRequest {
    /// Language of LLM texts.
    pub language: LlmLanguage,
}

/// Response of `GET /api/v1/repos/{owner}/{name}/settings` (read-only).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RepoSettings {
    /// Repository.
    pub repo: RepoRef,
    /// Built-in "not analysed" rules.
    pub rules: Vec<NotAnalysedRule>,
    /// Related repositories, auto-detected from Cargo files on the base ref.
    pub related_repos: Vec<RepoRef>,
    /// Project profile source.
    pub project_profile: ProjectProfileStatus,
}

/// A built-in "not analysed" rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct NotAnalysedRule {
    /// Detector used to classify the material.
    pub kind: RuleKind,
    /// The pattern or detector description.
    pub pattern: String,
    /// Section it fills.
    pub category: NotAnalysedCategory,
}

/// Kind of rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// Matches file paths.
    PathGlob,
    /// Matches lines.
    LineRegex,
    /// Comment-only lines recognised by a language lexer.
    Lexer,
    /// A generated-file marker in the header.
    Header,
    /// Git's binary-content detection.
    Binary,
}

/// Most recent observed MemCan connectivity; never an on-demand health probe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct MemcanStatus {
    /// Outcome observed by the server's client.
    pub state: MemcanState,
    /// Last successful tool call in this server process, retained after a later failure.
    pub last_success_at: Option<Timestamp>,
}

/// What the server knows about MemCan connectivity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum MemcanState {
    /// No connection configuration.
    NotConfigured,
    /// Configured, but no completed attempt since this server started.
    NotChecked,
    /// The most recent attempt reached MemCan.
    Reachable,
    /// The most recent attempt could not reach MemCan or timed out.
    NotReachable,
}

/// Allowed models and the installation-wide choice for newly submitted jobs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LlmSelection {
    /// Fixed server-supported model ids and labels.
    pub models: Vec<LlmModelOption>,
    /// Allowed reasoning efforts.
    pub efforts: Vec<crate::job::spec::ReasoningEffort>,
    /// Current choice, or the server default before the first save.
    pub current: crate::job::spec::ModelChoice,
    /// Reasoning effort of the dashboard ranking (`judge_work`; default low). Optional on the wire; absent on
    /// older servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub judge_effort: Option<crate::job::spec::ReasoningEffort>,
    /// Reasoning effort of Ask jobs (default medium); every other job uses `current`. Optional on the wire;
    /// absent on older servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ask_effort: Option<crate::job::spec::ReasoningEffort>,
    /// Login of the last editor of the model, effort, ranking effort or Ask effort; null for the default.
    pub changed_by: Option<String>,
    /// Time of the last save; null for the default.
    pub changed_at: Option<Timestamp>,
}

/// One model available in this deployment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LlmModelOption {
    /// Provider id.
    pub id: String,
    /// Plain display name.
    pub label: String,
}

/// Body of `PUT /api/v1/settings/llm/ask-effort`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SetAskEffortRequest {
    /// Reasoning effort of Ask jobs.
    pub reasoning_effort: crate::job::spec::ReasoningEffort,
}

/// Body of `PUT /api/v1/settings/llm/judge-effort`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SetJudgeEffortRequest {
    /// Reasoning effort of the dashboard ranking.
    pub reasoning_effort: crate::job::spec::ReasoningEffort,
}
