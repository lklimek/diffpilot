//! Repository for `jobs` and `job_events`: the queue behind `QueueExecutor` and the runner protocol.
//! Frozen signatures (cross-WP).

use contracts::agent::JobKind;
use contracts::ids::{AnalysisId, FlowId, JobId, PassId, QuestionId, RefreshId};
use contracts::job::access::AgentAccess;
use contracts::job::event::JobEvent;
use contracts::job::result::{JobFailureKind, JobOutcome, JobResult, LlmCallStatus};
use contracts::job::spec::{JobPriority, JobSpec, JobTarget};
use contracts::job::state::{JobState, JobTransition};
use contracts::time::Timestamp;
use jiff::SignedDuration;
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
    pub expires_at: Timestamp,
}

/// The stage a job serves: the durable link from analysis progress (or a dashboard ranking) to its job (restart
/// recovery). At most one job per (analysis or refresh, stage) is live or succeeded, so a repeated submit is a
/// `Conflict`.
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
    /// A `judge_work` batch of a dashboard refresh's ranking, numbered from 1.
    Judge {
        /// The refresh.
        refresh: RefreshId,
        /// Batch number.
        batch: u32,
    },
    /// The `verify_comments` job of an assessment pass (ADR-023).
    Verify(PassId),
}

impl JobStage {
    /// The stored key: `plan:<batch>`, `pr_context`, `flow:<id>`, `ask:<question id>`, `judge:<refresh>:<batch>`,
    /// `verify:<pass>`.
    pub fn key(&self) -> String {
        match self {
            Self::Plan { batch } => format!("plan:{batch}"),
            Self::PrContext => "pr_context".to_owned(),
            Self::Flow(flow) => format!("flow:{flow}"),
            Self::Ask(question) => format!("ask:{question}"),
            Self::Judge { refresh, batch } => format!("judge:{refresh}:{batch}"),
            Self::Verify(pass) => format!("verify:{pass}"),
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
            "verify" => serde_json::from_value(serde_json::Value::String(rest.to_owned()))
                .ok()
                .map(Self::Verify),
            "judge" => {
                let (refresh, batch) = rest.split_once(':')?;
                Some(Self::Judge {
                    refresh: serde_json::from_value(serde_json::Value::String(refresh.to_owned()))
                        .ok()?,
                    batch: batch.parse().ok()?,
                })
            }
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
            Self::Judge { .. } => JobKind::JudgeWork,
            Self::Verify(_) => JobKind::VerifyComments,
        }
    }
}

/// A job to enqueue; the tenant is the analysis's, the refresh's or the pass's (`spec.target`).
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
    /// Analysis of an analysis job; none for dashboard jobs.
    pub analysis_id: Option<AnalysisId>,
    /// Refresh of a dashboard job; none for other jobs.
    pub refresh_id: Option<RefreshId>,
    /// Assessment pass of a `verify_comments` job; none for other jobs.
    pub pass_id: Option<PassId>,
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
    pub lease_expires_at: Option<Timestamp>,
    /// When a paused job is queued again.
    pub resumes_at: Option<Timestamp>,
    /// The output posted before archival for the current attempt (provisional; the result is authoritative).
    pub ready_output: Option<Value>,
    /// The final result.
    pub result: Option<JobResult>,
    /// The pipeline stage.
    pub stage: JobStage,
    /// 0 for the first run, else the repair run number.
    pub generation: u32,
    /// The analysis's round (or the refresh's ranking attempt) when it was enqueued; an explicit Retry starts a new
    /// round.
    pub round: u32,
    /// The last claim gave the job a GitHub installation token (its agent had the GitHub tool).
    pub github_access_granted: bool,
    /// When it was enqueued.
    pub created_at: Timestamp,
    /// When it reached a terminal state.
    pub finished_at: Option<Timestamp>,
}

/// What a runner claim needs from the queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimParams<'p> {
    /// Usable credential ids the runner holds.
    pub credential_ids: &'p [String],
    /// Hash of the new job token.
    pub job_token_hash: &'p [u8],
    /// End of the lease.
    pub lease_until: Timestamp,
    /// Now.
    pub now: Timestamp,
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
    pub lease_expires_at: Option<Timestamp>,
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
    pub credential_paused_until: Option<Timestamp>,
}

/// Events kept per job: `job_events` is a bounded activity log.
const EVENTS_KEPT: i64 = 500;

/// How long a usage-limited job waits when the provider gave no reset time (ADR-003).
pub const PAUSE_WITHOUT_RESET: SignedDuration = SignedDuration::from_mins(30);

/// Result text of a job that failed because no claim could prepare its context.
const CONTEXT_UNPREPARED_MESSAGE: &str =
    "the job's session or context pack could not be prepared at any claim";

/// Message of the server-built result of a job whose leases expired on every attempt.
const RUNNER_LOST_MESSAGE: &str =
    "The runner stopped reporting progress on every attempt, so the job was given up.";

