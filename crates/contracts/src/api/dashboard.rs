//! GitHub work dashboard ("My work", ADR-016, plan `docs/v1.1-dashboard-plan.md` §E): summary, ranked items,
//! refreshes and their events, per-user settings. Integers only on the wire: scores in tenths of a point.

use crate::time::Timestamp;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::common::RepoRef;
use crate::ids::{AnalysisId, CommitSha, RefreshId};

/// Response of `GET /api/v1/dashboard`: the user's dashboard without its items.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DashboardSummary {
    /// Unanswered, unhidden mentions, oldest first; independent of the ranking.
    #[serde(default)]
    pub mentions: Vec<Mention>,
    /// Mention reads omitted in this snapshot, independent of item filters and pagination.
    #[serde(default)]
    pub mention_coverage: MentionCoverage,
    /// Opaque; changes whenever the stored snapshot or its ranking changes (poll this, then fetch the items).
    pub revision: String,
    /// The newest refresh of the user, live or ended; null before the first.
    pub latest_refresh: Option<RefreshStatus>,
    /// The snapshot the items come from; null before the first successful scan.
    pub snapshot: Option<SnapshotMeta>,
    /// The daily automatic refresh.
    pub schedule: ScheduleStatus,
    /// Items of the snapshot whose judgement came from an older ranking prompt (re-judged within the caps).
    pub older_prompt_judgements: u32,
}

/// Missing mention reads; zero items does not imply that every login was searched.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct MentionCoverage {
    /// Real candidates not checked because of the read budget, deadline or read failure.
    pub unchecked_items: u32,
    /// Logins whose mention search was unavailable; fallback covers only mention-only issues.
    pub unchecked_logins: Vec<String>,
    /// Logins whose PR mention search exceeded its request budget.
    pub timed_out_logins: Vec<String>,
}

/// What started a refresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RefreshTrigger {
    /// The user's Refresh action.
    Manual,
    /// The daily schedule.
    Scheduled,
}

/// State of a refresh; transitions in `docs/contracts/http-api.md` "Refresh states". At most one refresh per user
/// is `scanning` or `ranking`; only `ranking` and `finished` refreshes hold a snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RefreshState {
    /// Reading GitHub and MemCan; no snapshot yet.
    Scanning,
    /// The snapshot is stored and current; the LLM judges new and changed items.
    Ranking,
    /// The snapshot is current; the ranking ended with the outcome in `ranking.state`.
    Finished,
    /// The scan failed before its snapshot was complete; the previous snapshot stays.
    Failed,
    /// The scan was cancelled before its snapshot was complete; the previous snapshot stays.
    Cancelled,
}

/// One refresh: scan progress, ranking and outcome (`GET /api/v1/dashboard/refreshes/{id}`, SSE `refresh`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RefreshStatus {
    /// Id.
    pub id: RefreshId,
    /// What started it.
    pub trigger: RefreshTrigger,
    /// State.
    pub state: RefreshState,
    /// Items the LLM judged in this refresh.
    pub judged: u32,
    /// Items whose stored judgement was reused at no cost (judged inputs unchanged).
    pub reused: u32,
    /// Items left provisional by the per-refresh or daily ranking limit.
    pub left_provisional: u32,
    /// Scan steps in order.
    pub steps: Vec<ScanStep>,
    /// Scan progress, 0–100.
    pub percent: u32,
    /// Start time.
    pub started_at: Timestamp,
    /// When the scan finished and the snapshot was stored; null while scanning or when it failed.
    pub scanned_at: Option<Timestamp>,
    /// The LLM ranking of the snapshot; null before the scan finished.
    pub ranking: Option<RankingStatus>,
    /// Why the scan failed; null unless `failed`.
    pub failure: Option<RefreshFailure>,
    /// Plain log lines of the scan (at most 200, oldest first); no tokens, no query strings.
    pub log: Vec<String>,
}

/// A step of the scan, shown as a list with progress.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ScanStep {
    /// Which step.
    pub key: ScanStepKey,
    /// Its state.
    pub state: StepState,
    /// Units done (e.g. PR details read); null when the step has no count.
    pub done: Option<u32>,
    /// Units in total; null when not known yet or not counted.
    pub total: Option<u32>,
    /// Plain detail, e.g. the item being read or why the step failed.
    pub detail: Option<String>,
}

