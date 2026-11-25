//! Atomic admission and fenced method results; all model rows are normalized comparison data.

pub use super::assessment_primary::{PrimaryCarry, PrimaryPoint};

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
    /// False when this attempt's metering is unavailable; never claim a measured zero.
    pub usage_known: bool,
    /// Commit actually reported, distinct from the frozen checkout.
    pub reported_head: Option<contracts::ids::CommitSha>,
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
        self.save_config(tenant, user, config, None).await
    }

    /// Effective configuration and its opaque content revision from one snapshot.
    pub async fn config_snapshot(
        &self,
        user: &UserId,
    ) -> Result<(AssessmentConfig, String), DbError> {
        let config = self.config(user).await?;
        let revision = serde_json::to_string(&config)?;
        Ok((config, revision))
    }

    /// Compare and save under the same write lock, preventing lost updates between tabs.
    pub async fn set_config_if_current(
        &self,
        tenant: &str,
        user: &UserId,
        config: &AssessmentConfig,
        revision: &str,
    ) -> Result<(), DbError> {
        self.save_config(tenant, user, config, Some(revision)).await
    }

    async fn save_config(
        &self,
        tenant: &str,
        user: &UserId,
        config: &AssessmentConfig,
        revision: Option<&str>,
    ) -> Result<(), DbError> {
        let mut tx = write_tx(self.pool).await?;
        if let Some(revision) = revision {
            let current = config_in(&mut tx, user).await?;
            if serde_json::to_string(&current)? != revision {
                return Err(DbError::Conflict);
            }
        }
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
        let mut tx = read_tx(self.pool).await?;
        let count: Option<u32> = sqlx::query_scalar(
            "SELECT passes FROM assessment_daily_usage WHERE user_id=? AND day_utc=?",
        )
        .bind(&user.0)
        .bind(day)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
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
        self.admit_inner(tenant, user, request, None, now).await
    }

    /// Admit the visible review pass, logical quota slot and all method jobs atomically.
    pub async fn admit_review(
        &self,
        tenant: &str,
        user: &UserId,
        request: &NewAssessmentPass,
        review: &super::review_loop::NewPass,
        now: Timestamp,
    ) -> Result<AssessmentPass, AssessmentError> {
        self.admit_inner(tenant, user, request, Some(review), now)
            .await
    }

    async fn admit_inner(
        &self,
        tenant: &str,
        user: &UserId,
        request: &NewAssessmentPass,
        review: Option<&super::review_loop::NewPass>,
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
        let input = &request.input;
        if let Some(review) = review {
            if review.pull_request_id != input.pull_request_id
                || review.head != input.head
                || review.user_id.as_deref() != Some(user.0.as_str())
                || review.question.is_some()
            {
                return Err(DbError::Conflict.into());
            }
            let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM comment_passes WHERE pull_request_id=? AND state IN ('queued','running'))")
                .bind(input.pull_request_id).fetch_one(&mut *tx).await.map_err(DbError::from)?;
            if busy {
                return Err(DbError::Conflict.into());
            }
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
        if let Some(review) = review {
            sqlx::query("INSERT INTO comment_passes(id,tenant_id,pull_request_id,user_id,cause,head,items,state,created_at,sent,job_id) VALUES(?,?,?,?,?,?,?,'running',?,?,?)")
                .bind(request.id.to_string()).bind(tenant).bind(input.pull_request_id).bind(&user.0)
                .bind(wire_name(review.cause)?).bind(&input.head.0).bind(review.sent.len() as i64).bind(ts(now))
                .bind(serde_json::to_string(&review.sent).map_err(DbError::from)?).bind(request.jobs[0].spec.job_id.to_string())
                .execute(&mut *tx).await.map_err(DbError::from)?;
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
        self.finish_with_primary(tenant, user, run, attempt, result, &[])
            .await
    }

    /// Whether the pass projected at least one primary assessment card.
    pub async fn primary_made_progress(&self, pass: &AssessmentPass) -> Result<bool, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM assessment_comparison_items c
             JOIN comment_assessments a ON a.item_id=c.primary_item_id AND a.pass_id=c.pass_id
             WHERE c.pass_id=? AND c.method=? AND c.payer_id=?)",
        )
        .bind(pass.id.to_string())
        .bind(wire_name(pass.config.primary_method)?)
        .bind(&pass.payer.0)
        .fetch_one(self.pool)
        .await?)
    }

    /// Atomically accept a method and project only its frozen primary's validated cards.
    pub async fn finish_with_primary(
        &self,
        tenant: &str,
        user: &UserId,
        run: &AssessmentRunRef,
        attempt: u32,
        result: &MethodResult,
        primary: &[PrimaryPoint],
    ) -> Result<bool, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let pass = pass_in(&mut tx, tenant, user, run.pass_id).await?;
        if !primary.is_empty()
            && (run.method != pass.config.primary_method
                || result.status != AssessmentRunStatus::Succeeded)
        {
            return Err(DbError::Conflict);
        }

        let current = runs_in(&mut tx, tenant, user, run.pass_id)
            .await?
            .into_iter()
            .find(|r| r.reference == *run)
            .ok_or(DbError::NotFound)?;
        if attempt == 0
            && current.attempt == 0
            && result.status == AssessmentRunStatus::Cancelled
            && result.items.is_empty()
            && primary.is_empty()
        {
            let cancelled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE id=? AND attempt=0 AND state='cancelled' AND json_extract(result,'$.attempt')=0)")
                .bind(current.job_id.to_string()).fetch_one(&mut *tx).await?;
            if !cancelled {
                return Err(DbError::Conflict);
            }
            let changed=sqlx::query("UPDATE assessment_method_runs SET status='cancelled' WHERE job_id=? AND status='queued' AND attempt=0")
                .bind(current.job_id.to_string()).execute(&mut *tx).await?.rows_affected()==1;
            tx.commit().await?;
            return Ok(changed);
        }
        // Complete retains the claim token; expiry and release clear it, including on exhausted jobs.
        let live_attempt: bool = sqlx::query_scalar(
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
        let terminal_status = !live_attempt
            && result.items.is_empty()
            && matches!(
                result.status,
                AssessmentRunStatus::Cancelled | AssessmentRunStatus::Failed
            )
            && sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM jobs WHERE id=? AND attempt=? AND state=?
                 AND json_extract(result,'$.attempt')=attempt
                 AND json_extract(result,'$.outcome.outcome')=state)",
            )
            .bind(current.job_id.to_string())
            .bind(attempt)
            .bind(wire_name(result.status)?)
            .fetch_one(&mut *tx)
            .await?;
        if attempt == 0 || current.attempt != attempt || !(live_attempt || terminal_status) {
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
        meter_in(
            &mut tx,
            &current,
            attempt,
            result.usage,
            result.duration_ms,
            result.usage_known,
        )
        .await?;
        let accepted = (result.status == AssessmentRunStatus::Succeeded).then_some(attempt);
        sqlx::query("UPDATE assessment_method_runs SET status=?,accepted_attempt=?,failure=?,reported_head=? WHERE pass_id=? AND method=? AND tenant_id=? AND payer_id=? AND attempt=? AND status='running'")
            .bind(wire_name(result.status)?).bind(accepted).bind(result.failure.map(wire_name).transpose()?)
            .bind(result.reported_head.as_ref().map(|head| &head.0)).bind(run.pass_id.to_string()).bind(wire_name(run.method)?).bind(tenant).bind(&user.0).bind(attempt).execute(&mut *tx).await?;
        let mut items = result.items.clone();
        super::assessment_primary::apply(&mut tx, tenant, &pass, primary, &mut items).await?;
        if run.method != pass.config.primary_method {
            for item in &mut items {
                let aligned: Option<(Option<String>,Option<String>,String)> = sqlx::query_as("SELECT primary_item_id,primary_item_hash,source FROM assessment_comparison_items WHERE pass_id=? AND method=? AND comparison_key=? AND status='matched'")
                    .bind(run.pass_id.to_string()).bind(wire_name(pass.config.primary_method)?).bind(&item.comparison_key).fetch_optional(&mut *tx).await?;
                if let Some((id, hash, source)) = aligned.filter(|(id, hash, source)| {
                    serde_json::from_str::<AssessmentSource>(source)
                        .ok()
                        .is_some_and(|source| {
                            sources_align(
                                &source,
                                &item.source,
                                id.as_ref().is_some_and(|id| {
                                    item.primary_item_id
                                        .is_some_and(|other| other.to_string() == *id)
                                }) && hash == &item.primary_item_hash,
                            )
                        })
                }) {
                    let _ = source;
                    item.primary_item_id = id
                        .map(|id| parse_wire(&id, "primary_item_id"))
                        .transpose()?;
                    item.primary_item_hash = hash;
                } else if item.status == ComparisonItemStatus::Matched {
                    item.status = ComparisonItemStatus::Unmatched;
                }
            }
        }
        // Canonical rows precede direct duplicate links; no secondary row allocates a comment item.
        for item in items
            .iter()
            .filter(|i| i.duplicate_of.is_none())
            .chain(items.iter().filter(|i| i.duplicate_of.is_some()))
        {
            sqlx::query("INSERT INTO assessment_comparison_items(pass_id,method,tenant_id,payer_id,pull_request_id,comparison_key,local_id,method_version,source,primary_item_id,primary_item_hash,status,duplicate_of,verdict,native_verdict,proposal,recorded_decision) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
                .bind(run.pass_id.to_string()).bind(wire_name(run.method)?).bind(tenant).bind(&user.0).bind(pass.input.pull_request_id)
                .bind(&item.comparison_key).bind(&item.local_id).bind(&item.method_version).bind(serde_json::to_string(&item.source)?)
                .bind(item.primary_item_id.map(|id|id.to_string())).bind(&item.primary_item_hash).bind(wire_name(item.status)?).bind(&item.duplicate_of)
                .bind(item.verdict.map(wire_name).transpose()?).bind(&item.native_verdict).bind(item.proposal.as_ref().map(serde_json::to_string).transpose()?).bind(item.recorded_decision.as_ref().map(serde_json::to_string).transpose()?)
                .execute(&mut *tx).await?;
        }
        if run.method == pass.config.primary_method
            && result.status == AssessmentRunStatus::Succeeded
        {
            let owners:Vec<String>=sqlx::query_scalar("SELECT DISTINCT d.id FROM comment_decisions d JOIN assessment_comparison_items c ON c.primary_item_id=d.item_id AND c.tenant_id=d.tenant_id JOIN assessment_passes p ON p.id=c.pass_id AND p.primary_method=c.method WHERE p.id=? AND d.tenant_id=? AND c.status='matched' AND d.head=p.head AND d.body_hash=c.primary_item_hash AND (json_extract(c.source,'$.kind')='inline' OR d.source_version=json_extract(c.source,'$.version'))")
                .bind(pass.id.to_string()).bind(tenant).fetch_all(&mut *tx).await?;
            for owner in owners {
                super::assessment_primary::capture_owner(
                    &mut tx,
                    tenant,
                    parse_wire(&owner, "decision_id")?,
                )
                .await?;
            }
        }
        if run.method == pass.config.primary_method
            && result.status == AssessmentRunStatus::Succeeded
            && result
                .reported_head
                .as_ref()
                .is_none_or(|head| head == &pass.input.head)
        {
            for row in &items {
                if !matches!(
                    row.status,
                    ComparisonItemStatus::MissingRow
                        | ComparisonItemStatus::HeadMismatch
                        | ComparisonItemStatus::Context
                ) && let AssessmentSource::Body { url, version, .. } = &row.source
                {
                    sqlx::query("UPDATE comment_sources SET read_at=coalesce(read_at,?) WHERE tenant_id=? AND pull_request_id=? AND url=? AND body_hash=?")
                        .bind(ts(Timestamp::now())).bind(tenant).bind(pass.input.pull_request_id).bind(url).bind(version).execute(&mut *tx).await?;
                    if run.method == AssessmentMethod::ClaudiusHost {
                        sqlx::query("UPDATE comment_items SET gone_at=?,revision=revision+1 WHERE tenant_id=? AND pull_request_id=? AND url=? AND kind IN ('review_body','pr_comment') AND source_id IS NULL AND gone_at IS NULL AND state IN ('assessing','undecided','asked_back','reopened') AND NOT EXISTS(SELECT 1 FROM assessment_comparison_items c WHERE c.pass_id=? AND c.method=? AND c.primary_item_id=comment_items.id AND c.status NOT IN ('missing_row','context','head_mismatch'))")
                            .bind(ts(Timestamp::now())).bind(tenant).bind(pass.input.pull_request_id).bind(url).bind(pass.id.to_string()).bind(wire_name(run.method)?).execute(&mut *tx).await?;
                    }
                }
            }
            for row in &items {
                if row.status == ComparisonItemStatus::Unmatched
                    && let AssessmentSource::Body { url, quote, .. } = &row.source
                    && quote.is_empty()
                {
                    sqlx::query("UPDATE comment_items SET gone_at=?,revision=revision+1 WHERE tenant_id=? AND pull_request_id=? AND url=? AND kind IN ('review_body','pr_comment') AND source_id IS NULL AND NOT EXISTS(SELECT 1 FROM comment_assessments a WHERE a.item_id=comment_items.id)")
                        .bind(ts(Timestamp::now())).bind(tenant).bind(pass.input.pull_request_id).bind(url).execute(&mut *tx).await?;
                }
            }
            // Source-bound points survive GitHub reads; only the accepted primary supersedes old bot rounds.
            sqlx::query("UPDATE comment_items SET gone_at=?,revision=revision+1 WHERE tenant_id=? AND pull_request_id=? AND gone_at IS NULL AND source_id IN (SELECT s.id FROM comment_sources s WHERE s.role='context' AND s.source_kind='review_body' AND EXISTS(SELECT 1 FROM comment_sources n WHERE n.pull_request_id=s.pull_request_id AND n.author=s.author AND n.role='source' AND n.source_kind='review_body' AND n.read_at IS NOT NULL))")
                .bind(ts(Timestamp::now())).bind(tenant).bind(pass.input.pull_request_id).execute(&mut *tx).await?;
        }
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
        let added = meter_in(&mut tx, &current, attempt, usage, duration_ms, true).await?;
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

    /// Elapsed wall time of the queue attempt, excluding time spent waiting for admission.
    pub async fn attempt_duration(
        &self,
        job: contracts::ids::JobId,
        attempt: u32,
    ) -> Result<u64, DbError> {
        let mut tx = read_tx(self.pool).await?;
        let duration: i64 = sqlx::query_scalar("SELECT CAST(max(0,round((julianday(finished_at)-julianday(claimed_at))*86400000)) AS INTEGER) FROM jobs WHERE id=? AND attempt=? AND claimed_at IS NOT NULL AND finished_at IS NOT NULL")
            .bind(job.to_string()).bind(attempt).fetch_optional(&mut *tx).await?.unwrap_or(0);
        tx.commit().await?;
        Ok(duration as u64)
    }

    /// Read the immutable pass under its tenant and payer boundary.
    pub async fn pass(
        &self,
        tenant: &str,
        user: &UserId,
        id: PassId,
    ) -> Result<AssessmentPass, DbError> {
        let mut tx = read_tx(self.pool).await?;
        let pass = pass_in(&mut tx, tenant, user, id).await?;
        tx.commit().await?;
        Ok(pass)
    }

    /// Latest admitted pass of this PR for this payer only.
    pub async fn latest(
        &self,
        tenant: &str,
        user: &UserId,
        pr: i64,
    ) -> Result<Option<AssessmentPass>, DbError> {
        let mut tx = read_tx(self.pool).await?;
        let row = sqlx::query("SELECT * FROM assessment_passes WHERE tenant_id=? AND payer_id=? AND pull_request_id=? ORDER BY admitted_at DESC,rowid DESC LIMIT 1")
            .bind(tenant).bind(&user.0).bind(pr).fetch_optional(&mut *tx).await?;
        let pass = row.as_ref().map(pass_from_row).transpose()?;
        tx.commit().await?;
        Ok(pass)
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
                recorded_decision: r
                    .try_get::<Option<String>, _>("recorded_decision")?
                    .map(|raw| serde_json::from_str(&raw))
                    .transpose()?,
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
            summary: Default::default(),
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
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(DbError::NotFound)?;
    pass_from_row(&row)
}

async fn runs_in(
    tx: &mut SqliteConnection,
    tenant: &str,
    user: &UserId,
    pass: PassId,
) -> Result<Vec<AssessmentMethodRun>, DbError> {
    let rows=sqlx::query("SELECT r.*,((SELECT count(*) FROM assessment_attempt_usage u WHERE u.pass_id=r.pass_id AND u.method=r.method)=r.attempt AND NOT EXISTS(SELECT 1 FROM assessment_attempt_usage u WHERE u.pass_id=r.pass_id AND u.method=r.method AND u.known=0)) AS usage_complete FROM assessment_method_runs r WHERE pass_id=? AND tenant_id=? AND payer_id=? ORDER BY method").bind(pass.to_string()).bind(tenant).bind(&user.0).fetch_all(&mut *tx).await?;
    rows.into_iter()
        .map(|r| {
            Ok(AssessmentMethodRun {
                usage_complete: r.try_get("usage_complete")?,
                reported_head: r
                    .try_get::<Option<String>, _>("reported_head")?
                    .map(contracts::ids::CommitSha),
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
    known: bool,
) -> Result<bool, DbError> {
    if !known && usage != Usage::default() {
        return Err(DbError::Conflict);
    }
    let run = &current.reference;
    let recorded:Option<(i64,i64,i64,Option<i64>,i64,bool)>=sqlx::query_as("SELECT input_tokens,cached_input_tokens,output_tokens,requests,duration_ms,known FROM assessment_attempt_usage WHERE pass_id=? AND method=? AND attempt=?")
        .bind(run.pass_id.to_string()).bind(wire_name(run.method)?).bind(attempt).fetch_optional(&mut *tx).await?;
    let values = (
        count(usage.input_tokens)?,
        count(usage.cached_input_tokens)?,
        count(usage.output_tokens)?,
        usage.requests.map(count).transpose()?,
        count(duration_ms)?,
        known,
    );
    let extra_duration = if let Some(previous) = recorded {
        if previous == values {
            return Ok(false);
        }
        if previous.5 || !known || previous.4 != values.4 {
            return Err(DbError::Conflict);
        }
        sqlx::query("UPDATE assessment_attempt_usage SET input_tokens=?,cached_input_tokens=?,output_tokens=?,requests=?,known=1 WHERE pass_id=? AND method=? AND attempt=? AND known=0")
            .bind(values.0).bind(values.1).bind(values.2).bind(values.3).bind(run.pass_id.to_string()).bind(wire_name(run.method)?).bind(attempt).execute(&mut *tx).await?;
        0
    } else {
        sqlx::query("INSERT INTO assessment_attempt_usage(pass_id,method,tenant_id,payer_id,attempt,input_tokens,cached_input_tokens,output_tokens,requests,duration_ms,known,recorded_at) SELECT pass_id,method,tenant_id,payer_id,?,?,?,?,?,?,?,COALESCE((SELECT finished_at FROM jobs WHERE id=r.job_id AND attempt=?),?) FROM assessment_method_runs r WHERE pass_id=? AND method=?")
            .bind(attempt).bind(values.0).bind(values.1).bind(values.2).bind(values.3).bind(values.4).bind(known).bind(attempt).bind(ts(Timestamp::now())).bind(run.pass_id.to_string()).bind(wire_name(run.method)?).execute(&mut *tx).await?;
        duration_ms
    };
    let total = if known {
        current
            .usage
            .checked_add(usage)
            .map_err(|_| DbError::Conflict)?
    } else {
        current.usage
    };
    let duration = current
        .duration_ms
        .checked_add(extra_duration)
        .ok_or(DbError::Conflict)?;
    sqlx::query("UPDATE assessment_method_runs SET input_tokens=?,cached_input_tokens=?,output_tokens=?,requests=?,duration_ms=? WHERE pass_id=? AND method=?")
        .bind(count(total.input_tokens)?).bind(count(total.cached_input_tokens)?).bind(count(total.output_tokens)?).bind(total.requests.map(count).transpose()?).bind(count(duration)?)
        .bind(run.pass_id.to_string()).bind(wire_name(run.method)?).execute(&mut *tx).await?;
    Ok(true)
}

/// Synthetic admission state for downstream tests; never built into release binaries.
#[cfg(feature = "test-util")]
pub mod testing {
    use super::*;
    /// Seed a UTC day's used slots without running any provider.
    pub async fn usage(db: &Db, user: &UserId, now: Timestamp, passes: u32) -> Result<(), DbError> {
        let (day, _) = utc_day(now)?;
        let mut tx = write_tx(&db.pool).await?;
        sqlx::query("INSERT INTO assessment_daily_usage(user_id,day_utc,passes) VALUES(?,?,?) ON CONFLICT(user_id,day_utc) DO UPDATE SET passes=excluded.passes")
            .bind(&user.0).bind(day).bind(passes).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}
