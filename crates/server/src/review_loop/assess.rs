//! Assessment passes (`verify_comments`, plan §4.2): the hook the review loop calls when Comments opens and when
//! a read finds new comments. Stub until the job path lands.

use crate::state::AppState;

/// What asks for a pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassReason {
    /// Comments or the PR page opened.
    Opened,
    /// A read found new comments or reviews.
    NewComments,
    /// A watched PR's detection.
    Watch,
    /// An owner question.
    Ask,
}

/// Outcome of [`request_pass`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassQueued {
    /// A pass was queued.
    Queued,
    /// A pass of the PR is queued or running already.
    AlreadyRunning,
    /// No pass is needed or allowed now.
    Skipped,
}

/// A pass could not be requested.
#[derive(Debug, thiserror::Error)]
pub enum AssessError {
    /// The database failed.
    #[error("the assessment pass could not be stored")]
    Db(#[from] db::error::DbError),
}

/// Requests an assessment pass of the pull request `pr` (`pull_requests.id`).
pub async fn request_pass(
    state: &AppState,
    pr: i64,
    reason: PassReason,
) -> Result<PassQueued, AssessError> {
    // TODO(WP13-JOB): queue a `verify_comments` job for the PR's unassessed items (one per PR, spacing, daily cap).
    let _ = state;
    tracing::trace!(
        pr,
        ?reason,
        "assessment pass not queued: the job path is not built yet"
    );
    Ok(PassQueued::Skipped)
}
