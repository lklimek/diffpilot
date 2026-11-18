//! Per-user refresh registry behind the dashboard API and SSE: the task of each live refresh (abortable, so a
//! cancel stops GitHub calls at the next await), the `refresh` event channel, the clock and the ranker seam.
//! WP11-SCHED and WP11-JUDGE extend it.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use contracts::api::dashboard::RefreshStatus;
use contracts::ids::RefreshId;
use contracts::time::Timestamp;
use jiff::SignedDuration;
use jiff::civil::Date;
use tokio::sync::broadcast;
use tokio::task::AbortHandle;

use super::judge::JudgeRanker;
#[cfg(test)]
use super::refresh::rank::NoopRanker;
use super::refresh::rank::Ranker;
use crate::github::work::SEARCH_INTERVAL;

/// A whole scan may take this long (plan §A.3); then the refresh fails as "took too long".
pub const SCAN_LIMIT: Duration = Duration::from_secs(5 * 60);
/// Status events buffered per subscriber; a slower reader re-reads the status.
const EVENTS_CAPACITY: usize = 256;

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
            }),
        }
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

    /// The ranking step.
    pub fn ranker(&self) -> Arc<dyn Ranker> {
        Arc::clone(&self.inner.ranker)
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

    #[test]
    fn production_hubs_pace_searches_three_seconds_apart() {
        assert_eq!(SEARCH_INTERVAL, Duration::from_secs(3));
        assert_eq!(DashboardHub::default().search_interval(), SEARCH_INTERVAL);
        let hub = DashboardHub::new(Clock::System, SCAN_LIMIT, Arc::new(NoopRanker));
        assert_eq!(hub.search_interval(), SEARCH_INTERVAL);
    }
}
