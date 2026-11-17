//! The runner loop around `execute`: claims jobs, streams events and heartbeats, posts the ready output and
//! the result, follows cancel requests, and fences itself by [`LeaseTiming`] measured from the send time of
//! the last acked request (`docs/contracts/runner-protocol.md` "Liveness and fencing", threat model T1d).

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use contracts::ids::JobId;
use contracts::job::access::Secret;
use contracts::job::event::JobEvent;
use contracts::job::protocol::{
    Claim, ClaimRequest, CredentialStatus, EventBatch, EventsAck, LeaseTiming, OutputReady,
};
use contracts::job::result::{JobOutcome, JobResult, LlmCallOutcome, LlmCallStatus};
use reqwest::StatusCode;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, sleep_until, timeout_at};

use crate::client::{ClientError, ServerClient};
use crate::execute::{EventSink, Executor};
use crate::provider::ProviderEvent;
use crate::scrub::{SUMMARY_MAX_CHARS, Scrubber, clip};
use crate::skills::Skills;

/// Events buffered between `execute` and the loop.
const EVENT_BUFFER: usize = 256;
/// Most events in one request.
const MAX_BATCH: usize = 100;
/// Most events held while requests fail; beyond it the loop stops receiving (backpressure to the agent).
const MAX_PENDING_EVENTS: usize = 1000;
/// Most summary bytes held while requests fail.
const MAX_PENDING_BYTES: usize = 256 * 1024;
/// How often expired agent sessions are looked for.
const SWEEP_INTERVAL: Duration = Duration::from_secs(3600);
/// Idle claim interval in [`LeaseTiming`] seconds.
const IDLE_POLL_SECS: u32 = 5;
/// Longest pause between two result attempts, in [`LeaseTiming`] seconds.
const RESULT_RETRY_MAX_SECS: u32 = 15;

/// [`LeaseTiming`] as durations. Production uses one second per unit; tests use a shorter unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// Server lease per accepted request.
    pub lease: Duration,
    /// Longest gap between two requests.
    pub heartbeat: Duration,
    /// Local deadline after the send time of the last acked request.
    pub fence_after_send: Duration,
    /// Wait for the provider to stop after a cancel before the call is dropped (killed).
    pub term_grace: Duration,
    /// Kill and cleanup budget after the grace.
    pub kill_and_cleanup: Duration,
    /// Pauses between `output` attempts.
    pub output_retry: [Duration; 3],
    /// Claim interval while idle.
    pub idle_poll: Duration,
    /// One [`LeaseTiming`] second.
    pub unit: Duration,
}

impl Timing {
    /// `lease` with each second lasting `unit`.
    pub fn scaled(lease: LeaseTiming, unit: Duration) -> Self {
        let secs = |value: u32| unit * value;
        Self {
            lease: secs(lease.lease_secs),
            heartbeat: secs(lease.heartbeat_secs),
            fence_after_send: secs(lease.fence_after_send_secs),
            term_grace: secs(lease.term_grace_secs),
            kill_and_cleanup: secs(lease.kill_and_cleanup_secs),
            output_retry: lease.output_retry_secs.map(secs),
            idle_poll: secs(IDLE_POLL_SECS),
            unit,
        }
    }

    /// The production timing, [`LeaseTiming::V1`].
    pub fn v1() -> Self {
        Self::scaled(LeaseTiming::V1, Duration::from_secs(1))
    }
}

/// The local validity of a job attempt: send time of the last acked request + `fence_after_send`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fence {
    after_send: Duration,
    deadline: Instant,
}

impl Fence {
    /// Validity from the claim, which the server acked by granting the lease.
    pub(crate) fn new(claim_sent: Instant, after_send: Duration) -> Self {
        Self {
            after_send,
            deadline: claim_sent + after_send,
        }
    }

    /// A request sent at `sent` was acked. A late ack never extends beyond `sent + after_send`.
    pub(crate) fn acked(&mut self, sent: Instant) {
        self.deadline = self.deadline.max(sent + self.after_send);
    }

    /// When the attempt must be stopped.
    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }
}

/// How a claimed job ended for this runner.
#[derive(Clone, Debug, PartialEq)]
pub enum JobEnd {
    /// The result was accepted.
    ResultPosted(JobOutcome),
    /// The server rejected the result (4xx, e.g. 409 for a stale attempt); dropped.
    ResultRejected(StatusCode),
    /// The result could not be delivered before the lease must have ended; dropped.
    ResultUndelivered,
    /// The local deadline passed: the agent was stopped, no result posted.
    Fenced,
    /// The server rejected the attempt (409 or another 4xx on events or output): stopped, no result posted.
    Stale(StatusCode),
}

