//! Comments in the user's GitHub pending review (ADR-006); nothing is stored locally.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::common::LineSide;

/// The user's pending review on GitHub, merged over every pending review of the user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PendingReview {
    /// False when the GitHub App is not installed on the repository.
    pub commenting_available: bool,
    /// Pending comments; the count is their number.
    pub comments: Vec<PendingComment>,
    /// The server stopped reading at its cap (more pending comments or reviews exist on GitHub than listed).
    pub incomplete: bool,
}

/// A comment in the pending review.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PendingComment {
    /// GitHub comment node id.
    pub id: String,
    /// GitHub thread node id.
    pub thread_id: String,
    /// Repository-relative path.
    pub path: String,
    /// Line or file-level comment.
    pub kind: CommentKind,
    /// Diff side of a line comment (GitHub LEFT = base, RIGHT = head).
    pub side: Option<LineSide>,
    /// First line of a multi-line comment; for a file-level comment, of the linked range.
    pub start_line: Option<u32>,
    /// Last line; for a file-level comment, of the linked range.
    pub line: Option<u32>,
    /// Comment text.
    pub body: String,
}

/// Where a pending comment sits on GitHub.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum CommentKind {
    /// A line thread inside the diff.
    Line,
    /// A file-level thread of a file in the PR diff whose body links to lines outside the diff hunks.
    File,
}

/// The displayed line(s) a comment is written on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommentTarget {
    /// Repository-relative path.
    pub path: String,
    /// `head` for head lines, `base` for removed lines.
    pub side: LineSide,
    /// First line of a multi-line target.
    pub start_line: Option<u32>,
    /// Last line.
    pub line: u32,
}

/// Body of `POST /api/v1/analyses/{id}/pending-review/comments`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CreateCommentRequest {
    /// Client-generated key; a repeated request returns the first response.
    pub idempotency_key: Uuid,
    /// Target lines.
    pub target: CommentTarget,
    /// Comment text.
    pub body: String,
}

/// Response of creating a comment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CreateCommentResponse {
    /// The created comment.
    pub comment: PendingComment,
    /// The line was changed and had no annotation, so `missed_by_llm` feedback was recorded.
    pub missed_by_llm_recorded: bool,
}

/// Body of `PATCH /api/v1/analyses/{id}/pending-review/comments/{comment_id}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct UpdateCommentRequest {
    /// New text.
    pub body: String,
}
