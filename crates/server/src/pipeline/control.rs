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
        AnalysisState::Planning { .. }
            | AnalysisState::Running { .. }
            | AnalysisState::Paused { .. }
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
    published(written)
}

fn published(written: Result<AnalysisEvent, Guarded>) -> Result<Option<AnalysisSummary>, DbError> {
    match written {
        Ok(AnalysisEvent::Analysis { summary }) => Ok(Some(summary)),
        Ok(AnalysisEvent::Question { .. } | AnalysisEvent::Heartbeat { .. }) => {
            Err(DbError::Corrupt {
                column: "analysis event",
            })
        }
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

/// Runs a failed analysis again in a new round: state `planning` and the round incremented atomically, the
/// earlier round's unfinished jobs cancelled, then a new driver. Stored (verified) results are kept as documents;
/// every other stage is submitted again as a new job, so the agent is called again.
pub async fn retry(
    state: &AppState,
    analysis: AnalysisId,
) -> Result<AnalysisSummary, ControlError> {
    let db = &state.db;
    let written = state
        .progress
        .update(analysis, || async {
            if !db.analyses().retry(analysis, Utc::now()).await? {
                return Err(Guarded::Refused);
            }
            let summary = db.analyses().summary(analysis).await?;
            Ok(AnalysisEvent::Analysis { summary })
        })
        .await;
    let summary = published(written)?.ok_or(ControlError::Conflict { action: "retried" })?;
    tracing::info!(%analysis, "analysis retried by the user");
    super::start(state, analysis);
    Ok(summary)
}

/// Finishes the clean-up of cancellations a crash interrupted (state stored, jobs, documents or session left);
/// idempotent. Runs at start-up before runner claims are served; claims skip jobs of cancelled analyses meanwhile.
pub async fn recover_cancellations(state: &AppState) -> Result<usize, ControlError> {
    let pending = state.db.analyses().cancellations_to_finish().await?;
    for analysis in &pending {
        discard(state, *analysis).await?;
    }
    if !pending.is_empty() {
        tracing::warn!(
            analyses = pending.len(),
            "finished the clean-up of interrupted cancellations"
        );
    }
    Ok(pending.len())
}

#[cfg(test)]
mod tests {

    use chrono::TimeDelta;
    use contracts::agent::JobKind;
    use contracts::ids::FlowId;
    use contracts::job::spec::JobPriority;
    use db::repo::Db;
    use db::repo::analyses::SessionState;
    use db::repo::analyses::testing::{CREDENTIAL, job_spec, seed_analysis};
    use db::repo::documents::DocumentKind;
    use db::repo::jobs::{ClaimParams, JobStage, NewJob};
    use serde_json::json;

    use super::*;
    use crate::config::tests::test_config;

    async fn enqueue(db: &Db, analysis: AnalysisId, flow: &str) -> contracts::ids::JobId {
        db.jobs()
            .enqueue(
                NewJob {
                    spec: job_spec(analysis, JobKind::AnalyseFlow),
                    priority: JobPriority::Batch,
                    stage: JobStage::Flow(FlowId(flow.to_owned())),
                    generation: 0,
                },
                Utc::now(),
            )
            .await
            .expect("enqueue")
            .id
    }

    /// A running analysis with a claimed and a queued job, a stored document and an active session.
    async fn live(db: &Db) -> (AnalysisId, [contracts::ids::JobId; 2]) {
        let analysis = seed_analysis(db).await.expect("seed");
        let claimed = enqueue(db, analysis, "f1").await;
        let now = Utc::now();
        db.jobs()
            .claim(ClaimParams {
                credential_ids: &[CREDENTIAL.to_owned()],
                job_token_hash: b"hash",
                lease_until: now + TimeDelta::seconds(60),
                now,
            })
            .await
            .expect("claim")
            .expect("a job");
        let queued = enqueue(db, analysis, "f2").await;
        db.documents()
            .put(
                analysis,
                DocumentKind::Flow,
                "f1",
                &json!({"flow_id": "f1"}),
                now,
            )
            .await
            .expect("document");
        db.analyses()
            .set_session(analysis, "thread-1", None)
            .await
            .expect("session");
        (analysis, [claimed, queued])
    }

    async fn assert_cleaned(db: &Db, analysis: AnalysisId, jobs: &[contracts::ids::JobId]) {
        for job in jobs {
            let job = db.jobs().get(*job).await.expect("job");
            assert!(job.cancel_requested, "job {} left live", job.id);
        }
        let documents: Vec<(String, serde_json::Value)> = db
            .documents()
            .list(analysis, DocumentKind::Flow)
            .await
            .expect("documents");
        assert!(documents.is_empty(), "documents left: {documents:?}");
        let record = db.analyses().get(analysis).await.expect("analysis");
        assert_eq!(record.session_state, SessionState::DropRequested);
    }

    /// A crash after any durable step of a cancel (state, jobs, documents, session) is completed at start-up.
    #[tokio::test]
    async fn restart_after_each_cancel_step_finishes_the_clean_up() {
        for done in 1..=4 {
            let db = Db::in_memory().await.expect("db");
            let state = AppState::with_queue(test_config(), db.clone());
            let (analysis, jobs) = live(&db).await;
            db.analyses()
                .set_state(analysis, &AnalysisState::Cancelled, Utc::now())
                .await
                .expect("cancelled");
            if done >= 2 {
                state
                    .executor
                    .cancel_analysis(analysis)
                    .await
                    .expect("jobs");
            }
            if done >= 3 {
                db.documents()
                    .delete_for_analysis(analysis)
                    .await
                    .expect("documents");
            }
            if done >= 4 {
                db.analyses()
                    .request_session_drop(analysis)
                    .await
                    .expect("session");
            }
            let finished = recover_cancellations(&state).await.expect("recover");
            assert_eq!(finished, usize::from(done < 4), "after step {done}");
            assert_cleaned(&db, analysis, &jobs).await;
            assert_eq!(
                recover_cancellations(&state).await.expect("again"),
                0,
                "idempotent"
            );
        }
    }
}
