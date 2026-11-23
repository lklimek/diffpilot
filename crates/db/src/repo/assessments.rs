//! Atomic admission and fenced method results; all model rows are normalized comparison data.

use super::jobs::{parse_ts, parse_wire, ts, wire_name};
use super::{Db, read_tx, write_tx};
use crate::error::DbError;
use contracts::api::assessment::AssessmentComparison;
use contracts::assessment::*;
use contracts::ids::{DecisionId, PassId, UserId};
use contracts::job::result::{JobFailureKind, Usage};
use contracts::job::spec::{JobSpec, JobTarget};
use contracts::time::Timestamp;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteConnection, SqlitePool};

/// Admission distinguishes safe pre-admission failures from repository failures.
#[derive(Debug, thiserror::Error)]
pub enum AssessmentError {
    /// No slot was consumed.
    #[error(transparent)]
    Admission(#[from] AssessmentAdmissionFailure),
    /// Storage or contract validation failed; the transaction is rolled back.
    #[error(transparent)]
    Database(#[from] DbError),
}

/// Adapter job over a frozen input, with its own attempt fence.
#[derive(Clone, Debug)]
pub struct MethodJob {
    /// Self-contained job, with an explicit assessment reference.
    pub spec: JobSpec,
    /// Native adapter version.
    pub version: String,
}

/// Caller-prepared snapshot and jobs; admission freezes current user configuration atomically.
#[derive(Clone, Debug)]
pub struct NewAssessmentPass {
    /// Proposed pass ID, ignored when the request key was already admitted.
    pub id: PassId,
    /// Payer-scoped logical request key, stable across retries/restarts.
    pub request_key: String,
    /// Immutable host-owned snapshot.
    pub input: AssessmentInput,
    /// One job per selected adapter, primary first.
    pub jobs: Vec<MethodJob>,
}

/// One method attempt's normalized, scrubbed result; no raw report is persisted.
#[derive(Clone, Debug)]
pub struct MethodResult {
    /// Succeeded, failed or cancelled.
    pub status: AssessmentRunStatus,
    /// Usage of this attempt alone, including compaction.
    pub usage: Usage,
    /// Duration of this attempt alone.
    pub duration_ms: u64,
    /// Safe failure kind; never arbitrary upstream text.
    pub failure: Option<JobFailureKind>,
    /// Normalized rows; secondary rows never create primary items.
    pub items: Vec<AssessmentComparisonItem>,
}

/// Repository for per-user settings, frozen passes, metering and comparison history.
#[derive(Debug)]
pub struct Assessments<'a> {
    pool: &'a SqlitePool,
}

impl Db {
    /// Assessment persistence and atomic quota primitives.
    pub fn assessments(&self) -> Assessments<'_> {
        Assessments { pool: &self.pool }
    }
}

