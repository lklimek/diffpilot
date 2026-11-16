//! `QueueExecutor`: jobs go on the database queue and are claimed by the long-lived runner. Bodies are filled
//! in by WP2-JOBS.

use futures::future::{self, BoxFuture};
use futures::stream::{self, BoxStream, StreamExt};

use crate::jobs::executor::{ExecutorError, JobExecutor, JobHandle, JobRequest, JobUpdate};

/// The v1 [`JobExecutor`]: the database queue behind the runner protocol.
#[derive(Clone, Debug)]
pub struct QueueExecutor {
    #[expect(dead_code, reason = "used once WP2-JOBS implements the queue")]
    db: db::repo::Db,
}

impl QueueExecutor {
    /// An executor over the database queue.
    pub fn new(db: db::repo::Db) -> Self {
        Self { db }
    }
}

impl JobExecutor for QueueExecutor {
    fn submit(&self, _request: JobRequest) -> BoxFuture<'_, Result<JobHandle, ExecutorError>> {
        Box::pin(future::ready(Err(ExecutorError::NotImplemented {
            operation: "QueueExecutor::submit",
        })))
    }

    fn events(&self, _handle: &JobHandle) -> BoxStream<'static, Result<JobUpdate, ExecutorError>> {
        stream::once(future::ready(Err(ExecutorError::NotImplemented {
            operation: "QueueExecutor::events",
        })))
        .boxed()
    }

    fn cancel(&self, _handle: &JobHandle) -> BoxFuture<'_, Result<(), ExecutorError>> {
        Box::pin(future::ready(Err(ExecutorError::NotImplemented {
            operation: "QueueExecutor::cancel",
        })))
    }
}
