//! Resolving a pull request URL.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::api::analyses::{AnalysisStatus, AnalysisSummary};
use crate::api::review_loop::WatchState;
use crate::common::{PrRef, RepoRef};
use crate::ids::{AnalysisId, CommitSha};
use crate::time::Timestamp;

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
    /// The latest analysis of this pull request, if any: a not cancelled one first, then the one in the user's
    /// language, else in another language (its `language` says which).
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

/// Query of `GET /api/v1/me/prs`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrHistoryQuery {
    /// Most rows to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
    /// Opaque cursor issued by the previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cursor: Option<String>,
}

/// Page of pull requests the reader analysed, watched, or opened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrHistory {
    /// One entry per pull request, newest activity first.
    pub prs: Vec<PrHistoryItem>,
    /// Cursor of the next page, or null at the end.
    pub next_cursor: Option<String>,
}

/// The reader's activity on one pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrHistoryItem {
    /// Pull request.
    pub pr: PrRef,
    /// Latest stored pull request title.
    pub title: String,
    /// The last known GitHub lifecycle state; absent until GitHub reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pr_state: Option<PrLifecycleState>,
    /// Latest activity across all included sources.
    pub last_activity_at: Timestamp,
    /// When the reader last opened the pull request page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub opened_at: Option<Timestamp>,
    /// Latest analysis requested by the reader.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub analysis: Option<PrHistoryAnalysis>,
    /// The reader's watch, including an ended watch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub watch: Option<PrHistoryWatch>,
}

/// A pull request's lifecycle state on GitHub.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PrLifecycleState {
    /// GitHub reports an open pull request.
    Open,
    /// GitHub reports a pull request closed without a merge.
    Closed,
    /// GitHub reports a merged pull request.
    Merged,
}

/// The reader's most recent analysis of a pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrHistoryAnalysis {
    /// Analysis id.
    pub id: AnalysisId,
    /// Stored analysis status.
    pub status: AnalysisStatus,
    /// Head the analysis was requested for.
    pub head_sha: CommitSha,
    /// When the reader requested the analysis.
    pub requested_at: Timestamp,
    /// Whether the pull request has moved to a different head.
    pub head_moved: bool,
}

/// The reader's watch of a pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrHistoryWatch {
    /// Current or ended watch state.
    pub state: WatchState,
    /// When the watch began.
    pub since: Timestamp,
    /// When the watch ended, if it did.
    pub ended_at: Option<Timestamp>,
}
