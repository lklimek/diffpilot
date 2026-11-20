//! The Files view: every file of the pull request with its hunks, who explains each changed line, and the
//! signed-in user's GitHub "Viewed" mark per file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::api::tour::UnexplainedReason;
use crate::common::{FileRange, LineRange, LineSide, NotAnalysedCategory, ProvenanceBadge};
use crate::ids::{CommitSha, FlowId, HunkId, StepId};

/// Response of `GET /api/v1/analyses/{id}/files`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FilesResponse {
    /// Every file of the diff, in diff (path) order.
    pub files: Vec<PrFile>,
    /// Whether `PrFile::viewed` was read from GitHub for every file.
    pub viewed_status: ViewedStatus,
    /// The pull request's head on GitHub when the Viewed state was read; null when not read. Marking Viewed is
    /// refused while it differs from the analysed head.
    pub current_head: Option<CommitSha>,
}

/// One file of the pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrFile {
    /// Path at the head commit (at the base commit for a deleted file); the path GitHub's Viewed mark uses.
    pub path: String,
    /// Path at the base commit of a renamed file.
    pub old_path: Option<String>,
    /// How the file changed.
    pub status: FileChangeStatus,
    /// Git treats the file as binary: no hunks.
    pub binary: bool,
    /// Added lines in the diff.
    pub added: u32,
    /// Removed lines in the diff.
    pub removed: u32,
    /// Group of the file tree: code, or recognised by a path rule.
    pub group: FileGroup,
    /// Why the file is never rendered; null when it is.
    pub not_rendered: Option<NotRenderedReason>,
    /// The user's GitHub Viewed mark; null when it was not read.
    pub viewed: Option<FileViewedState>,
    /// Hunks (runs of changed lines without context) in file order.
    pub hunks: Vec<FileHunk>,
}

/// How a file changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeStatus {
    /// New file.
    Added,
    /// Changed in place.
    Modified,
    /// Removed.
    Deleted,
    /// Renamed (possibly with changes).
    Renamed,
}

/// Group of a file in the file tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileGroup {
    /// No path rule matched.
    Code,
    /// Recognised by a path rule (or binary); not analysed.
    NotAnalysed {
        /// Section.
        category: NotAnalysedCategory,
        /// How it was recognised.
        badge: ProvenanceBadge,
        /// The matching rule or detector description.
        rule: String,
    },
}

/// Why a file is never rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum NotRenderedReason {
    /// Larger than the code reader's per-file limit.
    TooLarge,
    /// Generated file.
    Generated,
    /// Binary file.
    Binary,
}

/// The user's GitHub Viewed mark of a file (GraphQL `FileViewedState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum FileViewedState {
    /// Marked as viewed.
    Viewed,
    /// Not marked as viewed.
    Unviewed,
    /// Marked earlier; GitHub cleared the mark because new commits changed the file.
    Dismissed,
}

/// How completely the Viewed state was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ViewedStatus {
    /// Read for every file GitHub lists.
    Checked,
    /// Only the first files were read (read cap); the others are null.
    Partial,
    /// Not read (no token, GitHub refused or did not answer); nothing is assumed.
    NotChecked,
}

/// One hunk: a run of contiguous changed lines, removed and added together, without context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FileHunk {
    /// Hunk id of the pre-pass.
    pub id: HunkId,
    /// Removed lines (base side); null when the hunk removes nothing.
    pub base: Option<LineRange>,
    /// Added lines (head side); null when the hunk adds nothing.
    pub head: Option<LineRange>,
    /// Head line after which the removed lines were (0 = before line 1); set when `head` is null and the file
    /// exists at head.
    pub after_line: Option<u32>,
    /// The hunk's changed lines by owner, base parts first; together they hold every changed line once.
    pub parts: Vec<HunkPart>,
}

/// Changed lines of a hunk with one owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct HunkPart {
    /// Side of the lines.
    pub side: LineSide,
    /// The lines.
    pub range: LineRange,
    /// Where the lines are explained, or why they are in no flow.
    pub owner: PartOwner,
    /// Other steps (of any flow) whose lines include these lines, in Tour order.
    pub also_in: Vec<StepId>,
}

/// Where changed lines are explained, decided by the server from the same coverage that yields Unexplained.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PartOwner {
    /// The plan places the lines in this flow, and this step of it explains them.
    Step {
        /// The flow.
        flow_id: FlowId,
        /// The step.
        step_id: StepId,
    },
    /// The plan places the lines in this flow, but no step of it holds them: the flow is not analysed yet, or
    /// they are blank lines no step needs.
    Flow {
        /// The flow.
        flow_id: FlowId,
    },
    /// The plan places the lines in this flow, whose analysis names them an incidental change off its control flow:
    /// no step, one LLM sentence.
    Incidental {
        /// The flow.
        flow_id: FlowId,
        /// What changed there, as the LLM wrote it.
        text: String,
    },
    /// In no flow: "Unexplained — read these yourself".
    Unexplained {
        /// Why the server did not send them to the LLM; absent when the LLM did not explain them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        reason: Option<UnexplainedReason>,
    },
    /// In no flow: the file is recognised by a path rule.
    NotAnalysed {
        /// Section.
        category: NotAnalysedCategory,
        /// How it was recognised.
        badge: ProvenanceBadge,
        /// The matching rule.
        rule: String,
    },
    /// In no flow: a line rule covers them; sent to the LLM as context without the duty to explain them.
    ContextRule {
        /// How it was recognised.
        badge: ProvenanceBadge,
        /// The matching rule.
        rule: String,
    },
    /// In no flow: a block moved with identical text; says nothing about behaviour at the new place.
    IdenticalMove {
        /// Original block at the base commit.
        from: FileRange,
        /// New block at the head commit.
        to: FileRange,
    },
    /// In no flow: only whitespace changed; the code view shows the lines as unchanged.
    WhitespaceOnly,
}

/// Body of `PUT /api/v1/analyses/{id}/files/viewed`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SetViewedRequest {
    /// `PrFile::path` of a file of the analysis.
    pub path: String,
    /// True marks the file as viewed on GitHub, false removes the mark.
    pub viewed: bool,
}
