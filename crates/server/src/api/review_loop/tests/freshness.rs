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
async fn conflicting_lifecycle_signals_refresh_history_without_an_active_watch_or_reader() {
    let w = covered().await;
    w.get(PR).await;
    let signal = |action, state, merged| {
        json!({
            "action": action,
            "installation": {"id": 123},
            "repository": {"id": REPO_ID, "full_name": "acme/node-client", "private": false},
            "number": 482,
            "pull_request": {"state": state, "merged": merged, "updated_at": UPDATED}
        })
    };
    assert_eq!(
        delivery(&w, "pull_request", signal("closed", "closed", false)).await,
        StatusCode::ACCEPTED
    );
    let closed = w.get("/api/v1/me/prs").await;
    assert_eq!(closed["prs"][0]["pr_state"], "closed", "{closed}");
    assert_eq!(
        delivery(&w, "pull_request", signal("reopened", "open", false)).await,
        StatusCode::ACCEPTED
    );
    let mut probe = super::watching::probe_answer(UPDATED);
    probe["data"]["a0"]["databaseId"] = json!(REPO_ID);
    probe["data"]["a0"]["isPrivate"] = json!(false);
    super::watching::mount_probe(&w.server, probe).await;

    flush(&w).await;
    let reopened = w.get("/api/v1/me/prs").await;
    assert_eq!(reopened["prs"][0]["pr_state"], "open", "{reopened}");
    assert_eq!(
        pull_reads(&w).await,
        1,
        "only the initial Comments read was full"
    );
}

#[tokio::test]
async fn pr_page_reconciles_lifecycle_signals_dispatched_before_the_pr_was_stored() {
    let w = covered().await;
    serve(
        &w.server,
        &[("pr.json", &|answer| {
            let pr = &mut answer["data"]["repository"]["pullRequest"];
            pr["state"] = json!("OPEN");
            pr["updatedAt"] = json!(UPDATED);
        })],
    )
    .await;
    let mut old = recorded("pr.json");
    let pr = &mut old["data"]["repository"]["pullRequest"];
    pr["state"] = json!("CLOSED");
    pr["updatedAt"] = json!(UPDATED);
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_string_contains("reviewDecision"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(old)
                .set_delay(Duration::from_secs(1)),
        )
        .with_priority(1)
        .up_to_n_times(1)
        .mount(&w.server)
        .await;
    let events = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while pull_reads(&w).await == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("first GitHub request started");
        for (action, state) in [("closed", "closed"), ("reopened", "open")] {
            let mut signal = pr_signal(action, HEAD, HEAD, UPDATED);
            signal["pull_request"]["state"] = json!(state);
            signal["pull_request"]["merged"] = json!(false);
            assert_eq!(
                delivery(&w, "pull_request", signal).await,
                StatusCode::ACCEPTED
            );
        }
        flush(&w).await;
    };
    tokio::join!(w.get(PR), events);
    let history = w.get("/api/v1/me/prs").await;
    assert_eq!(history["prs"][0]["pr_state"], "open", "{history}");
    assert_eq!(pull_reads(&w).await, 2);
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

    let mut signal = pr_signal("synchronize", HEAD, NEW_HEAD, UPDATED);
    signal.as_object_mut().unwrap().remove("before");
    assert_eq!(
        delivery(&w, "pull_request", signal).await,
        StatusCode::ACCEPTED
    );
    let pending = rl.pr_state(id).await.unwrap().unwrap();
    assert!(pending.github_dirty_at.is_some());
    assert_eq!(pending.github_head.as_deref(), Some(NEW_HEAD));

    serve(
        &w.server,
        &[("pr.json", &|answer| {
            let pull = &mut answer["data"]["repository"]["pullRequest"];
            pull["headRefOid"] = json!(LATER_HEAD);
            pull["updatedAt"] = json!(UPDATED);
        })],
    )
    .await;
    let first = w.get(COMMENTS).await;
    assert_eq!(first["head"], LATER_HEAD);
    assert_ne!(first["badge"]["badge"], "github_outdated");
    assert_eq!(pull_reads(&w).await, 1);
}