impl Assessments<'_> {
    /// Read effective settings, defaulting to claudius_host and no pairing.
    pub async fn config(&self, user: &UserId) -> Result<AssessmentConfig, DbError> {
        let mut tx = read_tx(self.pool).await?;
        let config = config_in(&mut tx, user).await?;
        tx.commit().await?;
        Ok(config)
    }

    /// Store settings for an existing tenant member; never enqueue a job.
    pub async fn set_config(
        &self,
        tenant: &str,
        user: &UserId,
        config: &AssessmentConfig,
    ) -> Result<(), DbError> {
        let mut tx = write_tx(self.pool).await?;
        let inserted=sqlx::query("INSERT INTO assessment_config(user_id,primary_method,ab_prs,ab_until) SELECT ?,?,?,? WHERE EXISTS(SELECT 1 FROM memberships WHERE tenant_id=? AND user_id=?) ON CONFLICT(user_id) DO UPDATE SET primary_method=excluded.primary_method,ab_prs=excluded.ab_prs,ab_until=excluded.ab_until")
            .bind(&user.0).bind(wire_name(config.primary_method)?).bind(serde_json::to_string(&config.ab_prs)?).bind(config.ab_until.map(ts)).bind(tenant).bind(&user.0).execute(&mut *tx).await?;
        if inserted.rows_affected() != 1 {
            return Err(DbError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }

    /// Safe quota status for the UTC day containing now.
    pub async fn quota(&self, user: &UserId, now: Timestamp) -> Result<AssessmentQuota, DbError> {
        let (day, resets_at) = utc_day(now)?;
        let count: Option<u32> = sqlx::query_scalar(
            "SELECT passes FROM assessment_daily_usage WHERE user_id=? AND day_utc=?",
        )
        .bind(&user.0)
        .bind(day)
        .fetch_optional(self.pool)
        .await?;
        Ok(AssessmentQuota {
            remaining: 80 - count.unwrap_or(0),
            resets_at,
        })
    }

    /// Reserve one logical slot and enqueue all selected method jobs in one write transaction.
    pub async fn admit(
        &self,
        tenant: &str,
        user: &UserId,
        request: &NewAssessmentPass,
        now: Timestamp,
    ) -> Result<AssessmentPass, AssessmentError> {
        let mut tx = write_tx(self.pool).await?;
        let existing =
            sqlx::query("SELECT * FROM assessment_passes WHERE payer_id=? AND request_key=?")
                .bind(&user.0)
                .bind(&request.request_key)
                .fetch_optional(&mut *tx)
                .await
                .map_err(DbError::from)?;
        if let Some(row) = existing {
            if row
                .try_get::<String, _>("tenant_id")
                .map_err(DbError::from)?
                != tenant
            {
                return Err(DbError::NotFound.into());
            }
            let pass = pass_from_row(&row)?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(pass);
        }
        let usable:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM llm_credentials c JOIN memberships m ON m.user_id=c.user_id WHERE c.user_id=? AND m.tenant_id=? AND c.mode='chatgpt_plan' AND c.state='linked' AND (c.paused_until IS NULL OR julianday(c.paused_until)<=julianday(?)))")
            .bind(&user.0).bind(tenant).bind(ts(now)).fetch_one(&mut *tx).await.map_err(DbError::from)?;
        if !usable {
            return Err(AssessmentAdmissionFailure::CredentialMissing.into());
        }
        let config = config_in(&mut tx, user).await?;
        let paired = config.paired(&request.input.pr, now);
        validate_jobs(user, request, &config, paired)?;
        let (day, resets_at) = utc_day(now)?;
        let reserved=sqlx::query("INSERT INTO assessment_daily_usage(user_id,day_utc,passes) VALUES(?,?,1) ON CONFLICT(user_id,day_utc) DO UPDATE SET passes=passes+1 WHERE passes<80")
            .bind(&user.0).bind(&day).execute(&mut *tx).await.map_err(DbError::from)?;
        if reserved.rows_affected() != 1 {
            return Err(AssessmentAdmissionFailure::AssessmentDailyLimit { resets_at }.into());
        }
        let input = &request.input;
        sqlx::query("INSERT INTO assessment_passes(id,tenant_id,payer_id,pull_request_id,request_key,head,base,input_hash,scope_hash,snapshot_ref,primary_method,paired,config,input,day_utc,admitted_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(request.id.to_string()).bind(tenant).bind(&user.0).bind(input.pull_request_id).bind(&request.request_key)
            .bind(&input.head.0).bind(&input.base.0).bind(&input.input_hash).bind(&input.scope_hash).bind(&input.snapshot_ref)
            .bind(wire_name(config.primary_method)?).bind(paired).bind(serde_json::to_string(&config).map_err(DbError::from)?)
            .bind(serde_json::to_string(input).map_err(DbError::from)?).bind(&day).bind(ts(now)).execute(&mut *tx).await.map_err(DbError::from)?;
        for job in &request.jobs {
            let mut spec = job.spec.clone();
            spec.attempt = 0;
            spec.access = Default::default();
            let method = spec.assessment.as_ref().ok_or(DbError::Conflict)?.method;
            sqlx::query("INSERT INTO assessment_method_runs(pass_id,method,tenant_id,payer_id,job_id,version) VALUES(?,?,?,?,?,?)")
                .bind(request.id.to_string()).bind(wire_name(method)?).bind(tenant).bind(&user.0).bind(spec.job_id.to_string()).bind(&job.version)
                .execute(&mut *tx).await.map_err(DbError::from)?;
            sqlx::query("INSERT INTO jobs(id,tenant_id,assessment_pass_id,assessment_method,kind,priority,state,spec,payer_id,pull_request_id,stage,created_at) VALUES(?,?,?,?,'verify_comments',0,'queued',?,?,?,?,?)")
                .bind(spec.job_id.to_string()).bind(tenant).bind(request.id.to_string()).bind(wire_name(method)?)
                .bind(serde_json::to_string(&spec).map_err(DbError::from)?).bind(&user.0).bind(input.pull_request_id)
                .bind(format!("verify:{}",request.id)).bind(ts(now)).execute(&mut *tx).await.map_err(DbError::from)?;
        }
        tx.commit().await.map_err(DbError::from)?;
        Ok(AssessmentPass {
            id: request.id,
            payer: user.clone(),
            input: input.clone(),
            config,
            paired,
            day_utc: day,
            admitted_at: now,
        })
    }

    /// Bind a method to the queue's current claimed/running attempt, never reopening accepted output.
    pub async fn begin_attempt(
        &self,
        tenant: &str,
        user: &UserId,
        run: &AssessmentRunRef,
        attempt: u32,
    ) -> Result<(), DbError> {
        if attempt == 0 {
            return Err(DbError::Conflict);
        }
        let mut tx = write_tx(self.pool).await?;
        let changed=sqlx::query("UPDATE assessment_method_runs SET attempt=?,status='running',failure=NULL WHERE pass_id=? AND method=? AND tenant_id=? AND payer_id=? AND accepted_attempt IS NULL AND (attempt<? OR (attempt=? AND status='running')) AND EXISTS(SELECT 1 FROM jobs j WHERE j.id=assessment_method_runs.job_id AND j.attempt=? AND j.state IN ('claimed','running'))")
            .bind(attempt).bind(run.pass_id.to_string()).bind(wire_name(run.method)?).bind(tenant).bind(&user.0).bind(attempt).bind(attempt).bind(attempt).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(DbError::Conflict);
        }
        tx.commit().await?;
        Ok(())
    }

    /// Accept one current attempt exactly once; retries retain pass/day and sum metering once per attempt.
    pub async fn finish_attempt(
        &self,
        tenant: &str,
        user: &UserId,
        run: &AssessmentRunRef,
        attempt: u32,
        result: &MethodResult,
    ) -> Result<bool, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let pass = pass_in(&mut tx, tenant, user, run.pass_id).await?;
        let current = runs_in(&mut tx, tenant, user, run.pass_id)
            .await?
            .into_iter()
            .find(|r| r.reference == *run)
            .ok_or(DbError::NotFound)?;
        // Complete retains the claim token; expiry and release clear it, including on exhausted jobs.
        let eligible: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM jobs WHERE id=? AND attempt=? AND cancel_requested=0
             AND (state IN ('claimed','running') OR
                  (state IN ('succeeded','failed') AND job_token_hash IS NOT NULL
                   AND json_extract(result,'$.attempt')=attempt
                   AND json_extract(result,'$.outcome.outcome')=state
                   AND (state='succeeded' OR NOT ?))))",
        )
        .bind(current.job_id.to_string())
        .bind(attempt)
        .bind(result.status == AssessmentRunStatus::Succeeded)
        .fetch_one(&mut *tx)
        .await?;
        if attempt == 0 || current.attempt != attempt || !eligible {
            return Err(DbError::Conflict);
        }
        if current.status != AssessmentRunStatus::Running {
            tx.commit().await?;
            return Ok(false);
        }
        if !matches!(
            result.status,
            AssessmentRunStatus::Succeeded
                | AssessmentRunStatus::Failed
                | AssessmentRunStatus::Cancelled
        ) || (result.status == AssessmentRunStatus::Failed) != result.failure.is_some()
            || (result.status != AssessmentRunStatus::Succeeded && !result.items.is_empty())
        {
            return Err(DbError::Conflict);
        }
        for item in &result.items {
            if item.run != *run
                || item.method_version != current.version
                || item.owner_decision.is_some()
                || (item.status == ComparisonItemStatus::Duplicate) != item.duplicate_of.is_some()
                || (matches!(
                    item.status,
                    ComparisonItemStatus::Duplicate
                        | ComparisonItemStatus::MissingRow
                        | ComparisonItemStatus::MethodFailed
                ) && item.verdict.is_some())
            {
                return Err(DbError::Conflict);
            }
        }
        meter_in(&mut tx, &current, attempt, result.usage, result.duration_ms).await?;
        // Canonical rows precede direct duplicate links; no secondary row allocates a comment item.
        for item in result
            .items
            .iter()
            .filter(|i| i.duplicate_of.is_none())
            .chain(result.items.iter().filter(|i| i.duplicate_of.is_some()))
        {
            sqlx::query("INSERT INTO assessment_comparison_items(pass_id,method,tenant_id,payer_id,pull_request_id,comparison_key,local_id,method_version,source,primary_item_id,primary_item_hash,status,duplicate_of,verdict,native_verdict,proposal) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
                .bind(run.pass_id.to_string()).bind(wire_name(run.method)?).bind(tenant).bind(&user.0).bind(pass.input.pull_request_id)
                .bind(&item.comparison_key).bind(&item.local_id).bind(&item.method_version).bind(serde_json::to_string(&item.source)?)
                .bind(item.primary_item_id.map(|id|id.to_string())).bind(&item.primary_item_hash).bind(wire_name(item.status)?).bind(&item.duplicate_of)
                .bind(item.verdict.map(wire_name).transpose()?).bind(&item.native_verdict).bind(item.proposal.as_ref().map(serde_json::to_string).transpose()?)
                .execute(&mut *tx).await?;
        }
        let accepted = (result.status == AssessmentRunStatus::Succeeded).then_some(attempt);
        sqlx::query("UPDATE assessment_method_runs SET status=?,accepted_attempt=?,failure=? WHERE pass_id=? AND method=? AND tenant_id=? AND payer_id=? AND attempt=? AND status='running'")
            .bind(wire_name(result.status)?).bind(accepted).bind(result.failure.map(wire_name).transpose()?)
            .bind(run.pass_id.to_string()).bind(wire_name(run.method)?).bind(tenant).bind(&user.0).bind(attempt).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Record late metering for any started attempt without accepting or replacing its output.
    pub async fn record_attempt_usage(
        &self,
        tenant: &str,
        user: &UserId,
        run: &AssessmentRunRef,
        attempt: u32,
        usage: Usage,
        duration_ms: u64,
    ) -> Result<bool, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let current = runs_in(&mut tx, tenant, user, run.pass_id)
            .await?
            .into_iter()
            .find(|r| r.reference == *run)
            .ok_or(DbError::NotFound)?;
        let latest: u32 = sqlx::query_scalar("SELECT attempt FROM jobs WHERE id=?")
            .bind(current.job_id.to_string())
            .fetch_one(&mut *tx)
            .await?;
        if attempt == 0 || attempt > latest {
            return Err(DbError::Conflict);
        }
        let added = meter_in(&mut tx, &current, attempt, usage, duration_ms).await?;
        tx.commit().await?;
        Ok(added)
    }

    /// Requeue only a failed retryable method, preserving its frozen input and quota slot.
    pub async fn retry_method(
        &self,
        tenant: &str,
        user: &UserId,
        run: &AssessmentRunRef,
        attempt: u32,
    ) -> Result<(), DbError> {
        let mut tx = write_tx(self.pool).await?;
        let current = runs_in(&mut tx, tenant, user, run.pass_id)
            .await?
            .into_iter()
            .find(|r| r.reference == *run)
            .ok_or(DbError::NotFound)?;
        if current.status != AssessmentRunStatus::Failed
            || current.attempt != attempt
            || current.accepted_attempt.is_some()
            || !current.failure.is_some_and(JobFailureKind::retryable)
        {
            return Err(DbError::Conflict);
        }
        let changed=sqlx::query("UPDATE jobs SET state='queued',result=NULL,ready_output=NULL,ready_output_attempt=NULL,finished_at=NULL,lease_expires_at=NULL,job_token_hash=NULL,cancel_requested=0 WHERE id=? AND attempt=? AND state='failed' AND attempt<max_attempts")
            .bind(current.job_id.to_string()).bind(attempt).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(DbError::Conflict);
        }
        sqlx::query("UPDATE assessment_method_runs SET status='queued' WHERE pass_id=? AND method=? AND tenant_id=? AND payer_id=?")
            .bind(run.pass_id.to_string()).bind(wire_name(run.method)?).bind(tenant).bind(&user.0).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Read method summaries for the payer and pass.
    pub async fn runs(
        &self,
        tenant: &str,
        user: &UserId,
        pass: PassId,
    ) -> Result<Vec<AssessmentMethodRun>, DbError> {
        let mut tx = read_tx(self.pool).await?;
        let runs = runs_in(&mut tx, tenant, user, pass).await?;
        tx.commit().await?;
        Ok(runs)
    }

    /// Read normalized rows and the latest actual owner revision as of the cutoff.
    pub async fn comparison(
        &self,
        tenant: &str,
        user: &UserId,
        pass: PassId,
        cutoff: Timestamp,
    ) -> Result<AssessmentComparison, DbError> {
        let mut tx = read_tx(self.pool).await?;
        let frozen = pass_in(&mut tx, tenant, user, pass).await?;
        let runs = runs_in(&mut tx, tenant, user, pass).await?;
        let rows=sqlx::query("SELECT * FROM assessment_comparison_items WHERE pass_id=? AND tenant_id=? AND payer_id=? ORDER BY comparison_key,method")
            .bind(pass.to_string()).bind(tenant).bind(&user.0).fetch_all(&mut *tx).await?;
        let mut items = Vec::with_capacity(rows.len());
        for r in rows {
            let key: String = r.try_get("comparison_key")?;
            let owner=sqlx::query("SELECT * FROM assessment_owner_decisions WHERE pass_id=? AND comparison_key=? AND tenant_id=? AND payer_id=? AND primary_item_id=? AND ?='matched' AND julianday(decided_at)<=julianday(?) ORDER BY revision DESC LIMIT 1")
                .bind(pass.to_string()).bind(&key).bind(tenant).bind(&user.0).bind(r.try_get::<Option<String>,_>("primary_item_id")?).bind(r.try_get::<String,_>("status")?).bind(ts(cutoff)).fetch_optional(&mut *tx).await?;
            let owner_decision = owner
                .map(|o| {
                    Ok::<_, DbError>(AssessmentOwnerDecision {
                        id: parse_wire(
                            &o.try_get::<String, _>("decision_id")?,
                            "assessment_owner_decisions.decision_id",
                        )?,
                        revision: o.try_get("revision")?,
                        owner: UserId(o.try_get("owner_id")?),
                        at: parse_ts(
                            &o.try_get::<String, _>("decided_at")?,
                            "assessment_owner_decisions.decided_at",
                        )?,
                        kind: parse_wire(
                            &o.try_get::<String, _>("kind")?,
                            "assessment_owner_decisions.kind",
                        )?,
                        target: o
                            .try_get::<Option<String>, _>("target")?
                            .map(|s| serde_json::from_str(&s))
                            .transpose()?,
                    })
                })
                .transpose()?;
            items.push(AssessmentComparisonItem {
                comparison_key: key,
                local_id: r.try_get("local_id")?,
                run: AssessmentRunRef {
                    pass_id: pass,
                    method: parse_wire(
                        &r.try_get::<String, _>("method")?,
                        "assessment_comparison_items.method",
                    )?,
                },
                method_version: r.try_get("method_version")?,
                source: serde_json::from_str(&r.try_get::<String, _>("source")?)?,
                primary_item_hash: r.try_get("primary_item_hash")?,
                primary_item_id: r
                    .try_get::<Option<String>, _>("primary_item_id")?
                    .map(|s| parse_wire(&s, "assessment_comparison_items.primary_item_id"))
                    .transpose()?,
                status: parse_wire(
                    &r.try_get::<String, _>("status")?,
                    "assessment_comparison_items.status",
                )?,
                duplicate_of: r.try_get("duplicate_of")?,
                verdict: r
                    .try_get::<Option<String>, _>("verdict")?
                    .map(|s| parse_wire(&s, "assessment_comparison_items.verdict"))
                    .transpose()?,
                native_verdict: r.try_get("native_verdict")?,
                proposal: r
                    .try_get::<Option<String>, _>("proposal")?
                    .map(|s| serde_json::from_str(&s))
                    .transpose()?,
                owner_decision,
            });
        }
        tx.commit().await?;
        Ok(AssessmentComparison {
            pass: frozen,
            cutoff,
            runs,
            items,
        })
    }

    /// Attach an actual diffpilot decision through an existing primary row; never trust model attribution.
    pub async fn record_owner_decision(
        &self,
        tenant: &str,
        user: &UserId,
        pass: PassId,
        key: &str,
        decision: DecisionId,
    ) -> Result<(), DbError> {
        let mut tx = write_tx(self.pool).await?;
        // Revision follows the actual supersession chain, never caller/model-supplied metadata.
        let inserted=sqlx::query("WITH RECURSIVE history(id,supersedes) AS (SELECT id,supersedes FROM comment_decisions WHERE id=? UNION SELECT d.id,d.supersedes FROM comment_decisions d JOIN history h ON d.id=h.supersedes) INSERT INTO assessment_owner_decisions(pass_id,method,comparison_key,tenant_id,payer_id,primary_item_id,decision_id,revision,owner_id,decided_at,kind,target) SELECT p.id,p.primary_method,c.comparison_key,p.tenant_id,p.payer_id,c.primary_item_id,d.id,(SELECT count(*) FROM history),d.user_id,d.decided_at,d.kind,d.target FROM assessment_passes p JOIN assessment_comparison_items c ON c.pass_id=p.id AND c.method=p.primary_method JOIN comment_decisions d ON d.item_id=c.primary_item_id AND d.tenant_id=p.tenant_id JOIN memberships m ON m.tenant_id=d.tenant_id AND m.user_id=d.user_id WHERE p.id=? AND p.tenant_id=? AND p.payer_id=? AND c.comparison_key=? AND c.status='matched' AND d.id=? AND d.via<>'carried' ON CONFLICT(pass_id,comparison_key,decision_id) DO NOTHING")
            .bind(decision.to_string()).bind(pass.to_string()).bind(tenant).bind(&user.0).bind(key).bind(decision.to_string()).execute(&mut *tx).await?;
        if inserted.rows_affected() == 0 {
            let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM assessment_owner_decisions WHERE pass_id=? AND comparison_key=? AND decision_id=? AND tenant_id=? AND payer_id=?)")
                .bind(pass.to_string()).bind(key).bind(decision.to_string()).bind(tenant).bind(&user.0).fetch_one(&mut *tx).await?;
            if !exists {
                return Err(DbError::NotFound);
            }
        }
        tx.commit().await?;
        Ok(())
    }
}

