//! Durable demand ordering for lazy flow analysis; no runner scheduling policy changes.
use super::{
    Db,
    documents::DocumentKind,
    jobs::{parse_ts, ts},
};
use crate::error::DbError;
use chrono::{DateTime, Utc};
use contracts::agent::plan::PlanOutput;
use contracts::api::tour::{FlowAnalysisState, FlowAnalysisStatus};
use contracts::ids::{AnalysisId, FlowId};
use sqlx::Row;

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
        let rows = sqlx::query("SELECT id,attempt FROM jobs WHERE analysis_id=? AND kind='analyse_flow' AND state='queued' AND flow_id<>?")
            .bind(id.to_string()).bind(&flow.0).fetch_all(&mut *tx).await?;
        withdraw(&mut tx, rows, now).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Next demand, otherwise plan rank; failed flows wait for an explicit request.
    pub async fn next(&self, id: AnalysisId) -> Result<Option<FlowId>, DbError> {
        if let Some(flow) = sqlx::query_scalar::<_, String>("SELECT flow_id FROM jobs WHERE analysis_id=? AND kind='analyse_flow' AND state IN ('claimed','running','paused') ORDER BY created_at LIMIT 1")
            .bind(id.to_string()).fetch_optional(&self.db.pool).await? { return Ok(Some(FlowId(flow))); }

        let row=sqlx::query("SELECT json_extract(f.value,'$.id') AS flow, w.error FROM documents d, json_each(d.body,'$.flows') f JOIN analyses a ON a.id=d.analysis_id LEFT JOIN flow_work w ON w.analysis_id=a.id AND w.flow_id=json_extract(f.value,'$.id') WHERE d.analysis_id=? AND d.kind='plan' AND d.key='' AND NOT EXISTS(SELECT 1 FROM documents done WHERE done.analysis_id=a.id AND done.kind='flow' AND done.key=json_extract(f.value,'$.id')) AND (w.requested_at IS NOT NULL OR (a.background_analysis=1 AND w.error IS NULL)) ORDER BY w.requested_at IS NOT NULL DESC, w.requested_at DESC, CAST(json_extract(f.value,'$.importance_rank') AS INTEGER) LIMIT 1")
            .bind(id.to_string()).fetch_optional(&self.db.pool).await?;
        let Some(row) = row else { return Ok(None) };
        let flow: String = row.try_get("flow")?;
        let error: Option<String> = row.try_get("error")?;
        if error.is_some() {
            // Wait for outstanding Ask too: changing round must not strand a queued question. A new round separates successful
            // but rejected repair outputs from the requested retry, preserving all accepted documents.
            let mut tx = super::write_tx(&self.db.pool).await?;
            let live: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE analysis_id=? AND state IN ('queued','claimed','running','paused'))")
                .bind(id.to_string()).fetch_one(&mut *tx).await?;
            if live {
                return Ok(None);
            }
            sqlx::query("UPDATE analyses SET round=round+1 WHERE id=?")
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE flow_work SET error=NULL WHERE analysis_id=? AND flow_id=?")
                .bind(id.to_string())
                .bind(&flow)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        Ok(Some(FlowId(flow)))
    }
    /// Mark a flow failed, retaining the accepted plan and other flow outputs.
    pub async fn fail(&self, id: AnalysisId, flow: &FlowId, message: &str) -> Result<(), DbError> {
        sqlx::query("INSERT INTO flow_work(tenant_id,analysis_id,flow_id,error) SELECT tenant_id,id,?,? FROM analyses WHERE id=? ON CONFLICT(analysis_id,flow_id) DO UPDATE SET error=excluded.error, requested_at=NULL")
            .bind(&flow.0).bind(message).bind(id.to_string()).execute(&self.db.pool).await?;
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

    /// Status comes from accepted documents, durable requests/failures and current jobs.
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
        if self
            .db
            .documents()
            .get::<serde_json::Value>(id, DocumentKind::Flow, &flow.0)
            .await?
            .is_some()
        {
            status.state = FlowAnalysisState::Analysed;
            return Ok(status);
        }
        if let Some(row) = sqlx::query(
            "SELECT requested_at,error FROM flow_work WHERE analysis_id=? AND flow_id=?",
        )
        .bind(id.to_string())
        .bind(&flow.0)
        .fetch_optional(&self.db.pool)
        .await?
        {
            status.message = row.try_get("error")?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::analyses::testing::seed_analysis;
    use contracts::agent::plan::PlannedFlow;
    use contracts::common::Classification;
    #[tokio::test]
    async fn demand_is_durable_newest_first_and_background_is_ranked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lazy.sqlite");
        let db = Db::open(&path).await.unwrap();
        let id = seed_analysis(&db).await.unwrap();
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
            Some(FlowId("f1".into()))
        );
        db.flow_work()
            .set_background(id, false, Utc::now())
            .await
            .unwrap();
        assert_eq!(db.flow_work().next(id).await.unwrap(), None);
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
        assert_eq!(
            db.flow_work().next(id).await.unwrap(),
            Some(FlowId("f2".into()))
        );
        db.documents()
            .put(id, DocumentKind::Flow, "f2", &serde_json::json!({}), now)
            .await
            .unwrap();
        assert_eq!(
            db.flow_work().next(id).await.unwrap(),
            Some(FlowId("f3".into()))
        );
        db.flow_work()
            .fail(id, &FlowId("f3".into()), "Failed")
            .await
            .unwrap();
        assert_eq!(db.flow_work().next(id).await.unwrap(), None);
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
            None,
            "retry must not strand an existing Ask in an old round"
        );
        db.jobs().request_cancel(ask.id, now).await.unwrap();
        assert_eq!(
            db.flow_work().next(id).await.unwrap(),
            Some(FlowId("f3".into()))
        );
        let round: i64 = sqlx::query_scalar("SELECT round FROM analyses WHERE id=?")
            .bind(id.to_string())
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(round, 1);
    }
}