/// The scan steps, in order (plan §A.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ScanStepKey {
    /// Pull request searches (authored, assigned, review requested).
    SearchPrs,
    /// Issue searches (watched repositories, assigned, mentions).
    Issues,
    /// Pull request details (CI, reviews, threads, timeline).
    PrDetails,
    /// Ownership policies, releases, milestones, reach.
    Context,
    /// Pull request and review history for the charts.
    History,
    /// MemCan TODOs.
    Todos,
    /// LLM ranking of new and changed items.
    Ranking,
}

/// State of a scan step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    /// Not started.
    Waiting,
    /// In progress.
    Running,
    /// Done.
    Done,
    /// Failed; `detail` says why.
    Failed,
    /// Not run (e.g. TODOs switched off).
    Skipped,
}

/// The LLM ranking of a stored snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RankingStatus {
    /// State.
    pub state: RankingState,
    /// Items judged so far.
    pub judged: u32,
    /// Items queued for judgement in this refresh.
    pub total: u32,
    /// When a usage-limit pause ends; null unless `paused`.
    pub resumes_at: Option<Timestamp>,
    /// Plain text of a failure; null unless `failed` or `interrupted`.
    pub message: Option<String>,
}

/// State of the LLM ranking (plan §B.3); the last four end it and leave the refresh `finished`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RankingState {
    /// Queued behind a review analysis that holds the LLM credential.
    WaitingForAnalysis,
    /// Judging.
    Running,
    /// Paused by the provider's usage limit; continues by itself at `resumes_at`.
    Paused,
    /// Failed; provisional scores stay approximate (Retry ranking).
    Failed,
    /// Stopped by a server restart (Retry ranking).
    Interrupted,
    /// Cancelled by the user (Retry ranking).
    Cancelled,
    /// Done, or nothing needed judging.
    Finished,
}

/// Why a refresh failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RefreshFailure {
    /// Cause.
    pub code: RefreshFailureCode,
    /// Plain message for the user.
    pub message: String,
    /// Earliest time a new refresh can succeed (GitHub rate-limit reset); null when not limited.
    pub retry_at: Option<Timestamp>,
}

/// Cause of a failed refresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RefreshFailureCode {
    /// GitHub's rate limit; `retry_at` is its reset.
    GithubRateLimited,
    /// GitHub failed or answered unexpectedly.
    GithubUnavailable,
    /// The user's GitHub sign-in is no longer usable; sign in again.
    SignInRequired,
    /// The refresh took longer than its limit (5 minutes).
    TimedOut,
    /// A server restart interrupted the scan.
    Interrupted,
    /// Anything else.
    Internal,
}

/// The daily automatic refresh (ADR-020).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ScheduleStatus {
    /// On or off.
    pub enabled: bool,
    /// Local time of day, `HH:MM`.
    pub time: String,
    /// IANA time zone; null until known (the schedule does not run without it).
    pub time_zone: Option<String>,
    /// Next scheduled instant; null when off or without a time zone.
    pub next_run_at: Option<Timestamp>,
    /// The most recent scheduled run; null before the first.
    pub last_run: Option<ScheduledRun>,
}

/// One scheduled run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ScheduledRun {
    /// Its scheduled instant (the configured local time on that local date).
    pub at: Timestamp,
    /// What happened.
    pub outcome: ScheduledOutcome,
    /// Plain reason for `skipped`, `needs_sign_in` and `failed`.
    pub reason: Option<String>,
}

/// Outcome of a scheduled run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledOutcome {
    /// A scheduled refresh was started (its result is that refresh's).
    Started,
    /// Not needed: a refresh succeeded after the scheduled time, or one was scanning (a ranking is superseded).
    Skipped,
    /// No usable GitHub sign-in; shown until a later refresh succeeds.
    NeedsSignIn,
    /// The scheduled refresh failed; shown until a later refresh succeeds.
    Failed,
}