/// Fixed-width RFC 3339 UTC text, so stored timestamps compare correctly as text.
pub(crate) fn ts(at: Timestamp) -> String {
    format!("{:.6}", at.get())
}

/// Parses a stored timestamp.
pub(crate) fn parse_ts(value: &str, column: &'static str) -> Result<Timestamp, DbError> {
    value
        .parse::<Timestamp>()
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
        "id, analysis_id, refresh_id, pass_id, kind, priority, state, attempt, max_attempts, spec, cancel_requested, lease_expires_at,
         resumes_at, ready_output, result, stage, generation, round, github_access_granted, created_at, finished_at"
    };
}

fn optional_ts(row: &SqliteRow, column: &'static str) -> Result<Option<Timestamp>, DbError> {
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
    let analysis: Option<String> = row.try_get("analysis_id")?;
    let refresh: Option<String> = row.try_get("refresh_id")?;
    let pass: Option<String> = row.try_get("pass_id")?;
    let kind: String = row.try_get("kind")?;
    let state: String = row.try_get("state")?;
    let spec: String = row.try_get("spec")?;
    let stage: String = row.try_get("stage")?;
    let created_at: String = row.try_get("created_at")?;
    Ok(JobRecord {
        id: parse_wire(&id, "jobs.id")?,
        analysis_id: analysis
            .map(|id| parse_wire(&id, "jobs.analysis_id"))
            .transpose()?,
        refresh_id: refresh
            .map(|id| parse_wire(&id, "jobs.refresh_id"))
            .transpose()?,
        pass_id: pass.map(|id| parse_wire(&id, "jobs.pass_id")).transpose()?,
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
    now: Timestamp,
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
    /// Enqueues a job in state `queued` in the analysis's current round (a dashboard job: the refresh's ranking
    /// attempt); `Conflict` if a live or succeeded job of the same stage, generation and round exists (look it up
    /// with [`Jobs::current_for_stage`], [`Jobs::current_for_judge_stage`] or [`Jobs::current_for_pass`] and reuse
    /// it). `NotFound` when the target's analysis does not exist, its refresh does not exist or no longer ranks, or
    /// its pass does not exist or ended (a job that could never be claimed).
    pub async fn enqueue(&self, job: NewJob, now: Timestamp) -> Result<JobRecord, DbError> {
        let mut spec = job.spec;
        // The stored spec never holds a credential (threat model T3b); access is filled at claim.
        spec.access = AgentAccess::default();
        spec.attempt = 0;
        let (flow_id, question_id) = match &job.stage {
            JobStage::Flow(flow) => (Some(flow.0.clone()), None),
            JobStage::Ask(question) => (None, Some(question.to_string())),
            JobStage::Plan { .. }
            | JobStage::PrContext
            | JobStage::Judge { .. }
            | JobStage::Verify(_) => (None, None),
        };
        let (sql, owner) = match &spec.target {
            JobTarget::PullRequest(target) => (
                "INSERT INTO jobs (id, tenant_id, analysis_id, kind, priority, state, spec, credential_id, stage,
                     generation, flow_id, question_id, created_at, round)
                 SELECT ?, tenant_id, id, ?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, round FROM analyses WHERE id = ?",
                target.analysis_id.to_string(),
            ),
            JobTarget::Dashboard { refresh_id } => (
                "INSERT INTO jobs (id, tenant_id, refresh_id, kind, priority, state, spec, credential_id, stage,
                     generation, flow_id, question_id, created_at, round)
                 SELECT ?, tenant_id, id, ?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, ranking_attempt
                 FROM dashboard_refreshes WHERE id = ? AND state = 'ranking'",
                refresh_id.to_string(),
            ),
            JobTarget::PullRequestComments(target) => (
                "INSERT INTO jobs (id, tenant_id, pass_id, kind, priority, state, spec, credential_id, stage,
                     generation, flow_id, question_id, created_at, round)
                 SELECT ?, tenant_id, id, ?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, 0
                 FROM comment_passes WHERE id = ? AND state IN ('queued', 'running')",
                target.pass_id.to_string(),
            ),
        };
        let inserted = sqlx::query(sql)
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
            .bind(owner)
            .execute(self.pool)
            .await?;
        if inserted.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        self.get(spec.job_id).await
    }

    /// Claims the next runnable job: `queued`, credential usable and idle, no other running job of the same
    /// analysis, a dashboard job only while its refresh is ranking in the job's attempt, a `verify_comments` job only
    /// while its pass is live; interactive before batch, Ask first and `verify_comments` second within interactive,
    /// then oldest first. Never pre-empts a claimed or running job. Increments the attempt.
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
                 AND NOT EXISTS (SELECT 1 FROM dashboard_refreshes d WHERE d.id = j.refresh_id AND (d.state <> 'ranking' OR d.ranking_attempt <> j.round))
                 AND NOT EXISTS (SELECT 1 FROM comment_passes p WHERE p.id = j.pass_id AND p.state NOT IN ('queued', 'running'))
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
        query.push(
            ")) ORDER BY j.priority, j.kind <> 'ask', j.kind <> 'verify_comments', j.created_at, j.rowid LIMIT 1)
             RETURNING id",
        );
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
        lease_until: Timestamp,
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

    /// Stores the output of `attempt` posted before session archival, moves `claimed` to `running` and renews the lease.
    /// A different output for the same attempt, a stale attempt or a job that is not claimed or running is a
    /// `Conflict`.
    pub async fn record_output(
        &self,
        job: JobId,
        attempt: u32,
        output: &Value,
        lease_until: Timestamp,
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
        now: Timestamp,
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
        let kind: String = row.try_get("kind")?;
        // A dashboard or `verify_comments` job has no session: nothing it reports touches one (ADR-018, ADR-023).
        if kind == JobKind::JudgeWork.as_str() || kind == JobKind::VerifyComments.as_str() {
            tx.commit().await?;
            return Ok(Completion::Applied { state });
        }
        // Publish the reported analytic or private working checkpoint only for the first effective,
        // not cancelled result: a repeated or late result never restores an older session. A session already asked
        // to be dropped waits for its deletion acknowledgement before another job can start.
        if let (Some(session), false) = (
            &stored.session,
            matches!(stored.outcome, JobOutcome::Cancelled),
        ) {
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
    pub async fn request_cancel(&self, job: JobId, now: Timestamp) -> Result<JobState, DbError> {
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
    pub async fn expire_leases(&self, now: Timestamp) -> Result<Vec<ExpiredLease>, DbError> {
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
    pub async fn resume_paused(&self, now: Timestamp) -> Result<Vec<JobId>, DbError> {
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
        now: Timestamp,
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

    /// The live or succeeded job of an assessment pass (`JobStage::Verify`), if any: what an idempotent submit of a
    /// `verify_comments` job answers with.
    pub async fn current_for_pass(
        &self,
        pass: PassId,
        generation: u32,
    ) -> Result<Option<JobRecord>, DbError> {
        let row = sqlx::query(concat!(
            "SELECT ",
            job_columns!(),
            " FROM jobs WHERE pass_id = ? AND stage = ? AND generation = ? AND state NOT IN ('failed', 'cancelled')
             ORDER BY created_at DESC, rowid DESC LIMIT 1"
        ))
        .bind(pass.to_string())
        .bind(JobStage::Verify(pass).key())
        .bind(generation)
        .fetch_optional(self.pool)
        .await?;
        row.as_ref().map(job_from_row).transpose()
    }

    /// The latest job of an assessment pass (`JobStage::Verify`) in any state, ended ones too: what recovers a pass
    /// whose link to its job was lost.
    pub async fn latest_for_pass(
        &self,
        pass: PassId,
        generation: u32,
    ) -> Result<Option<JobRecord>, DbError> {
        let row = sqlx::query(concat!(
            "SELECT ",
            job_columns!(),
            " FROM jobs WHERE pass_id = ? AND stage = ? AND generation = ?
             ORDER BY created_at DESC, rowid DESC LIMIT 1"
        ))
        .bind(pass.to_string())
        .bind(JobStage::Verify(pass).key())
        .bind(generation)
        .fetch_optional(self.pool)
        .await?;
        row.as_ref().map(job_from_row).transpose()
    }

    /// The live or succeeded job of a judge `stage` and `generation` in the refresh's current ranking attempt, if
    /// any: what an idempotent submit of a dashboard job answers with.
    pub async fn current_for_judge_stage(
        &self,
        refresh: RefreshId,
        stage: &JobStage,
        generation: u32,
    ) -> Result<Option<JobRecord>, DbError> {
        let row = sqlx::query(concat!(
            "SELECT ",
            job_columns!(),
            " FROM jobs j WHERE refresh_id = ? AND stage = ? AND generation = ?
                 AND state NOT IN ('failed', 'cancelled')
                 AND round = (SELECT ranking_attempt FROM dashboard_refreshes WHERE id = j.refresh_id)
             ORDER BY created_at DESC, rowid DESC LIMIT 1"
        ))
        .bind(refresh.to_string())
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

    /// Whether a claimed or running job holds `credential`.
    pub async fn credential_busy(&self, credential: &str) -> Result<bool, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM jobs WHERE credential_id = ? AND state IN ('claimed', 'running'))",
        )
        .bind(credential)
        .fetch_one(self.pool)
        .await?)
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

    /// Jobs of a dashboard refresh, oldest first; only those in `states` unless `states` is empty.
    pub async fn list_for_refresh(
        &self,
        refresh: RefreshId,
        states: &[JobState],
    ) -> Result<Vec<JobRecord>, DbError> {
        let mut query = QueryBuilder::<Sqlite>::new(concat!(
            "SELECT ",
            job_columns!(),
            " FROM jobs WHERE refresh_id = "
        ));
        query.push_bind(refresh.to_string());
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
        expires_at: Timestamp,
    ) -> Result<(), DbError> {
        let inserted = sqlx::query(
            "INSERT INTO installation_tokens (job_id, tenant_id, attempt, ciphertext, expires_at)
             SELECT id, tenant_id, ?, ?, ? FROM jobs WHERE id = ?",
        )
        .bind(attempt)
        .bind(ciphertext)
        .bind(expires_at.to_rfc3339_offset())
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
                    expires_at: expires_at.parse().map_err(|_| DbError::Corrupt {
                        column: "installation_tokens.expires_at",
                    })?,
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
    use contracts::job::access::{McpHttpEndpoint, Secret};
    use contracts::job::event::ActivityKind;
    use contracts::job::result::LlmCallOutcome;
    use contracts::job::spec::{CredentialKind, LlmCredentialRef};
    use serde_json::json;

    use super::*;
    use crate::repo::Db;
    use crate::repo::analyses::testing::{
        CREDENTIAL, job_spec, judge_spec, seed_analysis, seed_pass, seed_ranking_refresh,
        verify_spec,
    };

    fn at(second: i64) -> Timestamp {
        "2026-11-16T12:00:00Z"
            .parse::<Timestamp>()
            .unwrap_or_default()
            + SignedDuration::from_secs(second)
    }

    async fn db() -> Db {
        Db::in_memory().await.expect("in-memory database")
    }

    /// Stored text is fixed-width microseconds with `Z`, truncated; rows written by the running stack read back.
    #[test]
    fn stored_timestamps_keep_their_text_form() {
        for (stored, rewritten) in [
            ("2026-11-16T12:00:00Z", "2026-11-16T12:00:00.000000Z"),
            ("2026-11-16T12:00:00.000000Z", "2026-11-16T12:00:00.000000Z"),
            ("2026-11-16T12:00:00.120000Z", "2026-11-16T12:00:00.120000Z"),
            ("2026-11-16T12:00:00.123456Z", "2026-11-16T12:00:00.123456Z"),
            (
                "2026-11-16T12:00:00.123456789Z",
                "2026-11-16T12:00:00.123456Z",
            ),
            (
                "2026-11-16T12:00:00.999999999Z",
                "2026-11-16T12:00:00.999999Z",
            ),
            ("2026-11-16T14:00:00.5+02:00", "2026-11-16T12:00:00.500000Z"),
            (
                "2026-11-16T12:00:00.123456789+00:00",
                "2026-11-16T12:00:00.123456Z",
            ),
        ] {
            let at = parse_ts(stored, "test").expect("stored text parses");
            assert_eq!(ts(at), rewritten, "from {stored}");
        }
        for corrupt in ["", "2026-11-16 12:00", "2026-11-16T12:00:00"] {
            assert!(
                matches!(
                    parse_ts(corrupt, "c"),
                    Err(DbError::Corrupt { column: "c" })
                ),
                "{corrupt:?}"
            );
        }
    }

    /// Installation token expiries keep their RFC 3339 `+00:00` text with the fewest of 0, 3, 6 or 9 digits.
    #[tokio::test]
    async fn installation_token_expiry_keeps_its_offset_text_form() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let job = enqueue(&db, analysis, "f1", 0).await;
        let cases = [
            (1, "2026-11-16T19:00:00Z", "2026-11-16T19:00:00+00:00"),
            (
                2,
                "2026-11-16T19:00:00.120Z",
                "2026-11-16T19:00:00.120+00:00",
            ),
            (
                3,
                "2026-11-16T19:00:00.123450Z",
                "2026-11-16T19:00:00.123450+00:00",
            ),
            (
                4,
                "2026-11-16T19:00:00.123456789Z",
                "2026-11-16T19:00:00.123456789+00:00",
            ),
        ];
        for (attempt, expires_at, stored) in cases {
            let at = expires_at.parse().expect("time");
            db.jobs()
                .record_github_token(job, attempt, b"ct", at)
                .await
                .expect("record");
            let text: String = sqlx::query_scalar(
                "SELECT expires_at FROM installation_tokens WHERE job_id = ? AND attempt = ?",
            )
            .bind(job.to_string())
            .bind(attempt)
            .fetch_one(&db.pool)
            .await
            .expect("stored");
            assert_eq!(text, stored);
        }
        let listed = db.jobs().github_tokens_to_revoke().await.expect("list");
        let expiries: Vec<_> = listed.iter().map(|t| t.expires_at).collect();
        let expected: Vec<Timestamp> = cases
            .iter()
            .map(|(_, at, _)| at.parse().expect("time"))
            .collect();
        assert_eq!(expiries, expected, "stored text reads back");
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
        sqlx::query("INSERT INTO analyses (id, tenant_id, pull_request_id, base_sha, head_sha, language, state, created_by, created_at, background_analysis) SELECT ?, tenant_id, pull_request_id, base_sha, 'new-head', 'pl', 'planning', created_by, created_at, background_analysis FROM analyses WHERE id = ?")
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
    async fn released_claim_of_a_job_flagged_for_cancel_ends_cancelled() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let job = enqueue(&db, analysis, "f1", 0).await;
        claim(&db, 0).await.expect("claim");
        assert_eq!(
            db.jobs().request_cancel(job, at(1)).await.expect("cancel"),
            JobState::Claimed,
            "a claimed job is only flagged"
        );
        assert_eq!(
            db.jobs()
                .release_claim(job, 1, at(2))
                .await
                .expect("release"),
            Some(JobState::Cancelled)
        );
        let record = db.jobs().get(job).await.expect("get");
        assert_eq!(record.state, JobState::Cancelled);
        assert_eq!(
            record.result.map(|r| r.outcome),
            Some(JobOutcome::Cancelled)
        );
        assert_eq!(claim(&db, 100).await, None, "never re-claimed");
        assert_eq!(
            db.jobs().release_claim(job, 1, at(3)).await.expect("again"),
            None,
            "no longer claimed: nothing changes"
        );
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
        let pass = PassId::new_random();
        let refresh = RefreshId::new_random();
        let stages = [
            JobStage::Plan { batch: 1 },
            JobStage::Plan { batch: 2 },
            JobStage::PrContext,
            JobStage::Flow(FlowId("f3".to_owned())),
            JobStage::Ask(question),
            JobStage::Judge { refresh, batch: 2 },
            JobStage::Verify(pass),
        ];
        let keys: Vec<String> = stages.iter().map(JobStage::key).collect();
        assert_eq!(keys[6], format!("verify:{pass}"));
        assert_eq!(JobStage::Verify(pass).kind(), JobKind::VerifyComments);
        assert_eq!(keys[0], "plan:1");
        assert_eq!(keys[2], "pr_context");
        assert_eq!(keys[3], "flow:f3");
        assert_eq!(keys[4], format!("ask:{question}"));
        assert_eq!(keys[5], format!("judge:{refresh}:2"));
        for (stage, key) in stages.iter().zip(&keys) {
            assert_eq!(JobStage::parse(key).as_ref(), Some(stage), "{key}");
        }
        let bad_judge = [
            "judge:".to_owned(),
            format!("judge:{refresh}"),
            format!("judge:{refresh}:x"),
            "judge:not-a-uuid:1".to_owned(),
        ];
        for bad in bad_judge {
            assert_eq!(JobStage::parse(&bad), None, "{bad}");
        }
        for bad in [
            "",
            "plan",
            "plan:x",
            "flow:",
            "ask:not-a-uuid",
            "verify:",
            "verify:x",
            "other",
        ] {
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
        assert_eq!(
            JobStage::Judge {
                refresh: RefreshId::new_random(),
                batch: 1
            }
            .kind(),
            JobKind::JudgeWork
        );
    }

    fn judge_job(refresh: RefreshId, batch: u32) -> NewJob {
        NewJob {
            spec: judge_spec(refresh),
            priority: JobPriority::Interactive,
            stage: JobStage::Judge { refresh, batch },
            generation: 0,
        }
    }

    #[tokio::test]
    async fn judge_job_belongs_to_its_refresh_and_is_idempotent_per_ranking_attempt() {
        let db = db().await;
        let refresh = seed_ranking_refresh(&db).await.expect("refresh");
        let record = db
            .jobs()
            .enqueue(judge_job(refresh, 1), at(0))
            .await
            .expect("enqueue");
        assert_eq!(
            (record.analysis_id, record.refresh_id, record.kind),
            (None, Some(refresh), JobKind::JudgeWork)
        );
        assert_eq!(record.round, 0, "the refresh's ranking attempt");
        assert!(matches!(
            db.jobs().enqueue(judge_job(refresh, 1), at(1)).await,
            Err(DbError::Conflict)
        ));
        let stage = JobStage::Judge { refresh, batch: 1 };
        assert_eq!(
            db.jobs()
                .current_for_judge_stage(refresh, &stage, 0)
                .await
                .expect("current")
                .map(|job| job.id),
            Some(record.id)
        );
        sqlx::query("UPDATE dashboard_refreshes SET ranking_attempt = 1 WHERE id = ?")
            .bind(refresh.to_string())
            .execute(&db.pool)
            .await
            .expect("retry ranking");
        assert_eq!(
            db.jobs()
                .current_for_judge_stage(refresh, &stage, 0)
                .await
                .expect("current"),
            None,
            "a new ranking attempt starts fresh"
        );
        let again = db
            .jobs()
            .enqueue(judge_job(refresh, 1), at(2))
            .await
            .expect("same batch in the next attempt");
        assert_eq!(again.round, 1);
        let listed: Vec<JobId> = db
            .jobs()
            .list_for_refresh(refresh, &[JobState::Queued])
            .await
            .expect("list")
            .iter()
            .map(|job| job.id)
            .collect();
        assert_eq!(listed, [record.id, again.id]);
        assert!(matches!(
            db.jobs()
                .enqueue(judge_job(RefreshId::new_random(), 1), at(3))
                .await,
            Err(DbError::NotFound)
        ));
        sqlx::query(
            "UPDATE dashboard_refreshes SET state = 'finished', ranking_state = 'cancelled', finished_at = 't'
             WHERE id = ?",
        )
        .bind(refresh.to_string())
        .execute(&db.pool)
        .await
        .expect("ranking ended");
        assert!(
            matches!(
                db.jobs().enqueue(judge_job(refresh, 2), at(4)).await,
                Err(DbError::NotFound)
            ),
            "no job for a ranking that ended"
        );
    }

    #[tokio::test]
    async fn judge_job_is_claimed_only_while_its_refresh_ranks() {
        let db = db().await;
        let refresh = seed_ranking_refresh(&db).await.expect("refresh");
        let job = db
            .jobs()
            .enqueue(judge_job(refresh, 1), at(0))
            .await
            .expect("enqueue")
            .id;
        sqlx::query(
            "UPDATE dashboard_refreshes SET state = 'finished', ranking_state = 'cancelled', finished_at = 't'
             WHERE id = ?",
        )
        .bind(refresh.to_string())
        .execute(&db.pool)
        .await
        .expect("superseded");
        assert_eq!(
            claim(&db, 1).await,
            None,
            "a superseded ranking runs no job"
        );
        sqlx::query("UPDATE dashboard_refreshes SET state = 'ranking', ranking_state = 'running', finished_at = NULL WHERE id = ?")
            .bind(refresh.to_string())
            .execute(&db.pool)
            .await
            .expect("ranking again");
        let claimed = claim(&db, 2).await.expect("claimed");
        assert_eq!(claimed.id, job);
        assert_eq!(claimed.spec.target.pull_request(), None);
    }

    /// A restart interrupts the ranking and leaves its queued judge job queued; Retry ranking starts the next
    /// attempt, which runs only its own jobs.
    #[tokio::test]
    async fn judge_job_of_a_superseded_ranking_attempt_is_never_claimed() {
        let db = db().await;
        let refresh = seed_ranking_refresh(&db).await.expect("refresh");
        let stale = db
            .jobs()
            .enqueue(judge_job(refresh, 1), at(0))
            .await
            .expect("enqueue")
            .id;
        db.dashboard().interrupt_live(at(1)).await.expect("restart");
        let restarted = db
            .dashboard()
            .restart_ranking(crate::repo::DEFAULT_TENANT, "u1", refresh)
            .await
            .expect("retry ranking");
        assert!(
            matches!(
                restarted,
                crate::repo::dashboard::RestartOutcome::Restarted(_)
            ),
            "{restarted:?}"
        );
        assert_eq!(
            claim(&db, 2).await,
            None,
            "the first attempt's job stays unclaimed"
        );
        let current = db
            .jobs()
            .enqueue(judge_job(refresh, 1), at(3))
            .await
            .expect("enqueue again")
            .id;
        assert_ne!(current, stale, "a new attempt has its own job");
        assert_eq!(claim(&db, 4).await.expect("claimed").id, current);
    }

    #[tokio::test]
    async fn judge_job_and_analysis_job_never_run_together_on_one_credential() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let refresh = seed_ranking_refresh(&db).await.expect("refresh");
        enqueue(&db, analysis, "f1", 0).await;
        db.jobs()
            .enqueue(judge_job(refresh, 1), at(1))
            .await
            .expect("enqueue");
        let first = claim(&db, 2).await.expect("interactive judge job first");
        assert_eq!(first.kind, JobKind::JudgeWork);
        assert_eq!(claim(&db, 3).await, None, "the credential is busy");
    }

    fn ask_job(analysis: AnalysisId) -> NewJob {
        NewJob {
            spec: job_spec(analysis, JobKind::Ask),
            priority: JobPriority::Interactive,
            stage: JobStage::Ask(QuestionId::new_random()),
            generation: 0,
        }
    }

    async fn enqueue_new(db: &Db, job: NewJob, second: i64) -> JobId {
        db.jobs()
            .enqueue(job, at(second))
            .await
            .expect("enqueue")
            .id
    }

    #[tokio::test]
    async fn claim_takes_ask_then_ranking_then_pipeline_oldest_first_within_each() {
        let db = db().await;
        let pipeline_analysis = seed_analysis(&db).await.expect("seed");
        let ask_analysis = seed_analysis(&db).await.expect("seed");
        let refresh = seed_ranking_refresh(&db).await.expect("refresh");
        let pipeline = enqueue(&db, pipeline_analysis, "f1", 0).await;
        let older_ranking = enqueue_new(&db, judge_job(refresh, 1), 1).await;
        let newer_ranking = enqueue_new(&db, judge_job(refresh, 2), 2).await;
        let older_ask = enqueue_new(&db, ask_job(ask_analysis), 3).await;
        let newer_ask = enqueue_new(&db, ask_job(ask_analysis), 4).await;

        let mut order = Vec::new();
        for second in 10..15 {
            let job = claim(&db, second).await.expect("a job");
            db.jobs()
                .complete(&succeeded(job.id, 1), at(second))
                .await
                .expect("complete");
            order.push(job.id);
        }
        assert_eq!(
            order,
            [older_ask, newer_ask, older_ranking, newer_ranking, pipeline]
        );
    }

    #[tokio::test]
    async fn ask_never_preempts_a_running_ranking_job() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let refresh = seed_ranking_refresh(&db).await.expect("refresh");
        let ranking = enqueue_new(&db, judge_job(refresh, 1), 0).await;
        assert_eq!(claim(&db, 1).await.map(|job| job.id), Some(ranking));
        db.jobs()
            .record_events(ranking, 1, &[event(1)], at(61))
            .await
            .expect("running");
        let running = db.jobs().get(ranking).await.expect("job");
        let ask = enqueue_new(&db, ask_job(analysis), 2).await;
        enqueue(&db, analysis, "f1", 3).await;

        assert_eq!(claim(&db, 4).await, None, "no pre-emption");
        assert_eq!(
            db.jobs().get(ranking).await.expect("job"),
            running,
            "the running job is untouched"
        );
        db.jobs()
            .complete(&succeeded(ranking, 1), at(5))
            .await
            .expect("complete");
        assert_eq!(claim(&db, 6).await.map(|job| job.id), Some(ask));
    }

    #[tokio::test]
    async fn credential_busy_only_while_a_job_is_claimed_or_running() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        let jobs = db.jobs();
        let ask = enqueue_new(&db, ask_job(analysis), 0).await;
        assert!(
            !jobs.credential_busy(CREDENTIAL).await.expect("busy"),
            "queued only"
        );
        assert_eq!(claim(&db, 1).await.map(|job| job.id), Some(ask));
        assert!(
            jobs.credential_busy(CREDENTIAL).await.expect("busy"),
            "claimed"
        );
        assert!(
            !jobs.credential_busy("other").await.expect("busy"),
            "per credential"
        );
        jobs.record_events(ask, 1, &[event(1)], at(61))
            .await
            .expect("running");
        assert!(
            jobs.credential_busy(CREDENTIAL).await.expect("busy"),
            "running"
        );
        jobs.complete(&succeeded(ask, 1), at(2))
            .await
            .expect("complete");
        assert!(
            !jobs.credential_busy(CREDENTIAL).await.expect("busy"),
            "finished"
        );
    }

    /// Judge jobs leave the analysis-job rules as they were: per-PR, per-analysis and cancelled-analysis
    /// exclusions still hold, and a running judge job blocks only its own credential.
    #[tokio::test]
    async fn analysis_job_exclusions_unchanged() {
        let db = db().await;
        let cancelled = seed_analysis(&db).await.expect("seed");
        enqueue(&db, cancelled, "f1", -1).await;
        db.analyses()
            .set_state(
                cancelled,
                &contracts::api::analyses::AnalysisState::Cancelled,
                at(0),
            )
            .await
            .expect("cancel");
        let analysis = seed_analysis(&db).await.expect("seed");
        let refresh = seed_ranking_refresh(&db).await.expect("refresh");
        let mut judge = judge_job(refresh, 1);
        judge.spec.credential.id = "spare".to_owned();
        db.jobs().enqueue(judge, at(0)).await.expect("judge job");
        let first = enqueue(&db, analysis, "f1", 1).await;
        enqueue(&db, analysis, "f2", 2).await;
        let both = [CREDENTIAL.to_owned(), "spare".to_owned()];
        let jobs = db.jobs();
        let claim_on = |now: i64| {
            jobs.claim(ClaimParams {
                credential_ids: &both,
                job_token_hash: b"hash",
                lease_until: at(now + 60),
                now: at(now),
            })
        };
        let judging = claim_on(3).await.expect("claim").expect("judge job");
        assert_eq!(judging.kind, JobKind::JudgeWork);
        let analysing = claim_on(4).await.expect("claim").expect("analysis job");
        assert_eq!(
            analysing.id, first,
            "another credential is free; the cancelled analysis's older job is skipped"
        );
        assert_eq!(
            claim_on(5).await.expect("claim"),
            None,
            "one running job per analysis and per credential"
        );
    }

    #[tokio::test]
    async fn judge_result_never_touches_a_pull_request_session() {
        let db = db().await;
        let analysis = seed_analysis(&db).await.expect("seed");
        db.analyses()
            .set_session(analysis, "analytic", None)
            .await
            .expect("session");
        let generation = db.agent_sessions().generation(1).await.expect("generation");
        let refresh = seed_ranking_refresh(&db).await.expect("refresh");
        let job = db
            .jobs()
            .enqueue(judge_job(refresh, 1), at(0))
            .await
            .expect("enqueue")
            .id;
        claim(&db, 1).await.expect("claimed");
        let mut reported = succeeded(job, 1);
        reported.session = Some(contracts::job::result::SessionReport {
            thread_id: "judge-thread".to_owned(),
            outcome: contracts::job::result::SessionOutcome::Started,
        });
        assert_eq!(
            db.jobs()
                .complete(&reported, at(2))
                .await
                .expect("complete"),
            Completion::Applied {
                state: JobState::Succeeded
            }
        );
        let record = db.analyses().get(analysis).await.expect("analysis");
        assert_eq!(record.agent_thread_id.as_deref(), Some("analytic"));
        assert_eq!(
            db.agent_sessions().generation(1).await.expect("generation"),
            generation
        );
    }

    fn verify_job(pass: PassId) -> NewJob {
        NewJob {
            spec: verify_spec(pass),
            priority: JobPriority::Interactive,
            stage: JobStage::Verify(pass),
            generation: 0,
        }
    }

    /// A `verify_comments` job belongs to its pass only, is idempotent per pass and cannot be enqueued for an ended
    /// pass.
    #[tokio::test]
    async fn verify_job_belongs_to_its_pass_and_is_idempotent() {
        let db = db().await;
        let pass = seed_pass(&db).await.expect("pass");
        let record = db
            .jobs()
            .enqueue(verify_job(pass), at(0))
            .await
            .expect("enqueue");
        assert_eq!(
            (
                record.analysis_id,
                record.refresh_id,
                record.pass_id,
                record.kind
            ),
            (None, None, Some(pass), JobKind::VerifyComments)
        );
        assert!(matches!(
            db.jobs().enqueue(verify_job(pass), at(1)).await,
            Err(DbError::Conflict)
        ));
        let current = db.jobs().current_for_pass(pass, 0).await.expect("current");
        assert_eq!(current.map(|job| job.id), Some(record.id));
        db.review_loop()
            .finish_pass(pass, false, at(2))
            .await
            .expect("ended");
        db.jobs()
            .request_cancel(record.id, at(3))
            .await
            .expect("cancel");
        assert!(matches!(
            db.jobs().enqueue(verify_job(pass), at(4)).await,
            Err(DbError::NotFound)
        ));
    }

    /// Claimed after Ask and before dashboard ranking, only while the pass is live.
    #[tokio::test]
    async fn verify_job_is_claimed_after_ask_before_ranking_while_its_pass_is_live() {
        let db = db().await;
        let pass = seed_pass(&db).await.expect("pass");
        let analysis = seed_analysis(&db).await.expect("analysis");
        let refresh = seed_ranking_refresh(&db).await.expect("refresh");
        let judge = enqueue_new(&db, judge_job(refresh, 1), 0).await;
        let verify = enqueue_new(&db, verify_job(pass), 1).await;
        let ask = enqueue_new(&db, ask_job(analysis), 2).await;
        let order = [ask, verify, judge];
        for (second, expected) in (3..).zip(order) {
            let claimed = claim(&db, second).await.expect("claimed");
            assert_eq!(claimed.id, expected);
            db.jobs()
                .complete(&succeeded(claimed.id, 1), at(second))
                .await
                .expect("complete");
        }
        let second_pass = {
            db.review_loop()
                .finish_pass(pass, true, at(10))
                .await
                .expect("finished");
            seed_pass(&db).await.expect("next pass")
        };
        enqueue_new(&db, verify_job(second_pass), 11).await;
        db.review_loop()
            .finish_pass(second_pass, false, at(12))
            .await
            .expect("failed");
        assert_eq!(
            claim(&db, 13).await,
            None,
            "an ended pass's job waits unclaimed"
        );
    }

    #[tokio::test]
    async fn verify_result_never_touches_a_pull_request_session() {
        let db = db().await;
        let pass = seed_pass(&db).await.expect("pass");
        let generation = db.agent_sessions().generation(1).await.expect("generation");
        let job = enqueue_new(&db, verify_job(pass), 0).await;
        claim(&db, 1).await.expect("claimed");
        let mut reported = succeeded(job, 1);
        reported.session = Some(contracts::job::result::SessionReport {
            thread_id: "verify-thread".to_owned(),
            outcome: contracts::job::result::SessionOutcome::Started,
        });
        db.jobs()
            .complete(&reported, at(2))
            .await
            .expect("complete");
        assert_eq!(
            db.agent_sessions().generation(1).await.expect("generation"),
            generation
        );
        let thread: Option<String> =
            sqlx::query_scalar("SELECT agent_thread_id FROM pull_requests WHERE id = 1")
                .fetch_one(&db.pool)
                .await
                .expect("thread");
        assert_eq!(thread, None);
    }
}
