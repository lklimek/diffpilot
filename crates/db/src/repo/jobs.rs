//! Repository for `jobs` and `job_events`: the queue behind `QueueExecutor` and the runner protocol.
//! Frozen signatures (cross-WP).

use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use contracts::agent::JobKind;
use contracts::ids::{AnalysisId, FlowId, JobId, QuestionId};
use contracts::job::access::AgentAccess;
use contracts::job::event::JobEvent;
use contracts::job::result::{JobFailureKind, JobOutcome, JobResult, LlmCallStatus};
use contracts::job::spec::{JobPriority, JobSpec};
use contracts::job::state::{JobState, JobTransition};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection, SqlitePool};

use crate::error::DbError;
use crate::repo::settings::CallFailure;

/// Access to the job queue.
#[derive(Debug)]
pub struct Jobs<'a> {
    pub(crate) pool: &'a SqlitePool,
}

/// Repair runs allowed after a first run whose output verification rejected (brief §5 Pipeline 6).
pub const MAX_REPAIR_RUNS: u32 = 2;

/// An installation token of a finished attempt still to be revoked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenToRevoke {
    /// The job.
    pub job: JobId,
    /// The attempt the token was minted for.
    pub attempt: u32,
    /// AES-GCM ciphertext of the token.
    pub ciphertext: Vec<u8>,
    /// Token expiry; an expired token needs no GitHub call, only the confirmation.
    pub expires_at: DateTime<Utc>,
}

/// The pipeline stage a job serves: the durable link from analysis progress to its job (restart recovery).
/// At most one job per (analysis, stage) is live or succeeded, so a repeated submit is a `Conflict`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum JobStage {
    /// A `plan` job; huge pull requests are planned in several directory batches, numbered from 1.
    Plan {
        /// Batch number.
        batch: u32,
    },
    /// The `pr_context` job.
    PrContext,
    /// The `analyse_flow` job of a flow.
    Flow(FlowId),
    /// The `ask` job of a question.
    Ask(QuestionId),
}

impl JobStage {
    /// The stored key: `plan:<batch>`, `pr_context`, `flow:<id>`, `ask:<question id>`.
    pub fn key(&self) -> String {
        match self {
            Self::Plan { batch } => format!("plan:{batch}"),
            Self::PrContext => "pr_context".to_owned(),
            Self::Flow(flow) => format!("flow:{flow}"),
            Self::Ask(question) => format!("ask:{question}"),
        }
    }

    /// Parses a stored key; `None` for anything [`JobStage::key`] does not produce.
    pub fn parse(key: &str) -> Option<Self> {
        if key == "pr_context" {
            return Some(Self::PrContext);
        }
        let (prefix, rest) = key.split_once(':')?;
        match prefix {
            "plan" => rest.parse().ok().map(|batch| Self::Plan { batch }),
            "flow" if !rest.is_empty() => Some(Self::Flow(FlowId(rest.to_owned()))),
            "ask" => serde_json::from_value(serde_json::Value::String(rest.to_owned()))
                .ok()
                .map(Self::Ask),
            _ => None,
        }
    }

    /// The job kind of the stage.
    pub fn kind(&self) -> JobKind {
        match self {
            Self::Plan { .. } => JobKind::Plan,
            Self::PrContext => JobKind::PrContext,
            Self::Flow(_) => JobKind::AnalyseFlow,
            Self::Ask(_) => JobKind::Ask,
        }
    }
}

/// A job to enqueue; the tenant is the analysis's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewJob {
    /// The spec; `access` must be empty (credentials are added at claim).
    pub spec: JobSpec,
    /// Claim priority.
    pub priority: JobPriority,
    /// The pipeline stage (its kind equals `spec.kind`).
    pub stage: JobStage,
    /// 0 for the first run, 1..=[`MAX_REPAIR_RUNS`] for repair runs after a rejected output.
    pub generation: u32,
}

/// A stored job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobRecord {
    /// Job id.
    pub id: JobId,
    /// Analysis.
    pub analysis_id: AnalysisId,
    /// Kind.
    pub kind: JobKind,
    /// Claim priority.
    pub priority: JobPriority,
    /// State.
    pub state: JobState,
    /// Current attempt (0 before the first claim).
    pub attempt: u32,
    /// Attempts allowed before an expired lease fails the job.
    pub max_attempts: u32,
    /// The stored spec (no credentials).
    pub spec: JobSpec,
    /// Cancellation was requested.
    pub cancel_requested: bool,
    /// End of the current lease.
    pub lease_expires_at: Option<DateTime<Utc>>,
    /// When a paused job is queued again.
    pub resumes_at: Option<DateTime<Utc>>,
    /// The output posted before archival for the current attempt (provisional; the result is authoritative).
    pub ready_output: Option<Value>,
    /// The final result.
    pub result: Option<JobResult>,
    /// The pipeline stage.
    pub stage: JobStage,
    /// 0 for the first run, else the repair run number.
    pub generation: u32,
    /// The analysis's round when it was enqueued; an explicit Retry starts a new round.
    pub round: u32,
    /// The last claim gave the job a GitHub installation token (its agent had the GitHub tool).
    pub github_access_granted: bool,
    /// When it was enqueued.
    pub created_at: DateTime<Utc>,
    /// When it reached a terminal state.
    pub finished_at: Option<DateTime<Utc>>,
}

/// What a runner claim needs from the queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimParams<'p> {
    /// Usable credential ids the runner holds.
    pub credential_ids: &'p [String],
    /// Hash of the new job token.
    pub job_token_hash: &'p [u8],
    /// End of the lease.
    pub lease_until: DateTime<Utc>,
    /// Now.
    pub now: DateTime<Utc>,
}

/// Reply to an accepted event batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventsRecorded {
    /// The job should be cancelled.
    pub cancel_requested: bool,
}

/// A job whose lease expired, and the state it moved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpiredLease {
    /// The job.
    pub job: JobId,
    /// `queued` (attempts left), `failed` (exhausted) or `cancelled` (cancel had been requested).
    pub state: JobState,
}

/// Outcome of recording the output posted before archival.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputRecorded {
    /// Stored for the attempt; the server may publish it unless `cancel_requested`.
    Stored {
        /// The job should be cancelled; the output is not published.
        cancel_requested: bool,
    },
    /// The same output was already stored for this attempt (a retried request); nothing changed.
    AlreadyStored,
}

/// Whether a result changed the job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completion {
    /// The result was stored and the job moved to `state`.
    Applied {
        /// The new state.
        state: JobState,
    },
    /// The same attempt's result was already stored; nothing changed.
    AlreadyRecorded,
}

/// The job-token hash with the claim it was issued for, read in one statement so they belong together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobAuthorization {
    /// Hash of the current claim's job token; none before a claim and after a lease expiry.
    pub token_hash: Option<Vec<u8>>,
    /// The attempt the token was issued for.
    pub attempt: u32,
    /// State.
    pub state: JobState,
    /// End of the current lease.
    pub lease_expires_at: Option<DateTime<Utc>>,
    /// Cancellation was requested.
    pub cancel_requested: bool,
}

/// A job with the events of its current attempt, read in one transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobSnapshot {
    /// The job.
    pub job: JobRecord,
    /// Events of `job.attempt`, in order.
    pub events: Vec<JobEvent>,
    /// Until when the job's LLM credential is paused by a usage limit (any job's), if it is or was.
    pub credential_paused_until: Option<DateTime<Utc>>,
}

/// Events kept per job: `job_events` is a bounded activity log.
const EVENTS_KEPT: i64 = 500;

/// How long a usage-limited job waits when the provider gave no reset time (ADR-003).
pub const PAUSE_WITHOUT_RESET: TimeDelta = TimeDelta::minutes(30);

/// Message of the server-built result of a job whose leases expired on every attempt.
/// Result text of a job that failed because no claim could prepare its context.
const CONTEXT_UNPREPARED_MESSAGE: &str =
    "the job's session or context pack could not be prepared at any claim";
const RUNNER_LOST_MESSAGE: &str =
    "The runner stopped reporting progress on every attempt, so the job was given up.";

