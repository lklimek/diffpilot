//! My work kept fresh while it is open (plan §M): a summary read in a browser session opens it for [`OPEN_FOR`];
//! while anyone has it open, one in-process task evaluates every [`TICK`] per open user, from the stored refreshes and
//! the in-memory presence only, whether to start an automatic scan (F2) or to check the listed pull requests' facts
//! (F3); with nobody open it sleeps and reads nothing.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use contracts::api::dashboard::{RefreshFailureCode, RefreshState, RefreshTrigger};
use contracts::ids::RefreshId;
use contracts::time::Timestamp;
use db::repo::dashboard::RefreshRecord;
use jiff::SignedDuration;
use tokio::sync::Notify;

use super::refresh::reread::{self, Reread};
use super::refresh::{self, Scope, StartError};
use crate::state::AppState;

/// My work counts as open this long after the last summary read (the page reads it every 60 s while visible).
pub const OPEN_FOR: SignedDuration = SignedDuration::from_mins(3);
/// Gap between two evaluations while anyone has My work open.
pub const TICK: Duration = Duration::from_secs(60);
/// A fact check runs when the facts were last read this long ago (F3).
pub const FACTS_EVERY: SignedDuration = SignedDuration::from_mins(5);
/// An automatic scan starts when the newest refresh started this long ago (F2).
pub const SCAN_EVERY: SignedDuration = SignedDuration::from_mins(30);
/// On opening, an automatic scan starts when the newest refresh started before the visit and this long ago (F2).
pub const OPEN_SCAN_AFTER: SignedDuration = SignedDuration::from_mins(15);

type UserKey = (String, String);

fn key(scope: &Scope) -> UserKey {
    (scope.tenant_id.clone(), scope.user_id.clone())
}

/// One visit: since when the user has My work open, and its last read.
#[derive(Clone, Copy, Debug)]
struct Visit {
    since: Timestamp,
    last: Timestamp,
}

#[derive(Debug, Default)]
struct Presence {
    open: HashMap<UserKey, Visit>,
    /// Each user's last fact check and the refresh whose snapshot it checked.
    checked: HashMap<UserKey, (RefreshId, Timestamp)>,
}

/// Who has My work open, and when each user's facts were last checked; in memory (a restart forgets both).
#[derive(Debug, Default)]
pub struct Readers {
    presence: Mutex<Presence>,
    woken: Notify,
}

impl Readers {
    fn presence(&self) -> MutexGuard<'_, Presence> {
        self.presence.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records a summary read of the user at `now`; a read after a gap longer than [`OPEN_FOR`] begins a new visit
    /// and wakes the task.
    pub fn mark_open(&self, scope: &Scope, now: Timestamp) {
        let mut presence = self.presence();
        let previous = presence.open.get(&key(scope)).copied();
        let new = previous.is_none_or(|visit| now.duration_since(visit.last) > OPEN_FOR);
        let since = previous.filter(|_| !new).map_or(now, |visit| visit.since);
        presence.open.insert(key(scope), Visit { since, last: now });
        drop(presence);
        if new {
            self.woken.notify_one();
        }
    }

    /// The users with My work open at `now` and when their visit began, in user order; forgets the others.
    pub fn open(&self, now: Timestamp) -> Vec<(Scope, Timestamp)> {
        let mut presence = self.presence();
        presence
            .open
            .retain(|_, visit| now.duration_since(visit.last) <= OPEN_FOR);
        let mut open: Vec<_> = presence
            .open
            .iter()
            .map(|((tenant_id, user_id), visit)| {
                (
                    Scope {
                        tenant_id: tenant_id.clone(),
                        user_id: user_id.clone(),
                    },
                    visit.since,
                )
            })
            .collect();
        open.sort_by(|a, b| (&a.0.tenant_id, &a.0.user_id).cmp(&(&b.0.tenant_id, &b.0.user_id)));
        open
    }

    /// Records a fact check of the snapshot of refresh `id` at `now`.
    pub fn checked(&self, scope: &Scope, id: RefreshId, now: Timestamp) {
        self.presence().checked.insert(key(scope), (id, now));
    }

    /// When the facts of the snapshot of refresh `id` were last checked.
    pub fn checked_at(&self, scope: &Scope, id: RefreshId) -> Option<Timestamp> {
        self.presence()
            .checked
            .get(&key(scope))
            .filter(|(checked, _)| *checked == id)
            .map(|(_, at)| *at)
    }

    fn last_check(&self, scope: &Scope) -> Option<Timestamp> {
        self.presence().checked.get(&key(scope)).map(|(_, at)| *at)
    }
}

/// What a tick does for one open user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Nothing now.
    Nothing,
    /// Start an automatic scan (F2).
    Scan,
    /// Check the listed pull requests' facts (F3).
    CheckFacts,
}

