//! Resolved webhook destinations and durable coalescing.

use contracts::common::RepoRef;
use contracts::time::Timestamp;
use jiff::SignedDuration;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{DeliveryError, Github, PendingDelivery, RESUME_WINDOW, WebhookChange, ts, write_tx};
use crate::error::DbError;

/// One independent destination of a webhook signal.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Destination {
    /// A member's dashboard.
    Dashboard {
        /// The tenant owning the dashboard.
        tenant: String,
        /// The member whose token and snapshot are used.
        user: String,
    },
    /// Review-loop state of a pull request with a watch or a stored Comments read.
    Watch {
        /// The stored PR id, scoped to its tenant.
        pr: i64,
    },
}

/// How a dashboard destination should read an item.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardRead {
    /// Pull request in the ranking or eligible for a history update.
    Pull,
    /// Existing issue.
    Issue,
    /// Pull request that may be new to the snapshot.
    Candidate,
}

/// An item already resolved to a specific dashboard.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DashboardItem {
    /// Repository name from the resolved target.
    pub repo: RepoRef,
    /// Pull request or issue number.
    pub number: u32,
    /// The read requested for this item.
    pub read: DashboardRead,
    /// Signed GitHub identity for a pull request lifecycle re-read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<i64>,
}

/// The unfinished work for one destination of a delivery.
#[derive(Debug, Deserialize, Serialize)]
pub struct DispatchTarget {
    /// The destination independently scheduled by the worker.
    pub destination: Destination,
    /// Items to re-read for a dashboard.
    pub items: Vec<DashboardItem>,
    /// A watch signal that requires the full review read.
    pub review: bool,
    attempts: i64,
    ready_at: Option<Timestamp>,
    /// The end of this target's current hold.
    pub hold_until: Option<Timestamp>,
    exhausted: bool,
}

impl DispatchTarget {
    /// Creates a target ready for dispatch.
    pub fn new(destination: Destination, attempts: i64) -> Self {
        Self {
            destination,
            items: Vec::new(),
            review: false,
            attempts,
            ready_at: None,
            hold_until: None,
            exhausted: false,
        }
    }

