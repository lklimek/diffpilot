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
    resume_with_retry(|| db.resume(Timestamp::now())).await;
    loop {
        match step(hooks, Timestamp::now, dispatch).await {
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

async fn resume_with_retry<F, Attempt>(mut resume: F)
where
    F: FnMut() -> Attempt,
    Attempt: Future<Output = Result<(), DbError>> + Send,
{
    let mut warned = false;
    while resume().await.is_err() {
        if !warned {
            warned = true;
            tracing::warn!(
                code = "resume_queue",
                "GitHub webhook worker could not resume the delivery queue; retrying"
            );
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

pub(super) async fn step<F, Dispatch>(
    hooks: &Webhooks,
    clock: impl Fn() -> Timestamp,
    dispatch: F,
) -> Result<Option<Duration>, DbError>
where
    F: FnOnce(Vec<WebhookChange>) -> Dispatch,
    Dispatch: Future<Output = Result<(), DeliveryError>> + Send,
{
    let db = hooks.db.github();
    let now = clock();
    let Some(mut delivery) = db.next_pending(now).await? else {
        return Ok(db
            .next_retry(now)
            .await?
            .map(|next| (next - now).try_into().unwrap_or_default()));
    };
    match dispatch(std::mem::take(&mut delivery.changes)).await {
        Ok(()) => db.finish(&delivery, clock()).await?,
        Err(code) => {
            let now = clock();
            let retry = retry_at(delivery.attempts, now);
            if db.fail(&delivery, code, retry, now).await? && retry.is_none() {
                tracing::warn!(delivery_id = %delivery.id, %code, "GitHub webhook delivery exhausted its retries");
            }
        }
    }
    Ok(Some(Duration::ZERO))
}

async fn dispatch(_changes: Vec<WebhookChange>) -> Result<(), DeliveryError> {
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

    #[tokio::test(start_paused = true)]
    async fn webhook_resume_retries_and_warns_only_once() {
        use crate::test_support::logs::LogCapture;

        let (logs, _subscriber) = LogCapture::start_scoped();
        let start = tokio::time::Instant::now();
        let mut attempts = 0;
        resume_with_retry(|| {
            attempts += 1;
            std::future::ready(if attempts <= 2 {
                Err(DbError::Conflict)
            } else {
                Ok(())
            })
        })
        .await;
        assert_eq!(attempts, 3);
        let interval = Duration::from_secs(20);
        assert!((interval..=interval + Duration::from_millis(2)).contains(&start.elapsed()));
        assert_eq!(
            logs.text()
                .matches("GitHub webhook worker could not resume")
                .count(),
            1
        );
    }

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