/// The step for a user whose visit began at `since`, given their newest refresh and when their facts were last read
/// (the snapshot's scan or a fact check): the first of F1 (a live refresh or the rate limit: nothing), F2, F3.
pub(crate) fn step(
    now: Timestamp,
    since: Timestamp,
    latest: Option<&RefreshRecord>,
    facts_read_at: Option<Timestamp>,
) -> Step {
    let ago = |at: Timestamp| now.duration_since(at);
    if let Some(latest) = latest {
        let live = matches!(latest.state, RefreshState::Scanning | RefreshState::Ranking);
        let limited = latest.failure.as_ref().is_some_and(|failure| {
            failure.code == RefreshFailureCode::GithubRateLimited
                && failure.retry_at.is_some_and(|at| at > now)
        });
        if live || limited {
            return Step::Nothing;
        }
    }
    let scan = latest.is_none_or(|latest| {
        let started = latest.started_at;
        ago(started) >= SCAN_EVERY || (started < since && ago(started) >= OPEN_SCAN_AFTER)
    });
    if scan {
        Step::Scan
    } else if facts_read_at.is_none_or(|at| ago(at) >= FACTS_EVERY) {
        Step::CheckFacts
    } else {
        Step::Nothing
    }
}

/// One evaluation at `now` for every open user; at most one automatic scan starts.
pub(crate) async fn tick(state: &AppState, now: Timestamp) {
    let readers = state.dashboard.readers();
    let mut scan_started = false;
    for (scope, since) in readers.open(now) {
        let latest = match state
            .db
            .dashboard()
            .latest_refresh(&scope.tenant_id, &scope.user_id)
            .await
        {
            Ok(latest) => latest,
            Err(error) => {
                tracing::warn!(user_id = %scope.user_id, %error,
                    "the newest dashboard refresh could not be read for the open-page check; trying next minute");
                continue;
            }
        };
        let scanned = latest
            .as_ref()
            .filter(|r| matches!(r.state, RefreshState::Ranking | RefreshState::Finished))
            .and_then(|r| r.scanned_at);
        let facts_read_at = scanned.max(readers.last_check(&scope));
        match step(now, since, latest.as_ref(), facts_read_at) {
            Step::Nothing => {}
            Step::Scan if scan_started => {}
            Step::Scan => {
                scan_started = true;
                start_scan(state, &scope).await;
            }
            Step::CheckFacts => check_facts(state, &scope).await,
        }
    }
}

async fn start_scan(state: &AppState, scope: &Scope) {
    match refresh::start(state, scope, RefreshTrigger::Automatic).await {
        Ok(status) => {
            tracing::info!(user_id = %scope.user_id, refresh_id = %status.id,
                "automatic dashboard refresh started while My work is open");
        }
        Err(StartError::Running | StartError::RateLimited { .. }) => {}
        Err(error) => {
            tracing::warn!(user_id = %scope.user_id, %error, error_chain = ?error,
                "the automatic dashboard refresh could not start; trying again next minute");
        }
    }
}

async fn check_facts(state: &AppState, scope: &Scope) {
    match reread::check(state, scope).await {
        Ok(Reread::Written { updated, closed }) => {
            tracing::info!(user_id = %scope.user_id, updated, closed,
                "dashboard pull requests that changed on GitHub were read again while My work is open");
        }
        Ok(Reread::Unchanged) => {
            tracing::trace!(user_id = %scope.user_id, "dashboard fact check found no change");
        }
        Ok(Reread::Skipped(why)) => {
            tracing::trace!(user_id = %scope.user_id, why, "dashboard fact check skipped");
        }
        Err(error) => {
            tracing::debug!(user_id = %scope.user_id, %error, error_chain = ?error,
                "dashboard fact check failed; the next check or refresh reads them");
        }
    }
}

/// Starts the task: it sleeps while nobody has My work open, evaluates every [`TICK`] otherwise and stops with the
/// server.
pub fn spawn(state: AppState) {
    tokio::spawn(run(state));
}

async fn run(state: AppState) {
    let mut stopping = state.stopping.subscribe();
    let readers = state.dashboard.readers();
    tracing::info!("dashboard open-page freshness task started");
    loop {
        while readers.open(state.dashboard.now()).is_empty() {
            tokio::select! {
                () = readers.woken.notified() => {}
                _ = stopping.wait_for(|stop| *stop) => return,
            }
        }
        tick(&state, state.dashboard.now()).await;
        tokio::select! {
            () = tokio::time::sleep(TICK) => {}
            _ = stopping.wait_for(|stop| *stop) => return,
        }
    }
}

#[cfg(test)]
mod tests;