/// Why a running job stops before its result.
enum Stop {
    Fenced,
    Stale(StatusCode),
}

/// What the loop does after one wake-up.
enum Step {
    Continue,
    Stop(Stop),
    /// `execute` returned (its sink closed).
    Finished,
    /// The provider ignored the cancel for the grace period.
    KillAfterCancel,
}

impl From<Result<(), Stop>> for Step {
    fn from(result: Result<(), Stop>) -> Self {
        match result {
            Ok(()) => Self::Continue,
            Err(stop) => Self::Stop(stop),
        }
    }
}

/// Whether the runner's LLM credential may be used (false after a refresh failure or a rejected write-back
/// until the owner signs in again); reported with every claim.
pub type CredentialCheck = Arc<dyn Fn() -> bool + Send + Sync>;

/// Claims and runs jobs, one at a time.
pub struct Worker {
    client: ServerClient,
    executor: Arc<Executor>,
    runner_id: String,
    runner_secret: Secret,
    timing: Timing,
    credential_usable: CredentialCheck,
    /// Sessions the server asked to drop, acknowledged with the next claim.
    dropped_sessions: Vec<i64>,
    /// How long an unused agent session is kept; `None` keeps them.
    session_retention: Option<Duration>,
    last_sweep: Option<Instant>,
    /// The claudius skills, refreshed between jobs.
    skills: Option<Arc<Skills>>,
}

impl std::fmt::Debug for Worker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Worker")
            .field("runner_id", &self.runner_id)
            .field("timing", &self.timing)
            .finish_non_exhaustive()
    }
}

impl Worker {
    /// A worker named `runner_id`; `runner_secret` is scrubbed from everything it posts.
    pub fn new(
        client: ServerClient,
        executor: Arc<Executor>,
        runner_id: String,
        runner_secret: Secret,
        timing: Timing,
        credential_usable: CredentialCheck,
    ) -> Self {
        Self {
            client,
            executor,
            runner_id,
            runner_secret,
            timing,
            credential_usable,
            dropped_sessions: Vec::new(),
            session_retention: None,
            last_sweep: None,
            skills: None,
        }
    }

    /// Refreshes the claudius skills between jobs at their interval.
    pub fn with_skills(mut self, skills: Arc<Skills>) -> Self {
        self.skills = Some(skills);
        self
    }

    /// Deletes agent sessions not used for `retention` (checked between jobs).
    pub fn with_session_retention(mut self, retention: Duration) -> Self {
        self.session_retention = Some(retention);
        self
    }

    /// Claims and runs jobs until `shutdown` completes; a running job is then stopped and cleaned up.
    pub async fn run(mut self, shutdown: impl Future<Output = ()>) {
        tokio::pin!(shutdown);
        loop {
            self.remove_expired_sessions();
            if let Some(skills) = &self.skills {
                tokio::select! {
                    () = &mut shutdown => return,
                    () = skills.refresh_if_due() => {}
                }
            }
            let polled = tokio::select! {
                () = &mut shutdown => return,
                polled = self.poll_once() => polled,
            };
            match polled {
                Ok(Some(end)) => {
                    tracing::trace!(?end, "claiming the next job at once");
                    continue;
                }
                Ok(None) => {}
                Err(ClientError::Rejected(StatusCode::UNAUTHORIZED)) => {
                    tracing::error!(
                        "the server rejected the runner secret; check SR_RUNNER_SECRET"
                    );
                }
                Err(error) => tracing::warn!(%error, "claim failed; retrying"),
            }
            tokio::select! {
                () = &mut shutdown => return,
                () = sleep(self.timing.idle_poll) => {}
            }
        }
    }

    /// One claim; runs the claimed job to its end. `None` when nothing was runnable.
    pub async fn poll_once(&mut self) -> Result<Option<JobEnd>, ClientError> {
        // The previous attempt (and its credential copy-back) is over before the next claim.
        self.executor.settled().await;
        let request = ClaimRequest {
            runner_id: self.runner_id.clone(),
            credentials: vec![CredentialStatus {
                credential: self.executor.credential().clone(),
                usable: (self.credential_usable)(),
            }],
            dropped_sessions: self.dropped_sessions.clone(),
        };
        let sent = Instant::now();
        let response = self.client.claim(&request).await?;
        self.dropped_sessions = self.drop_sessions(response.drop_sessions);
        let Some(claim) = response.claim else {
            return Ok(None);
        };
        Ok(Some(self.run_job(claim, sent).await))
    }

