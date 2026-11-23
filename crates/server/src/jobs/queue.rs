//! `QueueExecutor`: jobs go on the database queue and are claimed by the long-lived runner. Followers poll the
//! database, so a job submitted before a server restart is followed the same way.

use std::collections::VecDeque;
use std::time::Duration;

use contracts::ids::{AnalysisId, JobId, PassId, RefreshId};
use contracts::job::spec::JobTarget;
use contracts::job::state::JobState;
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::Db;
use db::repo::analyses::SessionState;
use db::repo::jobs::{ExpiredLease, JobSnapshot, JobStage, NewJob};
use futures::future::BoxFuture;
use futures::stream::{self, BoxStream, StreamExt};

use crate::jobs::executor::{ExecutorError, JobExecutor, JobHandle, JobRequest, JobUpdate};
use crate::jobs::progress::ProgressHub;

/// How often a follower reads the job's state and new events.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The v1 [`JobExecutor`]: the database queue behind the runner protocol.
#[derive(Clone, Debug)]
pub struct QueueExecutor {
    db: Db,
    progress: ProgressHub,
}

impl QueueExecutor {
    /// An executor over the database queue; its followers publish stage changes to `progress`.
    pub fn new(db: Db, progress: ProgressHub) -> Self {
        Self { db, progress }
    }
}

impl QueueExecutor {
    /// Enqueues a payer-owned PR Ask once per question, independently of analysis and assessment passes.
    async fn submit_private_ask(&self, request: JobRequest) -> Result<JobHandle, ExecutorError> {
        let JobStage::Ask(question) = request.stage else {
            return Err(DbError::Conflict.into());
        };
        let payer = request.spec.payer.clone();
        let generation = request.generation;
        let current = || async {
            Ok::<_, ExecutorError>(
                self.db
                    .jobs()
                    .current_for_private_question(question, &payer, generation)
                    .await?
                    .map(|job| JobHandle { job_id: job.id }),
            )
        };
        if let Some(handle) = current().await? {
            return Ok(handle);
        }
        let mut spec = request.spec;
        let saved = self.db.settings().llm_selection().await?;
        spec.model = crate::model::select_model(
            spec.kind,
            &crate::model::ModelContext::default(),
            saved.as_ref(),
        );
        match self
            .db
            .jobs()
            .enqueue_linked(
                NewJob {
                    spec,
                    priority: request.priority,
                    stage: request.stage,
                    generation,
                },
                Timestamp::now(),
            )
            .await
        {
            Ok(job) => Ok(JobHandle { job_id: job.id }),
            Err(DbError::Conflict) => current().await?.ok_or(ExecutorError::Db(DbError::Conflict)),
            Err(error) => Err(error.into()),
        }
    }

    /// Enqueues a dashboard job: idempotent per (refresh, stage, generation) in the refresh's ranking attempt, with
    /// the model from `select_model`; no analysis to serialise on, pause or supersede.
    async fn submit_dashboard(
        &self,
        refresh: RefreshId,
        request: JobRequest,
    ) -> Result<JobHandle, ExecutorError> {
        let (stage, generation) = (request.stage.clone(), request.generation);
        let current = || async {
            Ok::<_, ExecutorError>(
                self.db
                    .jobs()
                    .current_for_judge_stage(refresh, &stage, generation)
                    .await?
                    .map(|job| JobHandle { job_id: job.id }),
            )
        };
        if let Some(existing) = current().await? {
            return Ok(existing);
        }
        let mut job = NewJob {
            spec: request.spec,
            priority: request.priority,
            stage: request.stage,
            generation,
        };
        let saved = self.db.settings().llm_selection().await?;
        job.spec.model = crate::model::select_model(
            job.spec.kind,
            &crate::model::ModelContext::default(),
            saved.as_ref(),
        );
        match self.db.jobs().enqueue_linked(job, Timestamp::now()).await {
            Ok(job) => Ok(JobHandle { job_id: job.id }),
            Err(DbError::Conflict) => current().await?.ok_or(ExecutorError::Db(DbError::Conflict)),
            Err(error) => Err(error.into()),
        }
    }
}

