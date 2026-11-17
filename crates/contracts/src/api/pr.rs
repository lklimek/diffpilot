//! Resolving a pull request URL.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::api::analyses::AnalysisSummary;
use crate::common::RepoRef;
use crate::ids::CommitSha;

/// Body of `POST /api/v1/pr/resolve`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ResolvePrRequest {
    /// GitHub pull request URL.
    pub url: String,
}

/// A pull request as GitHub reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrSummary {
    /// Repository.
    pub repo: RepoRef,
    /// Number.
    pub number: u32,
    /// Title.
    pub title: String,
    /// Author login.
    pub author: String,
    /// Pull request page on GitHub.
    pub html_url: String,
    /// Base commit.
    pub base_sha: CommitSha,
    /// Head commit.
    pub head_sha: CommitSha,
}

/// Response of `POST /api/v1/pr/resolve`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ResolvePrResponse {
    /// Non-blocking notice for a large PR, using this same GitHub metadata response.
    #[serde(default)]
    pub size_warning: Option<SizeWarning>,
    /// The pull request now.
    pub pr: PrSummary,
    /// False when the GitHub App is not installed on the repository: analysis works, comments do not.
    pub commenting_available: bool,
    /// The latest analysis of this pull request in the user's language, if any.
    pub analysis: Option<AnalysisSummary>,
    /// The analysis is for an older head than the pull request now has.
    pub head_moved: bool,
}

/// Large-PR facts and a rough estimate, never a prerequisite for starting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SizeWarning {
    /// Additions plus deletions reported by GitHub.
    #[ts(type = "number")]
    pub changed_lines: u64,
    /// Changed files reported by GitHub.
    pub changed_files: u32,
    /// Rough estimate from recent local runs; null without enough history.
    pub estimate: Option<crate::api::analyses::AnalysisEstimate>,
}
