//! Per-user refresh registry behind the dashboard API and SSE: the task of each live refresh (abortable, so a
//! cancel stops GitHub calls at the next await), the `refresh` event channel, the clock and the ranker seam.
//! WP11-SCHED and WP11-JUDGE extend it.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use contracts::api::dashboard::{ItemKey, RefreshStatus};
use contracts::ids::RefreshId;
use contracts::time::Timestamp;
use jiff::SignedDuration;
use jiff::civil::Date;
use tokio::sync::broadcast;
use tokio::task::AbortHandle;

use super::judge::JudgeRanker;
use super::refresh::Scope;
#[cfg(test)]
use super::refresh::rank::NoopRanker;
use super::refresh::rank::Ranker;
use crate::github::work::SEARCH_INTERVAL;

/// A whole scan may take this long (plan §A.3); then the refresh fails as "took too long".
pub const SCAN_LIMIT: Duration = Duration::from_secs(5 * 60);
/// Status events buffered per subscriber; a slower reader re-reads the status.
const EVENTS_CAPACITY: usize = 256;
/// Minimum gap between warnings for the same dashboard tally failure.
const TALLY_WARNING_INTERVAL: Duration = Duration::from_secs(60);

/// Where the dashboard reads the time.
#[derive(Clone, Copy, Debug)]
pub enum Clock {
    /// The system clock.
    System,
    /// `at` when created, advancing with tokio's clock (tests with paused time).
    Fixed {
        /// Time at `since`.
        at: Timestamp,
        /// Tokio instant of `at`.
        since: tokio::time::Instant,
    },
}

impl Clock {
    /// A clock reading `at` now and advancing with tokio's (possibly paused) clock.
    pub fn starting_at(at: Timestamp) -> Self {
        Self::Fixed {
            at,
            since: tokio::time::Instant::now(),
        }
    }

    /// The current time.
    pub fn now(&self) -> Timestamp {
        match self {
            Self::System => Timestamp::now(),
            Self::Fixed { at, since } => {
                *at + SignedDuration::try_from(since.elapsed()).unwrap_or(SignedDuration::MAX)
            }
        }
    }
}

/// In-process dashboard state shared by all handlers; cheap to clone.
#[derive(Clone)]
pub struct DashboardHub {
    inner: Arc<Inner>,
}

struct Inner {
    clock: Clock,
    scan_limit: Duration,
    search_interval: Duration,
    ranker: Arc<dyn Ranker>,
    tasks: Mutex<Tasks>,
    events: broadcast::Sender<RefreshStatus>,
    stopped: broadcast::Sender<RefreshId>,
    /// Scheduled runs given up for a user's local date after a failure the run record could not hold.
    given_up: Mutex<HashSet<(String, Date)>>,
    scheduler_started: AtomicBool,
    // None is the batch itself. Pull-request ids share one database across users.
    tally_warnings: Mutex<HashMap<Option<i64>, tokio::time::Instant>>,
    readers: Readers,
}

type UserKey = (String, String);

fn key(scope: &Scope) -> UserKey {
    (scope.tenant_id.clone(), scope.user_id.clone())
}

#[derive(Debug, Default)]
struct ReaderState {
    limited_until: HashMap<UserKey, Timestamp>,
    writers: HashMap<UserKey, Arc<tokio::sync::Mutex<()>>>,
    rejudges: HashMap<UserKey, Rejudges>,
}

#[derive(Debug, Default)]
struct Rejudges {
    waiting: HashSet<ItemKey>,
    dedicated: Option<DedicatedRejudges>,
}

#[derive(Debug)]
struct DedicatedRejudges {
    refresh: RefreshId,
    attempt: u32,
    targets: HashSet<ItemKey>,
}

/// Serializes snapshot reads that write facts and ranking transitions, and remembers GitHub rate limits.
#[derive(Debug, Default)]
pub(crate) struct Readers {
    state: Mutex<ReaderState>,
}