fn utc_day(now: Timestamp) -> Result<(String, Timestamp), DbError> {
    let seconds = now.as_second().div_euclid(86400) * 86400;
    let midnight = Timestamp::from_second(seconds).ok_or(DbError::Corrupt {
        column: "admission time",
    })?;
    let next = Timestamp::from_second(seconds + 86400).ok_or(DbError::Corrupt {
        column: "admission reset",
    })?;
    Ok((midnight.to_string()[..10].to_owned(), next))
}

async fn config_in(tx: &mut SqliteConnection, user: &UserId) -> Result<AssessmentConfig, DbError> {
    let row = sqlx::query("SELECT * FROM assessment_config WHERE user_id=?")
        .bind(&user.0)
        .fetch_optional(&mut *tx)
        .await?;
    row.map(|r| {
        Ok(AssessmentConfig {
            primary_method: parse_wire(
                &r.try_get::<String, _>("primary_method")?,
                "assessment_config.primary_method",
            )?,
            ab_prs: serde_json::from_str(&r.try_get::<String, _>("ab_prs")?)?,
            ab_until: r
                .try_get::<Option<String>, _>("ab_until")?
                .map(|s| parse_ts(&s, "assessment_config.ab_until"))
                .transpose()?,
        })
    })
    .unwrap_or_else(|| Ok(AssessmentConfig::default()))
}

