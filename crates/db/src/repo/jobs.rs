//! Repository for `jobs` and `job_events`: the queue behind `QueueExecutor` and the runner protocol.
//! Frozen signatures (cross-WP); bodies are filled in by WP2-JOBS.

use chrono::{DateTime, Utc};
use contracts::agent::JobKind;
use contracts::ids::{AnalysisId, FlowId, JobId, QuestionId};
use contracts::job::event::JobEvent;
use contracts::job::result::JobResult;
use contracts::job::spec::{JobPriority, JobSpec};
use contracts::job::state::JobState;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::error::DbError;

/// Access to the job queue.
#[derive(Debug)]
pub struct Jobs<'a> {
    #[expect(dead_code, reason = "used once WP2-JOBS implements the operations")]
    pub(crate) pool: &'a SqlitePool,
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
    /// The output posted before compaction for the current attempt (provisional; the result is authoritative).
    pub ready_output: Option<Value>,
    /// The final result.
    pub result: Option<JobResult>,
    /// The pipeline stage.
    pub stage: JobStage,
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

/// Outcome of recording the output posted before compaction.
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

#[expect(
    unused_variables,
    reason = "signatures frozen by WP1-A; bodies are filled in by WP2-JOBS"
)]
impl Jobs<'_> {
    /// Enqueues a job in state `queued`; `Conflict` if a live or succeeded job of the same stage exists (look it
    /// up with [`Jobs::list_for_analysis`] and reuse it).
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

    /// Stores the output of `attempt` posted before compaction and renews the lease. A different output for the
    /// same attempt, a stale attempt or a job that is not running is a `Conflict`.
    pub async fn record_output(
        &self,
        job: JobId,
        attempt: u32,
        output: &Value,
        lease_until: DateTime<Utc>,
    ) -> Result<OutputRecorded, DbError> {
        Err(DbError::NotImplemented {
            operation: "jobs::record_output",
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

    /// Ends the lease of claimed or running jobs whose lease passed: `cancelled` when cancel was requested (never
    /// re-claimed), else `queued`, or `failed` without attempts left. Clears `ready_output` and the job token
    /// hash; the caller revokes the installation token (`take_github_token`) and ends the executor stream.
    pub async fn expire_leases(&self, now: DateTime<Utc>) -> Result<Vec<ExpiredLease>, DbError> {
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

    /// Jobs of an analysis, oldest first; only those in `states` unless `states` is empty.
    pub async fn list_for_analysis(
        &self,
        analysis: AnalysisId,
        states: &[JobState],
    ) -> Result<Vec<JobRecord>, DbError> {
        Err(DbError::NotImplemented {
            operation: "jobs::list_for_analysis",
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

#[cfg(test)]
mod tests {
    use super::*;

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
