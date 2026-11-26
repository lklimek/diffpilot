//! Budget shared by dashboard reads and watch probes triggered by GitHub events.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use contracts::time::Timestamp;
use serde_json::Value;
use tokio::sync::{Mutex, MutexGuard};
use tokio::time::Instant;

const HOUR: Duration = Duration::from_secs(3600);
const POINTS: u64 = 1000;

#[derive(Clone, Default)]
pub(crate) struct EventBudget(Arc<Mutex<Spend>>);

impl EventBudget {
    /// Earliest retry for a one-point event probe or its follow-up read.
    pub(crate) async fn blocked_for(&self, now: Instant, wall: Timestamp) -> Duration {
        self.0.lock().await.delay(now, wall, 1)
    }

    /// Reserves room for one request until its reported cost can be recorded.
    pub(crate) async fn reserve(
        &self,
        now: Instant,
        wall: Timestamp,
        estimate: u64,
    ) -> Result<EventCharge<'_>, Duration> {
        let mut spend = self.0.lock().await;
        let delay = spend.delay(now, wall, estimate);
        if !delay.is_zero() {
            return Err(delay);
        }
        Ok(EventCharge {
            spend,
            now,
            wall,
            estimate,
        })
    }
}

pub(crate) struct EventCharge<'a> {
    spend: MutexGuard<'a, Spend>,
    now: Instant,
    wall: Timestamp,
    estimate: u64,
}

impl EventCharge<'_> {
    pub(crate) fn record(
        mut self,
        rate: Option<&Value>,
        remaining: Option<u64>,
        reset: Option<Timestamp>,
    ) {
        self.spend
            .record(self.now, self.wall, self.estimate, rate, remaining, reset);
    }
}

#[derive(Default)]
struct Spend {
    reads: VecDeque<(Instant, u64)>,
    remaining: Option<(u64, Timestamp)>,
}

impl Spend {
    fn delay(&mut self, now: Instant, wall: Timestamp, estimate: u64) -> Duration {
        if estimate > POINTS {
            return HOUR;
        }
        while self
            .reads
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) >= HOUR)
        {
            self.reads.pop_front();
        }
        let mut spent = self.reads.iter().map(|(_, cost)| *cost).sum::<u64>();
        let mut hour_delay = Duration::ZERO;
        for (at, cost) in &self.reads {
            if spent.saturating_add(estimate) <= POINTS {
                break;
            }
            spent -= cost;
            hour_delay = (*at + HOUR).saturating_duration_since(now);
        }
        let floor_delay = self
            .remaining
            .filter(|(remaining, reset)| *remaining < POINTS && *reset > wall)
            .and_then(|(_, reset)| Duration::try_from(reset - wall).ok())
            .unwrap_or_default();
        hour_delay.max(floor_delay)
    }

    fn record(
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
        self.reads.push_back((now, cost.clamp(1, POINTS)));
        if let Some(remaining) = remaining.or_else(|| rate.and_then(|r| r["remaining"].as_u64())) {
            let reset = reset
                .or_else(|| rate.and_then(|r| super::timestamp(&r["resetAt"])))
                .unwrap_or(wall + jiff::SignedDuration::from_hours(1));
            self.remaining = Some((remaining, reset));
        }
    }
}