impl Readers {
    fn state(&self) -> MutexGuard<'_, ReaderState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Coordinates snapshot writers with re-judge requests and the ranking drain.
    pub(crate) async fn writer(&self, scope: &Scope) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = self.state().writers.entry(key(scope)).or_default().clone();
        lock.lock_owned().await
    }

    /// Tracks accepted clicks until a ranking takes responsibility for them. Writers hold the user's guard.
    pub(super) fn queue_rejudge(&self, scope: &Scope, item: ItemKey) {
        self.state()
            .rejudges
            .entry(key(scope))
            .or_default()
            .waiting
            .insert(item);
    }

    pub(super) fn has_waiting_rejudges(&self, scope: &Scope) -> bool {
        self.state()
            .rejudges
            .get(&key(scope))
            .is_some_and(|work| !work.waiting.is_empty())
    }

    pub(super) fn start_rejudge_attempt(&self, scope: &Scope, refresh: RefreshId, attempt: u32) {
        let mut state = self.state();
        let work = state.rejudges.entry(key(scope)).or_default();
        work.dedicated = Some(DedicatedRejudges {
            refresh,
            attempt,
            targets: std::mem::take(&mut work.waiting),
        });
    }

    pub(crate) fn rejudge_targets(
        &self,
        scope: &Scope,
        refresh: RefreshId,
        attempt: u32,
    ) -> Option<HashSet<ItemKey>> {
        let state = self.state();
        let work = state.rejudges.get(&key(scope))?;
        let dedicated = work.dedicated.as_ref()?;
        (dedicated.refresh == refresh && dedicated.attempt == attempt)
            .then(|| dedicated.targets.union(&work.waiting).cloned().collect())
    }

    pub(super) fn rejudges_dispatched<'a>(
        &self,
        scope: &Scope,
        refresh: RefreshId,
        attempt: u32,
        items: impl Iterator<Item = &'a ItemKey>,
    ) {
        let mut state = self.state();
        let Some(work) = state.rejudges.get_mut(&key(scope)) else {
            return;
        };
        for item in items {
            if let Some(item) = work.waiting.take(item)
                && let Some(dedicated) = work.dedicated.as_mut()
                && dedicated.refresh == refresh
                && dedicated.attempt == attempt
            {
                dedicated.targets.insert(item);
            }
        }
        if work.dedicated.is_none() && work.waiting.is_empty() {
            state.rejudges.remove(&key(scope));
        }
    }

    pub(super) fn end_rejudge_attempt(&self, scope: &Scope, refresh: RefreshId, attempt: u32) {
        let mut state = self.state();
        let Some(work) = state.rejudges.get_mut(&key(scope)) else {
            return;
        };
        if work
            .dedicated
            .as_ref()
            .is_some_and(|dedicated| dedicated.refresh == refresh && dedicated.attempt == attempt)
        {
            work.dedicated = None;
        }
        if work.dedicated.is_none() && work.waiting.is_empty() {
            state.rejudges.remove(&key(scope));
        }
    }

    pub(super) fn cancel_rejudges(&self, scope: &Scope) {
        self.state().rejudges.remove(&key(scope));
    }

    /// The user's GitHub backoff, shared by scans and targeted re-reads.
    pub(crate) fn limited_until(&self, scope: &Scope, now: Timestamp) -> Option<Timestamp> {
        self.state()
            .limited_until
            .get(&key(scope))
            .copied()
            .filter(|at| *at > now)
    }

    /// Remembers a rate limit found outside a full scan; unknown retry times wait five minutes.
    pub(crate) fn remember_limit(
        &self,
        scope: &Scope,
        error: &crate::github::work::WorkError,
        now: Timestamp,
    ) {
        use crate::github::work::WorkError;
        let retry = match error {
            WorkError::PrimaryRateLimit { reset } => *reset,
            WorkError::SecondaryRateLimit { retry_after } => retry_after
                .and_then(|wait| SignedDuration::try_from(wait).ok())
                .and_then(|wait| now.checked_add(wait)),
            _ => return,
        };
        let retry = retry.unwrap_or(now + SignedDuration::from_mins(5));
        let mut state = self.state();
        state
            .limited_until
            .entry(key(scope))
            .and_modify(|at| *at = (*at).max(retry))
            .or_insert(retry);
    }
}