#[tokio::test]
async fn webhooks_cover_a_pr_still_missing_during_its_first_read() {
    let w = covered().await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_string_contains("reviewDecision"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(recorded("pr.json"))
                .set_delay(Duration::from_secs(1)),
        )
        .with_priority(1)
        .up_to_n_times(1)
        .mount(&w.server)
        .await;
    let deliver_while_reading = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while pull_reads(&w).await == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("first GitHub request started");
        assert!(
            w.state
                .db
                .pull_requests()
                .find(
                    DEFAULT_TENANT,
                    &crate::github::review_loop::tests::pr482().repo,
                    482,
                )
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            delivery(
                &w,
                "pull_request",
                pr_signal("synchronize", HEAD, NEW_HEAD, UPDATED)
            )
            .await,
            StatusCode::ACCEPTED
        );
        flush(&w).await;
    };
    let (_, ()) = tokio::join!(w.items(), deliver_while_reading);
    let id = w.pr_id().await;
    let pending = w
        .state
        .db
        .review_loop()
        .pr_state(id)
        .await
        .unwrap()
        .unwrap();
    assert!(pending.github_dirty_at.is_some());
    assert_eq!(pending.github_head.as_deref(), Some(NEW_HEAD));

    serve(
        &w.server,
        &[("pr.json", &|answer| {
            let pull = &mut answer["data"]["repository"]["pullRequest"];
            pull["headRefOid"] = json!(NEW_HEAD);
            pull["updatedAt"] = json!(UPDATED);
        })],
    )
    .await;
    assert_eq!(w.get(COMMENTS).await["head"], NEW_HEAD);
    let done = w
        .state
        .db
        .review_loop()
        .pr_state(id)
        .await
        .unwrap()
        .unwrap();
    assert!(done.github_dirty_at.is_none());
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
    for (received, fingerprint, before, head) in [
        (now, "a", HEAD, NEW_HEAD),
        (now - SignedDuration::from_secs(1), "b", NEW_HEAD, HEAD),
    ] {
        let changes = [WebhookChange {
            kind: ChangeKind::PullRequest,
            repository: repo.clone(),
            head_repository: None,
            number: Some(482),
            head_sha: Some(head.into()),
            synchronize_at: None,
            synchronize_before: Some(before.into()),
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
                    effect: Effect::Signal(&repo, &changes, None),
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

async fn comments_with_two_threads(w: &World) {
    serve(
        &w.server,
        &[
            ("review-threads.json", &|answer| {
                answer["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|thread| {
                        matches!(
                            thread["id"].as_str(),
                            Some("PRRT_fixture482_l2" | "PRRT_fixture482_l6")
                        )
                    });
            }),
            ("reviews.json", &|answer| {
                answer["data"]["repository"]["pullRequest"]["reviews"]["nodes"] = json!([]);
            }),
            ("comments.json", &|answer| {
                answer["data"]["repository"]["pullRequest"]["comments"]["nodes"] = json!([]);
            }),
        ],
    )
    .await;
    w.get(COMMENTS).await;
    w.assess(L2, recommend(DecisionKind::FixHere, "correctness"), vec![])
        .await;
}

#[tokio::test]
async fn folded_duplicate_does_not_require_a_separate_assessment() {
    let w = world().await;
    comments_with_two_threads(&w).await;
    assert_eq!(w.get(COMMENTS).await["badge"]["badge"], "assessment_needed");

    propagation::link(&w, L6).await;
    let follower = w.item(L6).await;
    assert_eq!(follower["assessment_state"]["state"], "not_assessed");
    assert_eq!(w.get(COMMENTS).await["badge"]["badge"], "questions_waiting");

    let (status, body) = w
        .post(
            &format!("{COMMENTS}/items/C{}/gates", follower["no"]),
            json!({"gate":"G-DATA", "marked":true, "item_revision":follower["revision"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(w.get(COMMENTS).await["badge"]["badge"], "assessment_needed");
}

#[tokio::test]
async fn watched_pr_badge_counts_current_skips_of_its_reader() {
    let w = world().await;
    comments_with_two_threads(&w).await;
    let other_cookie = crate::test_support::other_user_cookie(&w.state).await;
    let id = w.pr_id().await;
    for user in ["u1", "u2"] {
        w.state
            .db
            .review_loop()
            .watch(DEFAULT_TENANT, user, id, 20, Timestamp::now())
            .await
            .unwrap();
    }
    let watch_path = "/api/v1/watches";
    assert_eq!(
        w.get(watch_path).await["watches"][0]["badge"]["badge"],
        "questions_waiting"
    );

    let item = w.item(L2).await;
    let (status, _) = w
        .post(
            &format!("{COMMENTS}/items/C{}/skip", item["no"]),
            json!({"item_revision": item["revision"]}),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_ne!(
        w.get(watch_path).await["watches"][0]["badge"]["badge"],
        "questions_waiting"
    );
    assert_eq!(w.get(COMMENTS).await["counts"]["needs_you"], 0);

    let other = w
        .app
        .clone()
        .oneshot(
            Request::get(watch_path)
                .header(COOKIE, other_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::OK);
    let other: Value =
        serde_json::from_slice(&other.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(other["watches"][0]["badge"]["badge"], "questions_waiting");
}

const LATER_HEAD: &str = "cccccccccccccccccccccccccccccccccccccccc";

#[tokio::test]
async fn in_flight_read_leaves_same_second_newer_head_pending_until_an_authoritative_read() {
    let w = covered().await;
    w.get(COMMENTS).await;
    let id = w.pr_id().await;
    let rl = w.state.db.review_loop();
    let started = Timestamp::now() - SignedDuration::from_secs(1);
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
    let mut snapshot = pending.github_snapshot.unwrap();
    snapshot["head"] = json!(LATER_HEAD);
    snapshot["updated_at"] = json!(UPDATED);
    let read = ReadResult::Read {
        snapshot,
        round_count: pending.round_count,
    };
    rl.record_read_started(id, &read, started, Timestamp::now())
        .await
        .unwrap();
    assert_eq!(w.get(COMMENTS).await["badge"]["badge"], "github_outdated");

    rl.record_read(id, &read, Timestamp::now()).await.unwrap();
    let current = w.get(COMMENTS).await;
    assert_eq!(current["head"], LATER_HEAD);
    assert_ne!(current["badge"]["badge"], "github_outdated");
    assert!(
        rl.pr_state(id)
            .await
            .unwrap()
            .unwrap()
            .github_dirty_at
            .is_none()
    );
}

#[tokio::test]
async fn current_head_reconciles_a_synchronize_without_an_update_time() {
    let w = covered().await;
    w.get(COMMENTS).await;
    w.age_read().await;
    let mut signal = pr_signal("synchronize", HEAD, NEW_HEAD, UPDATED);
    signal["pull_request"]["updated_at"] = Value::Null;
    assert_eq!(
        delivery(&w, "pull_request", signal).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(w.get(COMMENTS).await["badge"]["badge"], "github_outdated");

    let id = w.pr_id().await;
    let rl = w.state.db.review_loop();
    let pending = rl.pr_state(id).await.unwrap().unwrap();
    rl.record_read(
        id,
        &ReadResult::Read {
            snapshot: pending.github_snapshot.unwrap(),
            round_count: pending.round_count,
        },
        Timestamp::now(),
    )
    .await
    .unwrap();
    assert_eq!(w.get(COMMENTS).await["badge"]["badge"], "github_outdated");
    w.age_read().await;

    serve(
        &w.server,
        &[("pr.json", &|answer| {
            answer["data"]["repository"]["pullRequest"]["headRefOid"] = json!(LATER_HEAD);
        })],
    )
    .await;
    flush(&w).await;
    let current = w.get(COMMENTS).await;
    assert_eq!(current["head"], LATER_HEAD);
    assert_ne!(current["badge"]["badge"], "github_outdated");
    assert!(
        w.state
            .db
            .review_loop()
            .pr_state(w.pr_id().await)
            .await
            .unwrap()
            .unwrap()
            .github_dirty_at
            .is_none()
    );
}