/// Fixed-width RFC 3339 UTC text, so stored timestamps compare correctly as text.
pub(crate) fn ts(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Parses a stored timestamp.
pub(crate) fn parse_ts(value: &str, column: &'static str) -> Result<DateTime<Utc>, DbError> {
    DateTime::parse_from_rfc3339(value)
        .map(|at| at.with_timezone(&Utc))
        .map_err(|_| DbError::Corrupt { column })
}

/// The serde name of a unit enum variant or newtype id, as stored in a text column.
pub(crate) fn wire_name<T: Serialize>(value: T) -> Result<String, DbError> {
    match serde_json::to_value(value)? {
        Value::String(name) => Ok(name),
        _ => Err(DbError::Corrupt {
            column: "wire name",
        }),
    }
}

/// Parses a text column holding a serde name or id.
pub(crate) fn parse_wire<T: DeserializeOwned>(
    value: &str,
    column: &'static str,
) -> Result<T, DbError> {
    serde_json::from_value(Value::String(value.to_owned())).map_err(|_| DbError::Corrupt { column })
}

fn priority_value(priority: JobPriority) -> i64 {
    match priority {
        JobPriority::Interactive => 0,
        JobPriority::Batch => 1,
    }
}

/// A result the server records itself (cancel or failure without a runner result).
pub(crate) fn server_result(job: JobId, attempt: u32, outcome: JobOutcome) -> JobResult {
    JobResult {
        job_id: job,
        attempt,
        outcome,
        session: None,
        usage: None,
        skills_commit: None,
        llm_call: None,
    }
}

macro_rules! job_columns {
    () => {
        "id, analysis_id, kind, priority, state, attempt, max_attempts, spec, cancel_requested, lease_expires_at,
         resumes_at, ready_output, result, stage, generation, round, github_access_granted, created_at, finished_at"
    };
}

fn optional_ts(row: &SqliteRow, column: &'static str) -> Result<Option<DateTime<Utc>>, DbError> {
    row.try_get::<Option<String>, _>(column)?
        .map(|at| parse_ts(&at, column))
        .transpose()
}

fn optional_json<T: DeserializeOwned>(
    row: &SqliteRow,
    column: &'static str,
) -> Result<Option<T>, DbError> {
    row.try_get::<Option<String>, _>(column)?
        .map(|text| serde_json::from_str(&text))
        .transpose()
        .map_err(DbError::from)
}

fn job_from_row(row: &SqliteRow) -> Result<JobRecord, DbError> {
    let id: String = row.try_get("id")?;
    let analysis: String = row.try_get("analysis_id")?;
    let kind: String = row.try_get("kind")?;
    let state: String = row.try_get("state")?;
    let spec: String = row.try_get("spec")?;
    let stage: String = row.try_get("stage")?;
    let created_at: String = row.try_get("created_at")?;
    Ok(JobRecord {
        id: parse_wire(&id, "jobs.id")?,
        analysis_id: parse_wire(&analysis, "jobs.analysis_id")?,
        kind: parse_wire(&kind, "jobs.kind")?,
        priority: match row.try_get::<i64, _>("priority")? {
            0 => JobPriority::Interactive,
            1 => JobPriority::Batch,
            _ => {
                return Err(DbError::Corrupt {
                    column: "jobs.priority",
                });
            }
        },
        state: parse_wire(&state, "jobs.state")?,
        attempt: row.try_get("attempt")?,
        max_attempts: row.try_get("max_attempts")?,
        spec: serde_json::from_str(&spec)?,
        cancel_requested: row.try_get("cancel_requested")?,
        lease_expires_at: optional_ts(row, "lease_expires_at")?,
        resumes_at: optional_ts(row, "resumes_at")?,
        ready_output: optional_json(row, "ready_output")?,
        result: optional_json(row, "result")?,
        stage: JobStage::parse(&stage).ok_or(DbError::Corrupt {
            column: "jobs.stage",
        })?,
        generation: row.try_get("generation")?,
        round: row.try_get("round")?,
        github_access_granted: row.try_get("github_access_granted")?,
        created_at: parse_ts(&created_at, "jobs.created_at")?,
        finished_at: optional_ts(row, "finished_at")?,
    })
}

/// Credential effects of a runner result; they follow the runner's outcome even when cancel wins, since a usage
/// limit or a failed refresh is a fact about the credential, not the job.
async fn apply_credential_effects(
    tx: &mut SqliteConnection,
    credential: &str,
    result: &JobResult,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    if let Some(call) = &result.llm_call {
        let message = match &call.status {
            LlmCallStatus::Working => None,
            LlmCallStatus::NotWorking { message } => Some(message.as_str()),
        };
        let failure = match message {
            Some(_) => CallFailure::of(&result.outcome).map(CallFailure::wire),
            None => None,
        };
        sqlx::query(
            "UPDATE llm_credentials SET last_call_at = ?, last_call_ok = ?, last_call_message = ?,
                 last_call_failure = ? WHERE id = ?",
        )
        .bind(ts(call.at))
        .bind(message.is_none())
        .bind(message)
        .bind(failure)
        .bind(credential)
        .execute(&mut *tx)
        .await?;
    }
    match &result.outcome {
        JobOutcome::UsageLimited { resets_at } => {
            sqlx::query("UPDATE llm_credentials SET paused_until = ? WHERE id = ?")
                .bind(ts(resets_at.unwrap_or(now + PAUSE_WITHOUT_RESET)))
                .bind(credential)
                .execute(&mut *tx)
                .await?;
        }
        JobOutcome::Failed {
            kind: JobFailureKind::CredentialUnusable,
            ..
        } => {
            sqlx::query(
                "UPDATE llm_credentials SET unusable_since = COALESCE(unusable_since, ?) WHERE id = ?",
            )
            .bind(ts(now))
            .bind(credential)
            .execute(&mut *tx)
            .await?;
        }
        JobOutcome::Succeeded { .. } | JobOutcome::Failed { .. } | JobOutcome::Cancelled => {}
    }
    Ok(())
}

/// The part of a job row the write paths check.
struct Current {
    state: JobState,
    attempt: u32,
    cancel_requested: bool,
}

async fn current(tx: &mut SqliteConnection, job: JobId) -> Result<(Current, SqliteRow), DbError> {
    let row = sqlx::query(
        "SELECT kind, state, attempt, cancel_requested, max_attempts, ready_output, ready_output_attempt, credential_id,
             json_extract(result, '$.attempt') AS result_attempt
         FROM jobs WHERE id = ?",
    )
    .bind(job.to_string())
    .fetch_one(&mut *tx)
    .await?;
    let state: String = row.try_get("state")?;
    Ok((
        Current {
            state: parse_wire(&state, "jobs.state")?,
            attempt: row.try_get("attempt")?,
            cancel_requested: row.try_get("cancel_requested")?,
        },
        row,
    ))
}

impl Current {
    /// Requests of the runner are accepted only for the current attempt of a claimed or running job.
    fn accepts(&self, attempt: u32) -> Result<(), DbError> {
        if attempt == self.attempt && matches!(self.state, JobState::Claimed | JobState::Running) {
            Ok(())
        } else {
            Err(DbError::Conflict)
        }
    }

    /// The state after an accepted runner request: its first activity starts the job.
    fn started(&self) -> Result<JobState, DbError> {
        match self.state {
            JobState::Claimed => self
                .state
                .transition(JobTransition::Start)
                .map_err(|_| DbError::Conflict),
            state => Ok(state),
        }
    }
}

impl Jobs<'_> {
    /// Enqueues a job in state `queued` in the analysis's current round; `Conflict` if a live or succeeded job of
    /// the same stage, generation and round exists (look it up with [`Jobs::current_for_stage`] and reuse it).
    pub async fn enqueue(&self, job: NewJob, now: DateTime<Utc>) -> Result<JobRecord, DbError> {
        let mut spec = job.spec;
        // The stored spec never holds a credential (threat model T3b); access is filled at claim.
        spec.access = AgentAccess::default();
        spec.attempt = 0;
        let (flow_id, question_id) = match &job.stage {
            JobStage::Flow(flow) => (Some(flow.0.clone()), None),
            JobStage::Ask(question) => (None, Some(question.to_string())),
            JobStage::Plan { .. } | JobStage::PrContext => (None, None),
        };
        let inserted = sqlx::query(
            "INSERT INTO jobs (id, tenant_id, analysis_id, kind, priority, state, spec, credential_id, stage,
                 generation, flow_id, question_id, created_at, round)
             SELECT ?, tenant_id, id, ?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, round FROM analyses WHERE id = ?",
        )
        .bind(spec.job_id.to_string())
        .bind(wire_name(spec.kind)?)
        .bind(priority_value(job.priority))
        .bind(serde_json::to_string(&spec)?)
        .bind(&spec.credential.id)
        .bind(job.stage.key())
        .bind(job.generation)
        .bind(flow_id)
        .bind(question_id)
        .bind(ts(now))
        .bind(spec.analysis_id.to_string())
        .execute(self.pool)
        .await?;
        if inserted.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        self.get(spec.job_id).await
    }

    /// Claims the next runnable job: `queued`, credential usable and idle, no other running job of the same
    /// analysis; interactive before batch, then oldest first. Increments the attempt.
    pub async fn claim(&self, params: ClaimParams<'_>) -> Result<Option<JobRecord>, DbError> {
        if params.credential_ids.is_empty() {
            return Ok(None);
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "UPDATE jobs SET state = 'claimed', attempt = attempt + 1, spec = json_set(spec, '$.attempt', attempt + 1),
                 ready_output = NULL, ready_output_attempt = NULL, result = NULL, lease_expires_at = ",
        );
        query.push_bind(ts(params.lease_until));
        query.push(", job_token_hash = ");
        query.push_bind(params.job_token_hash);
        query.push(", claimed_at = ");
        query.push_bind(ts(params.now));
        query.push(
            " WHERE id = (SELECT j.id FROM jobs j WHERE j.state = 'queued'
                 AND NOT EXISTS (SELECT 1 FROM analyses a WHERE a.id = j.analysis_id AND (a.state = 'cancelled' OR (a.user_paused_at IS NOT NULL AND j.kind <> 'ask')))
                 AND NOT EXISTS (SELECT 1 FROM analyses a JOIN pull_requests p ON p.id=a.pull_request_id WHERE a.id=j.analysis_id AND p.session_state='drop_requested')
                 AND j.credential_id IN (",
        );
        let mut ids = query.separated(", ");
        for id in params.credential_ids {
            ids.push_bind(id.as_str());
        }
        query.push(
            ") AND NOT EXISTS (SELECT 1 FROM jobs r WHERE r.state IN ('claimed', 'running')
                   AND (r.analysis_id = j.analysis_id OR r.credential_id = j.credential_id
                       OR (SELECT pull_request_id FROM analyses WHERE id = r.analysis_id) =
                          (SELECT pull_request_id FROM analyses WHERE id = j.analysis_id)))
               AND NOT EXISTS (SELECT 1 FROM llm_credentials c WHERE c.id = j.credential_id
                   AND (c.unusable_since IS NOT NULL OR c.paused_until > ",
        );
        query.push_bind(ts(params.now));
        query.push(")) ORDER BY j.priority, j.created_at, j.rowid LIMIT 1) RETURNING id");
        let claimed: Option<String> = query.build_query_scalar().fetch_optional(self.pool).await?;
        match claimed {
            Some(id) => self.get(parse_wire(&id, "jobs.id")?).await.map(Some),
            None => Ok(None),
        }
    }

    /// Appends events of `attempt`, moves `claimed` to `running` and renews the lease. A stale attempt or a
    /// job that is not claimed or running is a `Conflict`.
    pub async fn record_events(
        &self,
        job: JobId,
        attempt: u32,
        events: &[JobEvent],
        lease_until: DateTime<Utc>,
    ) -> Result<EventsRecorded, DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let (current, _) = current(&mut tx, job).await?;
        current.accepts(attempt)?;
        let state = if events.is_empty() {
            current.state
        } else {
            current.started()?
        };
        for event in events {
            sqlx::query(
                "INSERT OR IGNORE INTO job_events (job_id, tenant_id, attempt, seq, at, body)
                 SELECT id, tenant_id, ?, ?, ?, ? FROM jobs WHERE id = ?",
            )
            .bind(attempt)
            .bind(event.seq)
            .bind(ts(event.at))
            .bind(serde_json::to_string(event)?)
            .bind(job.to_string())
            .execute(&mut *tx)
            .await?;
        }
        if !events.is_empty() {
            sqlx::query(
                "DELETE FROM job_events WHERE job_id = ?1 AND rowid NOT IN
                     (SELECT rowid FROM job_events WHERE job_id = ?1 ORDER BY attempt DESC, seq DESC LIMIT ?2)",
            )
            .bind(job.to_string())
            .bind(EVENTS_KEPT)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("UPDATE jobs SET state = ?, lease_expires_at = ? WHERE id = ?")
            .bind(wire_name(state)?)
            .bind(ts(lease_until))
            .bind(job.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(EventsRecorded {
            cancel_requested: current.cancel_requested,
        })
    }

    /// Stores the output of `attempt` posted before compaction, moves `claimed` to `running` and renews the lease.
    /// A different output for the same attempt, a stale attempt or a job that is not claimed or running is a
    /// `Conflict`.
    pub async fn record_output(
        &self,
        job: JobId,
        attempt: u32,
        output: &Value,
        lease_until: DateTime<Utc>,
    ) -> Result<OutputRecorded, DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let (current, row) = current(&mut tx, job).await?;
        current.accepts(attempt)?;
        let stored_attempt: Option<u32> = row.try_get("ready_output_attempt")?;
        let recorded = if stored_attempt == Some(attempt) {
            let stored: Option<Value> = optional_json(&row, "ready_output")?;
            if stored.as_ref() != Some(output) {
                return Err(DbError::Conflict);
            }
            OutputRecorded::AlreadyStored
        } else {
            OutputRecorded::Stored {
                cancel_requested: current.cancel_requested,
            }
        };
        sqlx::query(
            "UPDATE jobs SET state = ?, lease_expires_at = ?, ready_output = ?, ready_output_attempt = ?
             WHERE id = ?",
        )
        .bind(wire_name(current.started()?)?)
        .bind(ts(lease_until))
        .bind(serde_json::to_string(output)?)
        .bind(attempt)
        .bind(job.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(recorded)
    }

    /// Stores the result of `result.attempt` and applies the matching transition; idempotent per attempt. In the
    /// same transaction it records the LLM call outcome on the job's credential, pauses the credential after a
    /// usage limit and marks it unusable after a credential failure, so a crash never keeps one without the other.
    pub async fn complete(
        &self,
        result: &JobResult,
        now: DateTime<Utc>,
    ) -> Result<Completion, DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let (current, row) = current(&mut tx, result.job_id).await?;
        let recorded_attempt: Option<u32> = row.try_get("result_attempt")?;
        if recorded_attempt == Some(result.attempt) {
            return Ok(Completion::AlreadyRecorded);
        }
        current.accepts(result.attempt)?;
        let credential: String = row.try_get("credential_id")?;
        let mut stored = result.clone();
        let mut resumes_at = None;
        // Cancel wins: once requested, any result ends the job cancelled and its output is discarded.
        let transition = if current.cancel_requested {
            stored.outcome = JobOutcome::Cancelled;
            JobTransition::Cancel
        } else {
            match &result.outcome {
                JobOutcome::Succeeded { .. } => JobTransition::Succeed,
                JobOutcome::Failed { .. } => JobTransition::Fail,
                JobOutcome::Cancelled => JobTransition::Cancel,
                JobOutcome::UsageLimited { resets_at } => {
                    resumes_at = Some(resets_at.unwrap_or(now + PAUSE_WITHOUT_RESET));
                    JobTransition::Pause
                }
            }
        };
        let state = current
            .state
            .transition(transition)
            .map_err(|_| DbError::Conflict)?;
        sqlx::query(
            "UPDATE jobs SET state = ?1, result = ?2, resumes_at = ?3, finished_at = ?4, lease_expires_at = NULL,
                 ready_output = CASE WHEN ?1 = 'cancelled' THEN NULL ELSE ready_output END,
                 ready_output_attempt = CASE WHEN ?1 = 'cancelled' THEN NULL ELSE ready_output_attempt END
             WHERE id = ?5",
        )
        .bind(wire_name(state)?)
        .bind(serde_json::to_string(&stored)?)
        .bind(resumes_at.map(ts))
        .bind(state.is_terminal().then(|| ts(now)))
        .bind(result.job_id.to_string())
        .execute(&mut *tx)
        .await?;
        apply_credential_effects(&mut tx, &credential, result, now).await?;
        // Publish the reported analytic or private working checkpoint only for the first effective,
        // not cancelled result: a repeated or late result never restores an older session. A session already asked
        // to be dropped waits for its deletion acknowledgement before another job can start.
        if let (Some(session), false) = (
            &stored.session,
            matches!(stored.outcome, JobOutcome::Cancelled),
        ) {
            let kind: String = row.try_get("kind")?;
            if kind == "ask" {
                // The PR claim lock keeps the analytic generation unchanged for the entire Ask turn.
                // Derive the owner from the durable question, never the runner's result.
                sqlx::query("INSERT INTO working_sessions (tenant_id, pull_request_id, user_id, agent_thread_id, analytic_generation)
                    SELECT p.tenant_id, p.id, q.user_id, ?, p.analytic_generation FROM jobs j
                    JOIN questions q ON q.id = j.question_id JOIN analyses a ON a.id = j.analysis_id
                    JOIN pull_requests p ON p.id = a.pull_request_id
                    WHERE j.id = ? AND p.session_state IN ('none', 'active')
                    ON CONFLICT (pull_request_id, user_id) DO UPDATE SET
                        agent_thread_id = excluded.agent_thread_id, analytic_generation = excluded.analytic_generation")
                    .bind(&session.thread_id).bind(result.job_id.to_string()).execute(&mut *tx).await?;
            } else {
                sqlx::query("UPDATE pull_requests SET agent_thread_id = ?, analytic_generation = analytic_generation + 1,
                    session_state = CASE WHEN session_state IN ('none', 'active') THEN 'active' ELSE session_state END
                    WHERE id = (SELECT a.pull_request_id FROM jobs j JOIN analyses a ON a.id = j.analysis_id WHERE j.id = ?)")
                    .bind(&session.thread_id).bind(result.job_id.to_string()).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE analyses SET skills_commit = COALESCE(?, skills_commit) WHERE id = (SELECT analysis_id FROM jobs WHERE id = ?)")
                .bind(stored.skills_commit.as_deref()).bind(result.job_id.to_string()).execute(&mut *tx).await?;
        }
        let kind: String = row.try_get("kind")?;
        if kind != "ask"
            && stored.session.is_none()
            && matches!(stored.outcome, JobOutcome::Succeeded { .. })
        {
            // Valid output without a checkpoint means the old archive no longer represents this
            // generation. Neither an old analytic thread nor its private forks may be resumed.
            sqlx::query("UPDATE pull_requests SET agent_thread_id = NULL, analytic_generation = analytic_generation + 1,
                session_state = CASE WHEN session_state = 'active' THEN 'none' ELSE session_state END
                WHERE id = (SELECT a.pull_request_id FROM jobs j JOIN analyses a ON a.id = j.analysis_id WHERE j.id = ?)")
                .bind(result.job_id.to_string()).execute(&mut *tx).await?;
        }
        if kind != "ask"
            && stored.session.is_some()
            && matches!(stored.outcome, JobOutcome::Cancelled)
        {
            // Cancel wins the result, but a useful turn may already have checkpointed before cancellation.
            sqlx::query("UPDATE pull_requests SET analytic_generation = analytic_generation + 1
                WHERE id = (SELECT a.pull_request_id FROM jobs j JOIN analyses a ON a.id = j.analysis_id WHERE j.id = ?)")
                .bind(result.job_id.to_string()).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(Completion::Applied { state })
    }

    /// Cancels a queued or paused job at once, or flags a claimed or running one; returns the state after.
    pub async fn request_cancel(
        &self,
        job: JobId,
        now: DateTime<Utc>,
    ) -> Result<JobState, DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let (current, _) = current(&mut tx, job).await?;
        match current.state {
            JobState::Queued | JobState::Paused => {
                let result = server_result(job, current.attempt, JobOutcome::Cancelled);
                sqlx::query(
                    "UPDATE jobs SET state = 'cancelled', cancel_requested = 1, result = ?, finished_at = ?,
                         resumes_at = NULL, ready_output = NULL, ready_output_attempt = NULL
                     WHERE id = ?",
                )
                .bind(serde_json::to_string(&result)?)
                .bind(ts(now))
                .bind(job.to_string())
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(JobState::Cancelled)
            }
            JobState::Claimed | JobState::Running => {
                sqlx::query("UPDATE jobs SET cancel_requested = 1 WHERE id = ?")
                    .bind(job.to_string())
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                Ok(current.state)
            }
            JobState::Succeeded | JobState::Failed | JobState::Cancelled => Ok(current.state),
        }
    }

    /// Ends the lease of claimed or running jobs whose lease passed: `cancelled` when cancel was requested (never
    /// re-claimed), else `queued`, or `failed` without attempts left. Clears `ready_output` and the job token
    /// hash; the caller revokes the attempt's installation token (see [`Jobs::github_tokens_to_revoke`]) and ends
    /// the executor stream.
    pub async fn expire_leases(&self, now: DateTime<Utc>) -> Result<Vec<ExpiredLease>, DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let rows = sqlx::query(
            "SELECT id, state, attempt, max_attempts, cancel_requested FROM jobs
             WHERE state IN ('claimed', 'running') AND lease_expires_at < ?
             ORDER BY lease_expires_at, rowid",
        )
        .bind(ts(now))
        .fetch_all(&mut *tx)
        .await?;
        let mut expired = Vec::with_capacity(rows.len());
        for row in &rows {
            let id: String = row.try_get("id")?;
            let job: JobId = parse_wire(&id, "jobs.id")?;
            let state: String = row.try_get("state")?;
            let state: JobState = parse_wire(&state, "jobs.state")?;
            let attempt: u32 = row.try_get("attempt")?;
            let max_attempts: u32 = row.try_get("max_attempts")?;
            let next = state
                .transition(JobTransition::LeaseExpired {
                    exhausted: attempt >= max_attempts,
                    cancel_requested: row.try_get("cancel_requested")?,
                })
                .map_err(|_| DbError::Conflict)?;
            let result = match next {
                JobState::Cancelled => Some(server_result(job, attempt, JobOutcome::Cancelled)),
                JobState::Failed => Some(server_result(
                    job,
                    attempt,
                    JobOutcome::Failed {
                        kind: JobFailureKind::Internal,
                        message: RUNNER_LOST_MESSAGE.to_owned(),
                    },
                )),
                _ => None,
            };
            sqlx::query(
                "UPDATE jobs SET state = ?, result = ?, finished_at = ?, lease_expires_at = NULL,
                     job_token_hash = NULL, ready_output = NULL, ready_output_attempt = NULL
                 WHERE id = ?",
            )
            .bind(wire_name(next)?)
            .bind(result.as_ref().map(serde_json::to_string).transpose()?)
            .bind(next.is_terminal().then(|| ts(now)))
            .bind(&id)
            .execute(&mut *tx)
            .await?;
            expired.push(ExpiredLease { job, state: next });
        }
        tx.commit().await?;
        Ok(expired)
    }

    /// Re-queues paused jobs whose `resumes_at` has passed.
    pub async fn resume_paused(&self, now: DateTime<Utc>) -> Result<Vec<JobId>, DbError> {
        let ids: Vec<String> = sqlx::query_scalar(
            "UPDATE jobs SET state = 'queued', resumes_at = NULL
             WHERE state = 'paused' AND resumes_at <= ? RETURNING id",
        )
        .bind(ts(now))
        .fetch_all(self.pool)
        .await?;
        ids.iter().map(|id| parse_wire(id, "jobs.id")).collect()
    }

    /// Records whether the claimed `attempt` got a GitHub installation token (non-secret; related work is
    /// checked only by an agent that had the GitHub tool). A stale attempt changes nothing.
    pub async fn record_github_access(
        &self,
        job: JobId,
        attempt: u32,
        granted: bool,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE jobs SET github_access_granted = ? WHERE id = ? AND attempt = ?")
            .bind(granted)
            .bind(job.to_string())
            .bind(attempt)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// Ends the claim of `attempt` before anything was handed out (its context could not be prepared), like an
    /// expired lease: the attempt stays spent and the job is `queued` again without lease and job token,
    /// `failed` with an internal cause once no attempt is left, or `cancelled` when that was requested. `None`
    /// when the attempt is no longer claimed (nothing changes).
    pub async fn release_claim(
        &self,
        job: JobId,
        attempt: u32,
        now: DateTime<Utc>,
    ) -> Result<Option<JobState>, DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let (current, row) = current(&mut tx, job).await?;
        if current.state != JobState::Claimed || current.attempt != attempt {
            return Ok(None);
        }
        let max_attempts: u32 = row.try_get("max_attempts")?;
        let next = current
            .state
            .transition(JobTransition::LeaseExpired {
                exhausted: attempt >= max_attempts,
                cancel_requested: current.cancel_requested,
            })
            .map_err(|_| DbError::Conflict)?;
        let result = match next {
            JobState::Cancelled => Some(server_result(job, attempt, JobOutcome::Cancelled)),
            JobState::Failed => Some(server_result(
                job,
                attempt,
                JobOutcome::Failed {
                    kind: JobFailureKind::Internal,
                    message: CONTEXT_UNPREPARED_MESSAGE.to_owned(),
                },
            )),
            _ => None,
        };
        sqlx::query(
            "UPDATE jobs SET state = ?, result = ?, finished_at = ?, lease_expires_at = NULL, job_token_hash = NULL
             WHERE id = ?",
        )
        .bind(wire_name(next)?)
        .bind(result.as_ref().map(serde_json::to_string).transpose()?)
        .bind(next.is_terminal().then(|| ts(now)))
        .bind(job.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(next))
    }

    /// The live or succeeded job of `stage` and `generation` in the analysis's current round, if any: the job an
    /// idempotent submit answers with. Jobs of earlier rounds (before a Retry) are never returned.
    pub async fn current_for_stage(
        &self,
        analysis: AnalysisId,
        stage: &JobStage,
        generation: u32,
    ) -> Result<Option<JobRecord>, DbError> {
        let row = sqlx::query(concat!(
            "SELECT ",
            job_columns!(),
            " FROM jobs j WHERE analysis_id = ? AND stage = ? AND generation = ?
                 AND state NOT IN ('failed', 'cancelled')
                 AND round = (SELECT round FROM analyses WHERE id = j.analysis_id)
             ORDER BY created_at DESC, rowid DESC LIMIT 1"
        ))
        .bind(analysis.to_string())
        .bind(stage.key())
        .bind(generation)
        .fetch_optional(self.pool)
        .await?;
        row.as_ref().map(job_from_row).transpose()
    }

    /// A job.
    pub async fn get(&self, job: JobId) -> Result<JobRecord, DbError> {
        let row = sqlx::query(concat!(
            "SELECT ",
            job_columns!(),
            " FROM jobs WHERE id = ?"
        ))
        .bind(job.to_string())
        .fetch_one(self.pool)
        .await?;
        job_from_row(&row)
    }

    /// Jobs of an analysis, oldest first; only those in `states` unless `states` is empty.
    pub async fn list_for_analysis(
        &self,
        analysis: AnalysisId,
        states: &[JobState],
    ) -> Result<Vec<JobRecord>, DbError> {
        let mut query = QueryBuilder::<Sqlite>::new(concat!(
            "SELECT ",
            job_columns!(),
            " FROM jobs WHERE analysis_id = "
        ));
        query.push_bind(analysis.to_string());
        if !states.is_empty() {
            query.push(" AND state IN (");
            let mut list = query.separated(", ");
            for state in states {
                list.push_bind(state.as_str());
            }
            query.push(")");
        }
        query.push(" ORDER BY created_at, rowid");
        query
            .build()
            .fetch_all(self.pool)
            .await?
            .iter()
            .map(job_from_row)
            .collect()
    }

    /// The job-token hash of the current claim, for constant-time comparison by the server.
    pub async fn token_hash(&self, job: JobId) -> Result<Option<Vec<u8>>, DbError> {
        Ok(
            sqlx::query_scalar("SELECT job_token_hash FROM jobs WHERE id = ?")
                .bind(job.to_string())
                .fetch_one(self.pool)
                .await?,
        )
    }

    /// The token hash together with the attempt, state and lease it belongs to (one consistent read).
    pub async fn authorization(&self, job: JobId) -> Result<JobAuthorization, DbError> {
        let row = sqlx::query(
            "SELECT job_token_hash, attempt, state, lease_expires_at, cancel_requested FROM jobs WHERE id = ?",
        )
        .bind(job.to_string())
        .fetch_one(self.pool)
        .await?;
        let state: String = row.try_get("state")?;
        Ok(JobAuthorization {
            token_hash: row.try_get("job_token_hash")?,
            attempt: row.try_get("attempt")?,
            state: parse_wire(&state, "jobs.state")?,
            lease_expires_at: optional_ts(&row, "lease_expires_at")?,
            cancel_requested: row.try_get("cancel_requested")?,
        })
    }

    /// The job and the events of its current attempt after `after_seq` (all of them when the current attempt is
    /// not `attempt`), read in one transaction: state, ready output and events belong to one moment.
    pub async fn snapshot(
        &self,
        job: JobId,
        attempt: u32,
        after_seq: u32,
    ) -> Result<JobSnapshot, DbError> {
        let mut tx = super::read_tx(self.pool).await?;
        let row = sqlx::query(concat!(
            "SELECT ",
            job_columns!(),
            " FROM jobs WHERE id = ?"
        ))
        .bind(job.to_string())
        .fetch_one(&mut *tx)
        .await?;
        let record = job_from_row(&row)?;
        let after_seq = if record.attempt == attempt {
            after_seq
        } else {
            0
        };
        let bodies: Vec<String> = sqlx::query_scalar(
            "SELECT body FROM job_events WHERE job_id = ? AND attempt = ? AND seq > ? ORDER BY seq",
        )
        .bind(job.to_string())
        .bind(record.attempt)
        .bind(after_seq)
        .fetch_all(&mut *tx)
        .await?;
        let paused_until: Option<Option<String>> = sqlx::query_scalar(
            "SELECT paused_until FROM llm_credentials
                 WHERE id = (SELECT credential_id FROM jobs WHERE id = ?)",
        )
        .bind(job.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        let credential_paused_until = paused_until
            .flatten()
            .map(|at| parse_ts(&at, "llm_credentials.paused_until"))
            .transpose()?;
        let events = bodies
            .iter()
            .map(|body| serde_json::from_str(body).map_err(DbError::from))
            .collect::<Result<_, _>>()?;
        Ok(JobSnapshot {
            job: record,
            events,
            credential_paused_until,
        })
    }

    /// Events of a job after `after_seq` of the current attempt, in order.
    pub async fn events(&self, job: JobId, after_seq: u32) -> Result<Vec<JobEvent>, DbError> {
        let bodies: Vec<String> = sqlx::query_scalar(
            "SELECT e.body FROM job_events e JOIN jobs j ON j.id = e.job_id AND e.attempt = j.attempt
             WHERE e.job_id = ? AND e.seq > ? ORDER BY e.seq",
        )
        .bind(job.to_string())
        .bind(after_seq)
        .fetch_all(self.pool)
        .await?;
        bodies
            .iter()
            .map(|body| serde_json::from_str(body).map_err(DbError::from))
            .collect()
    }

    /// Stores the encrypted installation token minted for `attempt` at claim; `Conflict` if one exists.
    pub async fn record_github_token(
        &self,
        job: JobId,
        attempt: u32,
        ciphertext: &[u8],
        expires_at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        let inserted = sqlx::query(
            "INSERT INTO installation_tokens (job_id, tenant_id, attempt, ciphertext, expires_at)
             SELECT id, tenant_id, ?, ?, ? FROM jobs WHERE id = ?",
        )
        .bind(attempt)
        .bind(ciphertext)
        .bind(expires_at.to_rfc3339())
        .bind(job.to_string())
        .execute(self.pool)
        .await?;
        if inserted.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// The encrypted token of `attempt`, without removing it (revoke, then [`Jobs::confirm_github_token_revoked`]).
    pub async fn github_token(&self, job: JobId, attempt: u32) -> Result<Option<Vec<u8>>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT ciphertext FROM installation_tokens WHERE job_id = ? AND attempt = ?",
        )
        .bind(job.to_string())
        .bind(attempt)
        .fetch_optional(self.pool)
        .await?)
    }

    /// Tokens not in use by a claimed or running attempt and not yet confirmed revoked, soonest expiry first;
    /// run at start and after results and lease expiries, so a crash before revocation is recovered.
    pub async fn github_tokens_to_revoke(&self) -> Result<Vec<TokenToRevoke>, DbError> {
        let rows = sqlx::query(
            "SELECT t.job_id, t.attempt, t.ciphertext, t.expires_at
             FROM installation_tokens t JOIN jobs j ON j.id = t.job_id
             WHERE NOT (j.state IN ('claimed', 'running') AND j.attempt = t.attempt)
             ORDER BY t.expires_at, t.job_id, t.attempt",
        )
        .fetch_all(self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let job: String = row.try_get("job_id")?;
                let expires_at: String = row.try_get("expires_at")?;
                Ok(TokenToRevoke {
                    job: serde_json::from_value(Value::String(job)).map_err(|_| {
                        DbError::Corrupt {
                            column: "installation_tokens.job_id",
                        }
                    })?,
                    attempt: row.try_get("attempt")?,
                    ciphertext: row.try_get("ciphertext")?,
                    expires_at: DateTime::parse_from_rfc3339(&expires_at)
                        .map_err(|_| DbError::Corrupt {
                            column: "installation_tokens.expires_at",
                        })?
                        .with_timezone(&Utc),
                })
            })
            .collect()
    }

    /// Deletes the token of `attempt` after GitHub confirmed the revocation (or it expired); false if it was
    /// already gone (idempotent).
    pub async fn confirm_github_token_revoked(
        &self,
        job: JobId,
        attempt: u32,
    ) -> Result<bool, DbError> {
        let deleted =
            sqlx::query("DELETE FROM installation_tokens WHERE job_id = ? AND attempt = ?")
                .bind(job.to_string())
                .bind(attempt)
                .execute(self.pool)
                .await?;
        Ok(deleted.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use contracts::job::access::{McpHttpEndpoint, Secret};
    use contracts::job::event::ActivityKind;
    use contracts::job::result::LlmCallOutcome;
    use contracts::job::spec::{CredentialKind, LlmCredentialRef};
    use serde_json::json;

    use super::*;
    use crate::repo::Db;
    use crate::repo::analyses::testing::{CREDENTIAL, job_spec, seed_analysis};

    fn at(second: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 16, 12, 0, 0)
            .single()
            .unwrap_or_default()
            + TimeDelta::seconds(second)
    }

    async fn db() -> Db {
        Db::in_memory().await.expect("in-memory database")
    }

    fn new_job(analysis: AnalysisId, flow: &str, priority: JobPriority) -> NewJob {
        NewJob {
            spec: job_spec(analysis, JobKind::AnalyseFlow),
            priority,
            stage: JobStage::Flow(FlowId(flow.to_owned())),
            generation: 0,
        }
    }

    async fn enqueue(db: &Db, analysis: AnalysisId, flow: &str, second: i64) -> JobId {
        db.jobs()
            .enqueue(new_job(analysis, flow, JobPriority::Batch), at(second))
            .await
            .expect("enqueue")
            .id
    }

    async fn claim(db: &Db, second: i64) -> Option<JobRecord> {
        db.jobs()
            .claim(ClaimParams {
                credential_ids: &[CREDENTIAL.to_owned()],
                job_token_hash: b"hash",
                lease_until: at(second + 60),
                now: at(second),
            })
            .await
            .expect("claim")
    }

    fn event(seq: u32) -> JobEvent {
        JobEvent {
            seq,
            at: at(0),
            activity: ActivityKind::Command,
            summary: format!("step {seq}"),
        }
    }

    fn result(job: JobId, attempt: u32, outcome: JobOutcome) -> JobResult {
        JobResult {
            job_id: job,
            attempt,
            outcome,
            session: None,
            usage: None,
            skills_commit: None,
            llm_call: None,
        }
    }

    fn succeeded(job: JobId, attempt: u32) -> JobResult {
        result(
            job,
            attempt,
            JobOutcome::Succeeded {
                output: json!({"flows": []}),
            },
        )
    }

    #[tokio::test]
    async fn lost_analytic_checkpoint_invalidates_private_forks() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        db.analyses()
            .set_session(analysis, "analytic", None)
            .await
            .expect("session");
        let generation = db.agent_sessions().generation(1).await.expect("generation");
        sqlx::query("INSERT INTO working_sessions (tenant_id, pull_request_id, user_id, agent_thread_id, analytic_generation) VALUES ('default', 1, 'u1', 'private', ?)")
            .bind(generation).execute(&db.pool).await.expect("fork");
        let job = enqueue(&db, analysis, "f1", 0).await;
        claim(&db, 1).await.expect("claim");
        db.jobs()
            .complete(&succeeded(job, 1), at(2))
            .await
            .expect("complete without archive");
        assert_eq!(
            db.agent_sessions().working(1, "u1").await.expect("fork"),
            None
        );
        assert_eq!(
            db.analyses()
                .get(analysis)
                .await
                .expect("analysis")
                .agent_thread_id,
            None
        );
    }

    #[tokio::test]
    async fn jobs_spec_has_no_credentials() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let mut job = new_job(analysis, "f1", JobPriority::Batch);
        job.spec.access.memcan = Some(McpHttpEndpoint {
            url: "https://memcan.example/mcp".to_owned(),
            token: Secret::new("memcan-canary"),
            allow_write: true,
        });
        let record = db
            .jobs()
            .enqueue(job.clone(), at(0))
            .await
            .expect("enqueue");
        assert_eq!(record.spec.access, AgentAccess::default());
        assert_eq!(record.state, JobState::Queued);
        assert_eq!(record.attempt, 0);
        assert_eq!(record.max_attempts, 3);
        assert_eq!(record.stage, job.stage);
        assert_eq!(record.created_at, at(0));
        let stored: String = sqlx::query_scalar("SELECT spec FROM jobs")
            .fetch_one(&db.pool)
            .await
            .expect("spec");
        assert!(!stored.contains("memcan-canary"), "{stored}");
    }

    #[tokio::test]
    async fn enqueue_conflicts_for_a_live_stage_generation_and_unknown_analysis_is_not_found() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        enqueue(&db, analysis, "f1", 0).await;
        assert!(matches!(
            db.jobs()
                .enqueue(new_job(analysis, "f1", JobPriority::Batch), at(1))
                .await,
            Err(DbError::Conflict)
        ));
        let mut repair = new_job(analysis, "f1", JobPriority::Batch);
        repair.generation = 1;
        db.jobs()
            .enqueue(repair, at(2))
            .await
            .expect("a repair run is a new generation");
        assert!(matches!(
            db.jobs()
                .enqueue(
                    new_job(AnalysisId::new_random(), "f1", JobPriority::Batch),
                    at(3)
                )
                .await,
            Err(DbError::NotFound)
        ));
    }

    #[tokio::test]
    async fn claim_takes_interactive_first_then_oldest_and_sets_attempt_lease_and_token() {
        let db = db().await;
        let first = seed_analysis(&db).await.expect("seed");
        let second = seed_analysis(&db).await.expect("seed");
        let third = seed_analysis(&db).await.expect("seed");
        let oldest = enqueue(&db, first, "f1", 0).await;
        enqueue(&db, second, "f1", 1).await;
        let interactive = db
            .jobs()
            .enqueue(new_job(third, "f1", JobPriority::Interactive), at(2))
            .await
            .expect("enqueue")
            .id;

        let claimed = claim(&db, 10).await.expect("a job");
        assert_eq!(claimed.id, interactive);
        assert_eq!(claimed.state, JobState::Claimed);
        assert_eq!(claimed.attempt, 1);
        assert_eq!(claimed.spec.attempt, 1);
        assert_eq!(claimed.lease_expires_at, Some(at(70)));
        assert_eq!(
            db.jobs().token_hash(interactive).await.expect("hash"),
            Some(b"hash".to_vec())
        );
        // One credential runs one job at a time.
        assert_eq!(claim(&db, 11).await, None);
        db.jobs()
            .complete(&succeeded(interactive, 1), at(12))
            .await
            .expect("complete");
        assert_eq!(claim(&db, 13).await.map(|job| job.id), Some(oldest));
    }

    #[tokio::test]
    async fn claim_skips_unheld_unusable_and_paused_credentials() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        enqueue(&db, analysis, "f1", 0).await;
        let other = db
            .jobs()
            .claim(ClaimParams {
                credential_ids: &["other".to_owned()],
                job_token_hash: b"hash",
                lease_until: at(60),
                now: at(0),
            })
            .await
            .expect("claim");
        assert_eq!(other, None);
        assert_eq!(
            db.jobs()
                .claim(ClaimParams {
                    credential_ids: &[],
                    job_token_hash: b"hash",
                    lease_until: at(60),
                    now: at(0),
                })
                .await
                .expect("claim"),
            None
        );
        sqlx::query(
            "INSERT INTO llm_credentials (id, tenant_id, kind, paused_until) VALUES (?, 'default', 'codex_chatgpt', ?)",
        )
        .bind(CREDENTIAL)
        .bind(ts(at(100)))
        .execute(&db.pool)
        .await
        .expect("paused credential");
        assert_eq!(claim(&db, 50).await, None, "credential paused");
        assert!(claim(&db, 100).await.is_some(), "pause over");
    }

    #[tokio::test]
    async fn second_job_of_the_same_session_is_not_claimed_while_one_runs() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let first = enqueue(&db, analysis, "f1", 0).await;
        let second = enqueue(&db, analysis, "f2", 1).await;
        let claimed = db
            .jobs()
            .claim(ClaimParams {
                credential_ids: &[CREDENTIAL.to_owned(), "spare".to_owned()],
                job_token_hash: b"hash",
                lease_until: at(60),
                now: at(0),
            })
            .await
            .expect("claim");
        assert_eq!(claimed.map(|job| job.id), Some(first));
        // A second credential is idle, but the PR's session already runs a job.
        sqlx::query("UPDATE jobs SET credential_id = 'spare' WHERE id = ?")
            .bind(second.to_string())
            .execute(&db.pool)
            .await
            .expect("move");
        let blocked = db
            .jobs()
            .claim(ClaimParams {
                credential_ids: &["spare".to_owned()],
                job_token_hash: b"hash",
                lease_until: at(60),
                now: at(1),
            })
            .await
            .expect("claim");
        assert_eq!(blocked, None);
    }

    #[tokio::test]
    async fn another_analysis_of_the_pr_waits_even_on_another_credential() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let first = enqueue(&db, analysis, "f1", 0).await;
        let other = AnalysisId::new_random();
        sqlx::query("INSERT INTO analyses (id, tenant_id, pull_request_id, base_sha, head_sha, language, state, created_by, created_at) SELECT ?, tenant_id, pull_request_id, base_sha, 'new-head', 'pl', 'planning', created_by, created_at FROM analyses WHERE id = ?")
            .bind(other.to_string()).bind(analysis.to_string()).execute(&db.pool).await.unwrap();
        let second = enqueue(&db, other, "f2", 1).await;
        let claimed = db
            .jobs()
            .claim(ClaimParams {
                credential_ids: &[CREDENTIAL.to_owned(), "spare".to_owned()],
                job_token_hash: b"hash",
                lease_until: at(60),
                now: at(0),
            })
            .await
            .expect("claim");
        assert_eq!(claimed.map(|job| job.id), Some(first));
        // A second credential is idle, but the PR's session already runs a job.
        sqlx::query("UPDATE jobs SET credential_id = 'spare' WHERE id = ?")
            .bind(second.to_string())
            .execute(&db.pool)
            .await
            .expect("move");
        let blocked = db
            .jobs()
            .claim(ClaimParams {
                credential_ids: &["spare".to_owned()],
                job_token_hash: b"hash",
                lease_until: at(60),
                now: at(1),
            })
            .await
            .expect("claim");
        assert_eq!(blocked, None);
    }

    #[tokio::test]
    async fn lease_expiry_requeues_until_attempts_are_exhausted() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let job = enqueue(&db, analysis, "f1", 0).await;
        for attempt in 1..=3_u32 {
            let start = i64::from(attempt) * 100;
            let claimed = claim(&db, start).await.expect("claim");
            assert_eq!(claimed.attempt, attempt);
            db.jobs()
                .record_output(job, attempt, &json!({"draft": attempt}), at(start + 60))
                .await
                .expect("output");
            assert!(
                db.jobs()
                    .expire_leases(at(start + 60))
                    .await
                    .expect("not yet")
                    .is_empty(),
                "the lease ends only after lease_expires_at"
            );
            let expired = db
                .jobs()
                .expire_leases(at(start + 61))
                .await
                .expect("expire");
            let expected = if attempt < 3 {
                JobState::Queued
            } else {
                JobState::Failed
            };
            assert_eq!(
                expired,
                [ExpiredLease {
                    job,
                    state: expected
                }]
            );
            let record = db.jobs().get(job).await.expect("get");
            assert_eq!(record.state, expected);
            assert_eq!(record.ready_output, None);
            assert_eq!(record.lease_expires_at, None);
            assert_eq!(db.jobs().token_hash(job).await.expect("hash"), None);
        }
        let failed = db.jobs().get(job).await.expect("get");
        assert!(matches!(
            failed.result.map(|result| result.outcome),
            Some(JobOutcome::Failed {
                kind: JobFailureKind::Internal,
                ..
            })
        ));
        assert_eq!(failed.finished_at, Some(at(361)));
        assert_eq!(claim(&db, 400).await, None);
    }

    #[tokio::test]
    async fn cancel_at_lease_expiry_ends_cancelled_and_discards_output() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let job = enqueue(&db, analysis, "f1", 0).await;
        claim(&db, 0).await.expect("claim");
        db.jobs()
            .record_output(job, 1, &json!({"draft": true}), at(60))
            .await
            .expect("output");
        assert_eq!(
            db.jobs().request_cancel(job, at(1)).await.expect("cancel"),
            JobState::Running,
            "an output starts the job; cancelling a running job only flags it"
        );
        let expired = db.jobs().expire_leases(at(61)).await.expect("expire");
        assert_eq!(
            expired,
            [ExpiredLease {
                job,
                state: JobState::Cancelled
            }]
        );
        let record = db.jobs().get(job).await.expect("get");
        assert_eq!(record.ready_output, None);
        assert_eq!(
            record.result.map(|r| r.outcome),
            Some(JobOutcome::Cancelled)
        );
        assert_eq!(claim(&db, 100).await, None, "never re-claimed");
    }

    #[tokio::test]
    async fn events_start_the_job_renew_the_lease_and_ignore_duplicates() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let job = enqueue(&db, analysis, "f1", 0).await;
        claim(&db, 0).await.expect("claim");
        let heartbeat = db
            .jobs()
            .record_events(job, 1, &[], at(70))
            .await
            .expect("heartbeat");
        assert!(!heartbeat.cancel_requested);
        assert_eq!(
            db.jobs().get(job).await.expect("get").state,
            JobState::Claimed
        );
        db.jobs()
            .record_events(job, 1, &[event(1), event(2)], at(80))
            .await
            .expect("events");
        db.jobs()
            .record_events(job, 1, &[event(2), event(3)], at(90))
            .await
            .expect("retried batch");
        let record = db.jobs().get(job).await.expect("get");
        assert_eq!(record.state, JobState::Running);
        assert_eq!(record.lease_expires_at, Some(at(90)));
        let seqs: Vec<u32> = db
            .jobs()
            .events(job, 0)
            .await
            .expect("events")
            .iter()
            .map(|event| event.seq)
            .collect();
        assert_eq!(seqs, [1, 2, 3]);
        assert_eq!(db.jobs().events(job, 2).await.expect("after").len(), 1);
        db.jobs().request_cancel(job, at(91)).await.expect("cancel");
        assert!(
            db.jobs()
                .record_events(job, 1, &[], at(95))
                .await
                .expect("heartbeat")
                .cancel_requested
        );
    }

    #[tokio::test]
    async fn snapshot_returns_the_job_with_events_of_its_current_attempt() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let job = enqueue(&db, analysis, "f1", 0).await;
        claim(&db, 0).await.expect("claim");
        db.jobs()
            .record_events(job, 1, &[event(1), event(2)], at(60))
            .await
            .expect("events");
        let seqs = |snapshot: &JobSnapshot| -> Vec<u32> {
            snapshot.events.iter().map(|event| event.seq).collect()
        };
        let snapshot = db.jobs().snapshot(job, 1, 1).await.expect("snapshot");
        assert_eq!((snapshot.job.attempt, seqs(&snapshot)), (1, vec![2]));
        db.jobs().expire_leases(at(61)).await.expect("expire");
        claim(&db, 62).await.expect("re-claim");
        db.jobs()
            .record_events(job, 2, &[event(1)], at(120))
            .await
            .expect("events");
        let snapshot = db.jobs().snapshot(job, 1, 2).await.expect("snapshot");
        assert_eq!(
            (snapshot.job.attempt, seqs(&snapshot)),
            (2, vec![1]),
            "a new attempt starts from its first event"
        );
        assert!(matches!(
            db.jobs().snapshot(JobId::new_random(), 0, 0).await,
            Err(DbError::NotFound)
        ));
    }

    #[tokio::test]
    async fn event_log_keeps_the_last_500_events() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let job = enqueue(&db, analysis, "f1", 0).await;
        claim(&db, 0).await.expect("claim");
        let events: Vec<JobEvent> = (1..=510).map(event).collect();
        db.jobs()
            .record_events(job, 1, &events, at(60))
            .await
            .expect("events");
        let kept = db.jobs().events(job, 0).await.expect("events");
        assert_eq!(kept.len(), 500);
        assert_eq!(kept.first().map(|event| event.seq), Some(11));
    }

    #[tokio::test]
    async fn stale_attempt_or_finished_job_is_a_conflict() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let job = enqueue(&db, analysis, "f1", 0).await;
        assert!(matches!(
            db.jobs().record_events(job, 0, &[], at(60)).await,
            Err(DbError::Conflict)
        ));
        claim(&db, 0).await.expect("claim");
        db.jobs().expire_leases(at(100)).await.expect("expire");
        claim(&db, 100).await.expect("re-claim");
        assert!(matches!(
            db.jobs().record_events(job, 1, &[event(1)], at(160)).await,
            Err(DbError::Conflict)
        ));
        assert!(matches!(
            db.jobs().record_output(job, 1, &json!({}), at(160)).await,
            Err(DbError::Conflict)
        ));
        assert!(matches!(
            db.jobs().complete(&succeeded(job, 1), at(101)).await,
            Err(DbError::Conflict)
        ));
        assert!(matches!(
            db.jobs()
                .record_events(JobId::new_random(), 1, &[], at(160))
                .await,
            Err(DbError::NotFound)
        ));
        db.jobs()
            .complete(&succeeded(job, 2), at(102))
            .await
            .expect("complete");
        assert!(matches!(
            db.jobs().record_events(job, 2, &[], at(160)).await,
            Err(DbError::Conflict)
        ));
    }

    #[tokio::test]
    async fn output_is_stored_once_per_attempt() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let job = enqueue(&db, analysis, "f1", 0).await;
        claim(&db, 0).await.expect("claim");
        let output = json!({"answer": "a", "steps": [1, 2]});
        assert_eq!(
            db.jobs()
                .record_output(job, 1, &output, at(60))
                .await
                .expect("output"),
            OutputRecorded::Stored {
                cancel_requested: false
            }
        );
        assert_eq!(
            db.jobs()
                .record_output(job, 1, &output, at(70))
                .await
                .expect("repeat"),
            OutputRecorded::AlreadyStored
        );
        assert!(matches!(
            db.jobs()
                .record_output(job, 1, &json!({"answer": "b"}), at(70))
                .await,
            Err(DbError::Conflict)
        ));
        let record = db.jobs().get(job).await.expect("get");
        assert_eq!(record.ready_output, Some(output));
        assert_eq!(record.state, JobState::Running);
        assert_eq!(record.lease_expires_at, Some(at(70)));
    }

    #[tokio::test]
    async fn output_after_cancel_request_is_stored_but_flagged() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let job = enqueue(&db, analysis, "f1", 0).await;
        claim(&db, 0).await.expect("claim");
        db.jobs().request_cancel(job, at(1)).await.expect("cancel");
        assert_eq!(
            db.jobs()
                .record_output(job, 1, &json!({}), at(60))
                .await
                .expect("output"),
            OutputRecorded::Stored {
                cancel_requested: true
            }
        );
    }

    #[tokio::test]
    async fn result_posted_twice_is_stored_once() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let job = enqueue(&db, analysis, "f1", 0).await;
        claim(&db, 0).await.expect("claim");
        let result = succeeded(job, 1);
        assert_eq!(
            db.jobs().complete(&result, at(5)).await.expect("complete"),
            Completion::Applied {
                state: JobState::Succeeded
            }
        );
        assert_eq!(
            db.jobs().complete(&result, at(9)).await.expect("repeat"),
            Completion::AlreadyRecorded
        );
        let record = db.jobs().get(job).await.expect("get");
        assert_eq!(record.result, Some(result));
        assert_eq!(record.finished_at, Some(at(5)));
        assert_eq!(record.lease_expires_at, None);
    }

    async fn register_credential(db: &Db) {
        db.llm_credentials()
            .register(
                "default",
                &LlmCredentialRef {
                    id: CREDENTIAL.to_owned(),
                    kind: CredentialKind::CodexChatgpt,
                },
                true,
                at(0),
            )
            .await
            .expect("register");
    }

    #[tokio::test]
    async fn complete_applies_credential_effects_in_the_same_transaction() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let job = enqueue(&db, analysis, "f1", 0).await;
        register_credential(&db).await;
        claim(&db, 0).await.expect("claim");
        let mut limited = result(
            job,
            1,
            JobOutcome::UsageLimited {
                resets_at: Some(at(900)),
            },
        );
        limited.llm_call = Some(LlmCallOutcome {
            at: at(9),
            status: LlmCallStatus::NotWorking {
                message: "Usage limit reached.".to_owned(),
            },
        });
        // A failure of the credential write (a crash between the two writes) leaves no result behind.
        sqlx::raw_sql(
            "CREATE TRIGGER fail_credential_write BEFORE UPDATE ON llm_credentials
             BEGIN SELECT RAISE(ABORT, 'injected'); END;",
        )
        .execute(&db.pool)
        .await
        .expect("trigger");
        assert!(db.jobs().complete(&limited, at(10)).await.is_err());
        let record = db.jobs().get(job).await.expect("get");
        assert_eq!((record.state, record.result), (JobState::Claimed, None));
        sqlx::raw_sql("DROP TRIGGER fail_credential_write")
            .execute(&db.pool)
            .await
            .expect("drop trigger");
        // The runner's retry applies result and credential effects together.
        assert_eq!(
            db.jobs().complete(&limited, at(11)).await.expect("retry"),
            Completion::Applied {
                state: JobState::Paused
            }
        );
        let credential = db
            .llm_credentials()
            .get(CREDENTIAL)
            .await
            .expect("credential");
        assert_eq!(credential.paused_until, Some(at(900)));
        assert_eq!(credential.last_call, limited.llm_call);
        assert_eq!(
            db.jobs().complete(&limited, at(12)).await.expect("repeat"),
            Completion::AlreadyRecorded
        );
    }

    #[tokio::test]
    async fn unusable_credential_is_marked_even_when_cancel_wins() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let job = enqueue(&db, analysis, "f1", 0).await;
        register_credential(&db).await;
        claim(&db, 0).await.expect("claim");
        db.jobs().request_cancel(job, at(1)).await.expect("cancel");
        let failed = result(
            job,
            1,
            JobOutcome::Failed {
                kind: JobFailureKind::CredentialUnusable,
                message: "Sign in again.".to_owned(),
            },
        );
        assert_eq!(
            db.jobs().complete(&failed, at(5)).await.expect("complete"),
            Completion::Applied {
                state: JobState::Cancelled
            }
        );
        let credential = db
            .llm_credentials()
            .get(CREDENTIAL)
            .await
            .expect("credential");
        assert_eq!(credential.unusable_since, Some(at(5)));
        assert_eq!(credential.paused_until, None);
    }

    #[tokio::test]
    async fn cancel_wins_over_a_succeeded_result() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let job = enqueue(&db, analysis, "f1", 0).await;
        claim(&db, 0).await.expect("claim");
        db.jobs()
            .record_output(job, 1, &json!({"flows": []}), at(60))
            .await
            .expect("output");
        db.jobs().request_cancel(job, at(1)).await.expect("cancel");
        assert_eq!(
            db.jobs()
                .complete(&succeeded(job, 1), at(5))
                .await
                .expect("complete"),
            Completion::Applied {
                state: JobState::Cancelled
            }
        );
        let record = db.jobs().get(job).await.expect("get");
        assert_eq!(
            record.result.map(|r| r.outcome),
            Some(JobOutcome::Cancelled)
        );
        assert_eq!(record.ready_output, None);
    }

    #[tokio::test]
    async fn usage_limit_pauses_until_reset_then_requeues_with_a_new_attempt() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let job = enqueue(&db, analysis, "f1", 0).await;
        claim(&db, 0).await.expect("claim");
        let limited = result(job, 1, JobOutcome::UsageLimited { resets_at: None });
        assert_eq!(
            db.jobs()
                .complete(&limited, at(10))
                .await
                .expect("complete"),
            Completion::Applied {
                state: JobState::Paused
            }
        );
        let paused = db.jobs().get(job).await.expect("get");
        assert_eq!(paused.resumes_at, Some(at(10) + PAUSE_WITHOUT_RESET));
        assert_eq!(paused.finished_at, None);
        assert!(
            db.jobs()
                .resume_paused(at(10))
                .await
                .expect("too early")
                .is_empty()
        );
        assert_eq!(
            db.jobs()
                .resume_paused(at(10) + PAUSE_WITHOUT_RESET)
                .await
                .expect("resume"),
            [job]
        );
        let reclaimed = claim(&db, 4000).await.expect("claim");
        assert_eq!(reclaimed.attempt, 2);
        assert_eq!(reclaimed.result, None);
    }

    #[tokio::test]
    async fn user_pause_blocks_claim_after_usage_limit_expires_until_explicit_resume() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.unwrap();
        let job = enqueue(&db, analysis, "f1", 0).await;
        claim(&db, 0).await.unwrap();
        db.analyses()
            .pause_by_user(analysis, "alice", at(1))
            .await
            .unwrap();
        db.jobs()
            .complete(
                &result(
                    job,
                    1,
                    JobOutcome::UsageLimited {
                        resets_at: Some(at(100)),
                    },
                ),
                at(2),
            )
            .await
            .unwrap();
        // Resuming the user's pause before the provider deadline leaves the usage-limit pause.
        db.analyses().resume_by_user(analysis, at(3)).await.unwrap();
        assert!(matches!(
            db.analyses().get(analysis).await.unwrap().state,
            contracts::api::analyses::AnalysisState::Paused {
                reason: Some(contracts::api::analyses::PauseReason::UsageLimit),
                ..
            }
        ));
        db.analyses()
            .pause_by_user(analysis, "alice", at(4))
            .await
            .unwrap();
        db.jobs().resume_paused(at(100)).await.unwrap();
        assert!(claim(&db, 101).await.is_none());
        assert!(matches!(
            db.analyses().get(analysis).await.unwrap().state,
            contracts::api::analyses::AnalysisState::Paused {
                reason: Some(contracts::api::analyses::PauseReason::User),
                ..
            }
        ));
        db.analyses()
            .resume_by_user(analysis, at(102))
            .await
            .unwrap();
        assert_eq!(claim(&db, 103).await.unwrap().id, job);
    }

    #[tokio::test]
    async fn cancel_ends_queued_and_paused_jobs_at_once_and_leaves_finished_ones() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let queued = enqueue(&db, analysis, "f1", 0).await;
        assert_eq!(
            db.jobs()
                .request_cancel(queued, at(1))
                .await
                .expect("cancel"),
            JobState::Cancelled
        );
        let record = db.jobs().get(queued).await.expect("get");
        assert_eq!(
            record.result.map(|r| r.outcome),
            Some(JobOutcome::Cancelled)
        );
        assert_eq!(record.finished_at, Some(at(1)));
        assert_eq!(
            db.jobs()
                .request_cancel(queued, at(2))
                .await
                .expect("again"),
            JobState::Cancelled
        );
        let paused = enqueue(&db, analysis, "f2", 3).await;
        claim(&db, 4).await.expect("claim");
        db.jobs()
            .complete(
                &result(
                    paused,
                    1,
                    JobOutcome::UsageLimited {
                        resets_at: Some(at(900)),
                    },
                ),
                at(5),
            )
            .await
            .expect("pause");
        assert_eq!(
            db.jobs()
                .request_cancel(paused, at(6))
                .await
                .expect("cancel"),
            JobState::Cancelled
        );
        assert!(matches!(
            db.jobs().request_cancel(JobId::new_random(), at(7)).await,
            Err(DbError::NotFound)
        ));
    }

    #[tokio::test]
    async fn list_for_analysis_filters_by_state_in_creation_order() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let first = enqueue(&db, analysis, "f1", 0).await;
        let second = enqueue(&db, analysis, "f2", 1).await;
        db.jobs()
            .request_cancel(second, at(2))
            .await
            .expect("cancel");
        let all: Vec<JobId> = db
            .jobs()
            .list_for_analysis(analysis, &[])
            .await
            .expect("list")
            .iter()
            .map(|job| job.id)
            .collect();
        assert_eq!(all, [first, second]);
        let queued = db
            .jobs()
            .list_for_analysis(analysis, &[JobState::Queued, JobState::Running])
            .await
            .expect("list");
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, first);
    }

    #[test]
    fn stage_keys_round_trip_and_are_distinct() {
        let question = QuestionId::new_random();
        let stages = [
            JobStage::Plan { batch: 1 },
            JobStage::Plan { batch: 2 },
            JobStage::PrContext,
            JobStage::Flow(FlowId("f3".to_owned())),
            JobStage::Ask(question),
        ];
        let keys: Vec<String> = stages.iter().map(JobStage::key).collect();
        assert_eq!(keys[0], "plan:1");
        assert_eq!(keys[2], "pr_context");
        assert_eq!(keys[3], "flow:f3");
        assert_eq!(keys[4], format!("ask:{question}"));
        for (stage, key) in stages.iter().zip(&keys) {
            assert_eq!(JobStage::parse(key).as_ref(), Some(stage), "{key}");
        }
        for bad in ["", "plan", "plan:x", "flow:", "ask:not-a-uuid", "other"] {
            assert_eq!(JobStage::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn stage_kind_matches_job_kind() {
        assert_eq!(JobStage::Plan { batch: 1 }.kind(), JobKind::Plan);
        assert_eq!(JobStage::PrContext.kind(), JobKind::PrContext);
        assert_eq!(
            JobStage::Flow(FlowId("f1".to_owned())).kind(),
            JobKind::AnalyseFlow
        );
        assert_eq!(JobStage::Ask(QuestionId::new_random()).kind(), JobKind::Ask);
    }
}
