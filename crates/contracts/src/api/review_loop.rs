//! Review loop (ADR-023, `docs/review-workflow-plan.md`): the PR page, review-comment items with the LLM assessment,
//! owner decisions and their effects, rounds, carry-overs, Watch, per-user events, notification channels and the
//! decision feed for external agents. Integers only on the wire. Enums documented as open: clients treat unknown
//! values as opaque.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::api::analyses::AnalysisSummary;
use crate::api::dashboard::PluginNote;
use crate::api::tour::CiStatus;
use crate::common::{LineRange, PrRef};
use crate::ids::{
    AnalysisId, CarryEventId, CommentItemId, CommitSha, DecisionId, EffectId, FlowId, JobId,
    StepId, UserEventId,
};
use crate::time::Timestamp;

// ---------------------------------------------------------------------------------------------------------------
// Items (plan §2.1, §2.7)

/// Where an item comes from (plan §2.1 I1–I4). Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum CommentItemKind {
    /// I1: a review thread.
    Thread,
    /// I2: an item of a review body (no thread).
    ReviewBody,
    /// I3: an item of a human PR-level comment.
    PrComment,
    /// I4: a PR-level question built by the server from rounds (merge now, dismiss stale reviews).
    PrQuestion,
}

/// Who wrote the item: the bot rule (`Bot` type, `[bot]` login or Settings `known_bots`, RQ17).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AuthorKind {
    /// A person (also `claude` unless listed as a bot).
    Human,
    /// A bot by the bot rule.
    Bot,
    /// The server (PR-level questions).
    Diffpilot,
}

/// Item state (plan §2.7). Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    /// Reserved: the server does not set it; a pass running for an item shows as its `assessment_state` `running`.
    Assessing,
    /// Waiting for the owner's decision.
    Undecided,
    /// The owner asked about it in the Ask chat; the answer is pending or unread.
    AskedBack,
    /// Decided; no effect left for an executor.
    Decided,
    /// Decided; effects are open or claimed.
    Executing,
    /// Every effect reported done.
    Done,
    /// A repeat that carried an earlier decision without a question.
    Repeat,
    /// A repeat asked again (bot edit or code change), a person's decided thread raised again, or reopened by the
    /// owner.
    Reopened,
    /// Resolved on GitHub outside diffpilot; out of the queue, nothing recorded.
    Outside,
}

/// A blocker gate of claudius `severity` §2. Gate items are never grouped or bulk-decided (RQ4, RQ5). Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
pub enum Gate {
    /// Loss or misdirection of funds.
    #[serde(rename = "G-FUNDS")]
    Funds,
    /// Secret exposure.
    #[serde(rename = "G-SECRET")]
    Secret,
    /// Cryptographic weakness.
    #[serde(rename = "G-CRYPTO")]
    Crypto,
    /// Data loss or corruption.
    #[serde(rename = "G-DATA")]
    Data,
}

/// Who marked a gate on an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum GateSource {
    /// The LLM assessment named it.
    Assessment,
    /// The owner marked it.
    Owner,
}

/// A gate on an item and who set it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct GateMark {
    /// The gate.
    pub gate: Gate,
    /// Who set it.
    pub source: GateSource,
}

/// The lines an item discusses: at the head for a thread; for a body item naming `path:range`, at the review's
/// commit, which may predate the head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ItemLocation {
    /// Repository-relative path.
    pub path: String,
    /// The lines; null for a file-level or outdated thread without a head line.
    pub lines: Option<LineRange>,
}

/// thepastaclaw's own status of a thread, read from its status block (spike S13-1 M4); a GitHub fact, not a
/// decision. Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum BotThreadStatus {
    /// "Resolved".
    Resolved,
    /// "Deferred (maintainer decision)".
    Deferred,
    /// "Withdrawn".
    Withdrawn,
    /// "No longer applies".
    NoLongerApplies,
    /// "Disputed (left to the maintainers)".
    Disputed,
}

/// Facts diffpilot read from GitHub about an item (badges name the check).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ItemGithubFacts {
    /// Thread resolved on GitHub; null for items without a thread.
    pub resolved: Option<bool>,
    /// Who resolved the thread.
    pub resolved_by: Option<String>,
    /// Thread outdated (its lines changed since it was written); null without a thread.
    pub outdated: Option<bool>,
    /// The bot's own thread status, when it writes one.
    pub bot_status: Option<BotThreadStatus>,
    /// The comment text changed after the latest decision (normalised text hash, spike S13-1 K1).
    pub edited_after_decision: bool,
    /// Replies in the thread after the first comment.
    pub replies: u32,
    /// The bot's thepastaclaw `dedupe=` marker, when present.
    pub marker: Option<String>,
}

/// An item resolved or answered on GitHub outside diffpilot (US17).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct OutsideResolution {
    /// GitHub login that resolved it.
    pub by: String,
    /// When diffpilot first read it resolved.
    pub seen_at: Timestamp,
}

/// Whether an item has an LLM assessment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AssessmentState {
    /// A `verify_comments` pass is running or queued for it.
    Running,
    /// Assessed; see `assessment`.
    Assessed,
    /// Not assessed: asked without a recommendation ("Not assessed: <reason>").
    NotAssessed {
        /// Why.
        reason: NotAssessedReason,
    },
}

/// Why an item has no assessment. Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum NotAssessedReason {
    /// The user's daily assessment limit is reached.
    DailyLimit,
    /// The pass failed.
    Failed,
    /// The pass left this item out of its output.
    Missing,
    /// No assessment pass has run for it yet.
    NotRun,
}