/// Facts of the stored snapshot besides its items.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SnapshotMeta {
    /// The refresh that produced it.
    pub refresh_id: RefreshId,
    /// When its scan finished.
    pub scanned_at: Timestamp,
    /// When its ranking finished; null while ranking or when it did not finish.
    pub ranked_at: Option<Timestamp>,
    /// Ranked items by kind and tier.
    pub item_counts: ItemCounts,
    /// Items left out of the ranking (`exclusions.total`).
    pub excluded: u32,
    /// The user's pull requests waiting on other people.
    pub awaiting: Vec<AwaitingItem>,
    /// The user's pull requests GitHub let the user merge at the scan, by display id; also ranked.
    pub ready_to_merge: Vec<ReadyToMergeItem>,
    /// The user's pull requests whose merge state GitHub had not computed by the scan (read again by the next
    /// refresh); null when the snapshot did not record merge states (stored before they were).
    pub merge_state_pending: Option<u32>,
    /// Dated pull request and review events of the chart window (12 weeks), across the scanned logins.
    pub history: Vec<HistoryEvent>,
    /// State of the MemCan TODOs.
    pub todos: TodosState,
    /// What the scan missed or approximated.
    pub notes: Vec<ScanNote>,
    /// Configured repositories and owners the user's GitHub token cannot see (`owner/name` or `owner`).
    pub unreachable: Vec<String>,
    /// What the scan read before exclusion.
    pub checked: KindCounts,
    /// Why the rules left items out, computed with the ranking; absent only from a response without it (a server
    /// older than this field), which the web shows as not recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub exclusions: Option<Exclusions>,
}

/// Most excluded items listed by `GET /api/v1/dashboard?excluded=true`; the counts cover all.
pub const EXCLUDED_ITEMS_LIMIT: usize = 200;

/// The items left out of the ranking, counted per reason. No LLM text; `judged_not_eligible` and
/// `merged_into_another` are LLM decisions, the other reasons deterministic rules.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Exclusions {
    /// All left-out items.
    pub total: u32,
    /// Non-zero counts, in the order of [`ExclusionReason`].
    pub counts: Vec<ExclusionCount>,
    /// With `?excluded=true` only: the items by reason, then display id, at most [`EXCLUDED_ITEMS_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub items: Option<Vec<ExcludedItem>>,
}

/// How many items one rule left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ExclusionCount {
    /// The rule.
    pub reason: ExclusionReason,
    /// Items it left out.
    pub count: u32,
}

/// The rule that left an item out of the ranking. Clients show an unknown value as a plain "other rule".
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    /// Written by an excluded author (Settings → Excluded authors).
    ExcludedAuthor,
    /// A pull request of someone else not directed at the user, with no or only a minor change in the user's
    /// owned paths (Settings → Ownership policy).
    OutsideOwnedAreas,
    /// A draft pull request of someone else (only the user's own drafts count).
    Draft,
    /// The LLM, asked because the ownership policy could not be read, judged it outside the user's areas.
    JudgedNotEligible,
    /// The LLM merged it into another item covering the same work.
    MergedIntoAnother,
}

/// One item the rules left out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ExcludedItem {
    /// Stable key.
    pub key: ItemKey,
    /// Kind.
    pub kind: ItemKind,
    /// Display id (`PLAT-1203`, `TODO-3fa91c2e`).
    pub id: String,
    /// Repository; null for TODOs.
    pub repo: Option<RepoRef>,
    /// Number; null for TODOs.
    pub number: Option<u32>,
    /// Link: GitHub, or the MemCan UI for a TODO when configured.
    pub url: Option<String>,
    /// Title, plain text.
    pub title: String,
    /// Author login; null for TODOs.
    pub author: Option<String>,
    /// The rule.
    pub reason: ExclusionReason,
    /// Display id of the item it was merged into (`merged_into_another` only).
    pub merged_into: Option<String>,
}

/// Query of `GET /api/v1/dashboard`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SummaryQuery {
    /// List the excluded items (`snapshot.exclusions.items`); without it only their counts are sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub excluded: Option<bool>,
}

/// Ranked items by kind and tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ItemCounts {
    /// All ranked items.
    pub total: u32,
    /// Pull requests.
    pub pr: u32,
    /// Issues.
    pub issue: u32,
    /// MemCan TODOs.
    pub todo: u32,
    /// Tier P0 (alarms).
    pub p0: u32,
    /// Tier P1.
    pub p1: u32,
    /// Tier P2.
    pub p2: u32,
    /// Tier P3.
    pub p3: u32,
    /// Items in the group "Not judged yet" (score state `unjudged`; also counted in their tier).
    pub not_judged: u32,
}

/// Counts by item kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct KindCounts {
    /// Pull requests.
    pub pr: u32,
    /// Issues.
    pub issue: u32,
    /// MemCan TODOs.
    pub todo: u32,
}

