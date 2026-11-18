//! Daily scheduled refresh (plan §A.5, ADR-020): one in-process task re-evaluates, at most a minute apart, which
//! users are due from their stored settings and run records only (no stored timers; request input never reaches
//! it). A due user's refresh starts through the manual pipeline ([`refresh::start`], trigger `scheduled`), one at
//! a time server-wide (the next starts once the previous scan ended), not before the scheduled instant plus a
//! per-user offset of 0–9 minutes and in that order. Each run is recorded per local date before it starts, and a
//! user whose run fails on the server (error or panic) is given up for that date, so a failing user is tried at most
//! once a day and never holds up the others. A user off the sign-in allow-list never runs and loses the GitHub
//! token pair kept for the schedule. A run a restart interrupted is recorded `failed` at start-up and does not count
//! for its date, so the day's catch-up decides again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use contracts::api::dashboard::{
    DashboardSettings, RefreshFailureCode, RefreshState, RefreshStatus, RefreshTrigger,
    ScheduledOutcome,
};
use contracts::ids::RefreshId;
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::dashboard::{RefreshRecord, ScheduleCandidate, ScheduledRunRecord};
use db::repo::tokens::PairCondition;
use jiff::SignedDuration;
use jiff::civil::{Date, Time};
use jiff::tz::{Offset, TimeZone};
use serde::de::IgnoredAny;
use tokio::sync::broadcast::{self, error::RecvError};

use super::refresh::{self, Scope, StartError};
use super::zone;
use crate::auth::{liveness, lock_user};
use crate::secrets::sha256;
use crate::state::AppState;

/// Longest sleep between two evaluations; a settings change takes effect within it.
pub const TICK: Duration = Duration::from_secs(60);
/// Shortest sleep between two evaluations that started nothing, so the scheduler never spins.
const MIN_SLEEP: Duration = Duration::from_secs(1);
/// Users are spread over this many minutes after their scheduled instant.
const STAGGER_MINUTES: u8 = 10;
/// A scheduled scan still running this long after the scan limit is left to end on its own.
const SCAN_END_MARGIN: Duration = Duration::from_secs(60);
/// How far past a DST gap the first valid local minute is searched.
const GAP_SEARCH_MINUTES: i64 = 48 * 60;

const REFRESHED_ALREADY: &str = "A refresh finished after the scheduled time.";
const ALREADY_RUNNING: &str = "A refresh was already running at the scheduled time.";
const SIGN_IN_AGAIN: &str =
    "Your GitHub sign-in has ended. Sign in again to resume the automatic refresh.";
const NOT_STARTED: &str = "The refresh could not be started on the server.";
const FAILED_WITHOUT_REASON: &str = "The refresh failed.";
const NO_PAIR: &str = "Sign in again to keep the daily refresh running.";
const FAILED_ON_SERVER: &str = "The automatic refresh failed on the server.";
/// Reason of a run a restart interrupted; such a record does not count for its date (see [`counted_run_date`]).
pub(crate) const INTERRUPTED: &str = "The automatic refresh was interrupted by a server restart.";

/// Set once the process's scheduler runs.
static SPAWNED: AtomicBool = AtomicBool::new(false);

/// A user's daily refresh time in their time zone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    zone: TimeZone,
    time: Time,
}

/// What the schedule asks for at a given time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Due {
    /// Nothing before `instant`, the scheduled instant of the next local date to run.
    Later {
        /// The next scheduled instant.
        instant: Timestamp,
    },
    /// The run of the user's local date `on`, scheduled at `instant`, is due (or overdue: caught up once).
    Now {
        /// The local date.
        on: Date,
        /// Its scheduled instant.
        instant: Timestamp,
    },
}

impl Schedule {
    /// The daily time `time` (`HH:MM`) in the IANA zone `time_zone`; `None` when either does not parse.
    pub fn parse(time_zone: &str, time: &str) -> Option<Self> {
        Some(Self {
            zone: zone::parse(time_zone)?,
            time: Time::strptime("%H:%M", time).ok()?,
        })
    }

    /// The user's schedule if it runs: switched on, with a known zone and a valid time.
    pub fn of(settings: &DashboardSettings) -> Option<Self> {
        if !settings.schedule_enabled {
            return None;
        }
        Self::parse(settings.time_zone.as_deref()?, &settings.schedule_time)
    }