impl QueueExecutor {
    /// Enqueues the `verify_comments` job of an assessment pass: idempotent per pass, with the model from
    /// `select_model`; no analysis to serialise on, pause or supersede (ADR-023).
    async fn submit_verify(
        &self,
        pass: PassId,
        request: JobRequest,
    ) -> Result<JobHandle, ExecutorError> {
        let generation = request.generation;
        let current = || async {
            Ok::<_, ExecutorError>(
                self.db
                    .jobs()
                    .current_for_pass(pass, generation)
                    .await?
                    .map(|job| JobHandle { job_id: job.id }),
            )
        };
        if let Some(existing) = current().await? {
            return Ok(existing);
        }
        let mut job = NewJob {
            spec: request.spec,
            priority: request.priority,
            stage: request.stage,
            generation,
        };
        let saved = self.db.settings().llm_selection().await?;
        job.spec.model = crate::model::select_model(
            job.spec.kind,
            &crate::model::ModelContext::default(),
            saved.as_ref(),
        );
        match self.db.jobs().enqueue(job, Timestamp::now()).await {
            Ok(job) => Ok(JobHandle { job_id: job.id }),
            Err(DbError::Conflict) => current().await?.ok_or(ExecutorError::Db(DbError::Conflict)),
            Err(error) => Err(error.into()),
        }
    }
}

/// Ends expired leases (requeue, fail or cancel); run before claims and by followers of a job past its lease.
pub async fn expire_leases(db: &Db, now: Timestamp) -> Result<Vec<ExpiredLease>, DbError> {
    let expired = db.jobs().expire_leases(now).await?;
    for lease in &expired {
        tracing::warn!(
            job = %lease.job,
            state = lease.state.as_str(),
            "job lease expired without a runner heartbeat"
        );
    }
    // Their installation tokens are revoked by the `github_tokens` worker.
    Ok(expired)
}

