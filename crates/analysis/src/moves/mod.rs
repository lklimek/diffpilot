//! Moved-block detection and whitespace-only changes. Body filled in by WP3-PREPASS.

use std::path::Path;

use contracts::common::FileRange;
use contracts::ids::CommitSha;

use crate::diff::FileDiff;

/// A block moved between base and head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovedBlock {
    /// Original block at the base commit.
    pub from: FileRange,
    /// New block at the head commit.
    pub to: FileRange,
    /// Equal after whitespace normalisation (badge "text identical"; a quiet row).
    pub identical: bool,
    /// Head lines that differ from the original.
    pub differing_head_lines: Vec<u32>,
}

/// Deterministic move and whitespace facts of a pull request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MovesReport {
    /// Moved blocks.
    pub moved: Vec<MovedBlock>,
    /// Changed head ranges whose change is whitespace only (kept in the agent's hunks, marked as a fact).
    pub whitespace_only: Vec<FileRange>,
}

/// Move detection failed.
#[derive(Debug, thiserror::Error)]
pub enum MovesError {
    /// `git` failed.
    #[error("git diff --color-moved failed: {message}")]
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

/// Moved blocks (`git diff --color-moved` candidates confirmed by token-normalised hashes) and whitespace-only
/// changes.
pub fn detect(
    repo: &Path,
    base: &CommitSha,
    head: &CommitSha,
    files: &[FileDiff],
) -> Result<MovesReport, MovesError> {
    let _ = (repo, base, head, files);
    Err(MovesError::NotImplemented {
        operation: "moves::detect",
    })
}