/// A review-loop item with its assessment, decisions and effects (RC4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommentItem {
    /// Grounded point read from a versioned source; absent on inline threads and PR questions.
    #[serde(default)]
    #[ts(optional)]
    pub point: Option<CommentPoint>,
    /// Per-PR number of the linked point; each inline thread keeps its own item.
    #[serde(default)]
    #[ts(optional)]
    pub duplicate_of: Option<u32>,
    /// Database id.
    pub id: CommentItemId,
    /// Per-PR number shown as `C<no>`, assigned at first sight, never reused.
    pub no: u32,
    /// Source.
    pub kind: CommentItemKind,
    /// Stable key: the bot's `dedupe=` marker, else the thread node id; review id + ordinal for body items;
    /// `pr:<round>:merge` / `pr:<round>:dismiss` for PR-level questions.
    pub stable_key: String,
    /// Author's GitHub login (`diffpilot` for PR-level questions).
    pub author: String,
    /// Author kind.
    pub author_kind: AuthorKind,
    /// GitHub link (thread, review or comment); null for PR-level questions.
    pub url: Option<String>,
    /// Discussed lines.
    pub location: Option<ItemLocation>,
    /// Title: the bot's finding heading or the first line; plain text.
    pub title: String,
    /// Comment text (GitHub): plain text, rendered as text; links only to `https://github.com/…`.
    pub body: String,
    /// The bot's own severity words (e.g. `Blocking`), shown with the GitHub tag; a claim, not a fact.
    pub bot_severity: Option<String>,
    /// Gates from the assessment or the owner.
    pub gates: Vec<GateMark>,
    /// State.
    pub state: ItemState,
    /// Revision; a decision names the revision it saw (409 `item_changed` otherwise).
    pub revision: u32,
    /// Head when diffpilot first read it (not the head the comment was written on).
    pub first_seen_head: CommitSha,
    /// Bot round when first read; null before the first round.
    pub first_round: Option<u32>,
    /// Where GitHub says the comment was written, from the latest read; absent when that read gives no time (an
    /// item it no longer produces, a read stored before this was kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub written: Option<Written>,
    /// GitHub facts.
    pub github: ItemGithubFacts,
    /// Set when resolved outside diffpilot.
    pub outside: Option<OutsideResolution>,
    /// When a complete read of the pull request first no longer produced it (deleted on GitHub, or no longer an
    /// item by the read rules, such as a person's earlier review); absent while reads produce it. Such an item
    /// leaves the queue, "Decide the rest" and bulk decisions; its decisions stay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub gone_at: Option<Timestamp>,
    /// The item whose decision this repeat carried or re-asks (`C<no>`).
    pub repeat_of: Option<u32>,
    /// Assessment state.
    pub assessment_state: AssessmentState,
    /// The latest LLM assessment.
    pub assessment: Option<CommentAssessment>,
    /// Decisions, oldest first; the last is current.
    pub decisions: Vec<Decision>,
    /// Answers in the Ask chat about this item.
    pub ask_answers: u32,
}

/// The commit and time a comment was written on, as GitHub records them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Written {
    /// The bot round whose own review raised it (then `commit` and `at` are that round's head and end); absent for
    /// any other comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub round: Option<u32>,
    /// The commit its review was made on; absent for a PR comment or a review GitHub did not list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub commit: Option<CommitSha>,
    /// When it was written (the review's submission, the comment's creation).
    pub at: Timestamp,
}

/// Query of RC4.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ItemsQuery {
    /// Only items in this state.
    pub state: Option<ItemState>,
    /// Only items of this kind.
    pub kind: Option<CommentItemKind>,
}

// ---------------------------------------------------------------------------------------------------------------
// LLM assessment (plan §2.2); every text field is LLM text in the user's LLM language.

/// The LLM's validity reading ("The LLM says: …"). Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Validity {
    /// Valid.
    Valid,
    /// Already fixed at the assessed head.
    AlreadyFixed,
    /// Partly valid.
    PartlyValid,
    /// Invalid.
    Invalid,
}

/// Whether the fix belongs in this pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum NecessityVerdict {
    /// In scope.
    In,
    /// Out of scope.
    Out,
}

/// Necessity verdict with its reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Necessity {
    /// Verdict.
    pub verdict: NecessityVerdict,
    /// Evidence (in) or reason (out).
    pub reason: String,
    /// Better-fitting pull request (`owner/repo#n` or URL) for `out`.
    pub better_pr: Option<String>,
}

/// Size of a fix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "UPPERCASE")]
pub enum FixSize {
    /// Extra small.
    Xs,
    /// Small.
    S,
    /// Medium.
    M,
}

/// Where else the matter is handled. Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ElsewhereKind {
    /// An open pull request.
    OpenPr,
    /// An issue.
    Issue,
    /// An upstream project.
    Upstream,
}

/// A place where the matter is handled already.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Elsewhere {
    /// Kind.
    pub kind: ElsewhereKind,
    /// URL or `owner/repo#n`.
    pub reference: String,
    /// One line on the overlap.
    pub note: String,
}

/// A line read that supports the validity reading.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AssessmentEvidence {
    /// Repository-relative path at the head.
    pub path: String,
    /// 1-based line at the head.
    pub line: u32,
    /// What the line shows.
    pub note: String,
}

/// The LLM's recommendation; on the card under the evidence, the recommended option first (RQ7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Recommendation {
    /// Recommended decision.
    pub kind: DecisionKind,
    /// Why.
    pub reason: String,
    /// Target the decision needs (D4–D7).
    pub target: Option<DecisionTarget>,
    /// Short class of the reason; groups share kind and class or target (plan §2.4).
    pub reason_class: String,
}

/// The enclosing code of the discussed lines and what it does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CodeSummary {
    /// Repository-relative path at the head.
    pub path: String,
    /// Enclosing range (function or block).
    pub enclosing: LineRange,
    /// One paragraph on what the code does (LLM).
    pub description: String,
}

