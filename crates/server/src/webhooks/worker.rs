//! Single consumer of the durable webhook queue.

use std::time::Duration;

use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::github::{DeliveryError, WebhookChange};
use jiff::SignedDuration;
use tokio::sync::watch;

use super::{Webhooks, stop};

const BACKOFF: [SignedDuration; 3] = [
    SignedDuration::from_secs(10),
    SignedDuration::from_mins(1),
    SignedDuration::from_mins(10),
];

pub(super) async fn run(hooks: Webhooks, mut stopped: watch::Receiver<bool>) {
    tokio::select! {
        _ = stop(&mut stopped) => {}
        _ = work(&hooks) => {}
    }
}

async fn work(hooks: &Webhooks) {
    let db = hooks.db.github();
    while db.resume(Timestamp::now()).await.is_err() {
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    loop {
        match step(hooks).await {
            Ok(Some(delay)) => {
                tokio::select! {
                    _ = hooks.wake.notified() => {}
                    _ = tokio::time::sleep(delay) => {}
                }
            }
            Ok(None) => hooks.wake.notified().await,
            Err(_) => {
                tracing::warn!(
                    code = "delivery_queue",
                    "GitHub webhook worker could not access the delivery queue"
                );
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    }
}

async fn step(hooks: &Webhooks) -> Result<Option<Duration>, DbError> {
    let db = hooks.db.github();
    let now = Timestamp::now();
    let Some(delivery) = db.next_pending(now).await? else {
        return Ok(db
            .next_retry(now)
            .await?
            .map(|next| (next - now).try_into().unwrap_or_default()));
    };
    match dispatch(&delivery.changes).await {
        Ok(()) => db.finish(&delivery, Timestamp::now()).await?,
        Err(code) => {
            let now = Timestamp::now();
            let retry = retry_at(delivery.attempts, now);
            if db.fail(&delivery, code, retry, now).await? && retry.is_none() {
                tracing::warn!(delivery_id = %delivery.id, %code, "GitHub webhook delivery exhausted its retries");
            }
        }
    }
    Ok(Some(Duration::ZERO))
}

async fn dispatch(_changes: &[WebhookChange]) -> Result<(), DeliveryError> {
    // B3 supplies the targeted GitHub reads for these durable hints.
    Ok(())
}

fn retry_at(attempts: i64, now: Timestamp) -> Option<Timestamp> {
    usize::try_from(attempts)
        .ok()
        .and_then(|attempt| BACKOFF.get(attempt))
        .map(|delay| now + *delay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_retries_three_times_at_the_documented_intervals() {
        let now = Timestamp::now();
        for (attempt, seconds) in [(0, 10), (1, 60), (2, 600)] {
            assert_eq!(
                retry_at(attempt, now),
                Some(now + SignedDuration::from_secs(seconds))
            );
        }
        assert_eq!(retry_at(3, now), None);
    }
}