fn pass_from_row(row: &SqliteRow) -> Result<AssessmentPass, DbError> {
    Ok(AssessmentPass {
        id: parse_wire(&row.try_get::<String, _>("id")?, "assessment_passes.id")?,
        payer: UserId(row.try_get("payer_id")?),
        input: serde_json::from_str(&row.try_get::<String, _>("input")?)?,
        config: serde_json::from_str(&row.try_get::<String, _>("config")?)?,
        paired: row.try_get("paired")?,
        day_utc: row.try_get("day_utc")?,
        admitted_at: parse_ts(
            &row.try_get::<String, _>("admitted_at")?,
            "assessment_passes.admitted_at",
        )?,
    })
}

async fn pass_in(
    tx: &mut SqliteConnection,
    tenant: &str,
    user: &UserId,
    pass: PassId,
) -> Result<AssessmentPass, DbError> {
    let row =
        sqlx::query("SELECT * FROM assessment_passes WHERE id=? AND tenant_id=? AND payer_id=?")
            .bind(pass.to_string())
            .bind(tenant)
            .bind(&user.0)
            .fetch_one(&mut *tx)
            .await?;
    pass_from_row(&row)
}

async fn runs_in(
    tx: &mut SqliteConnection,
    tenant: &str,
    user: &UserId,
    pass: PassId,
) -> Result<Vec<AssessmentMethodRun>, DbError> {
    let rows=sqlx::query("SELECT * FROM assessment_method_runs WHERE pass_id=? AND tenant_id=? AND payer_id=? ORDER BY method").bind(pass.to_string()).bind(tenant).bind(&user.0).fetch_all(&mut *tx).await?;
    rows.into_iter()
        .map(|r| {
            Ok(AssessmentMethodRun {
                reference: AssessmentRunRef {
                    pass_id: pass,
                    method: parse_wire(
                        &r.try_get::<String, _>("method")?,
                        "assessment_method_runs.method",
                    )?,
                },
                job_id: parse_wire(
                    &r.try_get::<String, _>("job_id")?,
                    "assessment_method_runs.job_id",
                )?,
                version: r.try_get("version")?,
                status: parse_wire(
                    &r.try_get::<String, _>("status")?,
                    "assessment_method_runs.status",
                )?,
                attempt: r.try_get("attempt")?,
                accepted_attempt: r.try_get("accepted_attempt")?,
                usage: Usage {
                    input_tokens: r.try_get::<i64, _>("input_tokens")?.try_into().map_err(
                        |_| DbError::Corrupt {
                            column: "input_tokens",
                        },
                    )?,
                    cached_input_tokens: r
                        .try_get::<i64, _>("cached_input_tokens")?
                        .try_into()
                        .map_err(|_| DbError::Corrupt {
                            column: "cached_input_tokens",
                        })?,
                    output_tokens: r.try_get::<i64, _>("output_tokens")?.try_into().map_err(
                        |_| DbError::Corrupt {
                            column: "output_tokens",
                        },
                    )?,
                    requests: r
                        .try_get::<Option<i64>, _>("requests")?
                        .map(|n| {
                            n.try_into()
                                .map_err(|_| DbError::Corrupt { column: "requests" })
                        })
                        .transpose()?,
                },
                duration_ms: r
                    .try_get::<i64, _>("duration_ms")?
                    .try_into()
                    .map_err(|_| DbError::Corrupt {
                        column: "duration_ms",
                    })?,
                failure: r
                    .try_get::<Option<String>, _>("failure")?
                    .map(|s| parse_wire(&s, "assessment_method_runs.failure"))
                    .transpose()?,
            })
        })
        .collect()
}

