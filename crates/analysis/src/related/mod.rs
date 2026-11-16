//! Related-repository detection from Cargo files, limited to the same organisation. Body filled in by
//! WP3-PREPASS.

use std::path::Path;

use contracts::common::RepoRef;
use contracts::ids::CommitSha;

/// Detection failed.
#[derive(Debug, thiserror::Error)]
pub enum RelatedError {
    /// `git` failed.
    #[error("git failed while reading Cargo files: {message}")]
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

/// Repositories of `pr_repo`'s organisation referenced by git or path dependencies in `Cargo.toml` /
/// `Cargo.lock` at `base`.
pub fn detect(
    checkout: &Path,
    base: &CommitSha,
    pr_repo: &RepoRef,
) -> Result<Vec<RepoRef>, RelatedError> {
    let _ = (checkout, base, pr_repo);
    Err(RelatedError::NotImplemented {
        operation: "related::detect",
    })
}
