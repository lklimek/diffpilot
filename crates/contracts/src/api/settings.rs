//! Settings (brief §4.4).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::api::tour::ProjectProfileStatus;
use crate::common::{LlmLanguage, NotAnalysedCategory, RepoRef};
use crate::job::result::LlmCallOutcome;

/// Response of `GET /api/v1/settings`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Settings {
    /// Outcome of the most recent LLM call; null before the first call. No on-demand check.
    pub llm_status: Option<LlmCallOutcome>,
    /// Language of LLM texts.
    pub language: LlmLanguage,
    /// GitHub App installations the user can access; null = not checked (GitHub could not be reached or the
    /// user's GitHub sign-in is not usable), never a stand-in for "none installed".
    pub app_installations: Option<Vec<AppInstallation>>,
    /// The tenant's known repositories (any pull request opened); each one's settings can be opened.
    pub repos: Vec<RepoRef>,
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
    /// Path glob or line regex.
    pub kind: RuleKind,
    /// The glob or regex.
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
}
