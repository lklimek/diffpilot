use std::time::Duration;

use contracts::api::dashboard::{DashboardSettings, RepositoryRule};
use tokio::time::Instant;
use uuid::Uuid;

use super::*;
use crate::test_support::github::HEAD;
use crate::webhooks::{Webhooks, dispatch::Dispatcher};

const SECRET: &str = "signed-comments-webhook-fixture-secret";
const REPO_ID: i64 = 100000482;
const COMMENTS: &str = "/api/v1/prs/acme/node-client/482/comments";
const UPDATED: &str = "2026-11-12T10:41:12Z";
const NEW_HEAD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

async fn covered() -> World {
    let w = world_with(|state| {
        std::sync::Arc::get_mut(&mut state.config)
            .unwrap()
            .github_webhook_secret = Some(SECRET.into());
    })
    .await;
    let repo = crate::github::review_loop::tests::pr482().repo;
    let now = Timestamp::now();
    w.state
        .db
        .dashboard()
        .save_settings(
            DEFAULT_TENANT,
            "u1",
            &DashboardSettings {
                repositories: vec![RepositoryRule {
                    repo: repo.clone(),
                    alias: None,
                    watch_issues: false,
                    ownership_policy: None,
                    branch_patterns: vec![],
                    ignored_checks: vec![],
                }],
                ..DashboardSettings::default()
            },
            now,
        )
        .await
        .unwrap();
    w.state
        .db
        .github()
        .record_installation_lookup(&repo, Some((123, false)), now)
        .await
        .unwrap();
    w
}

async fn pull_reads(w: &World) -> usize {
    w.server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| String::from_utf8_lossy(&request.body).contains("reviewDecision"))
        .count()
}

async fn delivery(w: &World, event: &str, body: Value) -> StatusCode {
    let (parts, body) = fake_github::signed_webhook(
        "/webhooks/github",
        w.state.config.github.app_id,
        event,
        Uuid::new_v4(),
        body.to_string().as_bytes(),
        SECRET.as_bytes(),
    )
    .unwrap()
    .into_parts();
    crate::api::webhook_router(Webhooks::from_app(&w.state))
        .oneshot(Request::from_parts(parts, Body::from(body)))
        .await
        .unwrap()
        .status()
}

async fn flush(w: &World) {
    let now = Timestamp::now();
    let mut dispatcher = Dispatcher::default();
    let db = w.state.db.github();
    let instant = Instant::now();
    while let Some(delivery) = db
        .next_pending_excluding(now, &dispatcher.in_flight())
        .await
        .unwrap()
    {
        dispatcher
            .enqueue(&w.state, delivery, instant)
            .await
            .unwrap();
    }
    dispatcher
        .flush(&w.state, instant + Duration::from_secs(10), now)
        .await
        .unwrap();
}

