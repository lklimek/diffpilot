//! Fixed dashboard windows and per-PR watch spacing over the durable delivery queue.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::github::{ChangeKind, DeliveryError, PendingDelivery};
use tokio::time::Instant;
use uuid::Uuid;

use super::worker;
use crate::dashboard::plugin::PrKey;
use crate::dashboard::refresh::{Scope, event};
use crate::github::work::{EventBudget, ItemRef};
use crate::review_loop::watch;
use crate::state::AppState;

const WINDOW: Duration = Duration::from_secs(10);
pub(super) const MAX_HELD: usize = 256;

struct Window<T> {
    due: Instant,
    deliveries: Vec<Uuid>,
    work: T,
}

impl<T: Default> Window<T> {
    fn new(now: Instant) -> Self {
        Self {
            due: now + WINDOW,
            deliveries: Vec::new(),
            work: T::default(),
        }
    }

    fn attach(&mut self, delivery: Uuid) -> usize {
        if self.deliveries.last() == Some(&delivery) {
            return 0;
        }
        self.deliveries.push(delivery);
        1
    }
}

struct Held {
    delivery: PendingDelivery,
    remaining: usize,
    failed: bool,
}

/// Single consumer's coalescing state. The Queue row remains pending until every routed window finishes.
#[derive(Default)]
pub(crate) struct Dispatcher {
    users: BTreeMap<Scope, Window<event::EventItems>>,
    watches: BTreeMap<i64, Window<bool>>,
    held: HashMap<Uuid, Held>,
    budgets: HashMap<String, EventBudget>,
}

impl Dispatcher {
    pub(crate) fn in_flight(&self) -> Vec<Uuid> {
        self.held.keys().copied().collect()
    }

    pub(super) fn next_due(&self) -> Option<Instant> {
        self.users
            .values()
            .map(|window| window.due)
            .chain(self.watches.values().map(|window| window.due))
            .min()
    }

    pub(crate) async fn enqueue(
        &mut self,
        state: &AppState,
        mut delivery: PendingDelivery,
        now: Instant,
    ) -> Result<(), DbError> {
        if delivery.changes.is_empty() {
            return state.db.github().finish(&delivery, Timestamp::now()).await;
        }
        let github = state.db.github();
        let (targets, watched) = tokio::join!(
            github.dashboard_targets(&delivery.changes),
            github.watch_targets(&delivery.changes),
        );
        let (Ok(targets), Ok(watched)) = (targets, watched) else {
            return worker::complete(
                &state.db,
                &delivery,
                Err(DeliveryError::Dispatch),
                Timestamp::now(),
            )
            .await;
        };

        let id = delivery.id;
        let mut remaining = 0;
        for target in targets {
            let window = self
                .users
                .entry(Scope {
                    tenant_id: target.tenant_id,
                    user_id: target.user_id,
                })
                .or_insert_with(|| Window::new(now));
            let items = if target.candidate {
                &mut window.work.candidates
            } else if matches!(
                delivery
                    .changes
                    .get(target.change)
                    .map(|change| change.kind),
                Some(ChangeKind::Issue)
            ) {
                &mut window.work.issues
            } else {
                &mut window.work.pulls
            };
            items.insert(
                PrKey::new(&target.repo, target.number),
                ItemRef {
                    repo: target.repo,
                    number: target.number,
                },
            );
            remaining += window.attach(id);
        }
        for (pr, review) in watched {
            let window = self.watches.entry(pr).or_insert_with(|| Window::new(now));
            window.work |= review;
            remaining += window.attach(id);
        }
        delivery.changes = Vec::new();
        self.held.insert(
            id,
            Held {
                delivery,
                remaining,
                failed: false,
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
        let scopes: Vec<_> = self
            .users
            .iter()
            .filter(|(_, window)| window.due <= now)
            .map(|(scope, _)| scope.clone())
            .collect();
        for scope in scopes {
            if let Some(window) = self.users.remove(&scope) {
                let budget = self
                    .budgets
                    .entry(scope.user_id.clone())
                    .or_default()
                    .clone();
                let error = event::reread_items(
                    state,
                    &scope,
                    window.work,
                    budget,
                    now + started.elapsed(),
                )
                .await
                .err();
                self.finished(window.deliveries, error.is_some());
            }
        }

        let prs: Vec<_> = self
            .watches
            .iter()
            .filter(|(_, window)| window.due <= now)
            .map(|(pr, _)| *pr)
            .collect();
        for pr in prs {
            if let Some(mut window) = self.watches.remove(&pr) {
                let elapsed = started.elapsed();
                let at =
                    jiff::SignedDuration::try_from(elapsed).map_or(wall, |elapsed| wall + elapsed);
                match watch::event_probe(state, pr, at, window.work).await {
                    Ok(Some(delay)) => {
                        window.due = now + started.elapsed() + delay;
                        self.watches.insert(pr, window);
                    }
                    result => self.finished(window.deliveries, result.is_err()),
                }
            }
        }
        let completed: Vec<_> = self
            .held
            .iter()
            .filter(|(_, held)| held.remaining == 0)
            .map(|(id, _)| *id)
            .collect();
        for id in completed {
            if let Some(held) = self.held.get(&id) {
                let outcome = if held.failed {
                    Err(DeliveryError::Dispatch)
                } else {
                    Ok(())
                };
                worker::complete(&state.db, &held.delivery, outcome, Timestamp::now()).await?;
                self.held.remove(&id);
            }
        }
        Ok(())
    }

    fn finished(&mut self, deliveries: Vec<Uuid>, failed: bool) {
        for id in deliveries {
            if let Some(held) = self.held.get_mut(&id) {
                held.remaining -= 1;
                held.failed |= failed;
            }
        }
    }
}