/// The LLM assessment of one item at one head (`verify_comments`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommentAssessment {
    /// Original host proposal, preserving targets beyond the legacy action form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub proposed_decision: Option<crate::agent::comment_report::ProposedDecision>,
    /// Evidence from the report, labelled LLM text.
    #[serde(default)]
    #[ts(optional)]
    pub evidence_text: Option<String>,
    /// Attributed prior decision; authority and grounding are verified by server ingest.
    #[serde(default)]
    #[ts(optional)]
    pub recorded_decision: Option<crate::agent::comment_report::RecordedDecision>,
    /// The job that wrote it.
    pub job: JobId,
    /// Head it was made at.
    pub head: CommitSha,
    /// When it was stored.
    pub assessed_at: Timestamp,
    /// Reused from this head: comment text, code at the lines and PR purpose unchanged since (fingerprint).
    pub reused_since: Option<CommitSha>,
    /// Validity.
    pub validity: Validity,
    /// Lines that support it.
    pub evidence: Vec<AssessmentEvidence>,
    /// The problem exists on the base already.
    pub pre_existing: Option<bool>,
    /// Category slug (claudius #118 catalog).
    pub category: String,
    /// Necessity.
    pub necessity: Necessity,
    /// Gates the LLM names.
    pub gates: Vec<Gate>,
    /// Where else it is handled.
    pub elsewhere: Vec<Elsewhere>,
    /// Fix sketch.
    pub fix_sketch: Option<String>,
    /// Fix size.
    pub fix_size: Option<FixSize>,
    /// Reply drafts (English, "per maintainer decision").
    pub reply_drafts: Vec<String>,
    /// Recommendation.
    pub recommendation: Recommendation,
    /// Enclosing code and its description.
    pub code: Option<CodeSummary>,
    /// Prompt version of the job.
    pub prompt_version: u32,
    /// Commit of the claudius checkout whose skills the job read ("Assessed with claudius <short sha>"); null when
    /// the runner gave none or the assessment predates it. A reused assessment keeps the commit it was made with.
    #[serde(default)]
    pub skills_commit: Option<String>,
}

// ---------------------------------------------------------------------------------------------------------------
// Decisions and effects (plan §2.3)

/// A decision (D1–D7; D8 is `reject` with a `blocker_gate` override; P1–P4 for PR-level questions). Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    /// D1: resolve, already fixed.
    Resolve,
    /// D2: fix in this pull request.
    FixHere,
    /// D3: reject (with a reason); D8 with a `blocker_gate` override.
    Reject,
    /// D4: handled in an existing pull request.
    HandledInPr,
    /// D5: create a follow-up pull request.
    FollowUpPr,
    /// D6: upstream issue (link or create).
    UpstreamIssue,
    /// D7: TODO, out of scope for this pull request.
    Todo,
    /// P1: merge now, remaining items become follow-ups.
    MergeWithFollowUps,
    /// P2: another round.
    AnotherRound,
    /// P3: dismiss the listed stale bot reviews.
    DismissStaleReviews,
    /// P4: keep the reviews.
    KeepReviews,
}

/// The target a decision needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DecisionTarget {
    /// D4: the existing pull request.
    ExistingPr {
        /// Its number in the same repository.
        number: u32,
    },
    /// D5: the follow-up pull request's title (prefilled; optional).
    FollowUpPr {
        /// Title.
        title: Option<String>,
    },
    /// D6: an issue URL, or null to create one.
    UpstreamIssue {
        /// Issue URL.
        url: Option<String>,
    },
    /// D7: the TODO title.
    Todo {
        /// Title.
        title: String,
    },
}

/// A decision against the assessment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Override {
    /// Fix although the necessity verdict is `out`.
    NecessityGate,
    /// D8: accept despite the gate (reason required, posted; thread always resolved).
    BlockerGate {
        /// The gate.
        gate: Gate,
    },
}

/// Where a decision was made. Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum DecisionVia {
    /// One question.
    Question,
    /// A group question.
    Group,
    /// "Decide the rest per recommendation".
    Bulk,
    /// The table page.
    Table,
    /// Carried from an earlier decision (repeat).
    Carried,
}

/// A stored decision (append-only) with its effects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Decision {
    /// Id.
    pub id: DecisionId,
    /// The item (`C<no>`).
    pub item_no: u32,
    /// Decision.
    pub kind: DecisionKind,
    /// Reason (D3, D8; posted as the reply).
    pub reason: Option<String>,
    /// Target (D4–D7).
    pub target: Option<DecisionTarget>,
    /// Override flag.
    pub r#override: Option<Override>,
    /// "Resolve this thread" as submitted: the explicit resolve decision.
    pub resolve_ticked: bool,
    /// The reply text that gets posted (English).
    pub reply_text: Option<String>,
    /// The owner's comment: stored, sent to the executing agent, never posted to GitHub.
    pub comment: Option<String>,
    /// The recommendation shown at decision time.
    pub recommendation: Option<Recommendation>,
    /// Where it was made.
    pub via: DecisionVia,
    /// The decision this one was carried from.
    pub carried_from: Option<DecisionId>,
    /// The decision this one supersedes.
    pub supersedes: Option<DecisionId>,
    /// Head at decision time.
    pub head: CommitSha,
    /// GitHub login of the deciding user (`carried` decisions: of the source decision).
    pub decided_by: String,
    /// When.
    pub decided_at: Timestamp,
    /// When the owner undid it (before any effect was claimed); its effects are cancelled and the item is asked
    /// again. Absent while it stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub undone_at: Option<Timestamp>,
    /// Effects in `seq` order.
    pub effects: Vec<Effect>,
}

/// Kind of an effect; also an executor capability. Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    /// Fix and push.
    Fix,
    /// Reply in the thread.
    Reply,
    /// Resolve the thread.
    Resolve,
    /// PR-level comment (replies to body and PR-comment items).
    PrComment,
    /// Create a draft follow-up pull request.
    FollowUpPr,
    /// Create an upstream issue (and the "Known limitation" body line).
    UpstreamIssue,
    /// Save a TODO.
    Todo,
    /// Dismiss a review.
    DismissReview,
}

/// Who executes an effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Executor {
    /// An external agent through the decision feed.
    Agent,
    /// diffpilot[bot] (phase 3).
    DiffpilotBot,
}

/// Effect state. Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum EffectState {
    /// Waits for the effect named by `after`.
    Waiting,
    /// Claimable.
    Open,
    /// Claimed under a lease.
    Claimed,
    /// Reported done.
    Done,
    /// Reported failed.
    Failed,
    /// Cancelled by a superseding decision before it was claimed.
    Cancelled,
}

/// Parameters of an effect, set by the server's effect function.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct EffectParams {
    /// Text to post (reply, PR-level comment, dismissal message).
    pub text: Option<String>,
    /// GitHub thread node id (reply, resolve).
    pub thread_id: Option<String>,
    /// GitHub review node id (dismiss; PR-level replies to a review's items).
    pub review_id: Option<String>,
    /// Title (follow-up PR, issue, TODO).
    pub title: Option<String>,
    /// Existing issue URL (D6 with a link).
    pub url: Option<String>,
    /// Items listed in a follow-up PR (P1), `C<no>`.
    pub item_nos: Vec<u32>,
}