    pub(super) fn ready(&self, eligible_at: &str) -> bool {
        // Match the SQLite queue's fixed-width, microsecond timestamp comparison.
        !self.exhausted
            && self
                .ready_at
                .is_none_or(|at| ts(at).as_str() <= eligible_at)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum Payload {
    Targets(Vec<DispatchTarget>),
    Hints(Vec<WebhookChange>),
}

/// How to settle the work attached to a destination's dispatch window.
pub enum DispatchOutcome {
    /// The destination has no work left.
    Complete,
    /// Keep one scheduled carrier for the merged work.
    Deferred {
        /// Combined items and review signal from the window.
        work: DispatchTarget,
        /// Earliest time to process the carrier again.
        ready_at: Timestamp,
        /// The current hold's deadline.
        hold_until: Timestamp,
    },
    /// The operation failed and uses the worker's retry intervals.
    Failed(&'static [SignedDuration]),
}

impl Github<'_> {
    /// Records a delivery's initial fan-out, unless redelivery replaced its lease.
    pub async fn save_targets(
        &self,
        delivery: &PendingDelivery,
        targets: &[DispatchTarget],
    ) -> Result<bool, DbError> {
        let result = sqlx::query(
            "UPDATE github_deliveries SET changes = ?
             WHERE delivery_id = ? AND received_at = ? AND attempts = ? AND state = 'pending'",
        )
        .bind(serde_json::to_string(targets)?)
        .bind(delivery.id.to_string())
        .bind(&delivery.received_at_raw)
        .bind(delivery.attempts)
        .execute(self.pool)
        .await?;
        Ok(result.rows_affected() != 0)
    }

    /// Finds the parked carrier for this destination, including one whose timer has not elapsed.
    pub async fn parked_target(
        &self,
        destination: &Destination,
        now: Timestamp,
    ) -> Result<Option<PendingDelivery>, DbError> {
        let row = sqlx::query_as(
            "SELECT delivery_id, changes, attempts, received_at FROM github_deliveries
             WHERE state = 'pending' AND (received_at >= ?1 OR processed_at >= ?1)
               AND EXISTS (SELECT 1 FROM json_each(changes)
                           WHERE value -> '$.destination' = json(?2)
                             AND value ->> '$.hold_until' IS NOT NULL)
             ORDER BY received_at, rowid LIMIT 1",
        )
        .bind(ts(now - RESUME_WINDOW))
        .bind(serde_json::to_string(destination)?)
        .fetch_optional(self.pool)
        .await?;
        row.map(|row| {
            let mut pending = PendingDelivery::from_row(row)?;
            pending
                .targets
                .retain(|target| &target.destination == destination);
            Ok(pending)
        })
        .transpose()
    }

    /// Checks that a dashboard destination still belongs to an admitted member.
    pub async fn dashboard_member(&self, tenant: &str, user: &str) -> Result<bool, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM memberships WHERE tenant_id = ? AND user_id = ?)",
        )
        .bind(tenant)
        .bind(user)
        .fetch_one(self.pool)
        .await?)
    }

    /// Settles only this destination on each delivery, atomically moving deferred work to the first valid lease.
    /// Returns the delivery ids whose failure exhausted the retry intervals.
    pub async fn settle_target(
        &self,
        destination: &Destination,
        deliveries: &[&PendingDelivery],
        outcome: DispatchOutcome,
        now: Timestamp,
    ) -> Result<Vec<Uuid>, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let mut exhausted = Vec::new();
        let (mut deferred, backoff) = match outcome {
            DispatchOutcome::Complete => (None, None),
            DispatchOutcome::Deferred {
                mut work,
                ready_at,
                hold_until,
            } => {
                work.ready_at = Some(ready_at);
                work.hold_until = Some(hold_until);
                (Some(work), None)
            }
            DispatchOutcome::Failed(backoff) => (None, Some(backoff)),
        };
        for delivery in deliveries {
            // Another window may have updated this carrier's other destinations.
            let Some(row) = sqlx::query_as(
                "SELECT delivery_id, changes, attempts, received_at FROM github_deliveries
                 WHERE delivery_id = ? AND received_at = ? AND state = 'pending'",
            )
            .bind(delivery.id.to_string())
            .bind(&delivery.received_at_raw)
            .fetch_optional(&mut *tx)
            .await?
            else {
                continue;
            };
            let mut current = PendingDelivery::from_row(row)?;
            let Some(index) = current
                .targets
                .iter()
                .position(|task| &task.destination == destination)
            else {
                continue;
            };
            let mut target = current.targets.swap_remove(index);
            if let Some(mut carrier) = deferred.take() {
                carrier.attempts = target.attempts;
                current.targets.push(carrier);
            } else if let Some(backoff) = backoff {
                let delay = usize::try_from(target.attempts)
                    .ok()
                    .and_then(|n| backoff.get(n));
                target.attempts += 1;
                target.ready_at = delay.map(|delay| now + *delay);
                target.hold_until = None;
                target.exhausted = delay.is_none();
                current.attempts = current.attempts.max(target.attempts);
                current.targets.push(target);
            }
            let next = current
                .targets
                .iter()
                .filter(|task| !task.exhausted)
                .map(|task| task.ready_at)
                .min();
            let (state, at) = match next {
                Some(at) => ("pending", Some(at.unwrap_or(now))),
                None if current.targets.is_empty() => ("done", Some(now)),
                None => {
                    exhausted.push(delivery.id);
                    ("failed", Some(now))
                }
            };
            let error = if backoff.is_some() || current.targets.iter().any(|task| task.exhausted) {
                Some(DeliveryError::Dispatch.to_string())
            } else {
                None
            };
            sqlx::query(
                "UPDATE github_deliveries SET changes = ?, processed_at = ?, state = ?, attempts = ?, error = ?
                 WHERE delivery_id = ?",
            )
            .bind(serde_json::to_string(&current.targets)?)
            .bind(at.map(ts))
            .bind(state)
            .bind(current.attempts)
            .bind(error)
            .bind(delivery.id.to_string())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(exhausted)
    }
}
