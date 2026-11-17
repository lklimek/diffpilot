//! Analysis state changes that race with a running driver: guarded state writes, cancel (partial results and the
//! agent session discarded) and retry. Every write goes through `ProgressHub::update`, so a guard and its write
//! are one step for every writer of the analysis.

use chrono::Utc;
use contracts::api::analyses::{AnalysisState, AnalysisSummary};
use contracts::api::events::AnalysisEvent;
use contracts::ids::AnalysisId;
use db::error::DbError;

use crate::jobs::executor::ExecutorError;
use crate::state::AppState;

/// A cancel or retry could not be applied.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    /// The analysis is not in a state that allows it (e.g. retry of a running analysis).
    #[error("the analysis cannot be {action} in its current state")]
    Conflict {
        /// What was asked.
        action: &'static str,
    },
    /// The database failed.
    #[error("the analysis state could not be changed")]
    Db(#[from] DbError),
    /// The job executor failed.
    #[error("the jobs of the analysis could not be cancelled")]
    Executor(#[from] ExecutorError),
}

/// Planning, running or paused: a driver works on it.
pub fn is_live(state: &AnalysisState) -> bool {
    matches!(
        state,
        AnalysisState::Planning | AnalysisState::Running { .. } | AnalysisState::Paused { .. }
    )
}

enum Guarded {
    Db(DbError),
    Refused,
}

impl From<DbError> for Guarded {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

/// Stores `next` when the current state passes `allowed`, under the analysis's update lock; `Ok(None)` when it
/// does not (nothing written, nothing published).
pub async fn set_state_if(
    state: &AppState,
    analysis: AnalysisId,
    next: &AnalysisState,
    allowed: impl FnOnce(&AnalysisState) -> bool,
) -> Result<Option<AnalysisSummary>, DbError> {
    let db = &state.db;
    let written = state
        .progress
        .update(analysis, || async {
            let current = db.analyses().summary(analysis).await?;
            if !allowed(&current.state) {
                return Err(Guarded::Refused);
            }
            db.analyses().set_state(analysis, next, Utc::now()).await?;
            let summary = db.analyses().summary(analysis).await?;
            Ok(AnalysisEvent::Analysis { summary })
        })
        .await;
    match written {
        Ok(AnalysisEvent::Analysis { summary }) => Ok(Some(summary)),
        Ok(AnalysisEvent::Question { .. }) => Err(DbError::Corrupt {
            column: "analysis event",
        }),
        Err(Guarded::Refused) => Ok(None),
        Err(Guarded::Db(error)) => Err(error),
    }
}

/// Cancels a live or failed analysis: state `cancelled`, then every unfinished job cancelled, the stored results
/// deleted and the agent session dropped. A running driver ends when its jobs report `cancelled`.
pub async fn cancel(
    state: &AppState,
    analysis: AnalysisId,
) -> Result<AnalysisSummary, ControlError> {
    let cancellable = |current: &AnalysisState| {
        is_live(current) || matches!(current, AnalysisState::Failed { .. })
    };
    let summary = set_state_if(state, analysis, &AnalysisState::Cancelled, cancellable)
        .await?
        .ok_or(ControlError::Conflict {
            action: "cancelled",
        })?;
    discard(state, analysis).await?;
    tracing::info!(%analysis, "analysis cancelled by the user");
    Ok(summary)
}

/// Cancels the analysis's jobs, deletes its stored documents and asks the runner to delete its agent session.
/// Idempotent: the driver repeats it when it sees the cancellation, so a document it stored meanwhile goes too.
pub async fn discard(state: &AppState, analysis: AnalysisId) -> Result<(), ControlError> {
    state.executor.cancel_analysis(analysis).await?;
    let deleted = state.db.documents().delete_for_analysis(analysis).await?;
    state.db.analyses().request_session_drop(analysis).await?;
    tracing::debug!(%analysis, documents = deleted, "partial results and agent session discarded");
    Ok(())
}

/// Runs a failed analysis again: state `planning`, then a new driver. Stages with a stored result are kept;
/// failed jobs are submitted again.
pub async fn retry(
    state: &AppState,
    analysis: AnalysisId,
) -> Result<AnalysisSummary, ControlError> {
    let failed = |current: &AnalysisState| matches!(current, AnalysisState::Failed { .. });
    let summary = set_state_if(state, analysis, &AnalysisState::Planning, failed)
        .await?
        .ok_or(ControlError::Conflict { action: "retried" })?;
    tracing::info!(%analysis, "analysis retried by the user");
    super::start(state, analysis);
    Ok(summary)
}