/// A pull request of the user that waits on other people.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AwaitingItem {
    /// Display alias, e.g. `PLAT-1176`.
    pub id: String,
    /// Repository.
    pub repo: RepoRef,
    /// Pull request number.
    pub number: u32,
    /// Title.
    pub title: String,
    /// GitHub link, built by the server.
    pub url: String,
    /// Logins and teams (`team:<slug>`) whose review is requested.
    pub waiting_on: Vec<String>,
    /// Since when it waits (the oldest open request).
    pub waiting_since: Timestamp,
    /// Deterministic tags from scan facts (e.g. CI passing).
    pub tags: Vec<FactTag>,
    /// Size mark of its full diff (display only); absent when the totals are unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub change_size: Option<ChangeSizeMark>,
    /// Always null: a pull request waiting only on others is not ranked (kept for compatibility).
    pub total_tenths: Option<i32>,
}

/// A pull request of the user that GitHub lets the user merge now: not a draft, write access to the base
/// repository, merge state clean, has hooks or unstable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReadyToMergeItem {
    /// Display alias, e.g. `PLAT-1176`.
    pub id: String,
    /// Repository.
    pub repo: RepoRef,
    /// Pull request number.
    pub number: u32,
    /// Title.
    pub title: String,
    /// GitHub link, built by the server.
    pub url: String,
    /// Deterministic tags from scan facts (CI state; checks not passing that are not required).
    pub tags: Vec<FactTag>,
    /// Size mark of its full diff (display only); absent when the totals are unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub change_size: Option<ChangeSizeMark>,
}

/// A dated event for the charts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct HistoryEvent {
    /// What happened.
    pub kind: HistoryKind,
    /// When.
    pub at: Timestamp,
    /// The pull request, `owner/name#number`; one event per pull request and kind.
    pub item_id: String,
}

/// Kind of a history event; pull requests and reviews only (OD16).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum HistoryKind {
    /// A pull request authored by a scanned login was opened.
    PrOpened,
    /// It was merged.
    PrMerged,
    /// It was closed without merging.
    PrClosedUnmerged,
    /// A scanned login submitted a review on a pull request.
    ReviewGiven,
}

/// State of the MemCan TODOs in the snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TodosState {
    /// State.
    pub status: TodosStatus,
    /// When the TODO rows shown were read; null when none were read.
    pub read_at: Option<Timestamp>,
    /// When MemCan could not be reached; null unless `not_refreshed`.
    pub failed_at: Option<Timestamp>,
}

/// Whether the TODO rows are current.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum TodosStatus {
    /// Read in this refresh.
    Read,
    /// MemCan was unreachable; the rows come from an earlier snapshot.
    NotRefreshed,
    /// The user switched TODOs off.
    Off,
    /// MemCan is not configured on this server.
    NotConfigured,
}

/// Something the scan missed or approximated (plain text).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ScanNote {
    /// The note.
    pub text: String,
    /// The item it concerns, if one.
    pub item: Option<ItemKey>,
}

/// Query of `GET /api/v1/dashboard/items`; every filter is optional.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ItemQuery {
    /// Only this tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tier: Option<Tier>,
    /// Only pull requests or only issues; absent = both. TODO items are listed whatever this filter says (while the
    /// user shows MemCan TODOs, plan §G) unless `todos` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<GithubKind>,
    /// `false` leaves TODO items out; absent = true. A TODOs-only list is not supported in v1.1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub todos: Option<bool>,
    /// Only this repository, `owner/name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub repo: Option<String>,
    /// Only this score state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub state: Option<ScoreState>,
    /// Only items changed at or after this time (TODOs: their change time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub updated_since: Option<Timestamp>,
    /// Page size, [`ItemQuery::DEFAULT_LIMIT`] when absent, at most [`ItemQuery::MAX_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
    /// `next_cursor` of the previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cursor: Option<String>,
}

impl ItemQuery {
    /// Page size without `limit`.
    pub const DEFAULT_LIMIT: u32 = 100;
    /// Largest page size.
    pub const MAX_LIMIT: u32 = 500;
}

/// Response of `GET /api/v1/dashboard/items`: ranked items of the latest snapshot, in rank order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ItemPage {
    /// Unanswered, unhidden mentions on the first page only; empty on cursor pages.
    #[serde(default)]
    pub mentions: Vec<Mention>,
    /// Mention reads omitted in this snapshot, independent of item filters and pagination.
    #[serde(default)]
    pub mention_coverage: MentionCoverage,
    /// The summary's `revision` the page belongs to.
    pub revision: String,
    /// Items of this page.
    pub items: Vec<WorkItem>,
    /// Cursor of the next page; null on the last.
    pub next_cursor: Option<String>,
}