impl JobExecutor for QueueExecutor {
    fn submit(&self, request: JobRequest) -> BoxFuture<'_, Result<JobHandle, ExecutorError>> {
        Box::pin(async move {
            tracing::info!(job = %request.spec.job_id, skills = ?request.spec.skills, "job agent skills requested");
            let analysis = match &request.spec.target {
                JobTarget::PullRequest(target) => target.analysis_id,
                JobTarget::Dashboard { refresh_id } => {
                    return self.submit_dashboard(*refresh_id, request).await;
                }
                JobTarget::PullRequestAsk(_) => return self.submit_private_ask(request).await,
                JobTarget::PullRequestComments(target) => {
                    return self.submit_verify(target.pass_id, request).await;
                }
            };
            let stage = request.stage.clone();
            let generation = request.generation;
            let job = NewJob {
                spec: request.spec,
                priority: request.priority,
                stage: request.stage,
                generation,
            };

            loop {
                let handle = self
                    .progress
                    .serialise(analysis, || async {
                        if let Some(existing) = self
                            .db
                            .jobs()
                            .current_for_stage(analysis, &stage, generation)
                            .await?
                        {
                            return Ok::<_, ExecutorError>(Some(JobHandle {
                                job_id: existing.id,
                            }));
                        }
                        let current = self.db.analyses().get(analysis).await?;
                        if !matches!(stage, db::repo::jobs::JobStage::Ask(_))
                            && matches!(
                                current.state,
                                contracts::api::analyses::AnalysisState::Paused {
                                    reason: Some(contracts::api::analyses::PauseReason::User),
                                    ..
                                }
                            )
                        {
                            return Ok(None);
                        }
                        if matches!(
                            current.state,
                            contracts::api::analyses::AnalysisState::Cancelled
                        ) {
                            return Err(DbError::Conflict.into());
                        }
                        if let db::repo::jobs::JobStage::Flow(flow) = &stage
                            && !self.db.flow_work().admitted(analysis, flow).await?
                        {
                            return Err(ExecutorError::Superseded);
                        }
                        let mut job = job.clone();
                        if !matches!(stage, JobStage::Ask(_)) {
                            job.spec.payer = contracts::ids::UserId(current.payer_id);
                        }
                        let saved = self.db.settings().llm_selection().await?;
                        job.spec.model = crate::model::select_model(
                            job.spec.kind,
                            &crate::model::ModelContext::default(),
                            saved.as_ref(),
                        );
                        match self.db.jobs().enqueue_linked(job, Timestamp::now()).await {
                            Ok(job) => Ok(Some(JobHandle { job_id: job.id })),
                            Err(DbError::Conflict) => self
                                .db
                                .jobs()
                                .current_for_stage(analysis, &stage, generation)
                                .await?
                                .map(|job| Some(JobHandle { job_id: job.id }))
                                .ok_or(ExecutorError::Db(DbError::Conflict)),
                            Err(error) => Err(error.into()),
                        }
                    })
                    .await?;
                if let Some(handle) = handle {
                    return Ok(handle);
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
    }

    fn events(&self, handle: &JobHandle) -> BoxStream<'static, Result<JobUpdate, ExecutorError>> {
        stream::unfold(
            Follower::new(self.db.clone(), self.progress.clone(), handle.job_id),
            Follower::next,
        )
        .boxed()
    }

    fn cancel(&self, handle: &JobHandle) -> BoxFuture<'_, Result<(), ExecutorError>> {
        let job = handle.job_id;
        Box::pin(async move {
            match self.db.jobs().request_cancel(job, Timestamp::now()).await {
                Ok(state) => {
                    tracing::info!(%job, state = state.as_str(), "job cancellation requested");
                    Ok(())
                }
                Err(DbError::NotFound) => Err(ExecutorError::UnknownJob(job)),
                Err(error) => Err(error.into()),
            }
        })
    }

    fn cancel_analysis(&self, analysis: AnalysisId) -> BoxFuture<'_, Result<(), ExecutorError>> {
        Box::pin(async move {
            let unfinished = self
                .db
                .jobs()
                .list_for_analysis(
                    analysis,
                    &[
                        JobState::Queued,
                        JobState::Claimed,
                        JobState::Running,
                        JobState::Paused,
                    ],
                )
                .await?;
            // Questions do not depend on failed pipeline work: their jobs end only with the analysis's results
            // (Cancel) or with the PR session (explicit removal).
            let record = self.db.analyses().get(analysis).await?;
            let questions_too = matches!(
                record.state,
                contracts::api::analyses::AnalysisState::Cancelled
            ) || matches!(
                record.session_state,
                SessionState::DropRequested | SessionState::Dropped
            );
            for job in &unfinished {
                if job.kind == contracts::agent::JobKind::Ask && !questions_too {
                    continue;
                }
                let state = self
                    .db
                    .jobs()
                    .request_cancel(job.id, Timestamp::now())
                    .await?;
                tracing::info!(%analysis, job = %job.id, state = state.as_str(), "job of the ended analysis cancelled");
            }
            Ok(())
        })
    }
}

/// Turns successive reads of one job into [`JobUpdate`]s.
struct Follower {
    db: Db,
    progress: ProgressHub,
    job: JobId,
    attempt: u32,
    seq: u32,
    last_state: Option<JobState>,
    output_sent: bool,
    /// The credential pause end last reported for the queued job.
    reported_pause: Option<Timestamp>,
    pending: VecDeque<JobUpdate>,
    done: bool,
    /// Runs after the follower's read and before it acts on it, to force interleavings in tests.
    #[cfg(test)]
    between_reads: Option<tests::Hook>,
}

