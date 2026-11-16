//! `QueueExecutor`: jobs go on the database queue and are claimed by the long-lived runner. Followers poll the
//! database, so a job submitted before a server restart is followed the same way.

use std::collections::VecDeque;
use std::time::Duration;

use chrono::{DateTime, Utc};
use contracts::ids::JobId;
use contracts::job::state::JobState;
use db::error::DbError;
use db::repo::Db;
use db::repo::jobs::{ExpiredLease, NewJob};
use futures::future::BoxFuture;
use futures::stream::{self, BoxStream, StreamExt};

use crate::jobs::executor::{ExecutorError, JobExecutor, JobHandle, JobRequest, JobUpdate};

/// How often a follower reads the job's state and new events.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The v1 [`JobExecutor`]: the database queue behind the runner protocol.
#[derive(Clone, Debug)]
pub struct QueueExecutor {
    db: Db,
}

impl QueueExecutor {
    /// An executor over the database queue.
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

/// Ends expired leases (requeue, fail or cancel); run before claims and by followers of a job past its lease.
pub async fn expire_leases(db: &Db, now: DateTime<Utc>) -> Result<Vec<ExpiredLease>, DbError> {
    let expired = db.jobs().expire_leases(now).await?;
    for lease in &expired {
        tracing::warn!(
            job = %lease.job,
            state = lease.state.as_str(),
            "job lease expired without a runner heartbeat"
        );
    }
    // TODO(WP3-APPTOKEN): revoke the installation tokens listed by `Jobs::github_tokens_to_revoke` here.
    Ok(expired)
}

impl JobExecutor for QueueExecutor {
    fn submit(&self, request: JobRequest) -> BoxFuture<'_, Result<JobHandle, ExecutorError>> {
        Box::pin(async move {
            let analysis = request.spec.analysis_id;
            let stage = request.stage.clone();
            let generation = request.generation;
            // TODO(WP3-APPTOKEN): `request.pr_repo` selects the installation token minted at claim.
            let job = NewJob {
                spec: request.spec,
                priority: request.priority,
                stage: request.stage,
                generation,
            };
            match self.db.jobs().enqueue(job, Utc::now()).await {
                Ok(job) => Ok(JobHandle { job_id: job.id }),
                Err(DbError::Conflict) => self
                    .db
                    .jobs()
                    .list_for_analysis(analysis, &[])
                    .await?
                    .into_iter()
                    .rev()
                    .find(|job| {
                        job.stage == stage
                            && job.generation == generation
                            && !matches!(job.state, JobState::Failed | JobState::Cancelled)
                    })
                    .map(|job| JobHandle { job_id: job.id })
                    .ok_or(ExecutorError::Db(DbError::Conflict)),
                Err(error) => Err(error.into()),
            }
        })
    }

    fn events(&self, handle: &JobHandle) -> BoxStream<'static, Result<JobUpdate, ExecutorError>> {
        let follower = Follower {
            db: self.db.clone(),
            job: handle.job_id,
            attempt: 0,
            seq: 0,
            last_state: None,
            output_sent: false,
            pending: VecDeque::new(),
            done: false,
        };
        stream::unfold(follower, Follower::next).boxed()
    }

    fn cancel(&self, handle: &JobHandle) -> BoxFuture<'_, Result<(), ExecutorError>> {
        let job = handle.job_id;
        Box::pin(async move {
            match self.db.jobs().request_cancel(job, Utc::now()).await {
                Ok(state) => {
                    tracing::info!(%job, state = state.as_str(), "job cancellation requested");
                    Ok(())
                }
                Err(DbError::NotFound) => Err(ExecutorError::UnknownJob(job)),
                Err(error) => Err(error.into()),
            }
        })
    }
}

/// Turns successive reads of one job into [`JobUpdate`]s.
struct Follower {
    db: Db,
    job: JobId,
    attempt: u32,
    seq: u32,
    last_state: Option<JobState>,
    output_sent: bool,
    pending: VecDeque<JobUpdate>,
    done: bool,
}

fn is_active(state: Option<JobState>) -> bool {
    matches!(state, Some(JobState::Claimed | JobState::Running))
}