/// GitHub item kinds of the list filter `kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum GithubKind {
    /// Pull requests.
    Pr,
    /// Issues.
    Issue,
}

/// Kind of a work item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// A GitHub pull request.
    Pr,
    /// A GitHub issue.
    Issue,
    /// A MemCan TODO.
    Todo,
}

/// One ranked item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WorkItem {
    /// Display alias, e.g. `PLAT-1187` or `TODO-3fa91c2e`.
    pub id: String,
    /// Stable, unique, URL-safe key.
    pub key: ItemKey,
    /// Kind.
    pub kind: ItemKind,
    /// Repository; null for TODOs (their MemCan project is a tag).
    pub repo: Option<RepoRef>,
    /// Pull request or issue number; null for TODOs.
    pub number: Option<u32>,
    /// Title.
    pub title: String,
    /// Link built by the server (GitHub, or the MemCan UI); null for a TODO without a configured MemCan UI.
    pub url: Option<String>,
    /// Author login; null for TODOs.
    pub author: Option<String>,
    /// From a private repository (shown only to this user).
    pub private: bool,
    /// Pull requests and issues: GitHub `updatedAt`. TODOs: when the dashboard first saw the current values of the
    /// fields of plan §G (MemCan has no `updated_at`).
    pub updated_at: Timestamp,
    /// Deterministic tags from scan facts.
    pub tags: Vec<FactTag>,
    /// Size mark of a pull request's full diff (display only); absent for issues, TODOs and unknown totals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub change_size: Option<ChangeSizeMark>,
    /// Deterministic facts from the scan.
    pub facts: Vec<Fact>,
    /// Score with its parts.
    pub score: ItemScore,
    /// LLM-written text; null before the first judgement.
    pub llm: Option<ItemText>,
    /// The user's diffpilot analysis of this pull request, read at request time; null when none.
    pub review: Option<ReviewLink>,
}

/// Size mark of a pull request: a fixed table of changed lines (additions + deletions), plan §C.2 K16.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum SizeMark {
    /// Fewer than 150 changed lines.
    Tiny,
    /// 150 to 499 changed lines.
    Small,
    /// 500 to 2999 changed lines.
    Medium,
    /// 3000 to 9999 changed lines.
    Large,
    /// 10 000 changed lines or more.
    Huge,
}

/// A pull request's size mark with its evidence (a scan fact, not a score input).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ChangeSizeMark {
    /// The mark.
    pub size: SizeMark,
    /// Added lines of the full diff.
    pub additions: u32,
    /// Removed lines of the full diff.
    pub deletions: u32,
}

/// A deterministic tag from scan facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FactTag {
    /// Short text.
    pub text: String,
    /// Tooltip with the underlying fact.
    pub tip: String,
}

/// A labelled scan fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Fact {
    /// Label.
    pub label: String,
    /// Value.
    pub value: String,
}

/// Score of an item: the sum of its parts, in tenths of a point.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ItemScore {
    /// Sum of the parts' points, in tenths.
    pub total_tenths: i32,
    /// Whether every part is final.
    pub state: ScoreState,
    /// Tier, computed by the server (P0 = alarm; otherwise P1 ≥ 65, P2 35–<65, P3 < 35 points).
    pub tier: Tier,
    /// The components, in display order.
    pub parts: Vec<ScorePart>,
}

/// Whether a score is final.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ScoreState {
    /// Every part is final.
    Final,
    /// Some LLM part is carried over from before a change, or a computed default awaits confirmation (`~N`).
    Provisional,
    /// New item the LLM has not judged; computed parts plus named estimates (group "Not judged yet").
    Unjudged,
}

/// Priority tier.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Alarm, independent of the score.
    P0,
    /// Score ≥ 65.
    P1,
    /// Score 35–<65.
    P2,
    /// Score < 35.
    P3,
}

/// One score component.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ScorePart {
    /// Which component.
    pub component: ScoreComponent,
    /// Points in tenths; null while pending.
    pub points_tenths: Option<i32>,
    /// Who decides it.
    pub source: PartSource,
    /// How the value was reached.
    pub status: PartStatus,
    /// Plain basis of the value (an estimate names its rule).
    pub basis: String,
}

