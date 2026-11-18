//! Scan facts the scorer reads, in the meaning of the daily-priorities `dp-scan.json` fields (plan §A.2); ages are
//! timestamps, measured against the `now` the caller passes in.

use chrono::{DateTime, Utc};
use contracts::api::dashboard::{ItemKey, ScoreComponent};
use contracts::common::RepoRef;
use uuid::Uuid;

/// How an item reached the scan (skill `relations`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    /// Authored by a scanned login.
    Authored,
    /// Assigned to a scanned login.
    Assigned,
    /// Review requested from a scanned login, directly or through a team.
    ReviewRequested,
    /// Open issue of a watched repository.
    RepoOpen,
    /// Mentions a scanned login.
    Mentioned,
}

/// Effective CI state (skill `ciEffective`): policy gates excluded, latest run per check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiEffective {
    /// Every build, test and lint check passed.
    Green,
    /// A build, test or lint check failed.
    Red,
    /// A check is still running.
    Pending,
    /// The check list was truncated and nothing failed in the part read.
    Unknown,
    /// No checks on the head commit.
    None,
}

/// GitHub `mergeable`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mergeable {
    /// Merges cleanly.
    Mergeable,
    /// Has conflicts.
    Conflicting,
    /// Not computed yet.
    Unknown,
}

/// GitHub `reviewDecision`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    /// Changes requested.
    ChangesRequested,
    /// Approved.
    Approved,
    /// A review is required.
    ReviewRequired,
}

/// Milestone assignment of a pull request or issue.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Milestone {
    /// The assignment could not be read.
    Unknown,
    /// No milestone.
    Unassigned,
    /// Assigned milestone title.
    Assigned(String),
}

/// Release catalogue of a repository; `None` = the fetch failed.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MilestoneContext {
    /// Tags of published stable releases (drafts and prereleases left out).
    pub release_tags: Option<Vec<String>>,
    /// Titles of every milestone, open and closed.
    pub milestone_titles: Option<Vec<String>>,
}

/// Ownership-policy verdict of a pull request's change (skill `ownedChange.verdict`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnedChange {
    /// The repository has no ownership policy.
    NoPolicy,
    /// A policy applies but could not be read or evaluated.
    Unknown,
    /// Only comments or docs, or nothing in the owned areas.
    None,
    /// Few or thinly spread code lines.
    Minor,
    /// A concentrated code change in the owned areas.
    Significant,
}

/// Facts of one open pull request.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PrFacts {
    /// Repository.
    pub repo: RepoRef,
    /// Number.
    pub number: u32,
    /// Title.
    pub title: String,
    /// Body text (searched for blocked-by wording only).
    pub body: String,
    /// Author login; null for a deleted account.
    pub author: Option<String>,
    /// GitHub reports the author as a `Bot`.
    pub author_is_bot: bool,
    /// Relations to the scanned logins.
    pub relations: Vec<Relation>,
    /// Draft.
    pub is_draft: bool,
    /// Label names.
    pub labels: Vec<String>,
    /// GitHub `updatedAt`.
    pub updated_at: DateTime<Utc>,
    /// Added lines of the full diff; null when unknown.
    pub additions: Option<u32>,
    /// Removed lines of the full diff; null when unknown.
    pub deletions: Option<u32>,
    /// Base branch.
    pub base_ref: Option<String>,
    /// The base repository's default branch.
    pub default_branch: Option<String>,
    /// Open same-repository PRs based on this PR's head; null when the lookup failed.
    pub dependents: Option<Vec<u32>>,
    /// Open same-repository PRs whose head is this PR's base; null when the lookup failed.
    pub stacked_on: Option<Vec<u32>>,
    /// Milestone assignment.
    pub milestone: Milestone,
    /// Release catalogue of the repository.
    pub milestone_context: MilestoneContext,
    /// Effective CI state.
    pub ci: CiEffective,
    /// Mergeability.
    pub mergeable: Mergeable,
    /// Review decision; null when the repository requires no reviews.
    pub review_decision: Option<ReviewDecision>,
    /// Unresolved review threads.
    pub unresolved_threads: u32,
    /// A review is requested directly from a scanned login.
    pub review_pending_for_us: bool,
    /// When that request was made; null when the timeline page lacked it.
    pub review_requested_at: Option<DateTime<Utc>>,
    /// `review_requested_at` fell back to `updatedAt`.
    pub review_request_age_is_approx: bool,
    /// Logins whose latest review requests changes.
    pub changes_requested_by: Vec<String>,
    /// One of them is a human outside the scanned logins.
    pub changes_requested_by_human: bool,
    /// Newest such human change request not re-requested since; null when none or re-requested.
    pub changes_requested_human_at: Option<DateTime<Utc>>,
    /// More reviews exist than were fetched.
    pub reviews_truncated: bool,
    /// The last comment is a human's outside the scanned logins.
    pub awaiting_our_reply: bool,
    /// Author of the last comment.
    pub last_comment_author: Option<String>,
    /// Time of the last comment.
    pub last_comment_at: Option<DateTime<Utc>>,
    /// Ownership-policy verdict.
    pub owned_change: OwnedChange,
    /// Logins whose latest review approves.
    pub approved_by: Vec<String>,
    /// Pending review requests to people and teams (`team:<slug>`) outside the scanned logins and bots.
    pub review_requested_from_others: Vec<String>,
    /// The oldest of those requests still open (`AwaitingItem.waiting_since`); null when the timeline page lacked it.
    pub others_review_requested_at: Option<DateTime<Utc>>,
}

