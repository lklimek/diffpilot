//! Durable demand ordering for lazy flow analysis; no runner scheduling policy changes.
use super::{
    Db,
    documents::DocumentKind,
    jobs::{parse_ts, parse_wire, ts},
};
use crate::error::DbError;
use chrono::{DateTime, Utc};
use contracts::agent::plan::PlanOutput;
use contracts::api::error::ApiErrorCode;
use contracts::api::tour::{FlowAnalysisState, FlowAnalysisStatus};
use contracts::ids::{AnalysisId, FlowId};
use sqlx::Row;

/// Automatic retries of a failed flow, requested or analysed in the background; after them it stays failed until
/// a user requests it.
pub const AUTO_FLOW_RETRIES: u32 = 2;

/// Last sentence of the stored message of a failed flow that a retry can help.
pub const RETRY_BY_USER: &str = "Retry this flow.";

/// Stored for a flow whose stored document today's checks reject when a user requests it again
/// ([`FlowWork::request_again`]); shown like any failed flow's message.
const REJECTED_STORED: &str =
    "The stored analysis of this flow did not pass the server's current checks. Retry this flow.";

/// Shown in place of [`RETRY_BY_USER`] while the analysis is live and the flow runs again by itself: it is
/// requested, or an automatic retry is left and background analysis is on.
const RETRY_AUTOMATICALLY: &str = "It will be tried again automatically.";

/// Shown in place of [`RETRY_BY_USER`] in a failed analysis: a flow request is refused there until its Retry.
const RETRY_ANALYSIS: &str = "Retry the analysis; then this flow can be analysed again.";

/// Shown in place of [`RETRY_BY_USER`] in an analysis failed on server configuration: it offers no Retry.
const ASK_ADMINISTRATOR: &str = "Ask the administrator to fix the server configuration.";

/// Shown in place of [`RETRY_BY_USER`] in a cancelled analysis: neither a flow request nor a Retry works there.
const START_ANALYSIS: &str = "Start a new analysis to analyse this flow.";

/// What the driver does next for the flows of an analysis ([`FlowWork::next`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NextFlow {
    /// Analyse this flow now.
    Run(FlowId),
    /// A failed flow is due once the analysis's live jobs (a question being answered) have ended.
    Wait,
    /// No flow is admitted: each is analysed, failed for good, or not requested with background analysis off.
    Idle,
}

/// The flow [`FlowWork::next`] would pick.
struct Due {
    flow: FlowId,
    /// It failed before: running it again needs a new job round.
    failed: bool,
    /// A user requested it.
    requested: bool,
}

