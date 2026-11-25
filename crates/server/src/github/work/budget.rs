//! Budget shared by event-driven dashboard reads for a GitHub user.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use contracts::time::Timestamp;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::time::Instant;

const HOUR: Duration = Duration::from_secs(3600);
const POINTS: u64 = 1000;

#[derive(Clone, Default)]
pub(crate) struct EventBudget(pub(super) Arc<Mutex<Spend>>);

#[derive(Default)]
pub(super) struct Spend {
    reads: VecDeque<(Instant, u64)>,
    remaining: Option<(u64, Timestamp)>,
}

impl Spend {
    pub(super) fn permits(&mut self, now: Instant, wall: Timestamp, estimate: u64) -> bool {
        while self
            .reads
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) >= HOUR)
        {
            self.reads.pop_front();
        }
        let spent = self.reads.iter().map(|(_, cost)| *cost).sum::<u64>();
        spent.saturating_add(estimate) <= POINTS
            && self
                .remaining
                .is_none_or(|(remaining, reset)| wall >= reset || remaining >= POINTS)
    }

    pub(super) fn record(
        &mut self,
        now: Instant,
        wall: Timestamp,
        estimate: u64,
        rate: Option<&Value>,
        remaining: Option<u64>,
        reset: Option<Timestamp>,
    ) {
        let cost = rate
            .and_then(|rate| rate["cost"].as_u64())
            .unwrap_or(estimate);
        self.reads.push_back((now, cost.max(1)));
        if let Some(remaining) = remaining.or_else(|| rate.and_then(|r| r["remaining"].as_u64())) {
            let reset = reset
                .or_else(|| rate.and_then(|r| super::timestamp(&r["resetAt"])))
                .unwrap_or(wall + jiff::SignedDuration::from_hours(1));
            self.remaining = Some((remaining, reset));
        }
    }
}