/// Facts of one open issue.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IssueFacts {
    /// Repository.
    pub repo: RepoRef,
    /// Number.
    pub number: u32,
    /// Title.
    pub title: String,
    /// Body text (searched for blocked-by wording only).
    pub body: String,
    /// Author login.
    pub author: Option<String>,
    /// Relations to the scanned logins.
    pub relations: Vec<Relation>,
    /// Label names.
    pub labels: Vec<String>,
    /// GitHub `updatedAt`.
    pub updated_at: DateTime<Utc>,
    /// Milestone assignment.
    pub milestone: Milestone,
    /// Release catalogue of the repository.
    pub milestone_context: MilestoneContext,
}

/// Open MemCan TODO status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Not started.
    Pending,
    /// Started.
    InProgress,
    /// Waiting on something.
    Blocked,
    /// Deferred.
    Postponed,
}

/// Facts of one open MemCan TODO.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TodoFacts {
    /// MemCan id.
    pub id: Uuid,
    /// Title.
    pub title: String,
    /// Status.
    pub status: TodoStatus,
    /// MemCan priority (`high`, `medium`, `low`), as stored.
    pub priority: Option<String>,
    /// Change time (plan §G: first sight of the current values).
    pub changed_at: DateTime<Utc>,
}

/// A part value judged by the LLM; the judge's output after its own validation (plan §B.2).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(test, derive(serde::Deserialize))]
pub struct JudgedPart {
    /// Component.
    pub component: ScoreComponent,
    /// Points, one of the component's closed values.
    pub points: i32,
    /// Short basis.
    pub basis: String,
}

/// Whether a stored judgement still matches the item (plan §B.4 fingerprint and prompt version).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(serde::Deserialize), serde(rename_all = "snake_case"))]
pub enum Freshness {
    /// Same judged inputs and prompt version: final.
    Current,
    /// Judged before the item changed: carried as provisional until re-judged.
    Inherited,
}

/// The LLM's eligibility decision with its reason.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(serde::Deserialize))]
pub struct EligibilityJudgement {
    /// Ranked or left out.
    pub eligible: bool,
    /// Plain reason.
    pub reason: String,
}

/// A stored judgement of one item.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(test, derive(serde::Deserialize))]
pub struct Judgement {
    /// Judged parts.
    pub parts: Vec<JudgedPart>,
    /// Eligibility decision, when asked.
    pub eligibility: Option<EligibilityJudgement>,
    /// The LLM raised a P0 alarm.
    pub alarm: bool,
    /// Items merged into this one.
    pub also: Vec<ItemKey>,
    /// When it was judged.
    pub judged_at: DateTime<Utc>,
    /// Current or inherited.
    pub freshness: Freshness,
}