/// The claim on an effect's decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ClaimInfo {
    /// Name of the claiming access token.
    pub token_name: String,
    /// Lease end; renewed by each report.
    pub lease_until: Timestamp,
}

/// Outcome an agent reports for an effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ReportOutcome {
    /// Done, with a reference (commit, comment URL, PR, TODO id).
    Done,
    /// Failed, with a reason.
    Failed,
    /// Given back to the feed.
    Released,
}

/// A stored agent report ("Agent <token name> reports: …").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct EffectReport {
    /// Outcome.
    pub outcome: ReportOutcome,
    /// Reference (commit SHA, URL, TODO id).
    pub reference: Option<String>,
    /// Message (failure reason).
    pub message: Option<String>,
    /// Reporting token's name.
    pub token_name: String,
    /// When.
    pub at: Timestamp,
}

/// One effect of a decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Effect {
    /// Id.
    pub id: EffectId,
    /// Order within the decision, 1-based.
    pub seq: u32,
    /// `seq` of the effect this one waits for (a reply after a fix waits for its commit).
    pub after: Option<u32>,
    /// Kind.
    pub kind: EffectKind,
    /// Executor.
    pub executor: Executor,
    /// Parameters.
    pub params: EffectParams,
    /// State.
    pub state: EffectState,
    /// Current claim.
    pub claim: Option<ClaimInfo>,
    /// Reports, oldest first.
    pub reports: Vec<EffectReport>,
    /// When a server read found the effect on GitHub (fact badge).
    pub seen_on_github_at: Option<Timestamp>,
    /// Last change.
    pub updated_at: Timestamp,
}

/// Body of RC8 (`POST …/comments/items/{item}/decisions`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DecideRequest {
    /// Decision.
    pub kind: DecisionKind,
    /// Reason (required for D3 and D8).
    pub reason: Option<String>,
    /// Target (D4, D6, D7; optional title for D5).
    pub target: Option<DecisionTarget>,
    /// Override flag.
    pub r#override: Option<Override>,
    /// "Resolve this thread" as submitted.
    pub resolve_ticked: bool,
    /// Reply text to post (edited preview); null for the server's default.
    pub reply_text: Option<String>,
    /// The owner's comment for the executor; never posted.
    pub comment: Option<String>,
    /// The item revision the decision is based on.
    pub item_revision: u32,
    /// The `plan_digest` of the RC16 preview the owner confirmed (PR-level decisions); when the plan computed at
    /// storing differs, the decision is refused (409 `conflict`). Absent: stored as planned then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub plan_digest: Option<String>,
}

/// How a batch was decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum BatchVia {
    /// A group question.
    Group,
    /// "Decide the rest per recommendation" after confirmation.
    Bulk,
    /// The table page.
    Table,
}

/// One row of a decision batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DecisionBatchRow {
    /// The item, `C<no>`.
    pub item_no: u32,
    /// The decision fields.
    #[serde(flatten)]
    #[ts(flatten)]
    pub decision: DecideRequest,
}

/// Body of RC9 (`POST …/comments/decision-batches`); 201 `Decision[]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DecisionBatchRequest {
    /// Where.
    pub via: BatchVia,
    /// One row per item.
    pub rows: Vec<DecisionBatchRow>,
}

/// Body of the item actions Undo, Reopen and Skip: the item revision the reader saw (409 `item_changed`
/// otherwise).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ItemActionRequest {
    /// The item revision.
    pub item_revision: u32,
}

/// Body of the gate mark route: the owner marks or clears a gate (clearing removes the owner's mark only; a gate
/// the assessment names stays).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct GateMarkRequest {
    /// The gate.
    pub gate: Gate,
    /// Mark (true) or clear (false).
    pub marked: bool,
    /// The item revision.
    pub item_revision: u32,
}

/// An effect a decision would get ("What happens next", the confirmation list); not stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PlannedEffect {
    /// Order, 1-based.
    pub seq: u32,
    /// `seq` it waits for.
    pub after: Option<u32>,
    /// Kind.
    pub kind: EffectKind,
    /// Executor.
    pub executor: Executor,
    /// Parameters (the reply text that gets posted is in `text`).
    pub params: EffectParams,
}

/// Body of the effects preview: the decisions the reader is about to submit (the revisions are not checked).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct EffectsPreviewRequest {
    /// One row per item.
    pub rows: Vec<DecisionBatchRow>,
}

/// The effects one decision would get.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct EffectsPreview {
    /// The item, `C<no>`.
    pub item_no: u32,
    /// Effects in order.
    pub effects: Vec<PlannedEffect>,
    /// For a PR-level decision ("Merge now", dismissing stale reviews): a digest of what its plan rests on (the
    /// items it follows up and their revisions, or the reviews it dismisses); send it back as the decision's
    /// `plan_digest`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub plan_digest: Option<String>,
}

/// Why an undecided item is not part of "Decide the rest per recommendation". Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum LeftOutReason {
    /// A gate item: decided one by one (RQ5).
    Gate,
    /// No LLM assessment, so no recommendation.
    NotAssessed,
    /// A repeat asked again: its reason needs a look.
    AskedAgain,
    /// The recommendation lacks the reason or target its decision needs.
    Incomplete,
}

/// An undecided item left out of "Decide the rest".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LeftOutItem {
    /// The item, `C<no>`.
    pub item_no: u32,
    /// Why.
    pub reason: LeftOutReason,
}

/// One row of the "Decide the rest" confirmation list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DecideRestRow {
    /// The decision submitted on confirmation (RC9 `via: bulk`).
    pub row: DecisionBatchRow,
    /// The recommendation it follows.
    pub recommendation: Recommendation,
    /// Its effects.
    pub effects: Vec<PlannedEffect>,
}

/// The "Decide the rest per recommendation" confirmation list; nothing is stored until RC9 confirms it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DecideRest {
    /// Rows to decide; their count is the number shown.
    pub rows: Vec<DecideRestRow>,
    /// Undecided items left out, gate items included.
    pub left_out: Vec<LeftOutItem>,
}

// ---------------------------------------------------------------------------------------------------------------
// Rounds, carry-overs (plan §2.5, §2.6)

/// Decisions of one kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct KindCount {
    /// Kind.
    pub kind: DecisionKind,
    /// Count.
    pub count: u32,
}

