//! Repository for `jobs` and `job_events`: the queue behind `QueueExecutor` and the runner protocol.
//! Frozen signatures (cross-WP); bodies are filled in by WP2-JOBS.

use chrono::{DateTime, Utc};
use contracts::agent::JobKind;
use contracts::ids::{AnalysisId, FlowId, JobId, QuestionId};
use contracts::job::event::JobEvent;
use contracts::job::result::JobResult;
use contracts::job::spec::{JobPriority, JobSpec};
use contracts::job::state::JobState;
use sqlx::SqlitePool;

use crate::error::DbError;

/// Access to the job queue.
#[derive(Debug)]
pub struct Jobs<'a> {
    #[expect(dead_code, reason = "used once WP2-JOBS implements the operations")]
    pub(crate) pool: &'a SqlitePool,
}

/// A job to enqueue; the tenant is the analysis's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewJob {
    /// The spec; `access` must be empty (credentials are added at claim).
    pub spec: JobSpec,
    /// Claim priority.
    pub priority: JobPriority,
    /// The flow an `analyse_flow` job analyses.
    pub flow_id: Option<FlowId>,
    /// The question an `ask` job answers.
    pub question_id: Option<QuestionId>,
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
    /// The final result.
    pub result: Option<JobResult>,
    /// Flow of an `analyse_flow` job.
    pub flow_id: Option<FlowId>,
    /// Question of an `ask` job.
    pub question_id: Option<QuestionId>,
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

#[expect(
    unused_variables,
    reason = "signatures frozen by WP1-A; bodies are filled in by WP2-JOBS"
)]
impl Jobs<'_> {
    /// Enqueues a job in state `queued`.
    pub async fn enqueue(&self, job: NewJob, now: DateTime<Utc>) -> Result<JobRecord, DbError> {
        Err(DbError::NotImplemented {
            operation: "jobs::enqueue",
        })
    }

    /// Claims the next runnable job: `queued`, credential usable and idle, no other running job of the same
    /// analysis; interactive before batch, then oldest first. Increments the attempt.
    pub async fn claim(&self, params: ClaimParams<'_>) -> Result<Option<JobRecord>, DbError> {
        Err(DbError::NotImplemented {
            operation: "jobs::claim",
        })
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
        Err(DbError::NotImplemented {
            operation: "jobs::record_events",
        })
    }

    /// Stores the result of `result.attempt` and applies the matching transition; idempotent per attempt.
    pub async fn complete(
        &self,
        result: &JobResult,
        now: DateTime<Utc>,
    ) -> Result<Completion, DbError> {
        Err(DbError::NotImplemented {
            operation: "jobs::complete",
        })
    }

    /// Cancels a queued or paused job at once, or flags a claimed or running one; returns the state after.
    pub async fn request_cancel(
        &self,
        job: JobId,
        now: DateTime<Utc>,
    ) -> Result<JobState, DbError> {
        Err(DbError::NotImplemented {
            operation: "jobs::request_cancel",
        })
    }

    /// Re-queues claimed or running jobs whose lease ended (failing those without attempts left).
    pub async fn expire_leases(&self, now: DateTime<Utc>) -> Result<Vec<JobId>, DbError> {
        Err(DbError::NotImplemented {
            operation: "jobs::expire_leases",
        })
    }

    /// Re-queues paused jobs whose `resumes_at` has passed.
    pub async fn resume_paused(&self, now: DateTime<Utc>) -> Result<Vec<JobId>, DbError> {
        Err(DbError::NotImplemented {
            operation: "jobs::resume_paused",
        })
    }

    /// A job.
    pub async fn get(&self, job: JobId) -> Result<JobRecord, DbError> {
        Err(DbError::NotImplemented {
            operation: "jobs::get",
        })
    }

    /// The job-token hash of the current claim, for constant-time comparison by the server.
    pub async fn token_hash(&self, job: JobId) -> Result<Option<Vec<u8>>, DbError> {
        Err(DbError::NotImplemented {
            operation: "jobs::token_hash",
        })
    }

    /// Events of a job after `after_seq` of the current attempt, in order.
    pub async fn events(&self, job: JobId, after_seq: u32) -> Result<Vec<JobEvent>, DbError> {
        Err(DbError::NotImplemented {
            operation: "jobs::events",
        })
    }

    /// Keeps the encrypted installation token of the current claim so it can be revoked.
    pub async fn set_github_token(
        &self,
        job: JobId,
        ciphertext: &[u8],
        expires_at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "jobs::set_github_token",
        })
    }

    /// Removes and returns the encrypted installation token for revocation.
    pub async fn take_github_token(&self, job: JobId) -> Result<Option<Vec<u8>>, DbError> {
        Err(DbError::NotImplemented {
            operation: "jobs::take_github_token",
        })
    }
}