fn count(value: u64) -> Result<i64, DbError> {
    value.try_into().map_err(|_| DbError::Conflict)
}

fn validate_jobs(
    user: &UserId,
    request: &NewAssessmentPass,
    config: &AssessmentConfig,
    paired: bool,
) -> Result<(), DbError> {
    if request.jobs.len() != if paired { 2 } else { 1 } {
        return Err(DbError::Conflict);
    }
    for (index, job) in request.jobs.iter().enumerate() {
        let expected = if index == 0 {
            config.primary_method
        } else {
            config.primary_method.other()
        };
        let spec = &job.spec;
        let target = match &spec.target {
            JobTarget::PullRequestComments(target) => target,
            _ => return Err(DbError::Conflict),
        };
        if spec.kind != contracts::agent::JobKind::VerifyComments
            || spec.payer != *user
            || spec.model != request.input.model
            || spec.assessment
                != Some(AssessmentRunRef {
                    pass_id: request.id,
                    method: expected,
                })
            || target.pass_id != request.id
            || target.pull_request_id != request.input.pull_request_id
            || target.repo.head_sha != request.input.head
            || target.repo.base_sha != request.input.base
            || job.version.is_empty()
        {
            return Err(DbError::Conflict);
        }
    }
    Ok(())
}

async fn meter_in(
    tx: &mut SqliteConnection,
    current: &AssessmentMethodRun,
    attempt: u32,
    usage: Usage,
    duration_ms: u64,
) -> Result<bool, DbError> {
    let run = &current.reference;
    let recorded:Option<(i64,i64,i64,Option<i64>,i64)>=sqlx::query_as("SELECT input_tokens,cached_input_tokens,output_tokens,requests,duration_ms FROM assessment_attempt_usage WHERE pass_id=? AND method=? AND attempt=?")
        .bind(run.pass_id.to_string()).bind(wire_name(run.method)?).bind(attempt).fetch_optional(&mut *tx).await?;
    let values = (
        count(usage.input_tokens)?,
        count(usage.cached_input_tokens)?,
        count(usage.output_tokens)?,
        usage.requests.map(count).transpose()?,
        count(duration_ms)?,
    );
    if let Some(recorded) = recorded {
        return if recorded == values {
            Ok(false)
        } else {
            Err(DbError::Conflict)
        };
    }
    let total = current
        .usage
        .checked_add(usage)
        .map_err(|_| DbError::Conflict)?;
    let duration = current
        .duration_ms
        .checked_add(duration_ms)
        .ok_or(DbError::Conflict)?;
    sqlx::query("INSERT INTO assessment_attempt_usage(pass_id,method,tenant_id,payer_id,attempt,input_tokens,cached_input_tokens,output_tokens,requests,duration_ms) SELECT pass_id,method,tenant_id,payer_id,?,?,?,?,?,? FROM assessment_method_runs WHERE pass_id=? AND method=?")
        .bind(attempt).bind(values.0).bind(values.1).bind(values.2).bind(values.3).bind(values.4).bind(run.pass_id.to_string()).bind(wire_name(run.method)?).execute(&mut *tx).await?;
    sqlx::query("UPDATE assessment_method_runs SET input_tokens=?,cached_input_tokens=?,output_tokens=?,requests=?,duration_ms=? WHERE pass_id=? AND method=?")
        .bind(count(total.input_tokens)?).bind(count(total.cached_input_tokens)?).bind(count(total.output_tokens)?).bind(total.requests.map(count).transpose()?).bind(count(duration)?)
        .bind(run.pass_id.to_string()).bind(wire_name(run.method)?).execute(&mut *tx).await?;
    Ok(true)
}
