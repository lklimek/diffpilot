//! Coverage of every changed line by flows, Not analysed items or Unexplained (brief §1 principle 4). Body
//! filled in by WP3-VERIFY.

use contracts::common::FileRanges;

use crate::diff::FileDiff;
use crate::verify::VerifiedFlow;

/// Result of the coverage check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Changed lines in no flow and no quiet item: "Unexplained — read these yourself".
    pub unexplained: Vec<FileRanges>,
    /// Changed lines covered by more than one flow.
    pub double_covered: Vec<FileRanges>,
}

/// The coverage check could not run.
#[derive(Debug, thiserror::Error)]
pub enum CoverageError {
    /// Input ranges refer to a file that is not in the diff.
    #[error("coverage input refers to {path}, which is not in the diff")]
    UnknownFile {
        /// The path.
        path: String,
    },
    /// Declared but not implemented yet.
    #[error("the analysis operation {operation} is not implemented yet")]
    NotImplemented {
        /// The operation.
        operation: &'static str,
    },
}

/// Changed lines not covered by `flows` or the deterministic `quiet` lines (Not analysed, identical moves).
pub fn check(
    files: &[FileDiff],
    quiet: &[FileRanges],
    flows: &[VerifiedFlow],
) -> Result<Coverage, CoverageError> {
    let _ = (files, quiet, flows);
    Err(CoverageError::NotImplemented {
        operation: "coverage::check",
    })
}