/// A bot round: a thepastaclaw review with `phase=final sha=<head>` (spike S13-1 R1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Round {
    /// 1-based.
    pub number: u32,
    /// The round's review (GitHub link).
    pub url: String,
    /// Head reviewed.
    pub head: CommitSha,
    /// Submitted.
    pub finished_at: Timestamp,
    /// The bot's review state as GitHub reports it (`CHANGES_REQUESTED`, `APPROVED`, `DISMISSED`, …).
    pub verdict: String,
    /// Items first seen in this round.
    pub new: u32,
    /// Items still open at its end.
    pub still_open: u32,
    /// Repeats carried.
    pub carried: u32,
    /// Repeats asked again.
    pub asked_again: u32,
    /// Undecided items.
    pub undecided: u32,
    /// Decisions by kind.
    pub decisions: Vec<KindCount>,
}

/// Carry-over event kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum CarryKind {
    /// The earlier decision was carried without a question.
    Carried,
    /// The repeat was asked again.
    AskedAgain,
}

/// Why a repeat was asked again (RQ3). Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ReaskReason {
    /// The bot edited the finding text after the decision.
    BotEdited,
    /// A commit after the decision's head changed the code at the thread's lines.
    CodeChanged,
}

/// Evidence of a carry-over event, read by the server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CarryEvidence {
    /// Verbatim grounded quote of a restated point, when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub quote: Option<String>,
    /// GitHub link of the repeat.
    pub url: Option<String>,
    /// Normalised text hash at decision time (hex).
    pub body_hash_before: Option<String>,
    /// Normalised text hash now (hex).
    pub body_hash_after: Option<String>,
    /// The commit that changed the lines.
    pub commit: Option<CommitSha>,
    /// The decided lines mapped forward to the head.
    pub lines: Option<LineRange>,
}

/// A carry-over event (V5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CarryEvent {
    /// Id.
    pub id: CarryEventId,
    /// Pull request.
    pub pr: PrRef,
    /// The repeat, `C<no>`; may equal the source item when the bot re-raised it in place (spike S13-1 E2).
    pub item_no: u32,
    /// The item of the source decision, `C<no>`.
    pub source_item_no: u32,
    /// The decision carried or re-asked.
    pub source_decision: DecisionId,
    /// Its kind.
    pub source_kind: DecisionKind,
    /// Kind.
    pub kind: CarryKind,
    /// Why asked again.
    pub reason: Option<ReaskReason>,
    /// Evidence.
    pub evidence: CarryEvidence,
    /// Effects of the carried decision not yet done.
    pub effects_pending: u32,
    /// When.
    pub at: Timestamp,
}

/// Carry-over counts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CarryCounts {
    /// Carried.
    pub carried: u32,
    /// Asked again: bot edited.
    pub asked_again_bot_edited: u32,
    /// Asked again: code changed.
    pub asked_again_code_changed: u32,
    /// Carried decisions whose effects are pending.
    pub effects_pending: u32,
}

/// Counts of one pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrCarryCounts {
    /// Pull request.
    pub pr: PrRef,
    /// Counts.
    pub counts: CarryCounts,
}

/// Query of RC11.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CarryOverQuery {
    /// One pull request, `owner/name/number`; absent for all.
    pub pr: Option<String>,
    /// Opaque cursor of the next page.
    pub cursor: Option<String>,
}

/// Response of RC11: events newest first, counts per PR and in total.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CarryOverPage {
    /// Totals over the scope.
    pub totals: CarryCounts,
    /// Per pull request (all-PR scope; one row for a PR scope).
    pub per_pr: Vec<PrCarryCounts>,
    /// Events, newest first.
    pub events: Vec<CarryEvent>,
    /// Cursor of the next page.
    pub next_cursor: Option<String>,
}

// ---------------------------------------------------------------------------------------------------------------
// Comments overview, PR page, code context (RC1, RC2, RC5)

/// Why the last GitHub read failed. Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ReadFailure {
    /// GitHub's rate limit.
    RateLimited,
    /// GitHub did not answer as expected.
    Unavailable,
    /// The user must sign in again.
    SignInRequired,
}

/// The last read of the PR's threads, reviews and comments.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct GithubRead {
    /// Last successful read; null before the first.
    pub read_at: Option<Timestamp>,
    /// Threads read.
    pub threads: u32,
    /// Every page was read (no cap hit).
    pub complete: bool,
    /// Reviews read.
    pub reviews: u32,
    /// PR-level comments read.
    pub comments: u32,
    /// The latest attempt failed.
    pub failure: Option<ReadFailure>,
    /// Next automatic read.
    pub next_read_at: Option<Timestamp>,
}

/// Assessment progress of a PR ("LLM assessment running: 4 of 9 comments").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AssessmentProgress {
    /// A pass is queued or running.
    pub running: bool,
    /// Items assessed at the current head.
    pub assessed: u32,
    /// Items needing an assessment.
    pub total: u32,
    /// Items left out by the daily limit.
    pub limited: u32,
    /// Head of the latest pass.
    pub head: Option<CommitSha>,
    /// End of the latest pass.
    pub at: Option<Timestamp>,
}

/// Item counts of a PR.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommentCounts {
    /// Unresolved threads on GitHub (fact).
    pub unresolved_threads: u32,
    /// Undecided questions.
    pub undecided: u32,
    /// Repeats asked again.
    pub asked_again: u32,
    /// Decisions with open or claimed effects.
    pub waiting_for_agents: u32,
    /// Failed effects.
    pub failed_effects: u32,
    /// Items resolved outside diffpilot.
    pub outside: u32,
    /// All items.
    pub total: u32,
}

/// PR state badge (plan §3.9 B1–B5); first match wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PrBadge {
    /// B1.
    BlockedFailing,
    /// B2.
    QuestionsWaiting,
    /// B3.
    Executing,
    /// B4.
    WaitingForBotRound,
    /// B5.
    ReadyToMerge,
}

/// The badge with the facts behind it (tooltip).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrBadgeView {
    /// Badge; null when none applies or inputs are unknown.
    pub badge: Option<PrBadge>,
    /// Facts diffpilot read that select it.
    pub facts: Vec<String>,
    /// Inputs not known (no B5 while any is listed).
    pub missing: Vec<String>,
}