/// The twelve score components (daily-priorities `scoreParts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ScoreComponent {
    /// How far the work has advanced.
    Advancement,
    /// Severity of a pre-existing defect it addresses.
    DefectSeverity,
    /// Work it unblocks.
    Unblocks,
    /// A human waiting on the user.
    HumanWaiting,
    /// Recency of the last change.
    Recency,
    /// Impact of the change.
    ChangeImpact,
    /// How close it is to done.
    Completion,
    /// Size of the change (smaller scores higher).
    ChangeSize,
    /// Confirmed production defect or release blocker.
    UrgencyBonus,
    /// Blocked by something open.
    BlockedPenalty,
    /// Targets the default branch.
    DefaultBranchBonus,
    /// Milestone of the current or next release.
    MilestoneBonus,
}

impl ScoreComponent {
    /// Every component, in display order.
    pub const ALL: [ScoreComponent; 12] = [
        Self::Advancement,
        Self::DefectSeverity,
        Self::Unblocks,
        Self::HumanWaiting,
        Self::Recency,
        Self::ChangeImpact,
        Self::Completion,
        Self::ChangeSize,
        Self::UrgencyBonus,
        Self::BlockedPenalty,
        Self::DefaultBranchBonus,
        Self::MilestoneBonus,
    ];
}

/// Who decides a part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PartSource {
    /// Computed from scan facts by the server.
    Computed,
    /// Judged by the LLM.
    Llm,
    /// Does not apply to this kind of item (contributes 0).
    NotApplicable,
}

/// How a part's value was reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PartStatus {
    /// Computed from scan facts.
    Computed,
    /// Judged by the LLM for the current judged inputs.
    Judged,
    /// The LLM replaced a computed default, with a stated basis.
    Overridden,
    /// Carried over from a judgement made before the item changed; re-judged later.
    Inherited,
    /// A named constant estimate until the LLM judges the item.
    Estimated,
    /// Not known yet.
    Pending,
}

/// LLM-written text of an item (shown with the "LLM" tag).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ItemText {
    /// Why it ranks where it does.
    pub why: String,
    /// The next action.
    pub next: String,
    /// Reason of a P0 alarm; null when none.
    pub alarm: Option<String>,
    /// When it was judged.
    pub judged_at: Timestamp,
    /// Carried over from before the item changed.
    pub inherited: bool,
    /// Keys of items the LLM merged into this one (chosen only from server-suggested pairs).
    pub also_keys: Vec<ItemKey>,
}

/// The user's diffpilot analysis of a pull request item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReviewLink {
    /// The newest analysis of the pull request.
    pub analysis_id: AnalysisId,
    /// Head commit it analysed.
    pub analysed_head: CommitSha,
    /// The pull request head moved since.
    pub head_moved: bool,
    /// The user has saved review state on it.
    pub has_my_state: bool,
}

/// Server-sent event of `GET /api/v1/dashboard/refreshes/{id}/events`; the SSE `event` field is the tag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "event", rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "built once per send and serialised at once; boxing the status buys nothing"
)]
pub enum DashboardEvent {
    /// Full status of the refresh; sent on connect and on every change, including its ranking.
    Refresh {
        /// The refresh.
        status: RefreshStatus,
    },
    /// Sent every 15 s while the stream is open; each one re-checks the session or token.
    Heartbeat {
        /// Server time of sending.
        at: Timestamp,
    },
}

impl DashboardEvent {
    /// The SSE event name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Refresh { .. } => "refresh",
            Self::Heartbeat { .. } => "heartbeat",
        }
    }
}

/// Per-user dashboard settings (body of `PUT /api/v1/settings/dashboard`); validated by the server (plan §F.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DashboardSettings {
    /// Other GitHub logins of the user, scanned with the own login.
    pub linked_logins: Vec<String>,
    /// Repositories with their rules, one row each.
    pub repositories: Vec<RepositoryRule>,
    /// Owners listed when GitHub search refuses a scanned login.
    pub enumerate_owners: Vec<String>,
    /// Days an issue counts as new (1–30).
    pub lookback_days: u32,
    /// Label that marks a pull request ready for a human.
    pub ready_label: String,
    /// Excluded authors: every pull request and issue they author is left out of the dashboard. Being listed
    /// does not make a login a bot.
    pub skip_authors: Vec<String>,
    /// Logins treated as bots besides GitHub's `Bot` accounts and `[bot]` logins: never a waiting human.
    #[serde(default = "default_known_bots")]
    pub known_bots: Vec<String>,
    /// Show the user's MemCan TODOs.
    pub include_todos: bool,
    /// MemCan projects read besides the repositories' short names.
    pub memcan_extra_projects: Vec<String>,
    /// Daily automatic refresh on.
    pub schedule_enabled: bool,
    /// Its local time, `HH:MM` (24 h, 15-minute steps).
    pub schedule_time: String,
    /// IANA time zone; null until the browser sends one.
    pub time_zone: Option<String>,
}

