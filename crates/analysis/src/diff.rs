//! Hunks of a pull request from `git diff` between base and head. Body filled in by WP2-INGEST.

use std::path::Path;

use contracts::ids::{CommitSha, HunkId};

/// Lines of one side of a hunk, `git diff -U0` style: `len == 0` means "after line `start`".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineSpan {
    /// First line (1-based), or the line after which lines were added or removed.
    pub start: u32,
    /// Number of lines.
    pub len: u32,
}

/// How a file changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileStatus {
    /// New file.
    Added,
    /// Changed in place.
    Modified,
    /// Removed.
    Deleted,
    /// Renamed (possibly with changes).
    Renamed,
}

/// The diff of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    /// Path at the head commit (at the base commit for deleted files).
    pub path: String,
    /// Path at the base commit of a renamed file.
    pub old_path: Option<String>,
    /// How the file changed.
    pub status: FileStatus,
    /// Binary file: no hunks.
    pub binary: bool,
    /// Hunks in file order.
    pub hunks: Vec<Hunk>,
}

/// One hunk, without context lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// `h1`, `h2`, … in path order then file order; stable for one base/head pair.
    pub id: HunkId,
    /// Base side.
    pub base: LineSpan,
    /// Head side.
    pub head: LineSpan,
    /// Added head lines.
    pub added: Vec<u32>,
    /// Removed base lines.
    pub removed: Vec<u32>,
}

/// The diff could not be computed.
#[derive(Debug, thiserror::Error)]
pub enum DiffError {
    /// `git` failed.
    #[error("git diff failed: {message}")]
    Git {
        /// Plain git error output.
        message: String,
    },
    /// Running `git` failed.
    #[error("could not run git")]
    Io(#[from] std::io::Error),
    /// Declared but not implemented yet.
    #[error("the analysis operation {operation} is not implemented yet")]
    NotImplemented {
        /// The operation.
        operation: &'static str,
    },
}

/// The changed files between `base` and `head` in the repository at `repo` (read-only).
pub fn diff(repo: &Path, base: &CommitSha, head: &CommitSha) -> Result<Vec<FileDiff>, DiffError> {
    let _ = (repo, base, head);
    Err(DiffError::NotImplemented {
        operation: "diff::diff",
    })
}