    /// Deletes the sessions the server dropped; returns those to acknowledge (a failed one is asked again).
    fn drop_sessions(&self, analyses: Vec<i64>) -> Vec<i64> {
        let sessions = self.executor.sessions();
        analyses
            .into_iter()
            .filter(|analysis| match sessions.remove(*analysis) {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(%analysis, %error, "cannot delete a dropped agent session; retrying with the next claim");
                    false
                }
            })
            .collect()
    }

    /// Deletes agent sessions past `retention`, at most once per [`SWEEP_INTERVAL`].
    fn remove_expired_sessions(&mut self) {
        let Some(retention) = self.session_retention else {
            return;
        };
        if self
            .last_sweep
            .is_some_and(|last| last.elapsed() < SWEEP_INTERVAL)
        {
            return;
        }
        self.last_sweep = Some(Instant::now());
        match self.executor.sessions().remove_expired(retention) {
            Ok(0) => {}
            Ok(removed) => {
                tracing::info!(removed, "deleted agent sessions past the retention period")
            }
            Err(error) => {
                tracing::warn!(%error, "cannot delete expired agent sessions; retrying later")
            }
        }
    }

    async fn run_job(&self, claim: Claim, claim_sent: Instant) -> JobEnd {
        let Claim {
            job_token, spec, ..
        } = claim;
        let scrubber = Scrubber::new([self.runner_secret.expose(), job_token.expose()]);
        let job = spec.job_id;
        let attempt = spec.attempt;
        tracing::info!(%job, attempt, kind = spec.kind.as_str(), "job claimed");
        let mut run = JobRun {
            client: &self.client,
            job,
            attempt,
            token: job_token.clone(),
            scrubber: scrubber.clone(),
            fence: Fence::new(claim_sent, self.timing.fence_after_send),
            timing: self.timing,
            last_sent: claim_sent,
            retry_at: claim_sent,
            pending: Vec::new(),
            pending_bytes: 0,
            cancel_requested: false,
        };
        let (sink, mut events) = mpsc::channel(EVENT_BUFFER);
        let executor = Arc::clone(&self.executor);
        let sink = EventSink::new(sink, scrubber);
        let mut task = AbortOnDrop(tokio::spawn(
            async move { executor.execute(spec, sink).await },
        ));
        let mut abort_at: Option<Instant> = None;

        let stopped = loop {
            let post_at = if run.pending.is_empty() {
                (run.last_sent + self.timing.heartbeat).max(run.retry_at)
            } else {
                run.retry_at
            };
            let step = tokio::select! {
                biased;
                () = sleep_until(run.fence.deadline()) => Step::Stop(Stop::Fenced),
                () = sleep_until(abort_at.unwrap_or_else(Instant::now)), if abort_at.is_some() => Step::KillAfterCancel,
                event = events.recv(), if !run.pending_full() => match event {
                    Some(ProviderEvent::Activity(event)) => {
                        run.push(event);
                        // A steady stream keeps this branch ready; post here so heartbeats are never starved.
                        let overdue = Instant::now() >= run.last_sent + self.timing.heartbeat;
                        if (run.pending.len() >= MAX_BATCH || overdue) && Instant::now() >= run.retry_at {
                            run.post_events().await.into()
                        } else {
                            Step::Continue
                        }
                    }
                    Some(ProviderEvent::OutputReady(output)) => run.post_output(output).await.into(),
                    None => Step::Finished,
                },
                () = sleep_until(post_at) => run.post_events().await.into(),
            };
            match step {
                Step::Continue => {}
                Step::Stop(stop) => break Some(stop),
                Step::Finished => break None,
                Step::KillAfterCancel => {
                    tracing::warn!(%job, "the provider did not stop after the cancel request; killing the agent call");
                    task.0.abort();
                    break None;
                }
            }
            if run.cancel_requested && abort_at.is_none() {
                tracing::info!(%job, "cancel requested; stopping the agent");
                if let Err(error) = self.executor.provider().cancel(job).await {
                    tracing::debug!(%job, %error, "provider cancel failed; the call is killed after the grace period");
                }
                abort_at = Some(Instant::now() + self.timing.term_grace);
            }
        };

        // From here on the attempt's events are not read: closing the channel makes its sends fail at once,
        // so back-pressure never holds up a detached attempt's signal, sweep and clean-up.
        events.close();
        if let Some(stop) = stopped {
            self.stop_agent(job, &mut task).await;
            return stopped_end(stop, job, attempt);
        }

        let result = match (&mut task.0).await {
            Ok(result) => result,
            Err(error) if error.is_cancelled() => cancelled_result(job, attempt),
            Err(error) => {
                tracing::error!(%job, %error, "the job task panicked");
                internal_result(job, attempt)
            }
        };
        // A detached attempt finishes its stop, copy-back and cleanup before anything terminal is posted.
        self.executor.settled().await;
        while let Ok(event) = events.try_recv() {
            if let ProviderEvent::Activity(event) = event
                && !run.pending_full()
            {
                run.push(event);
            }
        }
        // Every event before the result; a stale attempt or the fence here still suppresses the result.
        if let Err(stop) = run.flush_events().await {
            return stopped_end(stop, job, attempt);
        }
        run.post_result(result).await
    }

    /// Stops the agent within `term_grace + kill_and_cleanup`: cancel, then drop (kill) the call.
    async fn stop_agent(&self, job: JobId, task: &mut AbortOnDrop<JobResult>) {
        if let Err(error) = self.executor.provider().cancel(job).await {
            tracing::debug!(%job, %error, "provider cancel failed; killing the agent call");
        }
        if tokio::time::timeout(self.timing.term_grace, &mut task.0)
            .await
            .is_err()
        {
            task.0.abort();
            // Dropping the call kills Codex (`kill_on_drop`) and removes the job directories.
            let _ = tokio::time::timeout(self.timing.kill_and_cleanup, &mut task.0).await;
        }
    }
}