impl Default for DashboardSettings {
    /// The defaults of plan §F.6 (an absent settings row means these).
    fn default() -> Self {
        Self {
            linked_logins: Vec::new(),
            repositories: Vec::new(),
            enumerate_owners: Vec::new(),
            lookback_days: 7,
            ready_label: "ready-for-human".to_owned(),
            skip_authors: vec!["dependabot".to_owned(), "dependabot[bot]".to_owned()],
            known_bots: default_known_bots(),
            include_todos: true,
            memcan_extra_projects: Vec::new(),
            schedule_enabled: true,
            schedule_time: "06:00".to_owned(),
            time_zone: None,
        }
    }
}

/// Review bots without a `[bot]` suffix: the default known bots.
pub const DEFAULT_KNOWN_BOTS: [&str; 3] = ["thepastaclaw", "Claudius-Maginificent", "DCG-Claude"];

/// [`DEFAULT_KNOWN_BOTS`] as a list (also the value of a settings document without `known_bots`).
pub fn default_known_bots() -> Vec<String> {
    DEFAULT_KNOWN_BOTS
        .iter()
        .map(|&bot| bot.to_owned())
        .collect()
}

/// Settings of one repository row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RepositoryRule {
    /// The repository.
    pub repo: RepoRef,
    /// Display alias prefix (`[A-Z0-9][A-Z0-9-]{0,11}`); null = the bare repository name.
    pub alias: Option<String>,
    /// Include its open issues.
    pub watch_issues: bool,
    /// File naming the owned paths; null = no ownership policy.
    pub ownership_policy: Option<PolicyRef>,
    /// Release and long-lived branch patterns; empty = the built-ins.
    pub branch_patterns: Vec<String>,
}

/// An ownership policy file (JSON format of `stale_prs_are_bad`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PolicyRef {
    /// Repository holding the file.
    pub repo: RepoRef,
    /// Relative path ending `.json`.
    pub path: String,
}

/// Response of `GET`/`PUT /api/v1/settings/dashboard`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DashboardSettingsView {
    /// The user's settings (the defaults when never saved).
    pub settings: DashboardSettings,
    /// The defaults ("Reset to defaults").
    pub defaults: DashboardSettings,
    /// The signed-in login, always scanned.
    pub own_login: String,
    /// MemCan is configured on this server.
    pub memcan_configured: bool,
    /// Effective MemCan projects: the repositories' short names plus the extras, deduplicated.
    pub memcan_projects: Vec<String>,
    /// Repositories and owners of these settings the GitHub token could not see at the latest refresh.
    pub unreachable: Vec<String>,
}

/// Body of `PUT /api/v1/settings/dashboard/time-zone`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SetTimeZoneRequest {
    /// IANA time zone name.
    pub time_zone: String,
}

/// Stable key of a work item, the normative form (`docs/contracts/http-api.md` "Item keys"):
/// `<owner>~<repo>~pr~<number>`, `<owner>~<repo>~issue~<number>` or `memcan~<uuid>`. Built and parsed only here;
/// deserialisation rejects any other form.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, TS)]
pub struct ItemKey(String);

/// JSON Schema pattern of [`ItemKey`]: exactly what [`ItemKey::parse`] accepts (repository not `.` or `..`, number
/// 1..=4294967295 without leading zeros).
const ITEM_KEY_PATTERN: &str = concat!(
    "^(memcan~[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
    "|[A-Za-z0-9_-]{1,39}",
    "~([A-Za-z0-9_-][A-Za-z0-9._-]{0,99}|\\.[A-Za-z0-9_-][A-Za-z0-9._-]{0,98}|\\.\\.[A-Za-z0-9._-]{1,98})",
    "~(pr|issue)",
    "~([1-9][0-9]{0,8}|[1-3][0-9]{9}|4[01][0-9]{8}|42[0-8][0-9]{7}|429[0-3][0-9]{6}|4294[0-8][0-9]{5}",
    "|42949[0-5][0-9]{4}|429496[0-6][0-9]{3}|4294967[01][0-9]{2}|42949672[0-8][0-9]|429496729[0-5]))$",
);

