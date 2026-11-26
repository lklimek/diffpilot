//! The runner loop around `execute`: claims jobs, streams events and heartbeats, posts the ready output and
//! the result, follows cancel requests, and fences itself by [`LeaseTiming`] measured from the send time of
//! the last acked request (`docs/contracts/runner-protocol.md` "Liveness and fencing", threat model T1d).

use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use contracts::ids::JobId;
use contracts::job::access::Secret;
use contracts::job::event::JobEvent;
use contracts::job::protocol::{
    Claim, ClaimRequest, EventBatch, EventsAck, LeaseTiming, OutputReady,
};
use contracts::job::result::{JobOutcome, JobResult, LlmCallOutcome, LlmCallStatus};
use reqwest::StatusCode;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep, sleep_until, timeout_at};

use crate::client::{ClientError, ServerClient};
use crate::execute::{AttemptControl, EventSink, Executor};
use crate::provider::ProviderEvent;
use crate::scrub::{SUMMARY_MAX_CHARS, Scrubber, clip};

/// Events buffered between `execute` and the loop.
const EVENT_BUFFER: usize = 256;
/// Most events in one request.
const MAX_BATCH: usize = 100;
/// Most events held while requests fail; beyond it the loop stops receiving (backpressure to the agent).
const MAX_PENDING_EVENTS: usize = 1000;
/// Most summary bytes held while requests fail.
const MAX_PENDING_BYTES: usize = 256 * 1024;
/// Idle claim interval in [`LeaseTiming`] seconds: the longest an Ask waits for an idle runner.
const IDLE_POLL_SECS: u32 = 1;
/// Claim interval after a failed claim, in [`LeaseTiming`] seconds.
const ERROR_POLL_SECS: u32 = 5;
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
    /// Claim interval after a failed claim.
    pub error_poll: Duration,
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
            error_poll: secs(ERROR_POLL_SECS),
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
    /// Another local execution already owns the job, so the claim was dropped.
    LocallyOccupied,
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

/// Claims work up to the local runtime capacity.
pub struct Worker {
    runner_id: String,
    job_runner: Arc<JobRunner>,
    max_running: usize,
    /// Completed PR retirements, acknowledged with the next claim.
    retired: Vec<i64>,
    /// Free space the scratch filesystem must keep for a claim.
    disk_reserve: Option<crate::scratch::Reserve>,
}

struct JobRunner {
    client: ServerClient,
    executor: Arc<Executor>,
    runner_secret: Secret,
    timing: Timing,
}