/// Why an entry is at its place in the queue (plan §2.4 order). Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum QueueReason {
    /// 1. PR-level question.
    PrQuestion,
    /// 2. Repeat asked again.
    AskedAgain,
    /// 3. Answer to the owner's question.
    Answered,
    /// 4. Gate item.
    Gate,
    /// 5. Marked Blocking by the reviewer.
    Blocking,
    /// 6. Other single item.
    Single,
    /// 7. Group.
    Group,
}

/// One question in the queue: a single item or a group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct QueueEntry {
    /// Items, `C<no>`; two or more for a group.
    pub item_nos: Vec<u32>,
    /// Reason.
    pub reason: QueueReason,
    /// The reader skipped it ("Skip for now") at its current revision; skipped entries come last. Absent when not.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub skipped: bool,
}

/// Response of RC2.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommentsOverview {
    /// Pull request.
    pub pr: PrRef,
    /// Title (GitHub).
    pub title: String,
    /// GitHub link.
    pub url: String,
    /// Head.
    pub head: CommitSha,
    /// PR purpose in one sentence (LLM), or the analysis's PR context.
    pub purpose: Option<String>,
    /// The last read.
    pub read: GithubRead,
    /// Counts.
    pub counts: CommentCounts,
    /// Rounds, oldest first.
    pub rounds: Vec<Round>,
    /// Assessment progress.
    pub assessment: AssessmentProgress,
    /// Questions in order.
    pub queue: Vec<QueueEntry>,
    /// State badge.
    pub badge: PrBadgeView,
    /// Latest analysis of this PR, if any (the view menu needs one for Flows, Files and Dependencies).
    pub analysis: Option<AnalysisId>,
}

/// Comments summary on the PR page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommentsSummary {
    /// Counts.
    pub counts: CommentCounts,
    /// Bot rounds so far.
    pub rounds: u32,
    /// Assessment progress.
    pub assessment: AssessmentProgress,
    /// The last read.
    pub read: GithubRead,
}

/// Response of RC1: the PR page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrPage {
    /// Pull request.
    pub pr: PrRef,
    /// Title.
    pub title: String,
    /// Author's GitHub login (`ghost` for a deleted account).
    pub author: String,
    /// GitHub link.
    pub url: String,
    /// GitHub state (`OPEN`, `CLOSED`, `MERGED`); empty while GitHub was never read (`comments.read.failure`).
    pub state: String,
    /// Draft.
    pub draft: bool,
    /// Base branch; empty while GitHub was never read.
    pub base: String,
    /// The PR whose branch is the base (stacked PR).
    pub stack_parent: Option<PrRef>,
    /// Head.
    pub head: CommitSha,
    /// CI of the head; null when not read.
    pub ci: Option<CiStatus>,
    /// Plug-in statements (PR Hygiene, thepastaclaw), attributed.
    pub plugin_notes: Vec<PluginNote>,
    /// The reader's watch; null when not watching.
    pub watch: Option<WatchedPr>,
    /// State badge.
    pub badge: PrBadgeView,
    /// The latest analysis (the progress content); null without one.
    pub analysis: Option<AnalysisSummary>,
    /// Process PR comments.
    pub comments: CommentsSummary,
}

/// A source of notes about code elsewhere in diffpilot. Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum NoteSource {
    /// A Flows flag on the lines.
    Flows,
    /// A Files incidental-change note.
    Files,
    /// Dependencies (changed callers).
    Dependencies,
    /// The PR Hygiene plug-in.
    PrHygiene,
    /// The thepastaclaw status plug-in.
    Thepastaclaw,
}

/// A note about this code elsewhere (an icon opening its place in a new tab).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct NoteElsewhere {
    /// Source.
    pub source: NoteSource,
    /// Accessible name and tooltip.
    pub label: String,
    /// Count shown on the icon.
    pub count: Option<u32>,
    /// Where it opens (app route or plug-in home).
    pub url: String,
}

/// A flow step whose range contains the commented lines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FlowStepRef {
    /// Flow.
    pub flow: FlowId,
    /// 1-based position of the flow in the tour.
    pub flow_position: u32,
    /// Flow title (LLM).
    pub flow_title: String,
    /// Step.
    pub step: StepId,
    /// 1-based number of the step within its flow.
    pub step_number: u32,
}

/// One line of code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CodeLine {
    /// 1-based line at the head.
    pub line: u32,
    /// Text.
    pub text: String,
}

/// Query of RC5: lines `from..=to` (defaults: the enclosing range, at most 40 lines).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ContextQuery {
    /// First line.
    pub from: Option<u32>,
    /// Last line.
    pub to: Option<u32>,
}

/// Response of RC5.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CodeContext {
    /// Repository-relative path.
    pub path: String,
    /// Head the lines come from.
    pub head: CommitSha,
    /// Lines served.
    pub lines: Vec<CodeLine>,
    /// Lines in the file.
    pub total_lines: u32,
    /// The commented lines.
    pub marked: Option<LineRange>,
    /// Enclosing range and description (LLM), when assessed.
    pub code: Option<CodeSummary>,
    /// The analysis the flow steps come from; null without one.
    pub analysis: Option<AnalysisId>,
    /// Head of that analysis (labelled when older than the PR head).
    pub analysis_head: Option<CommitSha>,
    /// Flow steps containing the lines.
    pub in_flows: Vec<FlowStepRef>,
    /// Notes elsewhere.
    pub notes: Vec<NoteElsewhere>,
}

/// Response of RC10: decisions with effects, claims and reports; carry events; feed state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommentLog {
    /// Agents subscribed now.
    pub subscribers: Vec<FeedSubscriber>,
    /// Open or claimed effects.
    pub open_effects: u32,
    /// Failed effects.
    pub failed_effects: u32,
    /// Decisions, oldest first.
    pub decisions: Vec<Decision>,
    /// Carry events of this PR, oldest first.
    pub carry_events: Vec<CarryEvent>,
}

// ---------------------------------------------------------------------------------------------------------------
// Watch and per-user events (plan §2.8, RW1–RW3)