/// Announces the end of a refresh's task, also when it is aborted (dropped at an await).
struct StopSignal {
    hub: DashboardHub,
    id: RefreshId,
}

impl Drop for StopSignal {
    fn drop(&mut self) {
        // No subscriber is fine: only tests wait for it.
        let _ = self.hub.inner.stopped.send(self.id);
    }
}

/// The task of each live refresh, with the generation that started it (a restarted ranking replaces it).
#[derive(Default)]
struct Tasks {
    next: u64,
    running: HashMap<RefreshId, (u64, AbortHandle)>,
}

impl fmt::Debug for DashboardHub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DashboardHub")
            .field("clock", &self.inner.clock)
            .field("ranker", &self.inner.ranker)
            .finish_non_exhaustive()
    }
}

impl Default for DashboardHub {
    fn default() -> Self {
        Self::new(Clock::System, SCAN_LIMIT, Arc::new(JudgeRanker))
    }
}

impl DashboardHub {
    /// A hub reading `clock`, failing scans after `scan_limit` and ranking with `ranker`.
    pub fn new(clock: Clock, scan_limit: Duration, ranker: Arc<dyn Ranker>) -> Self {
        Self::with_search_interval(clock, scan_limit, ranker, SEARCH_INTERVAL)
    }

    /// [`Self::new`] pacing searches `search_interval` apart: tests on real time only, production keeps 3 s.
    #[cfg(test)]
    pub(crate) fn testing(
        clock: Clock,
        scan_limit: Duration,
        ranker: Arc<dyn Ranker>,
        search_interval: Duration,
    ) -> Self {
        Self::with_search_interval(clock, scan_limit, ranker, search_interval)
    }