#[tokio::test]
async fn comments_get_polls_use_storage_while_repository_is_covered() {
    let w = covered().await;
    let first = w.get(COMMENTS).await;
    assert_eq!(first["head"], HEAD);
    for _ in 0..3 {
        w.age_read().await;
        w.get(&format!("{COMMENTS}/items")).await;
        w.get(PR).await;
        assert!(w.get(COMMENTS).await["read"]["next_read_at"].is_null());
    }
    assert_eq!(pull_reads(&w).await, 1);
    let (status, _) = w.post(&format!("{COMMENTS}/read"), json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(pull_reads(&w).await, 2);

    let repo = crate::github::review_loop::tests::pr482().repo;
    w.state
        .db
        .github()
        .record_installation_lookup(&repo, None, Timestamp::now())
        .await
        .unwrap();
    w.age_read().await;
    assert!(w.get(COMMENTS).await["read"]["next_read_at"].is_string());
    assert_eq!(pull_reads(&w).await, 3);
}

#[tokio::test]
async fn synchronize_exposes_stale_comments_before_refresh_without_creating_watch_or_pass() {
    let w = covered().await;
    w.link_llm().await;
    let (status, error) = w.post(&format!("{COMMENTS}/visit"), json!(null)).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{error}");
    let before = w.get(COMMENTS).await;
    assert_eq!(before["head"], HEAD);
    w.age_read().await;

    let synchronize = |head: &str| pr_signal("synchronize", HEAD, head, UPDATED);
    assert_eq!(
        delivery(&w, "pull_request", synchronize(HEAD)).await,
        StatusCode::ACCEPTED
    );
    assert_ne!(w.get(COMMENTS).await["badge"]["badge"], "github_outdated");
    assert_eq!(
        delivery(&w, "pull_request", synchronize(NEW_HEAD)).await,
        StatusCode::ACCEPTED
    );
    let stale = w.get(COMMENTS).await;
    assert_eq!(stale["head"], HEAD);
    assert_eq!(stale["badge"]["badge"], "github_outdated");
    assert_eq!(pull_reads(&w).await, 1);

    serve(
        &w.server,
        &[("pr.json", &|answer| {
            let pull = &mut answer["data"]["repository"]["pullRequest"];
            pull["headRefOid"] = json!(NEW_HEAD);
            pull["updatedAt"] = json!(UPDATED);
        })],
    )
    .await;
    flush(&w).await;
    let current = w.get(COMMENTS).await;
    assert_eq!(current["head"], NEW_HEAD);
    assert_eq!(current["badge"]["badge"], "assessment_needed");
    assert_eq!(
        w.state
            .db
            .review_loop()
            .latest_pass(w.pr_id().await)
            .await
            .unwrap(),
        None
    );
    assert!(
        w.state
            .db
            .review_loop()
            .watches_of(w.pr_id().await)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_review_signal_invalidates_an_inactive_view_and_the_next_visit_reads_it_once() {
    let w = covered().await;
    w.get(PR).await;
    w.age_read().await;

    let signal = json!({
        "action": "edited",
        "installation": {"id": 123},
        "repository": {"id": REPO_ID, "full_name": "acme/node-client"},
        "number": 482,
        "pull_request": {"head": {"sha": HEAD}}
    });
    assert_eq!(
        delivery(&w, "pull_request_review_comment", signal).await,
        StatusCode::ACCEPTED
    );
    flush(&w).await;
    assert_eq!(pull_reads(&w).await, 1);
    assert!(
        w.state
            .db
            .review_loop()
            .pr_state(w.pr_id().await)
            .await
            .unwrap()
            .unwrap()
            .github_dirty_at
            .is_some()
    );

    serve(
        &w.server,
        &[("pr.json", &|answer| {
            answer["data"]["repository"]["pullRequest"]["title"] =
                json!("Refreshed by the review event");
        })],
    )
    .await;
    let visit = format!("{COMMENTS}/visit");
    assert_eq!(w.post(&visit, json!(null)).await.0, StatusCode::NO_CONTENT);
    assert_eq!(
        w.get(COMMENTS).await["title"],
        "Refreshed by the review event"
    );
    assert_eq!(w.post(&visit, json!(null)).await.0, StatusCode::NO_CONTENT);
    assert_eq!(pull_reads(&w).await, 1);
}

fn pr_signal(action: &str, before: &str, after: &str, updated_at: &str) -> Value {
    json!({
        "action": action,
        "installation": {"id": 123},
        "repository": {"id": REPO_ID, "full_name": "acme/node-client"},
        "number": 482,
        "before": before,
        "after": after,
        "pull_request": {"head": {"sha": after}, "updated_at": updated_at}
    })
}

#[tokio::test]
async fn watched_close_updates_the_active_comments_snapshot() {
    let w = covered().await;
    w.get(COMMENTS).await;
    let id = w.pr_id().await;
    let rl = w.state.db.review_loop();
    rl.watch(DEFAULT_TENANT, "u1", id, 20, Timestamp::now())
        .await
        .unwrap();
    w.age_read().await;
    serve(
        &w.server,
        &[("pr.json", &|answer| {
            answer["data"]["repository"]["pullRequest"]["state"] = json!("CLOSED");
        })],
    )
    .await;
    let mut probe = watching::probe_answer(UPDATED);
    probe["data"]["a0"]["pullRequest"]["state"] = json!("CLOSED");
    watching::mount_probe(&w.server, probe).await;
    assert_eq!(
        delivery(&w, "pull_request", pr_signal("closed", HEAD, HEAD, UPDATED)).await,
        StatusCode::ACCEPTED
    );
    flush(&w).await;
    assert!(rl.watches_of(id).await.unwrap().is_empty());
    assert_eq!(w.get(PR).await["state"], "CLOSED");
    assert_eq!(pull_reads(&w).await, 1);
    assert!(rl.latest_pass(id).await.unwrap().is_none());
}

#[tokio::test]
async fn signed_signal_refreshes_for_an_active_reader_after_the_watcher_loses_access() {
    let w = covered().await;
    w.get(COMMENTS).await;
    let id = w.pr_id().await;
    let now = Timestamp::now();
    w.state
        .db
        .users()
        .upsert_member(
            DEFAULT_TENANT,
            "u2",
            &GithubProfile {
                github_id: 2,
                login: "former-watcher".into(),
                name: None,
                avatar_url: None,
            },
            now,
        )
        .await
        .unwrap();
    let rl = w.state.db.review_loop();
    rl.watch(DEFAULT_TENANT, "u2", id, 20, now).await.unwrap();
    w.age_read().await;
    serve(
        &w.server,
        &[("pr.json", &|answer| {
            answer["data"]["repository"]["pullRequest"]["title"] =
                json!("Fresh through the reader");
        })],
    )
    .await;
    assert_eq!(
        delivery(&w, "pull_request", pr_signal("edited", HEAD, HEAD, UPDATED)).await,
        StatusCode::ACCEPTED
    );
    flush(&w).await;
    let watches = rl.watches_of(id).await.unwrap();
    assert_eq!(watches.len(), 1);
    assert_eq!(watches[0].user_id, "u2");
    assert_eq!(
        watches[0].state,
        contracts::api::review_loop::WatchState::Paused
    );
    assert_eq!(w.get(COMMENTS).await["title"], "Fresh through the reader");
    assert_eq!(pull_reads(&w).await, 1);
    assert!(rl.latest_pass(id).await.unwrap().is_none());
}

#[tokio::test]
async fn signal_invalidates_a_pr_whose_first_comments_read_has_not_finished() {
    let w = covered().await;
    db::repo::analyses::testing::seed_analysis(&w.state.db)
        .await
        .unwrap();
    let id = w.pr_id().await;
    let rl = w.state.db.review_loop();
    rl.ensure_pr(DEFAULT_TENANT, id, Timestamp::now())
        .await
        .unwrap();
    let pending = rl.pr_state(id).await.unwrap().unwrap();
    assert!(pending.read_at.is_none());

    assert_eq!(
        delivery(
            &w,
            "pull_request",
            pr_signal("synchronize", HEAD, NEW_HEAD, UPDATED)
        )
        .await,
        StatusCode::ACCEPTED
    );
    let pending = rl.pr_state(id).await.unwrap().unwrap();
    assert!(pending.github_dirty_at.is_some());
    assert_eq!(pending.github_head.as_deref(), Some(NEW_HEAD));
}

#[tokio::test]
async fn equal_second_synchronize_requires_forward_lineage_to_mark_the_view_stale() {
    let w = covered().await;
    assert_eq!(w.get(COMMENTS).await["head"], HEAD);
    let updated = "2026-11-12T10:40:12Z";
    let older = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    assert_eq!(
        delivery(
            &w,
            "pull_request",
            pr_signal("synchronize", older, NEW_HEAD, updated)
        )
        .await,
        StatusCode::ACCEPTED
    );
    assert_ne!(w.get(COMMENTS).await["badge"]["badge"], "github_outdated");
    assert_eq!(
        delivery(
            &w,
            "pull_request",
            pr_signal("synchronize", HEAD, NEW_HEAD, updated)
        )
        .await,
        StatusCode::ACCEPTED
    );
    assert_eq!(w.get(COMMENTS).await["badge"]["badge"], "github_outdated");
}

#[tokio::test]
async fn receive_commit_order_cannot_rewind_the_comments_invalidation() {
    use db::repo::github::{ChangeKind, Effect, NewDelivery, Repository, WebhookChange};

    let w = covered().await;
    w.get(COMMENTS).await;
    let id = w.pr_id().await;
    let db = w.state.db.github();
    let repo = Repository {
        id: Some(REPO_ID),
        name: crate::github::review_loop::tests::pr482().repo,
    };
    let now = Timestamp::now();
    for (received, fingerprint, head) in [
        (now, "a", NEW_HEAD),
        (now - SignedDuration::from_secs(1), "b", HEAD),
    ] {
        let changes = [WebhookChange {
            kind: ChangeKind::PullRequest,
            repository: repo.clone(),
            head_repository: None,
            number: Some(482),
            head_sha: Some(head.into()),
            synchronize_at: None,
            synchronize_before: None,
            action: Some("synchronize".into()),
            installation_id: Some(123),
            base_repository: None,
            candidate_login: None,
        }];
        let outcome = db
            .record(
                &NewDelivery {
                    id: Uuid::new_v4(),
                    hash: &fingerprint.repeat(64),
                    event: "pull_request",
                    action: Some("synchronize"),
                    installation_id: Some(123),
                    effect: Effect::Signal(&repo, &changes),
                },
                received,
            )
            .await
            .unwrap();
        assert_eq!(outcome, db::repo::github::Outcome::Queued);
    }
    let row = w
        .state
        .db
        .review_loop()
        .pr_state(id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        row.github_dirty_at.is_some_and(
            |stamp| stamp.duration_since(now).unsigned_abs() < Duration::from_millis(1)
        )
    );
    assert_eq!(row.github_head.as_deref(), Some(NEW_HEAD));
}