    /// The time zone.
    pub fn zone(&self) -> &TimeZone {
        &self.zone
    }

    /// The scheduled instant of local date `on`: in a DST gap the first valid instant after it, in an overlap the
    /// first occurrence.
    pub fn instant(&self, on: Date) -> Timestamp {
        let local = on.to_datetime(self.time);
        (0..=GAP_SEARCH_MINUTES)
            .find_map(|minutes| {
                zone::earliest(
                    local.checked_add(SignedDuration::from_mins(minutes)).ok()?,
                    &self.zone,
                )
            })
            .or_else(|| Offset::UTC.to_timestamp(local).ok().map(Timestamp::from))
            .unwrap_or_else(|| jiff::Timestamp::MAX.into())
    }

    /// Whether the run of the user's current local date is due at `now`, given the local date of the last
    /// recorded run. One run per local date: a recorded date on or after today (also after a zone change) waits
    /// for the next date; a past date is never caught up.
    pub fn due(&self, now: Timestamp, last_on: Option<Date>) -> Due {
        let today = now.to_zoned(self.zone.clone()).date();
        let next = match last_on {
            Some(last) if last >= today => last.tomorrow().unwrap_or(last),
            _ => today,
        };
        let instant = self.instant(next);
        if next == today && now >= instant {
            Due::Now { on: today, instant }
        } else {
            Due::Later { instant }
        }
    }

    /// The next scheduled instant (R1 `next_run_at`): today's while still to run (also when overdue and about to
    /// be caught up), else the next date's; a refresh that succeeded after today's instant makes it tomorrow's.
    pub fn next_run_at(
        &self,
        now: Timestamp,
        last_on: Option<Date>,
        last_success: Option<Timestamp>,
    ) -> Timestamp {
        match self.due(now, last_on) {
            Due::Later { instant } => instant,
            Due::Now { on, instant } if last_success.is_some_and(|at| at >= instant) => {
                self.instant(on.tomorrow().unwrap_or(on))
            }
            Due::Now { instant, .. } => instant,
        }
    }
}

/// The local date of the last recorded run that counts for the due rule: a run a restart interrupted does not.
pub fn counted_run_date(last: Option<&ScheduledRunRecord>) -> Option<Date> {
    last.filter(|run| {
        !(run.outcome == ScheduledOutcome::Failed && run.reason.as_deref() == Some(INTERRUPTED))
    })
    .map(|run| run.on)
}

/// The user's fixed stagger after the scheduled instant: 0–9 whole minutes from a hash of the user id.
pub fn offset(user_id: &str) -> SignedDuration {
    let [first, ..] = sha256(user_id.as_bytes());
    SignedDuration::from_mins(i64::from(first % STAGGER_MINUTES))
}

/// Starts the process's scheduler; `false` (and nothing started) when one already runs.
pub fn spawn(state: AppState) -> bool {
    if SPAWNED.swap(true, Ordering::SeqCst) {
        tracing::warn!(
            "the dashboard scheduler already runs in this process; a second one was not started"
        );
        return false;
    }
    tokio::spawn(run(state));
    true
}

/// Result of one evaluation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tick {
    /// A due user was handled; evaluate again at once.
    Ran,
    /// Nobody was due; the earliest waiting user becomes due at `next`.
    Idle {
        /// Earliest start of a user waiting for their instant plus offset.
        next: Option<Timestamp>,
    },
}

/// The scheduler loop: recovers interrupted runs, then evaluates, sleeps until the next start (at most [`TICK`]),
/// and stops with the server. A failed or panicking evaluation is logged and retried after [`TICK`].
async fn run(state: AppState) {
    let mut stopping = state.stopping.subscribe();
    tracing::info!("dashboard scheduler started");
    if let Some(Err(error)) = guarded(recover(state.clone())).await {
        tracing::warn!(%error, "scheduled runs interrupted by the restart could not be marked failed");
    }
    loop {
        let sleep = match guarded(tick(state.clone())).await {
            Some(Ok(Tick::Ran)) => Duration::ZERO,
            Some(Ok(Tick::Idle { next })) => sleep_until(state.dashboard.now(), next),
            Some(Err(error)) => {
                tracing::warn!(%error, "the dashboard schedule could not be evaluated; it is evaluated again in a minute");
                TICK
            }
            None => TICK,
        };
        tokio::select! {
            biased;
            _ = stopping.wait_for(|stop| *stop) => {
                tracing::info!("dashboard scheduler stopped");
                return;
            }
            () = tokio::time::sleep(sleep) => {}
        }
    }
}