/// Protocol state of one running attempt.
struct JobRun<'a> {
    client: &'a ServerClient,
    job: JobId,
    attempt: u32,
    token: Secret,
    scrubber: Scrubber,
    fence: Fence,
    timing: Timing,
    /// Send time of the last request (acked or not).
    last_sent: Instant,
    /// No request before this (after a failed one).
    retry_at: Instant,
    pending: Vec<JobEvent>,
    /// Summary bytes in `pending`.
    pending_bytes: usize,
    cancel_requested: bool,
}

impl JobRun<'_> {
    fn push(&mut self, event: JobEvent) {
        self.pending_bytes += event.summary.len();
        self.pending.push(event);
    }

    /// Whether the loop must stop receiving until events are acked.
    fn pending_full(&self) -> bool {
        self.pending.len() >= MAX_PENDING_EVENTS || self.pending_bytes >= MAX_PENDING_BYTES
    }

    /// Posts up to [`MAX_BATCH`] pending events (none: heartbeat) within the fence.
    async fn post_events(&mut self) -> Result<(), Stop> {
        let count = self.pending.len().min(MAX_BATCH);
        let batch = EventBatch {
            attempt: self.attempt,
            events: self.pending[..count]
                .iter()
                .map(|event| self.scrub_event(event))
                .collect(),
        };
        let sent = Instant::now();
        self.last_sent = sent;
        let reply = timeout_at(
            self.fence.deadline(),
            self.client.events(self.job, &self.token, &batch),
        )
        .await;
        match reply {
            Err(_) => Err(Stop::Fenced),
            Ok(Ok(ack)) => {
                let bytes: usize = self.pending[..count].iter().map(|e| e.summary.len()).sum();
                self.pending.drain(..count);
                self.pending_bytes -= bytes;
                self.acked(sent, &ack);
                Ok(())
            }
            Ok(Err(ClientError::Rejected(status))) => Err(Stop::Stale(status)),
            Ok(Err(error)) => {
                tracing::debug!(job = %self.job, %error, "events not acked; retrying");
                self.retry_at = sent + self.timing.unit;
                Ok(())
            }
        }
    }

    /// Posts every pending event, then the ready output, with the `output` retry schedule within the fence.
    /// If earlier events are not acked the output is skipped (it must not overtake them; the result carries it).
    async fn post_output(&mut self, output: Value) -> Result<(), Stop> {
        while !self.pending.is_empty() {
            let before = self.pending.len();
            self.post_events().await?;
            if self.pending.len() == before {
                tracing::debug!(job = %self.job, "earlier events not acked; ready output skipped, the result carries it");
                return Ok(());
            }
        }
        let body = OutputReady {
            attempt: self.attempt,
            output: self.scrubber.value(output),
        };
        let mut pauses = self.timing.output_retry.iter();
        loop {
            let sent = Instant::now();
            self.last_sent = sent;
            match timeout_at(
                self.fence.deadline(),
                self.client.output(self.job, &self.token, &body),
            )
            .await
            {
                Err(_) => return Err(Stop::Fenced),
                Ok(Ok(ack)) => {
                    self.acked(sent, &ack);
                    return Ok(());
                }
                Ok(Err(ClientError::Rejected(status))) => return Err(Stop::Stale(status)),
                Ok(Err(error)) => match pauses.next() {
                    Some(pause) => {
                        tracing::debug!(job = %self.job, %error, "ready output not acked; retrying");
                        if timeout_at(self.fence.deadline(), sleep(*pause))
                            .await
                            .is_err()
                        {
                            return Err(Stop::Fenced);
                        }
                    }
                    None => {
                        tracing::warn!(job = %self.job, %error, "ready output not delivered; the result will carry it");
                        return Ok(());
                    }
                },
            }
        }
    }

    fn acked(&mut self, sent: Instant, ack: &EventsAck) {
        self.fence.acked(sent);
        self.retry_at = sent;
        if ack.cancel_requested {
            self.cancel_requested = true;
        }
    }

    /// Posts the remaining events before the result, retrying within the fence.
    async fn flush_events(&mut self) -> Result<(), Stop> {
        while !self.pending.is_empty() {
            if timeout_at(self.fence.deadline(), sleep_until(self.retry_at))
                .await
                .is_err()
            {
                return Err(Stop::Fenced);
            }
            self.post_events().await?;
        }
        Ok(())
    }

    /// Posts the result until a 2xx or 4xx reply, while the server lease can still be alive.
    async fn post_result(&self, result: JobResult) -> JobEnd {
        let result = scrub_result(result, &self.scrubber);
        let give_up = self.fence.deadline() - self.timing.fence_after_send + self.timing.lease;
        let mut pause = self.timing.unit;
        loop {
            match self.client.result(self.job, &self.token, &result).await {
                Ok(()) => {
                    tracing::info!(job = %self.job, attempt = self.attempt, "result posted");
                    return JobEnd::ResultPosted(result.outcome);
                }
                Err(ClientError::Rejected(status)) => {
                    tracing::warn!(job = %self.job, %status, "the server rejected the result; dropped");
                    return JobEnd::ResultRejected(status);
                }
                Err(error) => {
                    if Instant::now() + pause > give_up {
                        tracing::error!(job = %self.job, %error, "the result could not be delivered before the lease ended; dropped");
                        return JobEnd::ResultUndelivered;
                    }
                    tracing::debug!(job = %self.job, %error, "result not accepted; retrying");
                    sleep(pause).await;
                    pause = (pause * 2).min(self.timing.unit * RESULT_RETRY_MAX_SECS);
                }
            }
        }
    }

    fn scrub_event(&self, event: &JobEvent) -> JobEvent {
        JobEvent {
            summary: clip(&self.scrubber.text(&event.summary), SUMMARY_MAX_CHARS),
            ..event.clone()
        }
    }
}

