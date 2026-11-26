//! Fixed dashboard windows and per-PR watch spacing over the durable delivery queue.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::github::{
    ChangeKind, DashboardItem, DashboardRead, DeliveryError, Destination, DispatchOutcome,
    DispatchTarget, PendingDelivery, WebhookChange,
};
use jiff::SignedDuration;
use tokio::time::Instant;
use uuid::Uuid;

use super::worker;
use crate::dashboard::plugin::PrKey;
use crate::dashboard::refresh::{Scope, event};
use crate::dashboard::snapshot::SNAPSHOT_VERSION;
use crate::github::work::{EventBudget, ItemRef};
use crate::review_loop::watch;
use crate::state::AppState;

const WINDOW: Duration = Duration::from_secs(10);
const SCAN_RECHECK: Duration = Duration::from_secs(2);
// Covers an hourly limit reset and one probe interval.
const WATCH_DEFERRAL_LIMIT: SignedDuration = SignedDuration::from_mins(65);
pub(super) const MAX_HELD: usize = 256;

struct Window {
    due: Instant,
    deliveries: Vec<Uuid>,
    items: event::EventItems,
    review: bool,
}

impl Window {
    fn new(now: Instant) -> Self {
        Self {
            due: now + WINDOW,
            deliveries: Vec::new(),
            items: event::EventItems::default(),
            review: false,
        }
    }

    fn merge(&mut self, target: &DispatchTarget) {
        self.review |= target.review;
        for item in &target.items {
            let items = match item.read {
                DashboardRead::Pull => &mut self.items.pulls,
                DashboardRead::Issue => &mut self.items.issues,
                DashboardRead::Candidate => &mut self.items.candidates,
            };
            let key = PrKey::new(&item.repo, item.number);
            if let Some(id) = item.repository_id {
                self.items.pull_repo_ids.insert(key.clone(), id);
            }
            items.insert(
                key,
                ItemRef {
                    repo: item.repo.clone(),
                    number: item.number,
                },
            );
        }
    }

    fn defer(
        &self,
        destination: Destination,
        previous: Option<Timestamp>,
        now: Timestamp,
        delay: Duration,
        limit: SignedDuration,
    ) -> DispatchOutcome {
        if previous.is_some_and(|deadline| deadline <= now) {
            tracing::warn!(
                ?destination,
                "webhook signal held again: its destination is still blocked by a running scan or a GitHub limit; a scan stuck in 'scanning' clears at restart"
            );
        }
        let mut work = DispatchTarget::new(destination, 0);
        work.review = self.review;
        for (read, items) in [
            (DashboardRead::Pull, &self.items.pulls),
            (DashboardRead::Issue, &self.items.issues),
            (DashboardRead::Candidate, &self.items.candidates),
        ] {
            work.items
                .extend(items.iter().map(|(key, item)| DashboardItem {
                    read,
                    repo: item.repo.clone(),
                    number: item.number,
                    repository_id: match read {
                        DashboardRead::Pull => self.items.pull_repo_ids.get(key).copied(),
                        _ => None,
                    },
                }));
        }
        // If the destination is still blocked at expiry, start a fresh hold without spending a retry.
        let hold_until = previous
            .filter(|deadline| *deadline > now)
            .unwrap_or_else(|| now + limit);
        let ready_at = SignedDuration::try_from(delay)
            .ok()
            .and_then(|delay| now.checked_add(delay))
            .map_or(hold_until, |ready| ready.min(hold_until));
        DispatchOutcome::Deferred {
            work,
            ready_at,
            hold_until,
        }
    }
}

struct Held {
    delivery: PendingDelivery,
    remaining: usize,
}

/// Single consumer's fixed windows. Deferred destinations live in SQLite between attempts.
#[derive(Default)]
pub(crate) struct Dispatcher {
    windows: BTreeMap<Destination, Window>,
    held: HashMap<Uuid, Held>,
    budgets: HashMap<String, EventBudget>,
}

impl Dispatcher {
    pub(crate) fn in_flight(&self) -> Vec<Uuid> {
        self.held.keys().copied().collect()
    }

    pub(super) fn next_due(&self) -> Option<Instant> {
        self.windows.values().map(|window| window.due).min()
    }

    pub(crate) async fn enqueue(
        &mut self,
        state: &AppState,
        mut delivery: PendingDelivery,
        now: Instant,
    ) -> Result<(), DbError> {
        let db = state.db.github();
        if !delivery.changes.is_empty() {
            delivery.targets = match route(state, &delivery.changes, delivery.attempts).await {
                Ok(targets) => targets,
                Err(_) => {
                    return worker::complete(
                        &state.db,
                        &delivery,
                        Err(DeliveryError::Dispatch),
                        Timestamp::now(),
                    )
                    .await;
                }
            };
            if !delivery.targets.is_empty()
                && !db.save_targets(&delivery, &delivery.targets).await?
            {
                return Ok(());
            }
        }
        if delivery.targets.is_empty() {
            return db.finish(&delivery, Timestamp::now()).await;
        }
        let remaining = delivery.targets.len();
        for target in delivery.targets.drain(..) {
            let window = self
                .windows
                .entry(target.destination.clone())
                .or_insert_with(|| Window::new(now));
            window.merge(&target);
            window.deliveries.push(delivery.id);
        }
        delivery.changes.clear();
        self.held.insert(
            delivery.id,
            Held {
                delivery,
                remaining,
            },
        );
        Ok(())
    }

