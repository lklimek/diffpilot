//! Counters over material that is not analysed (lines removed from tests, changed dependencies). Body filled in
//! by WP3-PREPASS.

use std::path::Path;

use contracts::api::tour::Counters;
use contracts::ids::CommitSha;

use crate::diff::FileDiff;
use crate::tech::Classified;

/// Counting failed.
#[derive(Debug, thiserror::Error)]
pub enum CountersError {
    /// `git` failed.
    #[error("git failed while counting: {message}")]
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

/// Deterministic counters for the PR context.
pub fn count(
    repo: &Path,
    base: &CommitSha,
    head: &CommitSha,
    files: &[FileDiff],
    classified: &[Classified],
) -> Result<Counters, CountersError> {
    let _ = (repo, base, head, files, classified);
    Err(CountersError::NotImplemented {
        operation: "counters::count",
    })
}