/// The end of an attempt stopped before its result.
fn stopped_end(stop: Stop, job: JobId, attempt: u32) -> JobEnd {
    match stop {
        Stop::Fenced => {
            tracing::warn!(%job, attempt, "no request acked within the fence; agent stopped, no result posted");
            JobEnd::Fenced
        }
        Stop::Stale(status) => {
            tracing::warn!(%job, attempt, %status, "the server rejected the attempt; agent stopped, no result posted");
            JobEnd::Stale(status)
        }
    }
}

fn scrub_result(mut result: JobResult, scrubber: &Scrubber) -> JobResult {
    result.outcome = match result.outcome {
        JobOutcome::Succeeded { output } => JobOutcome::Succeeded {
            output: scrubber.value(output),
        },
        JobOutcome::Failed { kind, message } => JobOutcome::Failed {
            kind,
            message: scrubber.text(&message),
        },
        other => other,
    };
    if let Some(LlmCallOutcome {
        status: LlmCallStatus::NotWorking { message },
        ..
    }) = result.llm_call.as_mut()
    {
        *message = scrubber.text(message);
    }
    result
}

fn cancelled_result(job: JobId, attempt: u32) -> JobResult {
    JobResult {
        job_id: job,
        attempt,
        outcome: JobOutcome::Cancelled,
        session: None,
        usage: None,
        skills_commit: None,
        llm_call: None,
    }
}

fn internal_result(job: JobId, attempt: u32) -> JobResult {
    JobResult {
        outcome: JobOutcome::Failed {
            kind: contracts::job::result::JobFailureKind::Internal,
            message: "the runner failed while running the job".to_owned(),
        },
        ..cancelled_result(job, attempt)
    }
}

/// Aborts the job task when the loop is dropped (shutdown), so the agent is killed and directories removed.
struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests;