/// State of a watch. Open enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum WatchState {
    /// Probed on schedule.
    Active,
    /// A pass waits for the bot's round to end.
    WaitingForBotRound,
    /// An assessment job is queued or running.
    JobRunning,
    /// Today's automatic assessment limit is reached.
    LimitReached,
    /// No GitHub token pair: "sign in again".
    Paused,
    /// The PR was merged or closed; watching ended.
    Ended,
}

/// The latest automatic assessment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AutoPass {
    /// When it ended.
    pub at: Timestamp,
    /// Questions waiting for a decision when it ended.
    pub questions: u32,
}

/// A watched pull request (RW2, V6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WatchedPr {
    /// Pull request.
    pub pr: PrRef,
    /// Title.
    pub title: String,
    /// Watching since.
    pub since: Timestamp,
    /// Last probe.
    pub last_probe_at: Option<Timestamp>,
    /// Latest automatic assessment.
    pub last_auto_pass: Option<AutoPass>,
    /// State.
    pub state: WatchState,
    /// When the watch ended (merged or closed).
    pub ended_at: Option<Timestamp>,
    /// State badge.
    pub badge: PrBadgeView,
}

/// Response of RW2.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WatchList {
    /// Watches, newest first.
    pub watches: Vec<WatchedPr>,
    /// Most watched PRs per user.
    pub limit: u32,
    /// Assessment passes charged to the reader in the last 24 hours (automatic and question passes).
    pub passes_today: u32,
    /// Passes per user and 24 hours; automatic passes stop at it.
    pub daily_limit: u32,
}

/// Kind of a per-user event; also what a channel subscribes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum UserEventKind {
    /// A pass added questions on a watched PR.
    QuestionsReady,
    /// Agents reported every decision of a round executed.
    RoundExecuted,
    /// A channel test.
    Test,
}

/// A per-user event (RW3 SSE data, push channels).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct UserEvent {
    /// Id.
    pub id: UserEventId,
    /// Kind.
    pub kind: UserEventKind,
    /// Pull request; null for `test`.
    pub pr: Option<PrRef>,
    /// Questions (questions_ready) or decisions (round_executed).
    pub count: u32,
    /// Plain text, e.g. "acme/node-client #482: 5 questions ready".
    pub text: String,
    /// The path of the PR's Comments page on this instance (`/pr/<owner>/<name>/<number>/comments`).
    pub url: Option<String>,
    /// When.
    pub at: Timestamp,
}

/// SSE data of RW3; the SSE `event` field is the `event` tag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum UserStreamEvent {
    /// A stored per-user event, oldest first; a reconnect resends nothing older than the connection.
    UserEvent {
        /// The event.
        user_event: UserEvent,
    },
    /// Sent every 15 s while the stream is open; each one re-checks the session or token.
    Heartbeat {
        /// Server time of sending.
        at: Timestamp,
    },
}

impl UserStreamEvent {
    /// The SSE event name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::UserEvent { .. } => "user_event",
            Self::Heartbeat { .. } => "heartbeat",
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Notification channels and agent push targets (plan §5.5, RN1–RN3, RF5)

/// Push channel kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    /// ntfy: server URL, topic, optional access token.
    Ntfy,
    /// Generic webhook: URL, optional `Authorization` value.
    Webhook,
}

/// Outcome of a delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryOutcome {
    /// Delivered (2xx).
    Delivered,
    /// Failed after its attempts.
    Failed,
}

/// The latest delivery of a channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LastDelivery {
    /// Event kind (`decision_available` deliveries of push targets show as null).
    pub event: Option<UserEventKind>,
    /// Outcome.
    pub outcome: DeliveryOutcome,
    /// When.
    pub at: Timestamp,
}

/// Delivery state of a channel or push target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DeliveryStatus {
    /// Latest delivery.
    pub last: Option<LastDelivery>,
    /// Latest error in plain words ("connection refused (3 attempts)").
    pub last_error: Option<String>,
    /// Failing since; null while healthy.
    pub failing_since: Option<Timestamp>,
}

/// A user's channel; the secret is never returned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct NotificationChannel {
    /// Kind.
    pub kind: ChannelKind,
    /// ntfy server URL or webhook URL.
    pub url: String,
    /// ntfy topic.
    pub topic: Option<String>,
    /// Last 4 characters of the stored secret, or null without one.
    pub secret_last4: Option<String>,
    /// Events it receives.
    pub events: Vec<UserEventKind>,
    /// On.
    pub enabled: bool,
    /// Delivery state.
    pub delivery: DeliveryStatus,
}

/// Response of RN1. The browser notification toggle stays per browser (local storage).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct NotificationSettings {
    /// Channels, at most one per kind.
    pub channels: Vec<NotificationChannel>,
}

/// What a save does with a channel's or push target's stored secret (an absent field keeps it).
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SecretChange {
    /// Keep the stored secret (or none).
    #[default]
    Keep,
    /// Store this secret instead (not empty).
    Replace {
        /// The new secret.
        value: String,
    },
    /// Remove the stored secret.
    Remove,
}

impl std::fmt::Debug for SecretChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Keep => "Keep",
            Self::Replace { .. } => "Replace([redacted])",
            Self::Remove => "Remove",
        })
    }
}

/// Body of RN2 (`PUT …/channels/{kind}`); 200 `NotificationChannel`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PutChannelRequest {
    /// URL (`http`/`https` only; 422 `scheme_not_allowed`).
    pub url: String,
    /// ntfy topic.
    pub topic: Option<String>,
    /// The secret: ntfy access token or webhook `Authorization` value.
    #[serde(default)]
    pub secret: SecretChange,
    /// Events.
    pub events: Vec<UserEventKind>,
    /// On.
    pub enabled: bool,
}

/// Response of RN3 (one attempt, no retry).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ChannelTestResult {
    /// Outcome.
    pub outcome: DeliveryOutcome,
    /// HTTP status when one was received.
    pub http_status: Option<u16>,
    /// Error in plain words.
    pub error: Option<String>,
}

/// An agent token's push target; the secret is never returned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PushTarget {
    /// Kind.
    pub kind: ChannelKind,
    /// ntfy server URL or webhook URL; null in the decision log for another user's token.
    pub url: Option<String>,
    /// ntfy topic; null in the decision log for another user's token.
    pub topic: Option<String>,
    /// Last 4 characters of the stored secret; null in the decision log for another user's token.
    pub secret_last4: Option<String>,
    /// Delivery state (in the decision log for another user's token without `last_error`).
    pub delivery: DeliveryStatus,
}