impl std::fmt::Debug for Worker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Worker")
            .field("runner_id", &self.runner_id)
            .field("timing", &self.job_runner.timing)
            .field("max_running", &self.max_running)
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
    ) -> Self {
        Self {
            runner_id,
            job_runner: Arc::new(JobRunner {
                client,
                executor,
                runner_secret,
                timing,
            }),
            max_running: crate::config::DEFAULT_MAX_RUNNING,
            retired: Vec::new(),
            disk_reserve: None,
        }
    }

    /// Use the operator's physical worker limit when claiming jobs.
    pub fn with_max_running(mut self, limit: NonZeroUsize) -> Self {
        self.max_running = limit.get();
        self
    }

    /// Takes no job while the scratch filesystem has less free space than `reserve` asks.
    pub fn with_disk_reserve(mut self, reserve: crate::scratch::Reserve) -> Self {
        self.disk_reserve = Some(reserve);
        self
    }

    /// Stops taking work on shutdown and cancels and drains local attempts within the stop grace.
    pub async fn run(mut self, shutdown: impl Future<Output = ()>) {
        let mut jobs = JoinSet::new();
        tokio::select! {
            biased;
            () = shutdown => {}
            () = self.serve(&mut jobs) => {}
        }
        // Dropping the job monitors cancels their retained executions.
        jobs.shutdown().await;
        let JobRunner {
            executor, timing, ..
        } = &*self.job_runner;
        let graceful_wait = timing.term_grace * 3 + timing.kill_and_cleanup * 2;
        if tokio::time::timeout(graceful_wait, executor.settled())
            .await
            .is_err()
        {
            tracing::warn!(
                "some attempts did not finish stopping before the runner shutdown deadline"
            );
            let aborted = executor.abort_all();
            let runner = &self.job_runner;
            let _ = tokio::time::timeout(timing.kill_and_cleanup, async {
                futures::future::join_all(aborted.into_iter().map(|attempt| async move {
                    runner.fence_orphan(&attempt).await;
                }))
                .await;
            })
            .await;
        }
    }

    async fn serve(&mut self, jobs: &mut JoinSet<JobEnd>) {
        let timing = self.job_runner.timing;
        loop {
            while let Some(result) = jobs.try_join_next() {
                log_job_end(result);
            }
            if jobs.len() >= self.max_running {
                if let Some(result) = jobs.join_next().await {
                    log_job_end(result);
                }
                continue;
            }
            let pause = match self.claim_once().await {
                Ok(Some((claim, sent))) => {
                    let worker = Arc::clone(&self.job_runner);
                    jobs.spawn(async move { worker.run_job(claim, sent).await });
                    continue;
                }
                Ok(None) => timing.idle_poll,
                Err(ClientError::Rejected(StatusCode::UNAUTHORIZED)) => {
                    tracing::error!(
                        "the server rejected the runner secret; check SR_RUNNER_SECRET"
                    );
                    timing.error_poll
                }
                Err(error) => {
                    tracing::warn!(%error, "claim failed; retrying");
                    timing.error_poll
                }
            };
            sleep(pause).await;
        }
    }

    /// One claim; runs the claimed job to its end. `None` when nothing was runnable. Below the scratch
    /// filesystem's reserve it requests housekeeping and retirement without taking new work.
    pub async fn poll_once(&mut self) -> Result<Option<JobEnd>, ClientError> {
        match self.claim_once().await? {
            Some((claim, sent)) => Ok(Some(self.job_runner.run_job(claim, sent).await)),
            None => Ok(None),
        }
    }

    async fn claim_once(&mut self) -> Result<Option<(Claim, Instant)>, ClientError> {
        let accept_job = self
            .disk_reserve
            .as_mut()
            .is_none_or(crate::scratch::Reserve::allows_claim);
        let request = ClaimRequest {
            accept_job,
            runner_id: self.runner_id.clone(),
            retired: self.retired.clone(),
        };
        let sent = Instant::now();
        let response = self.job_runner.client.claim(&request).await?;
        self.retired.clear();
        if let Err(error) = self
            .job_runner
            .executor
            .provider()
            .retire(&response.retire)
            .await
        {
            tracing::warn!(%error, "cannot retire PR workers; retrying on the next claim");
        } else {
            self.retired = response.retire;
        }
        Ok(response.claim.map(|claim| (claim, sent)))
    }
}

fn log_job_end(result: Result<JobEnd, tokio::task::JoinError>) {
    match result {
        Ok(end) => tracing::trace!(?end, "job monitor finished"),
        Err(error) => tracing::error!(%error, "job monitor failed"),
    }
}

impl JobRunner {
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
        if Instant::now() >= run.fence.deadline() {
            return stopped_end(Stop::Fenced, job, attempt);
        }
        let (sink, mut events) = mpsc::channel(EVENT_BUFFER);
        let sink = EventSink::new(sink, scrubber);
        let (control, task) = self.executor.start(spec, sink);
        let Some(control) = control else {
            // A duplicate claim must not publish a result for its existing owner.
            return JobEnd::LocallyOccupied;
        };
        tokio::pin!(task);
        let mut abort_at: Option<Instant> = None;
        let mut killed = false;

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
                    control.abort();
                    killed = true;
                    break None;
                }
            }
            if run.cancel_requested && abort_at.is_none() {
                tracing::info!(%job, "cancel requested; stopping the agent");
                control.cancel();
                abort_at = Some(Instant::now() + self.timing.term_grace);
            }
        };

        // From here on the attempt's events are not read: closing the channel makes its sends fail at once,
        // so back-pressure never holds up an attempt's stop and clean-up.
        events.close();
        if let Some(stop) = stopped {
            self.stop_agent(&control, task).await;
            return stopped_end(stop, job, attempt);
        }

        let result = task.await;
        if killed {
            self.fence_orphan(&control).await;
        }
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
    async fn stop_agent(
        &self,
        control: &AttemptControl,
        task: Pin<&mut impl Future<Output = JobResult>>,
    ) {
        control.cancel();
        if tokio::time::timeout(self.timing.term_grace, task)
            .await
            .is_err()
        {
            control.abort();
            self.fence_orphan(control).await;
        }
    }

    async fn fence_orphan(&self, control: &AttemptControl) {
        let attempt = control.id();
        let job = attempt.job;
        let cleanup = async {
            control.settled().await;
            self.executor.provider().reap_abandoned(attempt).await
        };
        match tokio::time::timeout(self.timing.kill_and_cleanup, cleanup).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%job, %error, "cannot clean up a dropped attempt"),
            Err(_) => tracing::warn!(%job, "dropped attempt cleanup exceeded its time budget"),
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

#[cfg(test)]
mod tests;