fn is_active(state: Option<JobState>) -> bool {
    matches!(state, Some(JobState::Claimed | JobState::Running))
}

impl Follower {
    fn new(db: Db, progress: ProgressHub, job: JobId) -> Self {
        Self {
            db,
            progress,
            job,
            attempt: 0,
            seq: 0,
            last_state: None,
            output_sent: false,
            reported_pause: None,
            pending: VecDeque::new(),
            done: false,
            #[cfg(test)]
            between_reads: None,
        }
    }

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
        let now = Timestamp::now();
        let mut snapshot = self.snapshot().await?;
        if self.last_state.is_none() {
            self.attempt = snapshot.job.attempt;
        }
        if matches!(snapshot.job.state, JobState::Claimed | JobState::Running)
            && snapshot
                .job
                .lease_expires_at
                .is_some_and(|lease| lease < now)
        {
            expire_leases(&self.db, now).await?;
            // The follower saw the job active, so a requeue it caused is reported like any other.
            self.last_state.get_or_insert(snapshot.job.state);
            snapshot = self.snapshot().await?;
        }
        // State, ready output and events come from one read, so they belong to one attempt and one moment.
        let JobSnapshot {
            job,
            events,
            credential_paused_until,
        } = snapshot;
        // Every state change of the job (submit, claim, requeue, pause, resume, end) can change the analysis's
        // stage: the one place it is published for all of them.
        // A dashboard job's ranking progress is published by its submitter from these updates.
        if let Some(analysis) = job.analysis_id
            && (self.last_state != Some(job.state) || job.attempt != self.attempt)
        {
            self.publish_stage(analysis).await;
        }
        if self.last_state.is_none() && job.state.is_terminal() {
            return self.finish(job.result, job.github_access_granted);
        }
        #[cfg(test)]
        if let Some(hook) = &self.between_reads {
            hook().await;
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
        for event in events {
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
        // A queued job waits while its credential is paused, whichever job (any analysis, any round) hit the
        // usage limit: reported once per pause end.
        let credential_pause = credential_paused_until.filter(|until| *until > now);
        if job.state == JobState::Queued
            && credential_pause.is_some()
            && credential_pause != self.reported_pause
        {
            self.reported_pause = credential_pause;
            self.pending.push_back(JobUpdate::Paused {
                resumes_at: credential_pause,
            });
        }
        self.last_state = Some(job.state);
        if job.state.is_terminal() {
            return self.finish(job.result, job.github_access_granted);
        }
        Ok(())
    }

    /// Publishes the analysis's summary with its current stage; a failure only delays it to the next event.
    async fn publish_stage(&self, analysis: AnalysisId) {
        if let Err(error) = self.progress.publish_analysis(&self.db, analysis).await {
            tracing::warn!(%analysis, job = %self.job, ?error,
                "the stage change of a job could not be published to the progress stream");
        }
    }

    async fn snapshot(&self) -> Result<JobSnapshot, ExecutorError> {
        match self
            .db
            .jobs()
            .snapshot(self.job, self.attempt, self.seq)
            .await
        {
            Ok(snapshot) => Ok(snapshot),
            Err(DbError::NotFound) => Err(ExecutorError::UnknownJob(self.job)),
            Err(error) => Err(error.into()),
        }
    }

    fn finish(
        &mut self,
        result: Option<contracts::job::result::JobResult>,
        github_access: bool,
    ) -> Result<(), ExecutorError> {
        self.done = true;
        let result = result.ok_or(ExecutorError::Db(DbError::Corrupt {
            column: "jobs.result",
        }))?;
        self.pending.push_back(JobUpdate::Finished {
            result,
            github_access,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use contracts::agent::JobKind;
    use contracts::common::RepoRef;
    use contracts::ids::FlowId;
    use contracts::job::event::{ActivityKind, JobEvent};
    use contracts::job::result::{JobOutcome, JobResult};
    use contracts::job::spec::JobPriority;
    use db::repo::analyses::testing::{CREDENTIAL, job_spec, seed_analysis};
    use db::repo::jobs::{ClaimParams, JobStage};
    use jiff::SignedDuration;
    use serde_json::json;

    use super::*;

    const WAIT: Duration = Duration::from_secs(10);

    fn request(analysis: contracts::ids::AnalysisId, flow: &str) -> JobRequest {
        JobRequest {
            spec: job_spec(analysis, JobKind::AnalyseFlow),
            priority: JobPriority::Batch,
            stage: JobStage::Flow(FlowId(flow.to_owned())),
            generation: 0,
            pr_repo: Some(RepoRef {
                owner: "acme".to_owned(),
                name: "node-client".to_owned(),
            }),
        }
    }

    async fn setup() -> (Db, QueueExecutor, contracts::ids::AnalysisId) {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        crate::test_support::link_payer(&db, db::repo::DEFAULT_TENANT, CREDENTIAL).await;
        (
            db.clone(),
            QueueExecutor::new(db, ProgressHub::default()),
            analysis,
        )
    }

    #[tokio::test]
    async fn submitted_job_logs_requested_skill_names() {
        let capture = crate::test_support::logs::LogCapture::start();
        let (_, executor, analysis) = setup().await;
        let mut request = request(analysis, "f1");
        request.spec.skills = vec!["severity".to_owned()];
        let job = request.spec.job_id;
        executor.submit(request).await.unwrap();
        let logs = capture.text();
        assert!(logs.contains("job agent skills requested"), "{logs}");
        assert!(
            logs.contains("INFO") && logs.contains("severity") && logs.contains(&job.to_string()),
            "{logs}"
        );
    }

    async fn claim(db: &Db, lease_secs: i64) {
        let now = Timestamp::now();
        db.jobs()
            .claim(ClaimParams {
                payer_ids: &[CREDENTIAL.to_owned()],
                job_token_hash: b"hash",
                lease_until: now + SignedDuration::from_secs(lease_secs),
                now,
            })
            .await
            .expect("claim")
            .expect("a job");
    }

    fn event(seq: u32) -> JobEvent {
        JobEvent {
            seq,
            at: Timestamp::now(),
            activity: ActivityKind::AgentMessage,
            summary: format!("event {seq}"),
        }
    }

    fn succeeded(job: JobId, attempt: u32) -> JobResult {
        JobResult {
            assessment: None,
            report: None,
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

    async fn fail_analysis(db: &Db, analysis: contracts::ids::AnalysisId) {
        let failed = contracts::api::analyses::AnalysisState::Failed {
            reason: None,
            flow: None,
            message: "x".to_owned(),
        };
        db.analyses()
            .set_state(analysis, &failed, Timestamp::now())
            .await
            .expect("failed");
    }

    #[tokio::test]
    async fn latest_retry_payer_overrides_an_analytic_request_prepared_before_retry() {
        let (db, executor, analysis) = setup().await;
        let prepared = request(analysis, "f1");
        db::repo::analyses::testing::seed_payer(&db, "u2")
            .await
            .unwrap();
        crate::test_support::link_payer(&db, db::repo::DEFAULT_TENANT, "u2").await;
        fail_analysis(&db, analysis).await;
        assert!(
            db.analyses()
                .retry_as(analysis, "u2", Timestamp::now())
                .await
                .unwrap()
        );
        let job = executor.submit(prepared).await.unwrap();
        assert_eq!(db.jobs().get(job.job_id).await.unwrap().spec.payer.0, "u2");
    }

    #[tokio::test]
    async fn retry_round_submits_new_jobs_for_succeeded_stages() {
        let (db, executor, analysis) = setup().await;
        let first = executor
            .submit(request(analysis, "f1"))
            .await
            .expect("submit");
        claim(&db, 60).await;
        db.jobs()
            .complete(&succeeded(first.job_id, 1), Timestamp::now())
            .await
            .expect("complete");
        let replay = executor
            .submit(request(analysis, "f1"))
            .await
            .expect("resubmit");
        assert_eq!(
            replay, first,
            "same round: the succeeded job is re-used (restart recovery)"
        );
        fail_analysis(&db, analysis).await;
        assert!(
            db.analyses()
                .retry(analysis, Timestamp::now())
                .await
                .expect("retry")
        );
        let fresh = executor
            .submit(request(analysis, "f1"))
            .await
            .expect("submit after retry");
        assert_ne!(fresh, first, "a Retry calls the agent again");
        let again = executor
            .submit(request(analysis, "f1"))
            .await
            .expect("resubmit after retry");
        assert_eq!(again, fresh, "idempotent within the new round");
        assert!(
            !db.analyses()
                .retry(analysis, Timestamp::now())
                .await
                .expect("retry"),
            "only a failed analysis starts a new round"
        );
    }

    #[tokio::test]
    async fn retry_cancels_a_claimed_job_and_never_reuses_it() {
        let (db, executor, analysis) = setup().await;
        let claimed = executor
            .submit(request(analysis, "f2"))
            .await
            .expect("submit");
        claim(&db, 60).await;
        let queued = executor
            .submit(request(analysis, "f3"))
            .await
            .expect("submit");
        fail_analysis(&db, analysis).await;
        assert!(
            db.analyses()
                .retry(analysis, Timestamp::now())
                .await
                .expect("retry")
        );
        let old = db.jobs().get(claimed.job_id).await.expect("job");
        assert!(
            old.cancel_requested,
            "the earlier round's running job is cancelled"
        );
        let old_queued = db.jobs().get(queued.job_id).await.expect("job");
        assert_eq!(old_queued.state, JobState::Cancelled);
        let fresh = executor
            .submit(request(analysis, "f2"))
            .await
            .expect("submit after retry");
        assert_ne!(fresh, claimed);
    }

    /// A queued job whose credential another job paused reports the pause with its end, once per pause end.
    #[tokio::test]
    async fn queued_job_reports_the_pause_of_its_credential() {
        let (db, executor, analysis) = setup().await;
        let handle = executor
            .submit(request(analysis, "f1"))
            .await
            .expect("submit");
        let credential = contracts::job::spec::LlmCredentialRef {
            id: CREDENTIAL.to_owned(),
            kind: contracts::job::spec::CredentialKind::CodexChatgpt,
        };
        db.llm_credentials()
            .register(
                db::repo::DEFAULT_TENANT,
                &credential,
                true,
                Timestamp::now(),
            )
            .await
            .expect("register");
        let until = Timestamp::from_second(Timestamp::now().as_second() + 600).expect("reset time");
        db.llm_credentials()
            .pause(CREDENTIAL, until)
            .await
            .expect("pause");
        let mut updates = executor.events(&handle);
        assert_eq!(
            next(&mut updates).await,
            JobUpdate::Paused {
                resumes_at: Some(until)
            }
        );
        executor.cancel(&handle).await.expect("cancel");
        assert!(
            matches!(next(&mut updates).await, JobUpdate::Finished { .. }),
            "the pause is reported once"
        );
    }

    /// The result reports whether the attempt that produced it ran with a GitHub installation token.
    #[tokio::test]
    async fn finished_reports_the_attempts_github_access() {
        let (db, executor, analysis) = setup().await;
        let handle = executor
            .submit(request(analysis, "f1"))
            .await
            .expect("submit");
        let job = handle.job_id;
        claim(&db, 60).await;
        db.jobs()
            .record_github_access(job, 1, true)
            .await
            .expect("access");
        db.jobs()
            .complete(&succeeded(job, 1), Timestamp::now())
            .await
            .expect("complete");
        assert_eq!(
            next(&mut executor.events(&handle)).await,
            JobUpdate::Finished {
                result: succeeded(job, 1),
                github_access: true
            }
        );
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
                Timestamp::now() + SignedDuration::from_secs(60),
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
                Timestamp::now() + SignedDuration::from_secs(60),
            )
            .await
            .expect("output");
        assert_eq!(
            next(&mut updates).await,
            JobUpdate::OutputReady(json!({"ok": true}))
        );
        db.jobs()
            .complete(&succeeded(job, 1), Timestamp::now())
            .await
            .expect("complete");
        assert_eq!(
            next(&mut updates).await,
            JobUpdate::Finished {
                result: succeeded(job, 1),
                github_access: false
            }
        );
        assert!(updates.next().await.is_none());

        let mut finished = executor.events(&handle);
        assert_eq!(
            next(&mut finished).await,
            JobUpdate::Finished {
                result: succeeded(job, 1),
                github_access: false
            },
            "a finished job yields only its result"
        );
        assert!(finished.next().await.is_none());
    }

    #[tokio::test]
    async fn cancel_analysis_cancels_only_its_unfinished_jobs() {
        let (db, executor, analysis) = setup().await;
        let running = executor
            .submit(request(analysis, "f1"))
            .await
            .expect("submit f1");
        claim(&db, 60).await;
        let queued = executor
            .submit(request(analysis, "f2"))
            .await
            .expect("submit f2");
        let other_analysis = seed_analysis(&db).await.expect("seed");
        let other = executor
            .submit(request(other_analysis, "f1"))
            .await
            .expect("submit other");

        executor
            .cancel_analysis(analysis)
            .await
            .expect("cancel analysis");
        let running = db.jobs().get(running.job_id).await.expect("f1");
        assert_eq!(running.state, JobState::Claimed);
        assert!(running.cancel_requested);
        let queued = db.jobs().get(queued.job_id).await.expect("f2");
        assert_eq!(queued.state, JobState::Cancelled);
        let other = db.jobs().get(other.job_id).await.expect("other");
        assert_eq!(other.state, JobState::Queued);
        assert!(!other.cancel_requested);
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
        let after_lease = || Timestamp::now() + SignedDuration::from_secs(61);
        claim(&db, 60).await;
        db.jobs()
            .record_events(
                job,
                1,
                &[event(1)],
                Timestamp::now() + SignedDuration::from_secs(60),
            )
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
        let JobUpdate::Finished { result, .. } = next(&mut updates).await else {
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
                Timestamp::now() - SignedDuration::from_secs(1),
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

    /// Test hook run by the follower between its reads.
    pub(super) type Hook = std::sync::Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

    /// A hook that runs `step` once, on the follower's second poll.
    fn second_poll_hook<F, Fut>(step: F) -> Hook
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let step = std::sync::Arc::new(step);
        std::sync::Arc::new(move || {
            let call = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let step = step.clone();
            Box::pin(async move {
                if call == 1 {
                    step().await;
                }
            })
        })
    }

    fn kinds(updates: &[JobUpdate]) -> Vec<String> {
        updates
            .iter()
            .map(|update| match update {
                JobUpdate::Event(e) => format!("event {}", e.seq),
                JobUpdate::OutputReady(_) => "output".to_owned(),
                JobUpdate::Requeued { attempt } => format!("requeued {attempt}"),
                JobUpdate::Paused { .. } => "paused".to_owned(),
                JobUpdate::Finished { result, .. } => format!("finished {}", result.attempt),
            })
            .collect()
    }

    /// The runner posts event 3 and then `last` right after the follower's second read.
    async fn updates_after_a_late_event(last: Last) -> Vec<String> {
        let (db, executor, analysis) = setup().await;
        let handle = executor
            .submit(request(analysis, "f1"))
            .await
            .expect("submit");
        let job = handle.job_id;
        let lease = || Timestamp::now() + SignedDuration::from_secs(60);
        claim(&db, 60).await;
        db.jobs()
            .record_events(job, 1, &[event(1), event(2)], lease())
            .await
            .expect("events");
        let hook = second_poll_hook({
            let db = db.clone();
            move || {
                let db = db.clone();
                async move {
                    db.jobs()
                        .record_events(job, 1, &[event(3)], lease())
                        .await
                        .expect("late event");
                    match last {
                        Last::Output => {
                            db.jobs()
                                .record_output(job, 1, &json!({"ok": true}), lease())
                                .await
                                .expect("output");
                        }
                        Last::Result => {
                            db.jobs()
                                .complete(&succeeded(job, 1), Timestamp::now())
                                .await
                                .expect("result");
                        }
                    }
                }
            }
        });
        let mut follower = Follower::new(db.clone(), ProgressHub::default(), job);
        follower.between_reads = Some(hook);
        let mut updates = stream::unfold(follower, Follower::next).boxed();
        let mut seen = vec![next(&mut updates).await, next(&mut updates).await];
        loop {
            let update = next(&mut updates).await;
            let end = matches!(
                update,
                JobUpdate::OutputReady(_) | JobUpdate::Finished { .. }
            );
            seen.push(update);
            if end {
                return kinds(&seen);
            }
        }
    }

    #[derive(Clone, Copy)]
    enum Last {
        Output,
        Result,
    }

    #[tokio::test]
    async fn follower_emits_a_late_event_before_the_output() {
        assert_eq!(
            updates_after_a_late_event(Last::Output).await,
            ["event 1", "event 2", "event 3", "output"]
        );
    }

    #[tokio::test]
    async fn follower_emits_a_late_event_before_the_result() {
        assert_eq!(
            updates_after_a_late_event(Last::Result).await,
            ["event 1", "event 2", "event 3", "finished 1"]
        );
    }

    #[tokio::test]
    async fn follower_never_mixes_attempts_when_reclaimed_between_reads() {
        let (db, executor, analysis) = setup().await;
        let handle = executor
            .submit(request(analysis, "f1"))
            .await
            .expect("submit");
        let job = handle.job_id;
        let lease = || Timestamp::now() + SignedDuration::from_secs(60);
        claim(&db, 60).await;
        db.jobs()
            .record_events(job, 1, &[event(1)], lease())
            .await
            .expect("events");
        // On the follower's second poll, after its read: attempt 1 posts its output, the lease ends, attempt 2
        // is claimed and posts events.
        let hook = second_poll_hook({
            let db = db.clone();
            move || {
                let db = db.clone();
                async move {
                    db.jobs()
                        .record_output(job, 1, &json!({"attempt": 1}), lease())
                        .await
                        .expect("output of attempt 1");
                    db.jobs()
                        .expire_leases(Timestamp::now() + SignedDuration::from_secs(61))
                        .await
                        .expect("expire");
                    claim(&db, 60).await;
                    db.jobs()
                        .record_events(job, 2, &[event(1), event(2)], lease())
                        .await
                        .expect("events of attempt 2");
                }
            }
        });
        let mut follower = Follower::new(db.clone(), ProgressHub::default(), job);
        follower.between_reads = Some(hook);
        let mut updates = stream::unfold(follower, Follower::next).boxed();
        assert!(matches!(next(&mut updates).await, JobUpdate::Event(e) if e.seq == 1));
        let mut seen = Vec::new();
        loop {
            let update = next(&mut updates).await;
            let last = matches!(&update, JobUpdate::Event(e) if e.seq == 2);
            seen.push(update);
            if last {
                break;
            }
        }
        db.jobs()
            .complete(&succeeded(job, 2), Timestamp::now())
            .await
            .expect("complete");
        seen.push(next(&mut updates).await);
        assert_eq!(
            kinds(&seen),
            ["requeued 2", "event 1", "event 2", "finished 2"],
            "no output of the invalidated attempt, no attempt-2 event before the requeue, no repeats"
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