/// Access to an analysis's background switch and requested flows.
pub struct FlowWork<'a> {
    db: &'a Db,
}
impl Db {
    /// Lazy flow controls.
    pub fn flow_work(&self) -> FlowWork<'_> {
        FlowWork { db: self }
    }
}
impl FlowWork<'_> {
    /// Whether the accepted plan and PR context allow a partial Tour.
    pub async fn ready(&self, id: AnalysisId) -> Result<bool, DbError> {
        Ok(sqlx::query_scalar("SELECT state <> 'cancelled' AND EXISTS(SELECT 1 FROM documents WHERE analysis_id=a.id AND kind='plan' AND key='') AND EXISTS(SELECT 1 FROM documents WHERE analysis_id=a.id AND kind='pr_context' AND key='') FROM analyses a WHERE id=?")
            .bind(id.to_string()).fetch_one(&self.db.pool).await?)
    }
    /// Persisted automatic-work choice.
    pub async fn background(&self, id: AnalysisId) -> Result<bool, DbError> {
        Ok(
            sqlx::query_scalar("SELECT background_analysis FROM analyses WHERE id=?")
                .bind(id.to_string())
                .fetch_one(&self.db.pool)
                .await?,
        )
    }
    /// Switch automatic work without interrupting claimed jobs; unrequested queued work is withdrawn.
    pub async fn set_background(
        &self,
        id: AnalysisId,
        enabled: bool,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        let mut tx = super::write_tx(&self.db.pool).await?;
        sqlx::query("UPDATE analyses SET background_analysis=? WHERE id=?")
            .bind(enabled)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        if !enabled {
            let rows = sqlx::query("SELECT id,attempt FROM jobs WHERE analysis_id=? AND kind='analyse_flow' AND state='queued' AND NOT EXISTS (SELECT 1 FROM flow_work w WHERE w.analysis_id=jobs.analysis_id AND w.flow_id=jobs.flow_id AND w.requested_at IS NOT NULL)")
                .bind(id.to_string()).fetch_all(&mut *tx).await?;
            withdraw(&mut tx, rows, now).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    /// Request a planned flow, newest demand first. Claimed jobs are never interrupted.
    pub async fn request(
        &self,
        id: AnalysisId,
        flow: &FlowId,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        let plan: PlanOutput = self
            .db
            .documents()
            .get(id, DocumentKind::Plan, "")
            .await?
            .ok_or(DbError::NotFound)?;
        if !plan.flows.iter().any(|f| f.id == *flow) {
            return Err(DbError::NotFound);
        }
        if self
            .db
            .documents()
            .get::<serde_json::Value>(id, DocumentKind::Flow, &flow.0)
            .await?
            .is_some()
        {
            return Ok(());
        }
        let mut tx = super::write_tx(&self.db.pool).await?;
        sqlx::query("INSERT INTO flow_work (tenant_id,analysis_id,flow_id,requested_at) SELECT tenant_id,id,?,? FROM analyses WHERE id=? ON CONFLICT(analysis_id,flow_id) DO UPDATE SET requested_at=excluded.requested_at")
            .bind(&flow.0).bind(ts(now)).bind(id.to_string()).execute(&mut *tx).await?;
        demanded(&mut tx, id, flow, now).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Discards the stored document of `flow`, which today's checks reject, and requests the flow, in one
    /// transaction: it runs like a failed flow in a new job round (its earlier output is never replayed), with the
    /// automatic retries of a requested flow, and a `finished` analysis is `running` again. `false` (nothing
    /// changed) when no document is stored.
    pub async fn request_again(
        &self,
        id: AnalysisId,
        flow: &FlowId,
        now: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let mut tx = super::write_tx(&self.db.pool).await?;
        let discarded = rerun(&mut tx, id, flow, now).await?;
        tx.commit().await?;
        Ok(discarded)
    }
    /// "Analyse all remaining flows": background analysis on, and every flow of `rejected` (stored documents
    /// today's checks reject) discarded and requested as by [`FlowWork::request_again`], in one transaction.
    pub async fn analyse_remaining(
        &self,
        id: AnalysisId,
        rejected: &[FlowId],
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        let mut tx = super::write_tx(&self.db.pool).await?;
        sqlx::query("UPDATE analyses SET background_analysis=1 WHERE id=?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        for flow in rejected {
            rerun(&mut tx, id, flow, now).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    /// Makes a `finished` analysis `running` again when a requested flow has no document (discarded by
    /// [`FlowWork::request_again`] while its driver finished); whether it did.
    pub async fn reopen(&self, id: AnalysisId) -> Result<bool, DbError> {
        Ok(sqlx::query("UPDATE analyses SET state='running', finished_at=NULL WHERE id=?1 AND state='finished' AND EXISTS(SELECT 1 FROM flow_work w WHERE w.analysis_id=?1 AND w.requested_at IS NOT NULL AND NOT EXISTS(SELECT 1 FROM documents d WHERE d.analysis_id=?1 AND d.kind='flow' AND d.key=w.flow_id))")
            .bind(id.to_string())
            .execute(&self.db.pool)
            .await?
            .rows_affected()
            > 0)
    }
    /// The flow the driver works on next: the one a runner holds, else the newest demand, else by plan rank the
    /// flows not analysed yet and then failed ones with an automatic retry left ([`AUTO_FLOW_RETRIES`]; never a
    /// flow too large to send). Picking a failed flow starts a new job round and, without a request, uses one of
    /// its automatic retries; the choice and these writes are one transaction.
    pub async fn next(&self, id: AnalysisId) -> Result<NextFlow, DbError> {
        let mut tx = super::write_tx(&self.db.pool).await?;
        let Some(due) = due(&mut tx, id).await? else {
            return Ok(NextFlow::Idle);
        };
        if due.failed {
            // Wait for outstanding Ask too: changing round must not strand a queued question. A new round separates successful
            // but rejected repair outputs from the retry, preserving all accepted documents.
            let live: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE analysis_id=? AND state IN ('queued','claimed','running','paused'))")
                .bind(id.to_string()).fetch_one(&mut *tx).await?;
            if live {
                return Ok(NextFlow::Wait);
            }
            // Another flow's output waiting for its postponed repair moves to the new round with the analysis.
            sqlx::query("UPDATE jobs SET round=round+1 WHERE analysis_id=?1 AND kind='analyse_flow' AND state='succeeded' AND flow_id<>?2 AND round=(SELECT round FROM analyses WHERE id=?1) AND NOT EXISTS(SELECT 1 FROM documents d WHERE d.analysis_id=jobs.analysis_id AND d.kind='flow' AND d.key=jobs.flow_id) AND NOT EXISTS(SELECT 1 FROM flow_work w WHERE w.analysis_id=jobs.analysis_id AND w.flow_id=jobs.flow_id AND w.error IS NOT NULL)")
                .bind(id.to_string())
                .bind(&due.flow.0)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE analyses SET round=round+1 WHERE id=?")
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE flow_work SET error=NULL, auto_retries=auto_retries+? WHERE analysis_id=? AND flow_id=?")
                .bind(u32::from(!due.requested))
                .bind(id.to_string())
                .bind(&due.flow.0)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        Ok(NextFlow::Run(due.flow))
    }
    /// Whether [`FlowWork::next`] has a flow to give (now or once the live jobs ended); changes nothing.
    pub async fn has_work(&self, id: AnalysisId) -> Result<bool, DbError> {
        let mut connection = self.db.pool.acquire().await?;
        Ok(due(&mut connection, id).await?.is_some())
    }
    /// Whether the open review has flows left but nothing for a driver to do: no flow admitted
    /// ([`FlowWork::has_work`]) and no result to replay ([`FlowWork::unstored`]).
    pub async fn parked(&self, id: AnalysisId) -> Result<bool, DbError> {
        let flows_left: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM documents d, json_each(d.body,'$.flows') f WHERE d.analysis_id=?1 AND d.kind='plan' AND d.key='' AND NOT EXISTS(SELECT 1 FROM documents done WHERE done.analysis_id=?1 AND done.kind='flow' AND done.key=json_extract(f.value,'$.id')))")
            .bind(id.to_string()).fetch_one(&self.db.pool).await?;
        Ok(flows_left
            && self.ready(id).await?
            && !self.has_work(id).await?
            && self.unstored(id).await?.is_empty())
    }
    /// Flows with a succeeded job of the current round but neither a document nor a failure, oldest job first:
    /// results a restart interrupted before they were verified and stored, or whose repair was postponed.
    pub async fn unstored(&self, id: AnalysisId) -> Result<Vec<FlowId>, DbError> {
        let flows: Vec<String> = sqlx::query_scalar("SELECT j.flow_id FROM jobs j JOIN analyses a ON a.id=j.analysis_id AND a.round=j.round WHERE j.analysis_id=? AND j.kind='analyse_flow' AND j.state='succeeded' AND NOT EXISTS(SELECT 1 FROM documents d WHERE d.analysis_id=j.analysis_id AND d.kind='flow' AND d.key=j.flow_id) AND NOT EXISTS(SELECT 1 FROM flow_work w WHERE w.analysis_id=j.analysis_id AND w.flow_id=j.flow_id AND w.error IS NOT NULL) GROUP BY j.flow_id ORDER BY MIN(j.created_at), MIN(j.rowid)")
            .bind(id.to_string()).fetch_all(&self.db.pool).await?;
        Ok(flows.into_iter().map(FlowId).collect())
    }
    /// Mark a flow failed, retaining the accepted plan and other flow outputs. A requested flow keeps its demand
    /// while an automatic retry is left (using one), unless it was too large to send; otherwise the demand ends.
    /// A `message` that ends with [`RETRY_BY_USER`] is shown with the automatic retry instead while the flow
    /// will run again by itself, and with the action that works once the analysis ended ([`FlowWork::status`]).
    pub async fn fail(&self, id: AnalysisId, flow: &FlowId, message: &str) -> Result<(), DbError> {
        // Every assignment reads the row as it was before the update.
        macro_rules! retried {
            () => {
                "(requested_at IS NOT NULL AND auto_retries < ?4 AND NOT EXISTS(SELECT 1 FROM documents u WHERE u.analysis_id=flow_work.analysis_id AND u.kind='unsent' AND u.key=flow_work.flow_id))"
            };
        }
        sqlx::query(concat!(
            "INSERT INTO flow_work(tenant_id,analysis_id,flow_id,error) SELECT tenant_id,id,?1,?2 FROM analyses WHERE id=?3 ON CONFLICT(analysis_id,flow_id) DO UPDATE SET error=excluded.error, requested_at=CASE WHEN ",
            retried!(),
            " THEN requested_at END, auto_retries=auto_retries+",
            retried!()
        ))
        .bind(&flow.0)
        .bind(message)
        .bind(id.to_string())
        .bind(AUTO_FLOW_RETRIES)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }
    /// Recheck admission under the progress lock immediately before enqueue.
    pub async fn admitted(&self, id: AnalysisId, flow: &FlowId) -> Result<bool, DbError> {
        if !self.ready(id).await? {
            return Ok(true);
        }
        let demand:Option<String>=sqlx::query_scalar("SELECT w.flow_id FROM flow_work w WHERE w.analysis_id=? AND w.requested_at IS NOT NULL AND NOT EXISTS(SELECT 1 FROM documents d WHERE d.analysis_id=w.analysis_id AND d.kind='flow' AND d.key=w.flow_id) ORDER BY w.requested_at DESC LIMIT 1").bind(id.to_string()).fetch_optional(&self.db.pool).await?;
        Ok(match demand {
            Some(id) => id == flow.0,
            None => self.background(id).await?,
        })
    }

    /// Status of a flow the caller found no accepted document for: durable requests/failures and current jobs,
    /// never `analysed` (a document stored since the caller's read does not change what the caller shows).
    pub async fn status(
        &self,
        id: AnalysisId,
        flow: &FlowId,
    ) -> Result<FlowAnalysisStatus, DbError> {
        let mut status = FlowAnalysisStatus {
            state: FlowAnalysisState::NotAnalysed,
            started_at: None,
            message: None,
        };
        if let Some(row) = sqlx::query(
            "SELECT w.requested_at, w.error, a.state, a.failure_reason,
                    a.state IN ('planning','running','paused')
                    AND (w.requested_at IS NOT NULL OR (a.background_analysis=1 AND w.auto_retries < ?)) AS retried
             FROM flow_work w JOIN analyses a ON a.id=w.analysis_id WHERE w.analysis_id=? AND w.flow_id=?",
        )
        .bind(AUTO_FLOW_RETRIES)
        .bind(id.to_string())
        .bind(&flow.0)
        .fetch_optional(&self.db.pool)
        .await?
        {
            status.message = row.try_get("error")?;
            // The stored message names the flow's Retry; it says what works instead while the server runs the flow
            // again by itself or once the analysis ended (a flow request is refused then).
            let instead = match row.try_get::<String, _>("state")?.as_str() {
                _ if row.try_get::<bool, _>("retried")? => Some(RETRY_AUTOMATICALLY),
                "failed" => Some(
                    match row
                        .try_get::<Option<String>, _>("failure_reason")?
                        .map(|reason| parse_wire(&reason, "analyses.failure_reason"))
                        .transpose()?
                    {
                        Some(ApiErrorCode::Configuration) => ASK_ADMINISTRATOR,
                        _ => RETRY_ANALYSIS,
                    },
                ),
                "cancelled" => Some(START_ANALYSIS),
                _ => None,
            };
            if let Some(instead) = instead
                && let Some(message) = &mut status.message
                && let Some(failure) = message.strip_suffix(RETRY_BY_USER)
            {
                *message = format!("{failure}{instead}");
            }
            status.state = if row.try_get::<Option<String>, _>("requested_at")?.is_some() {
                FlowAnalysisState::Queued
            } else if status.message.is_some() {
                FlowAnalysisState::Failed
            } else {
                FlowAnalysisState::NotAnalysed
            };
        }
        if let Some(row)=sqlx::query("SELECT state,claimed_at FROM jobs WHERE analysis_id=? AND flow_id=? AND state IN ('queued','claimed','running','paused') ORDER BY created_at DESC LIMIT 1").bind(id.to_string()).bind(&flow.0).fetch_optional(&self.db.pool).await? {
            let job:String=row.try_get("state")?;
            status.state=if job=="claimed" || job=="running"{FlowAnalysisState::Analysing}else{FlowAnalysisState::Queued};
            status.started_at=row.try_get::<Option<String>,_>("claimed_at")?.map(|s|parse_ts(&s,"jobs.claimed_at")).transpose()?;
        }
        Ok(status)
    }
}

/// The flow [`FlowWork::next`] picks, read on `connection` (its transaction, when it writes).
async fn due(
    connection: &mut sqlx::SqliteConnection,
    id: AnalysisId,
) -> Result<Option<Due>, DbError> {
    if let Some(flow) = sqlx::query_scalar::<_, String>("SELECT flow_id FROM jobs WHERE analysis_id=? AND kind='analyse_flow' AND state IN ('claimed','running','paused') ORDER BY created_at LIMIT 1")
        .bind(id.to_string()).fetch_optional(&mut *connection).await? {
        return Ok(Some(Due { flow: FlowId(flow), failed: false, requested: false }));
    }
    // A flow too large to send (its `unsent` document) fails the same way again: only a request runs it.
    let row=sqlx::query("SELECT json_extract(f.value,'$.id') AS flow, w.error IS NOT NULL AS failed, w.requested_at IS NOT NULL AS requested FROM documents d, json_each(d.body,'$.flows') f JOIN analyses a ON a.id=d.analysis_id LEFT JOIN flow_work w ON w.analysis_id=a.id AND w.flow_id=json_extract(f.value,'$.id') WHERE d.analysis_id=? AND d.kind='plan' AND d.key='' AND NOT EXISTS(SELECT 1 FROM documents done WHERE done.analysis_id=a.id AND done.kind='flow' AND done.key=json_extract(f.value,'$.id')) AND (w.requested_at IS NOT NULL OR (a.background_analysis=1 AND (w.error IS NULL OR (w.auto_retries < ? AND NOT EXISTS(SELECT 1 FROM documents u WHERE u.analysis_id=a.id AND u.kind='unsent' AND u.key=json_extract(f.value,'$.id')))))) ORDER BY w.requested_at IS NOT NULL DESC, w.requested_at DESC, w.error IS NOT NULL, CAST(json_extract(f.value,'$.importance_rank') AS INTEGER) LIMIT 1")
        .bind(id.to_string()).bind(AUTO_FLOW_RETRIES).fetch_optional(&mut *connection).await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Due {
        flow: FlowId(row.try_get("flow")?),
        failed: row.try_get("failed")?,
        requested: row.try_get("requested")?,
    }))
}

/// [`FlowWork::request_again`] inside `tx`: `false` (nothing written) when `flow` has no stored document.
async fn rerun(
    tx: &mut sqlx::SqliteConnection,
    id: AnalysisId,
    flow: &FlowId,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    let deleted =
        sqlx::query("DELETE FROM documents WHERE analysis_id=? AND kind='flow' AND key=?")
            .bind(id.to_string())
            .bind(&flow.0)
            .execute(&mut *tx)
            .await?;
    if deleted.rows_affected() == 0 {
        return Ok(false);
    }
    sqlx::query("INSERT INTO flow_work (tenant_id,analysis_id,flow_id,requested_at,error) SELECT tenant_id,id,?,?,? FROM analyses WHERE id=? ON CONFLICT(analysis_id,flow_id) DO UPDATE SET requested_at=excluded.requested_at, error=excluded.error")
        .bind(&flow.0).bind(ts(now)).bind(REJECTED_STORED).bind(id.to_string()).execute(&mut *tx).await?;
    // The flows analysed are the flow documents left (`finished` stores no count).
    sqlx::query("UPDATE analyses SET flows_done=(SELECT COUNT(*) FROM documents WHERE analysis_id=?1 AND kind='flow') WHERE id=?1")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    demanded(&mut *tx, id, flow, now).await?;
    Ok(true)
}

/// After `flow` was requested: other flows' queued jobs withdrawn, and a `finished` analysis `running` again (it
/// has a flow to analyse).
async fn demanded(
    tx: &mut sqlx::SqliteConnection,
    id: AnalysisId,
    flow: &FlowId,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let rows = sqlx::query("SELECT id,attempt FROM jobs WHERE analysis_id=? AND kind='analyse_flow' AND state='queued' AND flow_id<>?")
        .bind(id.to_string()).bind(&flow.0).fetch_all(&mut *tx).await?;
    withdraw(&mut *tx, rows, now).await?;
    sqlx::query(
        "UPDATE analyses SET state='running', finished_at=NULL WHERE id=? AND state='finished'",
    )
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Withdraw only unclaimed work inside the same write transaction as the demand change.
async fn withdraw(
    tx: &mut sqlx::SqliteConnection,
    rows: Vec<sqlx::sqlite::SqliteRow>,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    for row in rows {
        let id: String = row.try_get("id")?;
        let job = super::jobs::parse_wire(&id, "jobs.id")?;
        let result = super::jobs::server_result(
            job,
            row.try_get("attempt")?,
            contracts::job::result::JobOutcome::Cancelled,
        );
        sqlx::query("UPDATE jobs SET state='cancelled', cancel_requested=1, result=?, finished_at=?, resumes_at=NULL, ready_output=NULL, ready_output_attempt=NULL WHERE id=?")
            .bind(serde_json::to_string(&result)?).bind(ts(now)).bind(id).execute(&mut *tx).await?;
    }
    Ok(())
}

/// Test helpers shared by dependent crates.
#[cfg(any(test, feature = "test-util"))]
pub mod testing {
    use super::{AnalysisId, Db, DbError, FlowId};

    /// The analysis's job round and the automatic retries `flow` has used.
    pub async fn retries(db: &Db, id: AnalysisId, flow: &FlowId) -> Result<(u32, u32), DbError> {
        Ok(sqlx::query_as(
            "SELECT a.round, COALESCE((SELECT auto_retries FROM flow_work w WHERE w.analysis_id=a.id AND w.flow_id=?), 0)
             FROM analyses a WHERE a.id=?",
        )
        .bind(&flow.0)
        .bind(id.to_string())
        .fetch_one(&db.pool)
        .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::analyses::testing::seed_analysis;
    use contracts::agent::plan::PlannedFlow;
    use contracts::api::analyses::AnalysisState;
    use contracts::common::Classification;
    #[tokio::test]
    async fn demand_is_durable_newest_first_and_background_is_ranked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lazy.sqlite");
        let db = Db::open(&path).await.unwrap();
        let id = seed_analysis(&db).await.unwrap();
        db.flow_work()
            .set_background(id, true, Utc::now())
            .await
            .unwrap();
        let plan = PlanOutput {
            flows: (1..=3)
                .rev()
                .map(|n| PlannedFlow {
                    id: FlowId(format!("f{n}")),
                    title: format!("Flow {n}"),
                    classification: Classification::Behavioural,
                    importance_rank: n,
                    intent: "Bound retries".into(),
                    hunks: vec![],
                    depends_on: vec![],
                })
                .collect(),
            unassigned: vec![],
        };
        db.documents()
            .put(id, DocumentKind::Plan, "", &plan, Utc::now())
            .await
            .unwrap();
        assert_eq!(
            db.flow_work().next(id).await.unwrap(),
            NextFlow::Run(FlowId("f1".into()))
        );
        db.flow_work()
            .set_background(id, false, Utc::now())
            .await
            .unwrap();
        assert_eq!(db.flow_work().next(id).await.unwrap(), NextFlow::Idle);
        assert!(!db.flow_work().has_work(id).await.unwrap());
        let now = Utc::now();
        db.flow_work()
            .request(id, &FlowId("f3".into()), now)
            .await
            .unwrap();
        db.flow_work()
            .request(
                id,
                &FlowId("f2".into()),
                now + chrono::TimeDelta::seconds(1),
            )
            .await
            .unwrap();
        // Close and reopen the file-backed database: demand and the switch survive restart.
        drop(db);
        let db = Db::open(&path).await.unwrap();
        assert!(!db.flow_work().background(id).await.unwrap());
        assert!(db.flow_work().has_work(id).await.unwrap());
        assert_eq!(
            db.flow_work().next(id).await.unwrap(),
            NextFlow::Run(FlowId("f2".into()))
        );
        db.documents()
            .put(id, DocumentKind::Flow, "f2", &serde_json::json!({}), now)
            .await
            .unwrap();
        assert_eq!(
            db.flow_work().next(id).await.unwrap(),
            NextFlow::Run(FlowId("f3".into()))
        );
        // The requested flow keeps its demand through its automatic retries, a new job round each.
        for _ in 0..AUTO_FLOW_RETRIES {
            db.flow_work()
                .fail(id, &FlowId("f3".into()), "Failed")
                .await
                .unwrap();
            assert_eq!(
                db.flow_work().next(id).await.unwrap(),
                NextFlow::Run(FlowId("f3".into()))
            );
        }
        db.flow_work()
            .fail(id, &FlowId("f3".into()), "Failed")
            .await
            .unwrap();
        assert_eq!(db.flow_work().next(id).await.unwrap(), NextFlow::Idle);
        db.flow_work()
            .request(id, &FlowId("f3".into()), now)
            .await
            .unwrap();
        let ask = db
            .jobs()
            .enqueue(
                super::super::jobs::NewJob {
                    spec: crate::repo::analyses::testing::job_spec(
                        id,
                        contracts::agent::JobKind::Ask,
                    ),
                    priority: contracts::job::spec::JobPriority::Interactive,
                    stage: super::super::jobs::JobStage::Ask(
                        contracts::ids::QuestionId::new_random(),
                    ),
                    generation: 0,
                },
                now,
            )
            .await
            .unwrap();
        assert_eq!(
            db.flow_work().next(id).await.unwrap(),
            NextFlow::Wait,
            "retry must not strand an existing Ask in an old round"
        );
        assert!(db.flow_work().has_work(id).await.unwrap());
        db.jobs().request_cancel(ask.id, now).await.unwrap();
        assert_eq!(
            db.flow_work().next(id).await.unwrap(),
            NextFlow::Run(FlowId("f3".into()))
        );
        let round: i64 = sqlx::query_scalar("SELECT round FROM analyses WHERE id=?")
            .bind(id.to_string())
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(round, i64::from(AUTO_FLOW_RETRIES) + 1);
    }

    /// A plan of `count` flows `f1..`, ranked in that order.
    async fn planned(db: &Db, count: u32) -> AnalysisId {
        let id = seed_analysis(db).await.unwrap();
        let plan = PlanOutput {
            flows: (1..=count)
                .map(|n| PlannedFlow {
                    id: FlowId(format!("f{n}")),
                    title: format!("Flow {n}"),
                    classification: Classification::Behavioural,
                    importance_rank: n,
                    intent: "Bound retries".into(),
                    hunks: vec![],
                    depends_on: vec![],
                })
                .collect(),
            unassigned: vec![],
        };
        db.documents()
            .put(id, DocumentKind::Plan, "", &plan, Utc::now())
            .await
            .unwrap();
        db.documents()
            .put(
                id,
                DocumentKind::PrContext,
                "",
                &serde_json::json!({}),
                Utc::now(),
            )
            .await
            .unwrap();
        id
    }

    #[tokio::test]
    async fn failed_flow_is_retried_automatically_after_the_others_up_to_the_limit() {
        let db = Db::in_memory().await.unwrap();
        let id = planned(&db, 2).await;
        db.flow_work()
            .set_background(id, true, Utc::now())
            .await
            .unwrap();
        let (f1, f2) = (FlowId("f1".into()), FlowId("f2".into()));
        db.flow_work().fail(id, &f1, "Failed").await.unwrap();
        assert_eq!(
            db.flow_work().next(id).await.unwrap(),
            NextFlow::Run(f2.clone()),
            "flows not analysed yet come before a retry"
        );
        db.documents()
            .put(
                id,
                DocumentKind::Flow,
                "f2",
                &serde_json::json!({}),
                Utc::now(),
            )
            .await
            .unwrap();
        for retry in 1..=AUTO_FLOW_RETRIES {
            assert_eq!(
                db.flow_work().next(id).await.unwrap(),
                NextFlow::Run(f1.clone()),
                "automatic retry {retry}"
            );
            db.flow_work().fail(id, &f1, "Failed").await.unwrap();
        }
        assert_eq!(db.flow_work().next(id).await.unwrap(), NextFlow::Idle);
        assert!(db.flow_work().parked(id).await.unwrap());
        let round: u32 = sqlx::query_scalar("SELECT round FROM analyses WHERE id=?")
            .bind(id.to_string())
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(round, AUTO_FLOW_RETRIES, "each retry is a new job round");
        // The user's Retry still runs it, without using the automatic budget.
        db.flow_work().request(id, &f1, Utc::now()).await.unwrap();
        assert!(!db.flow_work().parked(id).await.unwrap());
        assert_eq!(
            db.flow_work().next(id).await.unwrap(),
            NextFlow::Run(f1.clone())
        );
        let used: u32 = sqlx::query_scalar("SELECT auto_retries FROM flow_work WHERE flow_id='f1'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(used, AUTO_FLOW_RETRIES);
    }

    #[tokio::test]
    async fn succeeded_job_without_a_document_is_listed_for_replay_and_keeps_the_analysis_unparked()
    {
        let db = Db::in_memory().await.unwrap();
        let id = planned(&db, 2).await;
        let now = Utc::now();
        let job = db
            .jobs()
            .enqueue(
                super::super::jobs::NewJob {
                    spec: crate::repo::analyses::testing::job_spec(
                        id,
                        contracts::agent::JobKind::AnalyseFlow,
                    ),
                    priority: contracts::job::spec::JobPriority::Batch,
                    stage: super::super::jobs::JobStage::Flow(FlowId("f2".into())),
                    generation: 0,
                },
                now,
            )
            .await
            .unwrap();
        sqlx::query("UPDATE jobs SET state='succeeded' WHERE id=?")
            .bind(job.id.to_string())
            .execute(&db.pool)
            .await
            .unwrap();
        db.flow_work().set_background(id, false, now).await.unwrap();
        assert_eq!(db.flow_work().next(id).await.unwrap(), NextFlow::Idle);
        assert_eq!(
            db.flow_work().unstored(id).await.unwrap(),
            [FlowId("f2".into())]
        );
        assert!(!db.flow_work().parked(id).await.unwrap());
        db.documents()
            .put(id, DocumentKind::Flow, "f2", &serde_json::json!({}), now)
            .await
            .unwrap();
        assert!(db.flow_work().unstored(id).await.unwrap().is_empty());
        assert!(
            db.flow_work().parked(id).await.unwrap(),
            "f1 is left, not requested"
        );
        db.documents()
            .put(id, DocumentKind::Flow, "f1", &serde_json::json!({}), now)
            .await
            .unwrap();
        assert!(
            !db.flow_work().parked(id).await.unwrap(),
            "every flow analysed: the driver finishes the analysis"
        );
    }

    /// A succeeded job of `flow` (its output not stored), as left by a rejected output whose repair waits.
    async fn succeeded_job(db: &Db, id: AnalysisId, flow: &FlowId) -> contracts::ids::JobId {
        let job = db
            .jobs()
            .enqueue(
                super::super::jobs::NewJob {
                    spec: crate::repo::analyses::testing::job_spec(
                        id,
                        contracts::agent::JobKind::AnalyseFlow,
                    ),
                    priority: contracts::job::spec::JobPriority::Batch,
                    stage: super::super::jobs::JobStage::Flow(flow.clone()),
                    generation: 0,
                },
                Utc::now(),
            )
            .await
            .unwrap();
        sqlx::query("UPDATE jobs SET state='succeeded' WHERE id=?")
            .bind(job.id.to_string())
            .execute(&db.pool)
            .await
            .unwrap();
        job.id
    }

    #[tokio::test]
    async fn flow_too_large_to_send_is_not_retried_automatically() {
        let db = Db::in_memory().await.unwrap();
        let id = planned(&db, 2).await;
        db.flow_work()
            .set_background(id, true, Utc::now())
            .await
            .unwrap();
        let (f1, f2) = (FlowId("f1".into()), FlowId("f2".into()));
        let now = Utc::now();
        db.documents()
            .put(id, DocumentKind::Unsent, "f2", &serde_json::json!({}), now)
            .await
            .unwrap();
        db.flow_work()
            .fail(id, &f2, "This flow is too large for the agent request.")
            .await
            .unwrap();
        assert_eq!(
            db.flow_work().next(id).await.unwrap(),
            NextFlow::Run(f1.clone())
        );
        db.documents()
            .put(id, DocumentKind::Flow, "f1", &serde_json::json!({}), now)
            .await
            .unwrap();
        assert_eq!(
            db.flow_work().next(id).await.unwrap(),
            NextFlow::Idle,
            "the same plan and diff give the same prompt: no automatic retry"
        );
        assert!(db.flow_work().parked(id).await.unwrap());
        assert_eq!(testing::retries(&db, id, &f2).await.unwrap(), (0, 0));
        // The user's Retry still tries it (the model's budget may have changed), once.
        db.flow_work().request(id, &f2, now).await.unwrap();
        assert_eq!(
            db.flow_work().next(id).await.unwrap(),
            NextFlow::Run(f2.clone())
        );
        db.flow_work()
            .fail(id, &f2, "This flow is too large for the agent request.")
            .await
            .unwrap();
        assert_eq!(db.flow_work().next(id).await.unwrap(), NextFlow::Idle);
    }

    #[tokio::test]
    async fn result_kept_for_a_postponed_repair_moves_to_the_round_of_another_flows_retry() {
        let db = Db::in_memory().await.unwrap();
        let id = planned(&db, 2).await;
        let (f1, f2) = (FlowId("f1".into()), FlowId("f2".into()));
        // f1: output rejected, its repair postponed. f2: failed after its repairs.
        let kept = succeeded_job(&db, id, &f1).await;
        succeeded_job(&db, id, &f2).await;
        db.flow_work().fail(id, &f2, "Failed").await.unwrap();
        db.flow_work().request(id, &f2, Utc::now()).await.unwrap();
        assert_eq!(
            db.flow_work().next(id).await.unwrap(),
            NextFlow::Run(f2.clone())
        );
        assert_eq!(testing::retries(&db, id, &f2).await.unwrap().0, 1);
        let stage = super::super::jobs::JobStage::Flow;
        assert_eq!(
            db.jobs()
                .current_for_stage(id, &stage(f1.clone()), 0)
                .await
                .unwrap()
                .map(|job| job.id),
            Some(kept),
            "the postponed repair still starts from the stored output"
        );
        assert_eq!(db.flow_work().unstored(id).await.unwrap(), [f1]);
        assert!(
            db.jobs()
                .current_for_stage(id, &stage(f2), 0)
                .await
                .unwrap()
                .is_none(),
            "the retried flow starts with a new first run"
        );
    }

    /// The automatic retries belong to a flow the user asked for as well: its demand stands while one is left,
    /// also with background analysis off. A flow nobody asked for is never run.
    #[tokio::test]
    async fn requested_flow_that_fails_is_retried_automatically_without_background_analysis() {
        let db = Db::in_memory().await.unwrap();
        let id = planned(&db, 2).await;
        let f1 = FlowId("f1".into());
        let now = Utc::now();
        let failure = "The flow analysis failed. The LLM call took too long and was stopped.";
        let by_user = format!("{failure} Retry this flow.");
        let work = db.flow_work();
        work.set_background(id, false, now).await.unwrap();
        work.request(id, &f1, now).await.unwrap();
        assert_eq!(work.next(id).await.unwrap(), NextFlow::Run(f1.clone()));
        for retry in 1..=AUTO_FLOW_RETRIES {
            work.fail(id, &f1, &by_user).await.unwrap();
            let status = work.status(id, &f1).await.unwrap();
            assert_eq!(status.state, FlowAnalysisState::Queued, "retry {retry}");
            assert_eq!(
                status.message,
                Some(format!("{failure} It will be tried again automatically."))
            );
            assert!(!work.parked(id).await.unwrap());
            assert_eq!(work.next(id).await.unwrap(), NextFlow::Run(f1.clone()));
            assert_eq!(
                testing::retries(&db, id, &f1).await.unwrap(),
                (retry, retry)
            );
        }
        work.fail(id, &f1, &by_user).await.unwrap();
        let status = work.status(id, &f1).await.unwrap();
        assert_eq!(status.state, FlowAnalysisState::Failed);
        assert_eq!(status.message, Some(by_user.clone()));
        assert_eq!(
            work.next(id).await.unwrap(),
            NextFlow::Idle,
            "f2 was never requested"
        );
        assert!(work.parked(id).await.unwrap());
        // The user's Retry runs it once more; no automatic retry is left.
        work.request(id, &f1, now).await.unwrap();
        assert_eq!(work.next(id).await.unwrap(), NextFlow::Run(f1.clone()));
        work.fail(id, &f1, &by_user).await.unwrap();
        assert_eq!(work.next(id).await.unwrap(), NextFlow::Idle);
    }

    #[tokio::test]
    async fn ended_analysis_promises_no_automatic_flow_retry() {
        use contracts::api::analyses::AnalysisState;
        use contracts::api::error::ApiErrorCode;
        let db = Db::in_memory().await.unwrap();
        let id = planned(&db, 2).await;
        let f1 = FlowId("f1".into());
        db.flow_work()
            .fail(id, &f1, "The flow analysis failed. Retry this flow.")
            .await
            .unwrap();
        let failed = |reason, message: &str| AnalysisState::Failed {
            reason,
            flow: None,
            message: message.into(),
        };
        // A flow request is refused once the analysis ended: the message names what still works, whether or not
        // the flow would be retried automatically in a live analysis.
        for background in [true, false] {
            db.flow_work()
                .set_background(id, background, Utc::now())
                .await
                .unwrap();
            for (ended, action) in [
                (
                    failed(
                        None,
                        "The analysis stopped because of a server error. Retry the analysis.",
                    ),
                    "Retry the analysis; then this flow can be analysed again.",
                ),
                (
                    failed(
                        Some(ApiErrorCode::Configuration),
                        "The server is misconfigured: a required agent skill is missing. Ask the administrator.",
                    ),
                    "Ask the administrator to fix the server configuration.",
                ),
                (
                    AnalysisState::Cancelled,
                    "Start a new analysis to analyse this flow.",
                ),
            ] {
                db.analyses()
                    .set_state(id, &ended, Utc::now())
                    .await
                    .unwrap();
                assert_eq!(
                    db.flow_work().status(id, &f1).await.unwrap().message,
                    Some(format!("The flow analysis failed. {action}")),
                    "nothing runs in an analysis that is {ended:?} (background {background})"
                );
            }
        }
    }

    #[tokio::test]
    async fn failed_flow_says_whether_it_is_tried_again_automatically() {
        let db = Db::in_memory().await.unwrap();
        let id = planned(&db, 2).await;
        db.flow_work()
            .set_background(id, true, Utc::now())
            .await
            .unwrap();
        let f1 = FlowId("f1".into());
        let message = |status: FlowAnalysisStatus| {
            assert_eq!(status.state, FlowAnalysisState::Failed);
            status.message.unwrap()
        };
        let failure = "The flow analysis failed. The LLM call took too long and was stopped.";
        let by_user = format!("{failure} Retry this flow.");
        db.flow_work().fail(id, &f1, &by_user).await.unwrap();
        assert_eq!(
            message(db.flow_work().status(id, &f1).await.unwrap()),
            format!("{failure} It will be tried again automatically.")
        );
        db.flow_work()
            .set_background(id, false, Utc::now())
            .await
            .unwrap();
        assert_eq!(
            message(db.flow_work().status(id, &f1).await.unwrap()),
            by_user,
            "nothing is retried while background analysis is off"
        );
        db.flow_work()
            .set_background(id, true, Utc::now())
            .await
            .unwrap();
        sqlx::query("UPDATE flow_work SET auto_retries=?")
            .bind(AUTO_FLOW_RETRIES)
            .execute(&db.pool)
            .await
            .unwrap();
        assert_eq!(
            message(db.flow_work().status(id, &f1).await.unwrap()),
            by_user,
            "no automatic retry is left"
        );
    }

    #[tokio::test]
    async fn a_rejected_stored_flow_is_discarded_and_runs_again_in_a_new_round() {
        let db = Db::in_memory().await.unwrap();
        let id = planned(&db, 2).await;
        for flow in ["f1", "f2"] {
            db.documents()
                .put(
                    id,
                    DocumentKind::Flow,
                    flow,
                    &serde_json::json!({}),
                    Utc::now(),
                )
                .await
                .unwrap();
        }
        db.analyses()
            .set_state(id, &AnalysisState::Finished { total: 2 }, Utc::now())
            .await
            .unwrap();
        let f1 = FlowId("f1".into());
        assert!(
            db.flow_work()
                .request_again(id, &f1, Utc::now())
                .await
                .unwrap()
        );
        assert!(
            db.documents()
                .get::<serde_json::Value>(id, DocumentKind::Flow, "f1")
                .await
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            db.analyses().summary(id).await.unwrap().state,
            AnalysisState::Running {
                done: 1,
                total: 2,
                ..
            }
        ));
        let status = db.flow_work().status(id, &f1).await.unwrap();
        assert_eq!(status.state, FlowAnalysisState::Queued);
        assert!(status.message.is_some_and(|m| !m.ends_with(RETRY_BY_USER)));
        assert_eq!(
            db.flow_work().next(id).await.unwrap(),
            NextFlow::Run(f1.clone())
        );
        assert_eq!(
            testing::retries(&db, id, &f1).await.unwrap(),
            (1, 0),
            "a new round, no automatic retry used"
        );
        assert!(
            !db.flow_work()
                .request_again(id, &f1, Utc::now())
                .await
                .unwrap(),
            "nothing stored any more"
        );
    }

    #[tokio::test]
    async fn a_finished_analysis_with_a_requested_flow_left_runs_again() {
        let db = Db::in_memory().await.unwrap();
        let id = planned(&db, 2).await;
        for flow in ["f1", "f2"] {
            db.documents()
                .put(
                    id,
                    DocumentKind::Flow,
                    flow,
                    &serde_json::json!({}),
                    Utc::now(),
                )
                .await
                .unwrap();
        }
        let f1 = FlowId("f1".into());
        // Discarded while the driver, which had read every flow, finished the analysis.
        assert!(
            db.flow_work()
                .request_again(id, &f1, Utc::now())
                .await
                .unwrap()
        );
        db.analyses()
            .set_state(id, &AnalysisState::Finished { total: 2 }, Utc::now())
            .await
            .unwrap();
        assert!(db.flow_work().reopen(id).await.unwrap());
        assert!(matches!(
            db.analyses().summary(id).await.unwrap().state,
            AnalysisState::Running { .. }
        ));
        db.documents()
            .put(
                id,
                DocumentKind::Flow,
                "f1",
                &serde_json::json!({}),
                Utc::now(),
            )
            .await
            .unwrap();
        db.analyses()
            .set_state(id, &AnalysisState::Finished { total: 2 }, Utc::now())
            .await
            .unwrap();
        assert!(
            !db.flow_work().reopen(id).await.unwrap(),
            "every flow analysed"
        );
    }

    #[tokio::test]
    async fn analysing_all_remaining_flows_reruns_rejected_stored_flows() {
        let db = Db::in_memory().await.unwrap();
        let id = planned(&db, 3).await;
        for flow in ["f1", "f2"] {
            db.documents()
                .put(
                    id,
                    DocumentKind::Flow,
                    flow,
                    &serde_json::json!({}),
                    Utc::now(),
                )
                .await
                .unwrap();
        }
        let f2 = FlowId("f2".into());
        db.flow_work()
            .analyse_remaining(id, std::slice::from_ref(&f2), Utc::now())
            .await
            .unwrap();
        assert!(db.flow_work().background(id).await.unwrap());
        assert!(
            db.documents()
                .get::<serde_json::Value>(id, DocumentKind::Flow, "f2")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            db.documents()
                .get::<serde_json::Value>(id, DocumentKind::Flow, "f1")
                .await
                .unwrap()
                .is_some(),
            "an accepted stored flow stays"
        );
        assert_eq!(
            db.flow_work().status(id, &f2).await.unwrap().state,
            FlowAnalysisState::Queued
        );
        assert_eq!(
            db.flow_work().next(id).await.unwrap(),
            NextFlow::Run(f2.clone())
        );
        assert_eq!(
            testing::retries(&db, id, &f2).await.unwrap(),
            (1, 0),
            "a new round, like a single request"
        );
    }
}
