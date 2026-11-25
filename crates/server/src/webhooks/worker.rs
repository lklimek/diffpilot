//! Single consumer of the durable webhook queue.

use std::time::Duration;

use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::Db;
use db::repo::github::{DeliveryError, PendingDelivery};
use jiff::SignedDuration;
use tokio::{sync::watch, time::Instant};

use super::{
    Webhooks,
    dispatch::{Dispatcher, MAX_HELD},
    stop,
};
use crate::state::AppState;

const BACKOFF: [SignedDuration; 3] = [
    SignedDuration::from_secs(10),
    SignedDuration::from_mins(1),
    SignedDuration::from_mins(10),
];

pub(super) async fn run(hooks: Webhooks, state: AppState, mut stopped: watch::Receiver<bool>) {
    tokio::select! {
        _ = stop(&mut stopped) => {}
        _ = work(&hooks, &state) => {}
    }
}

async fn work(hooks: &Webhooks, state: &AppState) {
    let db = hooks.db.github();
    resume_with_retry(|| db.resume(Timestamp::now())).await;
    let mut dispatch = Dispatcher::default();
    loop {
        match work_step(state, &mut dispatch).await {
            Ok(Some(delay)) if delay.is_zero() => tokio::task::yield_now().await,
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

/// Flushes due windows and admits at most one durable delivery.
pub(crate) async fn work_step(
    state: &AppState,
    dispatch: &mut Dispatcher,
) -> Result<Option<Duration>, DbError> {
    dispatch
        .flush(state, Instant::now(), Timestamp::now())
        .await?;
    let now = Timestamp::now();
    let db = state.db.github();
    let held = dispatch.in_flight();
    let mut retry = None;
    if held.len() < MAX_HELD {
        if let Some(delivery) = db.next_pending_excluding(now, &held).await? {
            dispatch.enqueue(state, delivery, Instant::now()).await?;
            return Ok(Some(Duration::ZERO));
        }
        retry = db
            .next_retry(now)
            .await?
            .map(|at| (at - now).try_into().unwrap_or_default());
    }
    let window = dispatch
        .next_due()
        .map(|due| due.saturating_duration_since(Instant::now()));
    Ok(retry.into_iter().chain(window).min())
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

pub(super) async fn complete(
    db: &Db,
    delivery: &PendingDelivery,
    outcome: Result<(), DeliveryError>,
    now: Timestamp,
) -> Result<(), DbError> {
    let github = db.github();
    match outcome {
        Ok(()) => github.finish(delivery, now).await?,
        Err(code) => {
            let retry = retry_at(delivery.attempts, now);
            if github.fail(delivery, code, retry, now).await? && retry.is_none() {
                tracing::warn!(delivery_id = %delivery.id, %code, "GitHub webhook delivery exhausted its retries");
            }
        }
    }
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

    #[tokio::test]
    async fn work_step_excludes_held_retries_and_stops_at_capacity() {
        use contracts::api::dashboard::RefreshTrigger;
        use contracts::common::RepoRef;
        use db::repo::DEFAULT_TENANT;
        use db::repo::dashboard::StartOutcome;
        use db::repo::github::{ChangeKind, Effect, NewDelivery, Repository, WebhookChange};
        use db::repo::users::GithubProfile;
        use serde_json::json;
        use uuid::Uuid;

        let state = crate::state::tests::test_state().await;
        let now = Timestamp::now();
        state
            .db
            .users()
            .upsert_member(
                DEFAULT_TENANT,
                "u1",
                &GithubProfile {
                    github_id: 3,
                    login: "fixture".into(),
                    name: None,
                    avatar_url: None,
                },
                now,
            )
            .await
            .unwrap();
        let dashboard = state.db.dashboard();
        let StartOutcome::Started { refresh, .. } = dashboard
            .start_refresh(DEFAULT_TENANT, "u1", RefreshTrigger::Manual, now)
            .await
            .unwrap()
        else {
            panic!("expected scanning refresh")
        };
        let repo = RepoRef {
            owner: "acme".into(),
            name: "code".into(),
        };
        dashboard
            .store_snapshot(
                DEFAULT_TENANT,
                "u1",
                refresh.id,
                &json!({
                    "version": crate::dashboard::snapshot::SNAPSHOT_VERSION,
                    "pull_requests": [{"facts": {"repo": repo, "number": 1}}],
                    "issues": []
                }),
                now,
            )
            .await
            .unwrap();
        let github = state.db.github();
        let change = WebhookChange {
            kind: ChangeKind::PullRequest,
            repository: Repository {
                name: repo,
                id: None,
            },
            head_repository: None,
            number: Some(1),
            head_sha: None,
            action: None,
            installation_id: None,
            base_repository: None,
            candidate_login: None,
        };
        let queue = async |id: Uuid| {
            let hash = id.simple().to_string().repeat(2);
            github
                .record(
                    &NewDelivery {
                        id,
                        hash: &hash,
                        event: "pull_request",
                        action: None,
                        installation_id: None,
                        effect: Effect::Signal(&change.repository, std::slice::from_ref(&change)),
                    },
                    now,
                )
                .await
                .unwrap()
        };
        let first = Uuid::new_v4();
        queue(first).await;
        let delivery = github.next_pending(now).await.unwrap().unwrap();
        assert!(
            github
                .fail(&delivery, DeliveryError::Dispatch, Some(now), now)
                .await
                .unwrap()
        );

        let mut dispatch = Dispatcher::default();
        // Hold the first window through all of the intake steps regardless of host load.
        let seeded = github.next_pending(now).await.unwrap().unwrap();
        dispatch
            .enqueue(&state, seeded, Instant::now() + Duration::from_secs(120))
            .await
            .unwrap();
        let wait = work_step(&state, &mut dispatch).await.unwrap().unwrap();
        assert!(
            wait > Duration::from_secs(100),
            "held retry must not wake the worker"
        );
        assert_eq!(dispatch.in_flight(), [first]);

        queue(Uuid::new_v4()).await;
        let delayed = github
            .next_pending_excluding(now, &dispatch.in_flight())
            .await
            .unwrap()
            .unwrap();
        assert!(
            github
                .fail(
                    &delayed,
                    DeliveryError::Dispatch,
                    Some(now + SignedDuration::from_secs(30)),
                    now
                )
                .await
                .unwrap()
        );
        let wait = work_step(&state, &mut dispatch).await.unwrap().unwrap();
        assert!(wait <= Duration::from_secs(30));
        assert_eq!(dispatch.in_flight(), [first]);

        for _ in 1..=MAX_HELD {
            queue(Uuid::new_v4()).await;
        }
        for _ in 1..MAX_HELD {
            assert_eq!(
                work_step(&state, &mut dispatch).await.unwrap(),
                Some(Duration::ZERO)
            );
        }
        assert_eq!(dispatch.in_flight().len(), MAX_HELD);
        assert!(work_step(&state, &mut dispatch).await.unwrap().unwrap() > Duration::ZERO);
        assert_eq!(dispatch.in_flight().len(), MAX_HELD);
        assert!(
            github
                .next_pending_excluding(Timestamp::now(), &dispatch.in_flight())
                .await
                .unwrap()
                .is_some()
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
