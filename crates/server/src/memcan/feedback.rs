//! Recording and removing a reviewer's feedback (CCR-F1): stored locally first, then written to MemCan by the
//! writer. The caller has checked that the user may see the analysis.

use analysis::diff::FileDiff;
use analysis::lines::changed_lines_within;
use contracts::api::analyses::AnalysisSummary;
use contracts::api::feedback::{Feedback, FeedbackRequest, FeedbackTarget};
use contracts::common::LineSide;
use contracts::ids::{AnalysisId, FeedbackId};
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::Db;
use db::repo::feedback::NewFeedback;

use super::memory::subject;
use super::{stored_flows, writer};
use crate::pipeline::{PrDiffError, pr_diff};
use crate::state::AppState;

/// Longest path of a line target, in bytes.
const MAX_PATH_BYTES: usize = 1024;

/// Feedback could not be recorded.
#[derive(Debug, thiserror::Error)]
pub enum FeedbackError {
    /// The note is longer than `FeedbackRequest::MAX_NOTE_CHARS`.
    #[error("the note is too long")]
    NoteTooLong,
    /// The step or annotation is not in the analysis, or the line is not a changed line of its diff.
    #[error("the feedback target is not part of the analysis")]
    UnknownTarget,
    /// The analysed diff could not be read (needed to check a line target).
    #[error("the analysed diff could not be read")]
    Diff(#[from] PrDiffError),
    /// The database failed.
    #[error("the database failed")]
    Db(#[from] DbError),
}

/// Records `request` as the user's feedback on the analysis (a blank note is no note) and wakes the writer.
pub async fn record(
    state: &AppState,
    user_id: &str,
    summary: &AnalysisSummary,
    request: FeedbackRequest,
    now: Timestamp,
) -> Result<Feedback, FeedbackError> {
    let note = request
        .note
        .map(|note| note.trim().to_owned())
        .filter(|note| !note.is_empty());
    if note
        .as_ref()
        .is_some_and(|note| note.chars().count() > FeedbackRequest::MAX_NOTE_CHARS)
    {
        return Err(FeedbackError::NoteTooLong);
    }
    if !target_exists(state, summary, &request.target).await? {
        return Err(FeedbackError::UnknownTarget);
    }
    let feedback = state
        .db
        .feedback()
        .upsert(
            NewFeedback {
                id: FeedbackId::new_random(),
                analysis_id: summary.id,
                user_id: user_id.to_owned(),
                target: request.target,
                value: request.value.into(),
                note,
            },
            now,
        )
        .await?;
    writer::nudge();
    Ok(feedback)
}

/// Removes the user's entry `id` of `analysis` and wakes the writer; `NotFound` if there is none.
pub async fn remove(
    db: &Db,
    user_id: &str,
    analysis: AnalysisId,
    id: FeedbackId,
    now: Timestamp,
) -> Result<(), DbError> {
    db.feedback().remove(analysis, user_id, id, now).await?;
    writer::nudge();
    Ok(())
}

/// A step or annotation must exist in the analysis's flows under its canonical id; a line must be a changed
/// line of the analysed diff on its side.
async fn target_exists(
    state: &AppState,
    summary: &AnalysisSummary,
    target: &FeedbackTarget,
) -> Result<bool, FeedbackError> {
    if let FeedbackTarget::Line { path, side, line } = target {
        if *line == 0
            || path.is_empty()
            || path.len() > MAX_PATH_BYTES
            || path.chars().any(char::is_control)
        {
            return Ok(false);
        }
        let (_, files) = pr_diff(state, summary).await?;
        return Ok(is_changed_line(&files, path, *side, *line));
    }
    let flows = stored_flows(&state.db, summary.id).await?;
    Ok(subject(&flows, target).is_some())
}

/// Whether `line` of `path` on `side` is added (head) or removed (base) by the diff.
fn is_changed_line(files: &[FileDiff], path: &str, side: LineSide, line: u32) -> bool {
    !changed_lines_within(files, path, side, line, line).is_empty()
}

#[cfg(test)]
mod tests {
    use analysis::diff::{FileStatus, Hunk, LineSpan};
    use contracts::ids::HunkId;

    use super::*;

    fn diff() -> Vec<FileDiff> {
        vec![FileDiff {
            path: "src/rpc/client.rs".to_owned(),
            old_path: Some("src/client.rs".to_owned()),
            status: FileStatus::Renamed,
            binary: false,
            hunks: vec![Hunk {
                id: HunkId("h1".to_owned()),
                base: LineSpan { start: 90, len: 3 },
                head: LineSpan { start: 94, len: 4 },
                added: vec![95, 96],
                removed: vec![91],
            }],
        }]
    }

    #[test]
    fn line_targets_must_be_changed_lines_of_the_diff_on_their_side() {
        let files = diff();
        assert!(is_changed_line(
            &files,
            "src/rpc/client.rs",
            LineSide::Head,
            96
        ));
        assert!(is_changed_line(&files, "src/client.rs", LineSide::Base, 91));
        for (path, side, line, case) in [
            ("src/rpc/client.rs", LineSide::Base, 96, "wrong side"),
            (
                "src/client.rs",
                LineSide::Head,
                96,
                "base path on the head side",
            ),
            ("does-not-exist.rs", LineSide::Head, 96, "foreign path"),
            (
                "src/rpc/client.rs",
                LineSide::Head,
                u32::MAX,
                "out of range",
            ),
            (
                "src/rpc/client.rs",
                LineSide::Head,
                94,
                "unchanged line in the hunk",
            ),
            ("src/rpc/client.rs", LineSide::Head, 3, "unchanged line"),
        ] {
            assert!(!is_changed_line(&files, path, side, line), "{case}");
        }
    }
}
