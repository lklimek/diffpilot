//! Code by line range and the classic diff (brief §7: the API never serves whole files).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::LineRange;

/// Query of the `lines` and `diff` endpoints: `path`, lines `from..=to` at the head commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RangeQuery {
    /// Repository-relative path at the head commit.
    pub path: String,
    /// First line, 1-based.
    pub from: u32,
    /// Last line, inclusive.
    pub to: u32,
}

impl RangeQuery {
    /// Most lines one request may ask for.
    pub const MAX_LINES: u32 = 2000;
}

/// Head lines of a file with the deterministic gutter facts for them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LinesResponse {
    /// Repository-relative path.
    pub path: String,
    /// First returned line.
    pub from: u32,
    /// Last returned line (clamped to the file).
    pub to: u32,
    /// Lines in the file.
    pub total_lines: u32,
    /// Text of lines `from..=to`.
    pub lines: Vec<String>,
    /// Changed lines in the range (neutral gutter mark).
    pub changed: Vec<u32>,
    /// Code removed by the pull request at markers in the range.
    pub deleted: Vec<DeletedBlock>,
    /// Moved blocks in the range.
    pub moved: Vec<MovedLines>,
}

/// Removed code shown at its deletion marker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DeletedBlock {
    /// Head line after which the code was removed; 0 = before the first line.
    pub after_line: u32,
    /// First removed line at the base commit.
    pub base_start: u32,
    /// Removed lines.
    pub lines: Vec<String>,
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
    /// Text without the diff marker.
    pub text: String,
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