/// Runs `evaluation` as its own task, so a panic is logged and the scheduler survives it.
async fn guarded<T: Send + 'static>(
    evaluation: impl Future<Output = T> + Send + 'static,
) -> Option<T> {
    match tokio::spawn(evaluation).await {
        Ok(result) => Some(result),
        Err(error) => {
            tracing::error!(
                ?error,
                "a scheduled dashboard run panicked; the scheduler goes on with the next evaluation"
            );
            None
        }
    }
}

/// Sleep before the next evaluation: until `next`, between [`MIN_SLEEP`] and [`TICK`].
fn sleep_until(now: Timestamp, next: Option<Timestamp>) -> Duration {
    next.map_or(TICK, |at| {
        Duration::try_from(at - now).unwrap_or(Duration::ZERO)
    })
    .clamp(MIN_SLEEP, TICK)
}

/// A user due now.
#[derive(Clone)]
struct DueRun {
    start_at: Timestamp,
    on: Date,
    instant: Timestamp,
    schedule: Schedule,
    scope: Scope,
}

/// One evaluation: sweeps token pairs no longer kept (ADR-020), then handles the first due user, if any.
pub(crate) async fn tick(state: AppState) -> Result<Tick, DbError> {
    // Session times are real time, like the sign-in path's sweep.
    if let Err(error) = state
        .db
        .sessions()
        .delete_expired(liveness(Timestamp::now()))
        .await
    {
        tracing::warn!(%error, "expired sessions and token pairs could not be swept; the next evaluation tries again");
    }
    let now = state.dashboard.now();
    let mut next: Option<Timestamp> = None;
    let mut due = Vec::new();
    for candidate in state.db.dashboard().schedule_candidates().await? {
        let ScheduleCandidate {
            tenant_id,
            user_id,
            settings,
        } = match candidate {
            Ok(candidate) => candidate,
            Err(unreadable) => {
                tracing::warn!(user_id = %unreadable.user_id, error = %unreadable.error,
                    "a user's dashboard settings could not be read; their scheduled refresh is skipped");
                continue;
            }
        };
        let Some(schedule) = Schedule::of(&settings.settings) else {
            continue;
        };
        let offset = offset(&user_id);
        match schedule.due(now, counted_run_date(settings.last_scheduled.as_ref())) {
            Due::Now { on, .. } if state.dashboard.scheduled_run_given_up(&user_id, on) => {}
            Due::Now { on, instant } if now >= instant + offset => due.push(DueRun {
                start_at: instant + offset,
                on,
                instant,
                schedule,
                scope: Scope { tenant_id, user_id },
            }),
            Due::Now { instant, .. } | Due::Later { instant } => {
                let start_at = instant + offset;
                next = Some(next.map_or(start_at, |n| n.min(start_at)));
            }
        }
    }
    due.sort_by(|a, b| (a.start_at, &a.scope.user_id).cmp(&(b.start_at, &b.scope.user_id)));
    for run in due {
        if *state.stopping.borrow() {
            return Ok(Tick::Idle { next: None });
        }
        match guarded(handle(state.clone(), run.clone())).await {
            Some(Ok(true)) => return Ok(Tick::Ran),
            Some(Ok(false)) => {}
            Some(Err(error)) => {
                tracing::warn!(%error, user_id = %run.scope.user_id,
                    "a user's scheduled dashboard run failed on the server; it is not tried again today");
                give_up(&state, &run).await;
            }
            None => give_up(&state, &run).await,
        }
    }
    Ok(Tick::Idle { next })
}

/// Handles one due user; `false` when they are no longer on the allow-list.
async fn handle(state: AppState, run: DueRun) -> Result<bool, DbError> {
    if !allowed(&state, &run.scope).await? {
        return Ok(false);
    }
    run_due(&state, &run).await?;
    Ok(true)
}

