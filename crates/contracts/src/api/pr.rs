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
    /// The pull request's size from this same GitHub metadata response, whatever its size; absent when GitHub
    /// did not report it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub size: Option<PrSize>,
    /// Rough estimate of what starting the analysis runs: plan and PR context (flows are analysed on request).
    /// From recent local runs; absent without the size or enough history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub planning_estimate: Option<crate::api::analyses::AnalysisEstimate>,
    /// The pull request now.
    pub pr: PrSummary,
    /// False when the GitHub App is not installed on the repository: analysis works, comments do not.
    pub commenting_available: bool,
    /// The latest analysis of this pull request, if any: the one in the user's language when there is one, else
    /// in another language (its `language` says which).
    pub analysis: Option<AnalysisSummary>,
    /// The analysis is for an older head than the pull request now has.
    pub head_moved: bool,
}

/// Size of a pull request as GitHub reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrSize {
    /// Additions plus deletions.
    #[ts(type = "number")]
    pub changed_lines: u64,
    /// Changed files.
    pub changed_files: u32,
}

/// Large-PR facts and a rough estimate, never a prerequisite for starting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SizeWarning {
    /// Additions plus deletions reported by GitHub.
    #[ts(type = "number")]
    pub changed_lines: u64,
    /// Changed files reported by GitHub.
    pub changed_files: u32,
    /// Rough estimate of plan and PR context from recent local runs (the same as
    /// `ResolvePrResponse.planning_estimate`); null without enough history.
    pub estimate: Option<crate::api::analyses::AnalysisEstimate>,
}
