//! Recording and removing a reviewer's feedback (CCR-F1): stored locally first, then written to MemCan by the
//! writer. The caller has checked that the user may see the analysis.

use chrono::{DateTime, Utc};
use contracts::api::feedback::{Feedback, FeedbackRequest, FeedbackTarget};
use contracts::ids::{AnalysisId, FeedbackId};
use db::error::DbError;
use db::repo::Db;
use db::repo::feedback::NewFeedback;

use super::memory::subject;
use super::{stored_flows, writer};

/// Longest path of a line target, in bytes.
const MAX_PATH_BYTES: usize = 1024;

/// Feedback could not be recorded.
#[derive(Debug, thiserror::Error)]
pub enum FeedbackError {
    /// The note is longer than `FeedbackRequest::MAX_NOTE_CHARS`.
    #[error("the note is too long")]
    NoteTooLong,
    /// The step, annotation or line is not part of the analysis or not a valid line.
    #[error("the feedback target is not part of the analysis")]
    UnknownTarget,
    /// The database failed.
    #[error("the database failed")]
    Db(#[from] DbError),
}

/// Records `request` as the user's feedback on `analysis` (a blank note is no note) and wakes the writer.
pub async fn record(
    db: &Db,
    user_id: &str,
    analysis: AnalysisId,
    request: FeedbackRequest,
    now: DateTime<Utc>,
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
    if !target_exists(db, analysis, &request.target).await? {
        return Err(FeedbackError::UnknownTarget);
    }
    let feedback = db
        .feedback()
        .upsert(
            NewFeedback {
                id: FeedbackId::new_random(),
                analysis_id: analysis,
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
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    db.feedback().remove(analysis, user_id, id, now).await?;
    writer::nudge();
    Ok(())
}

/// A step or annotation must exist in the analysis's flows; a line needs a plain path and a line number.
async fn target_exists(
    db: &Db,
    analysis: AnalysisId,
    target: &FeedbackTarget,
) -> Result<bool, DbError> {
    if let FeedbackTarget::Line { path, line, .. } = target {
        return Ok(*line >= 1
            && !path.is_empty()
            && path.len() <= MAX_PATH_BYTES
            && !path.chars().any(char::is_control));
    }
    let flows = stored_flows(db, analysis).await?;
    Ok(subject(&flows, target).is_some())
}
