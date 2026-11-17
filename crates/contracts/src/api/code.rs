//! Code by line range and the classic diff (brief §7: the API never serves whole files).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::{LineRange, LineSide};

/// Query of the `lines` and `diff` endpoints: `path`, lines `from..=to` on `side`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RangeQuery {
    /// Repository-relative path on that side.
    pub path: String,
    /// `head` for final code; `base` for removed code of a deleted or emptied file.
    pub side: LineSide,
    /// First line, 1-based.
    pub from: u32,
    /// Last line, inclusive.
    pub to: u32,
}

impl RangeQuery {
    /// Most lines one request may ask for.
    pub const MAX_LINES: u32 = 2000;
}

/// Lines of a file with the deterministic gutter facts for them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LinesResponse {
    /// Repository-relative path.
    pub path: String,
    /// Side of the returned lines.
    pub side: LineSide,
    /// First returned line.
    pub from: u32,
    /// Last returned line (clamped to the file).
    pub to: u32,
    /// Lines in the file on that side (0 for an emptied file).
    pub total_lines: u32,
    /// Text of lines `from..=to`.
    pub lines: Vec<String>,
    /// Changed lines in the range (neutral gutter mark): added lines on the head side, removed lines on the
    /// base side.
    pub changed: Vec<u32>,
    /// Code removed by the pull request at markers in the range (head side only).
    pub deleted: Vec<DeletedBlock>,
    /// Moved blocks in the range (head side only).
    pub moved: Vec<MovedLines>,
    /// Whether and how these lines can be commented on (deterministic, ADR-006).
    pub comments: CommentScope,
}

/// Commenting on displayed lines, decided by the server from the pull request diff (ADR-006).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum CommentScope {
    /// The file is in the pull request diff. Lines inside `line_thread_ranges` (and removed lines at markers)
    /// get a line thread; other lines get a file-level thread whose body ends with the permalink.
    InDiff {
        /// Ranges, on the response's side, whose lines get a line thread.
        line_thread_ranges: Vec<LineRange>,
    },
    /// The file is not in the pull request diff: commenting is not available in v1; the UI offers the
    /// permalink to copy.
    NotInDiff,
}

/// Removed code shown at its deletion marker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DeletedBlock {
    /// Head line after which the code was removed; 0 = before the first line.
    pub after_line: u32,
    /// First removed line at the base commit.
    pub base_start: u32,
    /// Removed lines, from `base_start` on; fewer than `total_lines` when the response's line budget ran out
    /// (read the rest with `side=base`).
    pub lines: Vec<String>,
    /// Number of removed lines in the block.
    pub total_lines: u32,
}

/// A moved block in the range.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct MovedLines {
    /// Lines of the moved block.
    pub range: LineRange,
    /// Lines that differ from the original (`≠`).
    pub differing: Vec<u32>,
}

/// Classic diff of a range.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DiffResponse {
    /// Repository-relative path.
    pub path: String,
    /// Side the requested range refers to.
    pub side: LineSide,
    /// Hunks overlapping the range.
    pub hunks: Vec<DiffHunk>,
}

/// A diff hunk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DiffHunk {
    /// The `@@ … @@` header.
    pub header: String,
    /// Lines.
    pub lines: Vec<DiffLine>,
}

/// A line of a diff hunk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DiffLine {
    /// Kind.
    pub kind: DiffLineKind,
    /// Line number at the base commit.
    pub old_line: Option<u32>,
    /// Line number at the head commit.
    pub new_line: Option<u32>,
    /// Text without the diff marker and line ending.
    pub text: String,
    /// Line ending when it is not LF: `crlf`, or `no_newline` for a last line without one; null for LF.
    pub eol: Option<LineEnding>,
}

/// A line ending other than LF.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum LineEnding {
    /// CR LF.
    Crlf,
    /// The file's last line has no line ending.
    NoNewline,
}

/// Kind of a diff line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    /// Unchanged.
    Context,
    /// Added.
    Added,
    /// Removed.
    Removed,
}
