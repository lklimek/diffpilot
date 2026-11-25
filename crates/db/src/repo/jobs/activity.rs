//! The tenant's active queue and the reader's provider status, read together.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use contracts::agent::JobKind;
use contracts::common::{PrRef, RepoRef};
use contracts::ids::{AnalysisId, JobId};
use contracts::time::Timestamp;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use tokio::sync::watch;

use super::{Jobs, optional_ts, parse_wire};
use crate::error::DbError;
use crate::repo::settings::LastCall;

/// The active queue and credential facts visible to one reader.
#[derive(Debug)]
pub struct Activity {
    /// Active jobs of the reader's tenant.
    pub jobs: Vec<ActiveJob>,
    /// The reader's credential pause.
    pub paused_until: Option<Timestamp>,
    /// The last call on the reader's credential.
    pub last_call: Option<LastCall>,
    /// When the reader's credential became unusable.
    pub unusable_since: Option<Timestamp>,
}

/// The public-in-tenant facts about a job, with details only for its payer.
#[derive(Debug)]
pub struct ActiveJob {
    /// The kind of work.
    pub kind: JobKind,
    /// A runner has taken the job.
    pub running: bool,
    /// Private details when the reader paid for this job.
    pub own: Option<OwnJob>,
}

/// A reader's own job details.
#[derive(Debug)]
pub struct OwnJob {
    /// The actual job id.
    pub id: JobId,
    /// The pull request, when the job concerns one.
    pub pr: Option<PrRef>,
    /// The job's analysis, when it has one.
    pub analysis: Option<AnalysisId>,
    /// When the runner took the current attempt.
    pub claimed_at: Option<Timestamp>,
    /// When the job entered the queue.
    pub queued_at: Option<Timestamp>,
    /// When a usage-limited job can be requeued.
    pub resumes_at: Option<Timestamp>,
    /// A review analysis is active in this tenant (dashboard ranking wait).
    pub analysis_running: bool,
}

/// Coalesces committed job changes for each tenant's subscribers.
#[derive(Debug, Default)]
pub(crate) struct JobChanges(Mutex<HashMap<String, watch::Sender<()>>>);

impl JobChanges {
    pub(super) fn subscribe(&self, tenant: &str) -> watch::Receiver<()> {
        let mut channels = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        channels.retain(|_, channel| channel.receiver_count() > 0);
        channels
            .entry(tenant.to_owned())
            .or_insert_with(|| watch::channel(()).0)
            .subscribe()
    }

    pub(crate) fn publish(&self, tenant: &str) {
        let channels = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(channel) = channels.get(tenant) {
            let _ = channel.send(());
        }
    }
}

impl Jobs<'_> {
    /// Subscribe before reading the queue to catch changes committed during the read.
    pub fn subscribe_activity(&self, tenant: &str) -> watch::Receiver<()> {
        self.changes.subscribe(tenant)
    }

    /// Read the tenant's active queue and the reader's credential in one SQLite snapshot.
    pub async fn activity(&self, tenant: &str, reader: &str) -> Result<Activity, DbError> {
        let rows = sqlx::query(
            "WITH active AS (
                 SELECT j.id, j.kind, j.state, j.pull_request_id, j.analysis_id, j.claimed_at, j.created_at,
                     j.resumes_at, json_extract(j.spec, '$.payer') = ?2 AS own
                 FROM jobs j WHERE j.tenant_id = ?1 AND j.state IN ('queued', 'claimed', 'running', 'paused')
             ), reader AS (SELECT ?1 AS tenant_id, ?2 AS user_id)
             SELECT j.kind, j.state IN ('claimed', 'running') AS running,
                 CASE WHEN j.own THEN j.id END AS own_id,
                 CASE WHEN j.own THEN j.analysis_id END AS analysis_id,
                 CASE WHEN j.own AND j.state IN ('claimed', 'running') THEN j.claimed_at END AS claimed_at,
                 CASE WHEN j.own THEN j.created_at END AS queued_at,
                 CASE WHEN j.own THEN j.resumes_at END AS resumes_at,
                 r.owner, r.name, CASE WHEN r.id IS NOT NULL THEN p.number END AS pr_number,
                 CASE WHEN j.own AND j.kind = 'judge_work' AND j.state = 'queued' THEN EXISTS (
                     SELECT 1 FROM active a JOIN analyses an ON an.id = a.analysis_id
                     WHERE an.tenant_id = reader.tenant_id
                         AND a.state IN ('claimed', 'running') AND an.state IN ('planning', 'running', 'paused')
                 ) ELSE 0 END AS analysis_running,
                 c.paused_until, c.last_call_at, c.last_call_ok, c.last_call_message, c.last_call_failure,
                 CASE WHEN c.state = 'relink_needed' THEN c.relink_needed_at END AS unusable_since
             FROM reader
             LEFT JOIN active j ON 1 = 1
             LEFT JOIN pull_requests p ON j.own AND p.id = j.pull_request_id AND p.tenant_id = reader.tenant_id
             LEFT JOIN repos r ON r.id = p.repo_id AND r.tenant_id = reader.tenant_id
             LEFT JOIN llm_credentials c ON c.user_id = reader.user_id AND c.tenant_id = reader.tenant_id
             ORDER BY running DESC, j.own DESC, queued_at, j.kind, own_id",
        )
        .bind(tenant)
        .bind(reader)
        .fetch_all(self.pool)
        .await?;
        let credential = rows.first().ok_or(DbError::Corrupt {
            column: "activity reader",
        })?;
        Ok(Activity {
            paused_until: optional_ts(credential, "paused_until")?,
            last_call: LastCall::from_row(credential)?,
            unusable_since: optional_ts(credential, "unusable_since")?,
            jobs: rows
                .iter()
                .filter_map(|row| active_job(row).transpose())
                .collect::<Result<_, _>>()?,
        })
    }
}

fn active_job(row: &SqliteRow) -> Result<Option<ActiveJob>, DbError> {
    let Some(kind) = row.try_get::<Option<String>, _>("kind")? else {
        return Ok(None);
    };
    let own = row
        .try_get::<Option<String>, _>("own_id")?
        .map(|id| -> Result<OwnJob, DbError> {
            let pr = row
                .try_get::<Option<u32>, _>("pr_number")?
                .map(|number| -> Result<PrRef, DbError> {
                    Ok(PrRef {
                        repo: RepoRef {
                            owner: row.try_get("owner")?,
                            name: row.try_get("name")?,
                        },
                        number,
                    })
                })
                .transpose()?;
            Ok(OwnJob {
                id: parse_wire(&id, "jobs.id")?,
                pr,
                analysis: row
                    .try_get::<Option<String>, _>("analysis_id")?
                    .map(|id| parse_wire(&id, "jobs.analysis_id"))
                    .transpose()?,
                claimed_at: optional_ts(row, "claimed_at")?,
                queued_at: optional_ts(row, "queued_at")?,
                resumes_at: optional_ts(row, "resumes_at")?,
                analysis_running: row.try_get("analysis_running")?,
            })
        })
        .transpose()?;
    Ok(Some(ActiveJob {
        kind: parse_wire(&kind, "jobs.kind")?,
        running: row.try_get("running")?,
        own,
    }))
}