/// Body of RF5 (`PUT /api/v1/decision-feed/push-target`); 200 `PushTarget`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PutPushTargetRequest {
    /// Kind.
    pub kind: ChannelKind,
    /// URL (`http`/`https` only).
    pub url: String,
    /// ntfy topic.
    pub topic: Option<String>,
    /// The secret: ntfy access token or webhook `Authorization` value.
    #[serde(default)]
    pub secret: SecretChange,
}

/// JSON body of a generic webhook delivery (`POST`; plan §5.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WebhookPayload {
    /// Payload version, 1.
    pub v: u32,
    /// Event kind.
    pub event: UserEventKind,
    /// Plain text, e.g. "acme/node-client #482: 5 questions ready".
    pub text: String,
    /// The PR's Comments page.
    pub url: Option<String>,
    /// Pull request, `owner/name#number`.
    pub pr: Option<String>,
    /// When.
    pub at: Timestamp,
}

/// Event of an agent push message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AgentPushEvent {
    /// A decision within the token's capabilities became claimable.
    DecisionAvailable,
}

/// JSON body of an agent push-target webhook delivery (ntfy: the same fields as the message): ids and a fetch
/// link only, never reply text, reasons, comment bodies or secrets (T28).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AgentPushPayload {
    /// Payload version, 1.
    pub v: u32,
    /// Event.
    pub event: AgentPushEvent,
    /// The decision.
    pub decision_id: DecisionId,
    /// Pull request, `owner/name#number`.
    pub pr: String,
    /// API path to fetch the decision (RF1b).
    pub fetch: String,
}

// ---------------------------------------------------------------------------------------------------------------
// Decision feed (plan §5.2, RF1–RF4)

/// An agent subscribed to the feed (V3 header).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FeedSubscriber {
    /// Token name.
    pub token_name: String,
    /// Capabilities it declared.
    pub capabilities: Vec<EffectKind>,
    /// Holds a live stream now.
    pub connected: bool,
    /// Last request or stream activity.
    pub last_seen_at: Timestamp,
    /// Its push target, if registered.
    pub push: Option<PushTarget>,
}

/// Query of RF1 and RF2.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FeedQuery {
    /// Comma-separated `EffectKind` names the agent can execute.
    pub capabilities: String,
    /// Only this repository, `owner/name`.
    pub repo: Option<String>,
    /// Opaque cursor of the next page (RF1).
    pub cursor: Option<String>,
}

/// The item a feed decision is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FeedItem {
    /// `C<no>`.
    pub no: u32,
    /// Source.
    pub kind: CommentItemKind,
    /// GitHub link.
    pub url: Option<String>,
    /// Discussed lines.
    pub location: Option<ItemLocation>,
    /// Title.
    pub title: String,
    /// Comment text (GitHub; untrusted data).
    pub body: String,
}

/// A decision as an agent reads it (RF1 entries, RF1b).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FeedDecision {
    /// Pull request.
    pub pr: PrRef,
    /// The item.
    pub item: FeedItem,
    /// The decision with its effects; `comment` carries the owner's instructions for the executor.
    pub decision: Decision,
}

/// Response of RF1: claimable decisions within the capabilities, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DecisionFeed {
    /// Decisions.
    pub decisions: Vec<FeedDecision>,
    /// Cursor of the next page.
    pub next_cursor: Option<String>,
}

/// SSE data of RF2.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum FeedEvent {
    /// A decision within the subscriber's capabilities became claimable.
    DecisionAvailable {
        /// Decision.
        decision: DecisionId,
        /// Pull request.
        pr: PrRef,
        /// API path to fetch it (RF1b).
        fetch: String,
    },
    /// A decision left the feed (superseded or claimed by another agent).
    DecisionCancelled {
        /// Decision.
        decision: DecisionId,
        /// Pull request.
        pr: PrRef,
    },
    /// Keeps the stream open.
    Heartbeat {
        /// Server time.
        at: Timestamp,
    },
}

/// Body of RF3 (`POST …/decisions/{id}/claim`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ClaimRequest {
    /// Capabilities; every open effect of the decision must be within them (422 `capability_missing`).
    pub capabilities: Vec<EffectKind>,
}

/// Response of RF3: 201, the claimed decision and its lease (409 `claimed`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Claim {
    /// The decision.
    pub decision: FeedDecision,
    /// Lease end (30 minutes; renewed by each report).
    pub lease_until: Timestamp,
}

/// Body of RF4 (`POST …/effects/{id}/reports`); a repeated idempotency key is a no-op.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct EffectReportRequest {
    /// Outcome.
    pub outcome: ReportOutcome,
    /// Reference (required for `done`).
    pub reference: Option<String>,
    /// Message (required for `failed`).
    pub message: Option<String>,
    /// Client-generated key.
    pub idempotency_key: Uuid,
}

/// Point provenance, grounded in the source's cleaned GitHub text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommentPoint {
    /// Server-validated source URL.
    pub source_url: String,
    /// Submitted review body or PR comment.
    pub source_kind: CommentSourceKind,
    /// Distinct verbatim span: 12–600 characters, or the whole shorter source.
    pub quote: String,
}

/// Stored source kind; inline threads remain independent items.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum CommentSourceKind {
    /// Submitted, non-empty review body.
    ReviewBody,
    /// Non-empty PR comment.
    PrComment,
}

/// Host-assigned source role, independent of author membership.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum SourceRole {
    /// PR author's own text, or an earlier round of a round bot.
    Context,
    /// Commands, pending reviews, gate or status comments; omitted from the hand-in.
    Ignored,
    /// Everything else, including tenant members' comments.
    Source,
}

/// Read-only source version, including sources from which no points were read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommentSource {
    /// Source URL.
    pub url: String,
    /// Source kind.
    pub kind: CommentSourceKind,
    /// GitHub author login.
    pub author: String,
    /// Cleaned source text, retaining details content.
    pub body: String,
    /// SHA-256 of the cleaned text identifies this version.
    pub body_hash: String,
    /// Host-assigned source role.
    pub role: SourceRole,
    /// First successful assessment read of this source version.
    pub read_at: Option<Timestamp>,
}