    fn with_search_interval(
        clock: Clock,
        scan_limit: Duration,
        ranker: Arc<dyn Ranker>,
        search_interval: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                clock,
                scan_limit,
                search_interval,
                ranker,
                tasks: Mutex::new(Tasks::default()),
                events: broadcast::channel(EVENTS_CAPACITY).0,
                stopped: broadcast::channel(EVENTS_CAPACITY).0,
                given_up: Mutex::default(),
                scheduler_started: AtomicBool::default(),
                tally_warnings: Mutex::default(),
                readers: Readers::default(),
            }),
        }
    }

    pub(super) fn claim_scheduler(&self) -> bool {
        !self.inner.scheduler_started.swap(true, Ordering::SeqCst)
    }

    /// The current time.
    pub fn now(&self) -> Timestamp {
        self.inner.clock.now()
    }

    /// How long a scan may take.
    pub fn scan_limit(&self) -> Duration {
        self.inner.scan_limit
    }

    /// Gap between two GitHub searches of a scan.
    pub fn search_interval(&self) -> Duration {
        self.inner.search_interval
    }

    /// Coordinates targeted re-reads of the same snapshot.
    pub(crate) fn readers(&self) -> &Readers {
        &self.inner.readers
    }

    /// The ranking step.
    pub fn ranker(&self) -> Arc<dyn Ranker> {
        Arc::clone(&self.inner.ranker)
    }

    /// Reserves one warning per batch or PR across dashboard polls during the warning interval.
    pub(crate) fn tally_warning_due(&self, pr: Option<i64>) -> bool {
        let now = tokio::time::Instant::now();
        let mut warnings = self
            .inner
            .tally_warnings
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        warnings.retain(|_, deadline| *deadline > now);
        let std::collections::hash_map::Entry::Vacant(entry) = warnings.entry(pr) else {
            return false;
        };
        entry.insert(now + TALLY_WARNING_INTERVAL);
        true
    }

    /// Status events of every refresh; readers keep their own refresh's.
    pub fn subscribe(&self) -> broadcast::Receiver<RefreshStatus> {
        self.inner.events.subscribe()
    }

    /// Sends a refresh's status, already written to the database, to its subscribers.
    pub fn publish(&self, status: RefreshStatus) {
        // No subscriber is fine: a stream opened later reads the stored status first.
        let _ = self.inner.events.send(status);
    }

    fn tasks(&self) -> MutexGuard<'_, Tasks> {
        self.inner
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `task` as the live task of refresh `id`.
    pub(crate) fn spawn(&self, id: RefreshId, task: impl Future<Output = ()> + Send + 'static) {
        let mut tasks = self.tasks();
        tasks.next += 1;
        let generation = tasks.next;
        let hub = self.clone();
        let signal = StopSignal {
            hub: self.clone(),
            id,
        };
        let handle = tokio::spawn(async move {
            let _signal = signal;
            task.await;
            let mut tasks = hub.tasks();
            if tasks
                .running
                .get(&id)
                .is_some_and(|(g, _)| *g == generation)
            {
                tasks.running.remove(&id);
            }
        });
        tasks
            .running
            .insert(id, (generation, handle.abort_handle()));
    }

    /// Stops the task of refresh `id` at its next await; `false` when none runs.
    pub(crate) fn abort(&self, id: RefreshId) -> bool {
        let stopped = self.tasks().running.remove(&id);
        if let Some((_, handle)) = &stopped {
            handle.abort();
        }
        stopped.is_some()
    }

    /// Ids of refresh tasks as they end, completed or aborted.
    pub fn stopped(&self) -> broadcast::Receiver<RefreshId> {
        self.inner.stopped.subscribe()
    }

    fn given_up_runs(&self) -> MutexGuard<'_, HashSet<(String, Date)>> {
        self.inner
            .given_up
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Gives up the user's scheduled run of local date `on` (the scheduler skips it); forgets older dates.
    pub(crate) fn give_up_scheduled_run(&self, user_id: &str, on: Date) {
        let mut given_up = self.given_up_runs();
        given_up.retain(|(_, date)| *date >= on.yesterday().unwrap_or(on));
        given_up.insert((user_id.to_owned(), on));
    }

    /// Whether the user's scheduled run of local date `on` was given up.
    pub(crate) fn scheduled_run_given_up(&self, user_id: &str, on: Date) -> bool {
        self.given_up_runs().contains(&(user_id.to_owned(), on))
    }

    /// Whether a task of refresh `id` runs.
    pub fn is_running(&self, id: RefreshId) -> bool {
        self.tasks()
            .running
            .get(&id)
            .is_some_and(|(_, handle)| !handle.is_finished())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn tally_warnings_are_throttled_independently_per_pull_request_and_batch() {
        let hub = DashboardHub::default();
        assert!(hub.tally_warning_due(None));
        assert!(hub.tally_warning_due(Some(1)));
        assert!(hub.tally_warning_due(Some(2)));
        assert!(!hub.tally_warning_due(Some(1)));
        assert!(!hub.tally_warning_due(None));
        tokio::time::advance(std::time::Duration::from_secs(59)).await;
        assert!(!hub.tally_warning_due(Some(1)));
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        assert!(hub.tally_warning_due(Some(1)));
        assert!(hub.tally_warning_due(None));
    }

    #[test]
    fn production_hubs_pace_searches_three_seconds_apart() {
        assert_eq!(SEARCH_INTERVAL, Duration::from_secs(3));
        assert_eq!(DashboardHub::default().search_interval(), SEARCH_INTERVAL);
        let hub = DashboardHub::new(Clock::System, SCAN_LIMIT, Arc::new(NoopRanker));
        assert_eq!(hub.search_interval(), SEARCH_INTERVAL);
    }
}