/// Gives the user's run of the date up after a server-side failure, and records it `failed` if the record can be
/// written.
async fn give_up(state: &AppState, run: &DueRun) {
    state
        .dashboard
        .give_up_scheduled_run(&run.scope.user_id, run.on);
    if let Err(error) = record(state, run, ScheduledOutcome::Failed, Some(FAILED_ON_SERVER)).await {
        tracing::debug!(%error, user_id = %run.scope.user_id,
            "a given-up scheduled run could not be recorded; it stays given up until the server restarts");
    }
}

/// Start-up: a run recorded `started` whose refresh never started, or whose scan a restart interrupted, is recorded
/// `failed` ([`INTERRUPTED`]), which does not count for its date; a user whose record cannot be read or written is
/// logged and left as is. Runs after [`refresh::recover`].
pub(crate) async fn recover(state: AppState) -> Result<(), DbError> {
    let dashboard = state.db.dashboard();
    for candidate in dashboard.schedule_candidates().await?.into_iter().flatten() {
        let Some(last) = candidate.settings.last_scheduled else {
            continue;
        };
        let Some(schedule) = Schedule::of(&candidate.settings.settings) else {
            continue;
        };
        if last.outcome != ScheduledOutcome::Started {
            continue;
        }
        let instant = schedule.instant(last.on);
        let latest = match dashboard
            .latest_refresh(&candidate.tenant_id, &candidate.user_id)
            .await
        {
            Ok(latest) => latest,
            Err(error) => {
                tracing::warn!(%error, user_id = %candidate.user_id,
                    "a user's last scheduled run could not be checked after the restart");
                continue;
            }
        };
        let interrupted = match latest {
            None => true,
            Some(refresh) if refresh.started_at < instant => true,
            Some(refresh) => {
                refresh.trigger == RefreshTrigger::Scheduled
                    && refresh
                        .failure
                        .is_some_and(|f| f.code == RefreshFailureCode::Interrupted)
            }
        };
        if interrupted {
            let failed = ScheduledRunRecord {
                on: last.on,
                outcome: ScheduledOutcome::Failed,
                reason: Some(INTERRUPTED.to_owned()),
            };
            match dashboard
                .record_scheduled_run(&candidate.tenant_id, &candidate.user_id, &failed)
                .await
            {
                Ok(()) => tracing::info!(user_id = %candidate.user_id, on = %last.on,
                    "a scheduled dashboard run interrupted by the restart is recorded failed and may be caught up today"),
                Err(error) => tracing::warn!(%error, user_id = %candidate.user_id,
                    "a scheduled dashboard run interrupted by the restart could not be recorded failed"),
            }
        }
    }
    Ok(())
}

/// Whether the user is still on the tenant's sign-in allow-list; if not, the pair kept for the schedule is deleted.
async fn allowed(state: &AppState, scope: &Scope) -> Result<bool, DbError> {
    if state
        .db
        .sessions()
        .user_allowed(&scope.tenant_id, &scope.user_id)
        .await?
    {
        return Ok(true);
    }
    let tokens = state.db.user_tokens();
    if let Some(stored) = tokens.get(&scope.user_id).await? {
        let _lock = lock_user(&scope.user_id).await;
        if tokens
            .end_sign_in(&scope.user_id, PairCondition::Version(stored.version))
            .await?
        {
            tracing::info!(user_id = %scope.user_id,
                "user is no longer on the sign-in allow-list: the scheduled refresh does not run and the GitHub sign-in ended");
        }
    }
    Ok(false)
}