impl Follower {
    async fn next(mut self) -> Option<(Result<JobUpdate, ExecutorError>, Self)> {
        loop {
            if let Some(update) = self.pending.pop_front() {
                return Some((Ok(update), self));
            }
            if self.done {
                return None;
            }
            if let Err(error) = self.poll().await {
                self.done = true;
                return Some((Err(error), self));
            }
            if self.pending.is_empty() && !self.done {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    }

    async fn poll(&mut self) -> Result<(), ExecutorError> {
        let now = Utc::now();
        let mut job = self.read().await?;
        if self.last_state.is_none() {
            self.attempt = job.attempt;
        }
        if matches!(job.state, JobState::Claimed | JobState::Running)
            && job.lease_expires_at.is_some_and(|lease| lease < now)
        {
            expire_leases(&self.db, now).await?;
            // The follower saw the job active, so a requeue it caused is reported like any other.
            self.last_state.get_or_insert(job.state);
            job = self.read().await?;
        }
        if self.last_state.is_none() && job.state.is_terminal() {
            return self.finish(job.result);
        }
        let new_attempt = job.attempt != self.attempt;
        let requeued = (is_active(self.last_state)
            && (new_attempt || job.state == JobState::Queued))
            .then(|| JobUpdate::Requeued {
                attempt: if job.state == JobState::Queued {
                    job.attempt + 1
                } else {
                    job.attempt
                },
            });
        // Events of an ended attempt come before its requeue; events of a new attempt after it.
        if new_attempt {
            self.pending.extend(requeued.clone());
            self.attempt = job.attempt;
            self.seq = 0;
            self.output_sent = false;
        }
        for event in self.db.jobs().events(self.job, self.seq).await? {
            self.seq = event.seq;
            self.pending.push_back(JobUpdate::Event(event));
        }
        if !new_attempt {
            self.pending.extend(requeued);
        }
        if let Some(output) = job.ready_output.filter(|_| {
            !self.output_sent
                && !job.cancel_requested
                && matches!(job.state, JobState::Claimed | JobState::Running)
        }) {
            self.output_sent = true;
            self.pending.push_back(JobUpdate::OutputReady(output));
        }
        if job.state == JobState::Paused && self.last_state != Some(JobState::Paused) {
            self.pending.push_back(JobUpdate::Paused {
                resumes_at: job.resumes_at,
            });
        }
        self.last_state = Some(job.state);
        if job.state.is_terminal() {
            return self.finish(job.result);
        }
        Ok(())
    }

    async fn read(&self) -> Result<db::repo::jobs::JobRecord, ExecutorError> {
        match self.db.jobs().get(self.job).await {
            Ok(job) => Ok(job),
            Err(DbError::NotFound) => Err(ExecutorError::UnknownJob(self.job)),
            Err(error) => Err(error.into()),
        }
    }

    fn finish(
        &mut self,
        result: Option<contracts::job::result::JobResult>,
    ) -> Result<(), ExecutorError> {
        self.done = true;
        let result = result.ok_or(ExecutorError::Db(DbError::Corrupt {
            column: "jobs.result",
        }))?;
        self.pending.push_back(JobUpdate::Finished(result));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;
    use contracts::agent::JobKind;
    use contracts::common::RepoRef;
    use contracts::ids::FlowId;
    use contracts::job::event::{ActivityKind, JobEvent};
    use contracts::job::result::{JobOutcome, JobResult};
    use contracts::job::spec::JobPriority;
    use db::repo::analyses::testing::{CREDENTIAL, job_spec, seed_analysis};
    use db::repo::jobs::{ClaimParams, JobStage};
    use serde_json::json;

    use super::*;

    const WAIT: Duration = Duration::from_secs(10);

    fn request(analysis: contracts::ids::AnalysisId, flow: &str) -> JobRequest {
        JobRequest {
            spec: job_spec(analysis, JobKind::AnalyseFlow),
            priority: JobPriority::Batch,
            stage: JobStage::Flow(FlowId(flow.to_owned())),
            generation: 0,
            pr_repo: RepoRef {
                owner: "acme".to_owned(),
                name: "node-client".to_owned(),
            },
        }
    }

    async fn setup() -> (Db, QueueExecutor, contracts::ids::AnalysisId) {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        (db.clone(), QueueExecutor::new(db), analysis)
    }

    async fn claim(db: &Db, lease_secs: i64) {
        let now = Utc::now();
        db.jobs()
            .claim(ClaimParams {
                credential_ids: &[CREDENTIAL.to_owned()],
                job_token_hash: b"hash",
                lease_until: now + TimeDelta::seconds(lease_secs),
                now,
            })
            .await
            .expect("claim")
            .expect("a job");
    }

    fn event(seq: u32) -> JobEvent {
        JobEvent {
            seq,
            at: Utc::now(),
            activity: ActivityKind::AgentMessage,
            summary: format!("event {seq}"),
        }
    }

    fn succeeded(job: JobId, attempt: u32) -> JobResult {
        JobResult {
            job_id: job,
            attempt,
            outcome: JobOutcome::Succeeded {
                output: json!({"ok": true}),
            },
            session: None,
            usage: None,
            skills_commit: None,
            llm_call: None,
        }
    }

    async fn next(updates: &mut BoxStream<'static, Result<JobUpdate, ExecutorError>>) -> JobUpdate {
        tokio::time::timeout(WAIT, updates.next())
            .await
            .expect("an update in time")
            .expect("stream open")
            .expect("update")
    }

    #[tokio::test]
    async fn submit_is_idempotent_per_stage_and_generation() {
        let (_, executor, analysis) = setup().await;
        let first = executor
            .submit(request(analysis, "f1"))
            .await
            .expect("submit");
        let again = executor
            .submit(request(analysis, "f1"))
            .await
            .expect("resubmit");
        assert_eq!(first, again);
        let other = executor
            .submit(request(analysis, "f2"))
            .await
            .expect("submit");
        assert_ne!(first, other);
        executor.cancel(&first).await.expect("cancel");
        let retry = executor
            .submit(request(analysis, "f1"))
            .await
            .expect("retry");
        assert_ne!(first, retry, "a cancelled stage may be submitted again");
        assert!(matches!(
            executor
                .cancel(&JobHandle {
                    job_id: JobId::new_random()
                })
                .await,
            Err(ExecutorError::UnknownJob(_))
        ));
    }

    #[tokio::test]
    async fn events_follow_a_scripted_job_to_its_result() {
        let (db, executor, analysis) = setup().await;
        let handle = executor
            .submit(request(analysis, "f1"))
            .await
            .expect("submit");
        let job = handle.job_id;
        let mut updates = executor.events(&handle);
        claim(&db, 60).await;
        db.jobs()
            .record_events(
                job,
                1,
                &[event(1), event(2)],
                Utc::now() + TimeDelta::seconds(60),
            )
            .await
            .expect("events");
        assert!(matches!(next(&mut updates).await, JobUpdate::Event(e) if e.seq == 1));
        assert!(matches!(next(&mut updates).await, JobUpdate::Event(e) if e.seq == 2));
        db.jobs()
            .record_output(
                job,
                1,
                &json!({"ok": true}),
                Utc::now() + TimeDelta::seconds(60),
            )
            .await
            .expect("output");
        assert_eq!(
            next(&mut updates).await,
            JobUpdate::OutputReady(json!({"ok": true}))
        );
        db.jobs()
            .complete(&succeeded(job, 1), Utc::now())
            .await
            .expect("complete");
        assert_eq!(
            next(&mut updates).await,
            JobUpdate::Finished(succeeded(job, 1))
        );
        assert!(updates.next().await.is_none());

        let mut finished = executor.events(&handle);
        assert_eq!(
            next(&mut finished).await,
            JobUpdate::Finished(succeeded(job, 1)),
            "a finished job yields only its result"
        );
        assert!(finished.next().await.is_none());
    }

    #[tokio::test]
    async fn expired_lease_requeues_and_cancel_at_expiry_finishes_cancelled() {
        let (db, executor, analysis) = setup().await;
        let handle = executor
            .submit(request(analysis, "f1"))
            .await
            .expect("submit");
        let job = handle.job_id;
        let mut updates = executor.events(&handle);
        let after_lease = || Utc::now() + TimeDelta::seconds(61);
        claim(&db, 60).await;
        db.jobs()
            .record_events(job, 1, &[event(1)], Utc::now() + TimeDelta::seconds(60))
            .await
            .expect("events");
        assert!(matches!(next(&mut updates).await, JobUpdate::Event(_)));
        db.jobs()
            .expire_leases(after_lease())
            .await
            .expect("expire");
        assert_eq!(next(&mut updates).await, JobUpdate::Requeued { attempt: 2 });
        claim(&db, 60).await;
        executor.cancel(&handle).await.expect("cancel");
        db.jobs()
            .expire_leases(after_lease())
            .await
            .expect("expire");
        let JobUpdate::Finished(result) = next(&mut updates).await else {
            panic!("expected the end of the job");
        };
        assert_eq!(result.outcome, JobOutcome::Cancelled);
        assert_eq!(result.attempt, 2);
        assert!(updates.next().await.is_none());
    }

    #[tokio::test]
    async fn follower_expires_a_lease_it_sees_passed() {
        let (db, executor, analysis) = setup().await;
        let handle = executor
            .submit(request(analysis, "f1"))
            .await
            .expect("submit");
        let mut updates = executor.events(&handle);
        claim(&db, 60).await;
        db.jobs()
            .record_events(
                handle.job_id,
                1,
                &[event(1)],
                Utc::now() - TimeDelta::seconds(1),
            )
            .await
            .expect("events with a lease already over");
        assert!(matches!(next(&mut updates).await, JobUpdate::Event(_)));
        assert_eq!(next(&mut updates).await, JobUpdate::Requeued { attempt: 2 });
        assert_eq!(
            db.jobs().get(handle.job_id).await.expect("get").state,
            JobState::Queued
        );
    }

    #[tokio::test]
    async fn unknown_job_ends_the_stream_with_an_error() {
        let (_, executor, _) = setup().await;
        let mut updates = executor.events(&JobHandle {
            job_id: JobId::new_random(),
        });
        let first = tokio::time::timeout(WAIT, updates.next())
            .await
            .expect("in time")
            .expect("one item");
        assert!(matches!(first, Err(ExecutorError::UnknownJob(_))));
        assert!(updates.next().await.is_none());
    }
}