    pub(crate) async fn flush(
        &mut self,
        state: &AppState,
        now: Instant,
        wall: Timestamp,
    ) -> Result<(), DbError> {
        let started = Instant::now();
        let due: Vec<_> = self
            .windows
            .iter()
            .filter(|(_, window)| window.due <= now)
            .map(|(destination, _)| destination.clone())
            .collect();
        let db = state.db.github();
        for destination in due {
            let Some(window) = self.windows.get_mut(&destination) else {
                continue;
            };
            let elapsed = started.elapsed();
            let at = wall + SignedDuration::try_from(elapsed).unwrap_or_default();
            let instant = now + elapsed;
            let parked = db.parked_target(&destination, at).await?;
            let previous = parked
                .as_ref()
                .and_then(|row| row.targets.first())
                .and_then(|target| {
                    window.merge(target);
                    target.hold_until
                });
            let outcome = match &destination {
                Destination::Dashboard { tenant, user } => {
                    if !db.dashboard_member(tenant, user).await? {
                        DispatchOutcome::Complete
                    } else {
                        let scope = Scope {
                            tenant_id: tenant.clone(),
                            user_id: user.clone(),
                        };
                        let budget = self.budgets.entry(user.clone()).or_default().clone();
                        match event::reread_items(state, &scope, &window.items, budget, instant)
                            .await
                        {
                            Ok(event::EventRead::Scanning) => window.defer(
                                destination.clone(),
                                previous,
                                at,
                                SCAN_RECHECK,
                                SignedDuration::try_from(
                                    state.dashboard.scan_limit().saturating_add(WINDOW),
                                )
                                .unwrap_or(WATCH_DEFERRAL_LIMIT),
                            ),
                            Ok(event::EventRead::Complete) => DispatchOutcome::Complete,
                            Err(_) => DispatchOutcome::Failed(&worker::BACKOFF),
                        }
                    }
                }
                Destination::Watch { pr } => {
                    match watch::event_probe(
                        state,
                        *pr,
                        at,
                        window.review,
                        &mut self.budgets,
                        instant,
                    )
                    .await
                    {
                        Ok(Some(delay)) => window.defer(
                            destination.clone(),
                            previous,
                            at,
                            delay,
                            WATCH_DEFERRAL_LIMIT,
                        ),
                        Ok(None) => DispatchOutcome::Complete,
                        Err(_) => DispatchOutcome::Failed(&worker::BACKOFF),
                    }
                }
            };
            // Prefer the parked row so waves of new deliveries keep the same carrier.
            let leases: Vec<_> = parked
                .iter()
                .chain(
                    window
                        .deliveries
                        .iter()
                        .filter(|id| parked.as_ref().is_none_or(|parked| parked.id != **id))
                        .filter_map(|id| self.held.get(id).map(|held| &held.delivery)),
                )
                .collect();
            for id in db.settle_target(&destination, &leases, outcome, at).await? {
                worker::warn_exhausted(id, DeliveryError::Dispatch);
            }
            if let Some(window) = self.windows.remove(&destination) {
                for id in window.deliveries {
                    if let Some(held) = self.held.get_mut(&id) {
                        held.remaining -= 1;
                        if held.remaining == 0 {
                            self.held.remove(&id);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

async fn route(
    state: &AppState,
    changes: &[WebhookChange],
    attempts: i64,
) -> Result<Vec<DispatchTarget>, DbError> {
    let github = state.db.github();
    let (dashboard, watched) = tokio::try_join!(
        github.dashboard_targets(changes, SNAPSHOT_VERSION),
        github.watch_targets(changes),
    )?;
    let mut targets = BTreeMap::new();
    for target in dashboard {
        let destination = Destination::Dashboard {
            tenant: target.tenant_id,
            user: target.user_id,
        };
        let signal = changes.get(target.change);
        let read = if target.candidate {
            DashboardRead::Candidate
        } else if matches!(signal.map(|change| change.kind), Some(ChangeKind::Issue)) {
            DashboardRead::Issue
        } else {
            DashboardRead::Pull
        };
        let repository_id = signal
            .filter(|change| {
                matches!(change.kind, ChangeKind::PullRequest)
                    && matches!(change.action.as_deref(), Some("closed" | "reopened"))
            })
            .and_then(|change| change.repository.id);
        targets
            .entry(destination.clone())
            .or_insert_with(|| DispatchTarget::new(destination, attempts))
            .items
            .push(DashboardItem {
                repo: target.repo,
                number: target.number,
                read,
                repository_id,
            });
    }
    let watches = watched.into_iter().map(|(pr, review)| {
        let mut target = DispatchTarget::new(Destination::Watch { pr }, attempts);
        target.review = review;
        target
    });
    Ok(targets.into_values().chain(watches).collect())
}