/// What an [`ItemKey`] names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemTarget {
    /// A pull request.
    PullRequest {
        /// Repository.
        repo: RepoRef,
        /// Number.
        number: u32,
    },
    /// An issue.
    Issue {
        /// Repository.
        repo: RepoRef,
        /// Number.
        number: u32,
    },
    /// A MemCan TODO.
    Todo(Uuid),
}

/// A string is not an item key.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not an item key (<owner>~<repo>~pr|issue~<number> or memcan~<uuid>)")]
pub struct ItemKeyError(pub String);

/// A GitHub login: 1–39 ASCII letters, digits, `-` or `_` (Enterprise Managed Users).
fn valid_owner(owner: &str) -> bool {
    (1..=39).contains(&owner.len())
        && owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// A GitHub repository name: 1–100 ASCII letters, digits, `.`, `_` or `-`, not `.` or `..`.
fn valid_repo(name: &str) -> bool {
    (1..=100).contains(&name.len())
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

impl ItemKey {
    /// The key of `target`, owner and repository lowercased (GitHub names are case-insensitive); fails for an
    /// owner or repository name GitHub does not allow, or number 0.
    pub fn new(target: &ItemTarget) -> Result<Self, ItemKeyError> {
        let (repo, kind, number) = match target {
            ItemTarget::Todo(uuid) => return Ok(Self(format!("memcan~{}", uuid.hyphenated()))),
            ItemTarget::PullRequest { repo, number } => (repo, "pr", number),
            ItemTarget::Issue { repo, number } => (repo, "issue", number),
        };
        let key = format!("{}~{}~{kind}~{number}", repo.owner, repo.name).to_ascii_lowercase();
        if valid_owner(&repo.owner) && valid_repo(&repo.name) && *number > 0 {
            Ok(Self(key))
        } else {
            Err(ItemKeyError(key))
        }
    }

    /// Parses a key in its normative form; owner and repository in any case (lowercased).
    pub fn parse(key: &str) -> Result<Self, ItemKeyError> {
        let target = Self::target_of(key).ok_or_else(|| ItemKeyError(key.to_owned()))?;
        let built = Self::new(&target)?;
        let canonical = match target {
            ItemTarget::Todo(_) => built.0 == key,
            ItemTarget::PullRequest { .. } | ItemTarget::Issue { .. } => {
                built.0.eq_ignore_ascii_case(key)
            }
        };
        if canonical {
            Ok(built)
        } else {
            Err(ItemKeyError(key.to_owned()))
        }
    }

    /// The key as sent on the wire.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// What the key names.
    pub fn target(&self) -> ItemTarget {
        // A constructed key always parses.
        Self::target_of(&self.0).unwrap_or(ItemTarget::Todo(Uuid::nil()))
    }

    fn target_of(key: &str) -> Option<ItemTarget> {
        let parts: Vec<&str> = key.split('~').collect();
        match parts.as_slice() {
            ["memcan", uuid] => Uuid::parse_str(uuid).ok().map(ItemTarget::Todo),
            [owner, name, kind, number] => {
                let repo = RepoRef {
                    owner: (*owner).to_owned(),
                    name: (*name).to_owned(),
                };
                let number = number.parse().ok()?;
                match *kind {
                    "pr" => Some(ItemTarget::PullRequest { repo, number }),
                    "issue" => Some(ItemTarget::Issue { repo, number }),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

impl Serialize for ItemKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ItemKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let key = String::deserialize(deserializer)?;
        Self::parse(&key).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for ItemKey {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ItemKey".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type": "string", "pattern": ITEM_KEY_PATTERN})
    }
}

impl fmt::Display for ItemKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A request addressed to a scanned login, outside the ranked list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Mention {
    /// Only a bounded part of this conversation was read.
    #[serde(default)]
    pub partly_read: bool,
    /// Stable opaque identity of this post on this item (used by Hide).
    pub id: String,
    /// The containing item.
    pub item_key: ItemKey,
    /// Pull request or issue.
    pub kind: GithubKind,
    /// Repository.
    pub repo: RepoRef,
    /// Item number.
    pub number: u32,
    /// Item title, plain text.
    pub title: String,
    /// Mentioning author; a deleted account is shown as `ghost`.
    pub author: String,
    /// GitHub Bot type, [bot] suffix or the user's known bots.
    pub is_bot: bool,
    /// When the mentioning post was created.
    pub created_at: Timestamp,
    /// One line of the mentioning text, plain text only.
    pub text: String,
    /// Server-built GitHub link to the mentioning post.
    pub url: String,
}
