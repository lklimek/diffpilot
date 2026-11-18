//! The `JobExecutor` seam used by the pipeline and Ask (brief §5 Job execution interface, rule A3). v1 has one
//! implementation, `QueueExecutor`; a later `ContainerExecutor` starts a container per job.

use std::fmt::Debug;

use chrono::{DateTime, Utc};
use contracts::common::RepoRef;
use contracts::ids::{AnalysisId, JobId};
use contracts::job::event::JobEvent;
use contracts::job::result::JobResult;
use contracts::job::spec::{JobPriority, JobSpec};
use db::repo::jobs::JobStage;
use futures::future::BoxFuture;
use futures::stream::BoxStream;

/// Runs agent jobs. The server never assumes a long-lived runner or a filesystem shared with it.
pub trait JobExecutor: Send + Sync + Debug {
    /// Submits a job; it runs when an executor slot (v1: a runner claim) takes it. Idempotent per stage and
    /// generation: when a live or succeeded job of that (stage, generation) exists, its handle is returned.
    fn submit(&self, request: JobRequest) -> BoxFuture<'_, Result<JobHandle, ExecutorError>>;

    /// Updates of a job from now on, ending with [`JobUpdate::Finished`]; a job that already finished yields
    /// only that. Works for jobs submitted before a server restart.
    fn events(&self, handle: &JobHandle) -> BoxStream<'static, Result<JobUpdate, ExecutorError>>;

    /// Requests cancellation; the job ends with a `cancelled` result unless it finished first.
    fn cancel(&self, handle: &JobHandle) -> BoxFuture<'_, Result<(), ExecutorError>>;

    /// Requests cancellation of every unfinished job of `analysis`, including jobs submitted before a server
    /// restart; used when the analysis ends.
    fn cancel_analysis(&self, analysis: AnalysisId) -> BoxFuture<'_, Result<(), ExecutorError>>;
}

/// A job to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobRequest {
    /// The self-contained spec; `access` is empty and filled in when the job starts.
    pub spec: JobSpec,
    /// Ask jobs are interactive and start before batch jobs.
    pub priority: JobPriority,
    /// The pipeline stage (`flow:<id>`, `ask:<question>`, …); one live or succeeded job per stage, so a
    /// repeated submit after a restart fails with a conflict instead of duplicating work.
    pub stage: JobStage,
    /// 0 for the first run; 1..=`MAX_REPAIR_RUNS` for a repair run after verification rejected the output of
    /// the previous generation. Idempotency is per (stage, generation).
    pub generation: u32,
    /// The pull request's repository: the only repository an installation token is minted for (ADR-005); no
    /// token when the GitHub App is not installed on it.
    pub pr_repo: RepoRef,
}

/// Handle of a submitted job; persisted by callers, so it survives a restart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JobHandle {
    /// The job.
    pub job_id: JobId,
}

/// One update of a running job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobUpdate {
    /// Agent activity.
    Event(JobEvent),
    /// The final output arrived before session archival and runner cleanup (provisional: publish it as ready, persist only on
    /// [`JobUpdate::Finished`], which carries the same output or supersedes it).
    OutputReady(serde_json::Value),
    /// Usage limit reached; the job waits and is queued again later.
    Paused {
        /// When it is queued again, if known.
        resumes_at: Option<DateTime<Utc>>,
    },
    /// A lease expired; the job is queued again as `attempt`.
    Requeued {
        /// The next attempt number.
        attempt: u32,
    },
    /// The job ended (succeeded, failed or cancelled). A job cancelled at lease expiry without a runner result
    /// ends with a server-built `cancelled` result.
    Finished {
        /// The result.
        result: JobResult,
        /// The attempt that produced the result ran with a GitHub installation token (set when it started), so
        /// its agent could read GitHub.
        github_access: bool,
    },
}

/// A job could not be submitted, followed or cancelled.
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    /// The user selected different flow work before this job was enqueued.
    #[error("flow work was reprioritised")]
    Superseded,
    /// The job queue's storage failed.
    #[error("the job queue is not available")]
    Db(#[from] db::error::DbError),
    /// No such job.
    #[error("job {0} does not exist")]
    UnknownJob(JobId),
    /// The executor is shutting down.
    #[error("the job executor is shutting down")]
    Closed,
}