/// Handles a due user's run: skipped after a later successful refresh, `needs_sign_in` without a token pair, else
/// recorded `started` first (once per local date) and started; a failed start or scan updates the record.
async fn run_due(state: &AppState, run: &DueRun) -> Result<(), DbError> {
    let scope = &run.scope;
    let succeeded = state
        .db
        .dashboard()
        .latest_snapshot::<IgnoredAny>(&scope.tenant_id, &scope.user_id)
        .await?
        .and_then(|(record, _)| record.scanned_at)
        .is_some_and(|at| at >= run.instant);
    if succeeded {
        return record(
            state,
            run,
            ScheduledOutcome::Skipped,
            Some(REFRESHED_ALREADY),
        )
        .await;
    }
    // Without a pair nothing can start; an expired or rejected one fails the refresh and ends as in v1.
    if state.db.user_tokens().get(&scope.user_id).await?.is_none() {
        return record(state, run, ScheduledOutcome::NeedsSignIn, Some(NO_PAIR)).await;
    }
    record(state, run, ScheduledOutcome::Started, None).await?;
    // Before the start, so the scan's end is never missed.
    let mut events = state.dashboard.subscribe();
    let (outcome, reason) = match refresh::start(state, scope, RefreshTrigger::Scheduled).await {
        Ok(status) => match scan_end(state, scope, status.id, &mut events).await {
            Some(ended) => match outcome_of(&ended) {
                Some(outcome) => outcome,
                None => return Ok(()),
            },
            None => {
                tracing::warn!(user_id = %scope.user_id, refresh = %status.id,
                    "a scheduled dashboard refresh did not end its scan in time; the scheduler goes on");
                return Ok(());
            }
        },
        Err(StartError::Running) => (ScheduledOutcome::Skipped, ALREADY_RUNNING.to_owned()),
        Err(StartError::RateLimited { retry_at }) => (
            ScheduledOutcome::Failed,
            format!(
                "GitHub's rate limit is used up until {}.",
                retry_at
                    .to_zoned(run.schedule.zone().clone())
                    .strftime("%H:%M")
            ),
        ),
        Err(StartError::Db(error)) => {
            tracing::warn!(%error, user_id = %scope.user_id, "a scheduled dashboard refresh could not be started");
            (ScheduledOutcome::Failed, NOT_STARTED.to_owned())
        }
    };
    record(state, run, outcome, Some(&reason)).await
}

/// The record of a scan that ended: `needs_sign_in` or `failed` with the refresh's message; `None` keeps `started`.
fn outcome_of(ended: &RefreshRecord) -> Option<(ScheduledOutcome, String)> {
    if ended.state != RefreshState::Failed {
        return None;
    }
    Some(match &ended.failure {
        Some(failure) if failure.code == RefreshFailureCode::SignInRequired => {
            (ScheduledOutcome::NeedsSignIn, SIGN_IN_AGAIN.to_owned())
        }
        Some(failure) => (ScheduledOutcome::Failed, failure.message.clone()),
        None => (ScheduledOutcome::Failed, FAILED_WITHOUT_REASON.to_owned()),
    })
}

/// Records the run of the user's local date.
async fn record(
    state: &AppState,
    run: &DueRun,
    outcome: ScheduledOutcome,
    reason: Option<&str>,
) -> Result<(), DbError> {
    let scope = &run.scope;
    let record = ScheduledRunRecord {
        on: run.on,
        outcome,
        reason: reason.map(str::to_owned),
    };
    state
        .db
        .dashboard()
        .record_scheduled_run(&scope.tenant_id, &scope.user_id, &record)
        .await?;
    tracing::info!(user_id = %scope.user_id, on = %run.on, ?outcome, "scheduled dashboard run recorded");
    Ok(())
}

/// Waits until refresh `id` leaves `scanning` (bounded by the scan limit plus [`SCAN_END_MARGIN`]); `None` when it
/// did not, or its state could not be read.
async fn scan_end(
    state: &AppState,
    scope: &Scope,
    id: RefreshId,
    events: &mut broadcast::Receiver<RefreshStatus>,
) -> Option<RefreshRecord> {
    let limit = state.dashboard.scan_limit() + SCAN_END_MARGIN;
    let ended = tokio::time::timeout(limit, async {
        loop {
            match state
                .db
                .dashboard()
                .refresh(&scope.tenant_id, &scope.user_id, id)
                .await
            {
                Ok(record) if record.state != RefreshState::Scanning => return Some(record),
                Ok(_) => {}
                Err(error) => {
                    tracing::debug!(%error, refresh = %id, "a scheduled refresh's state could not be read");
                    return None;
                }
            }
            loop {
                match events.recv().await {
                    Ok(status) if status.id == id => break,
                    Ok(_) => {}
                    Err(RecvError::Lagged(_)) => break,
                    Err(RecvError::Closed) => return None,
                }
            }
        }
    })
    .await;
    ended.ok().flatten()
}

#[cfg(test)]
mod tests;
