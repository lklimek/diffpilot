use db::repo::github::{Effect, InstallationUpdate, NewDelivery, Repository};
use uuid::Uuid;

use super::*;

async fn installed(db: &Db, repo: RepoRef, repository_id: i64, now: Timestamp) {
    let repositories = [Repository {
        name: repo,
        id: Some(repository_id),
    }];
    let id = Uuid::new_v4();
    db.github()
        .record(
            &NewDelivery {
                id,
                hash: &format!("{:064x}", id.as_u128()),
                event: "installation",
                action: Some("created"),
                installation_id: Some(repository_id),
                effect: Effect::Installation(repository_id, InstallationUpdate::Add(&repositories)),
            },
            now,
        )
        .await
        .expect("installation");
}

#[tokio::test]
async fn no_scan_or_fact_check_while_my_work_open() {
    for age in [None, Some(6), Some(31)] {
        let mut w = world().await;
        let before = match age {
            Some(age) => {
                let id = w.refreshed().await.id;
                w.state.dashboard = DashboardHub::testing(
                    Clock::starting_at(now() + SignedDuration::from_mins(age)),
                    Duration::from_secs(20),
                    Arc::new(NoopRanker),
                    Duration::ZERO,
                );
                Some(id)
            }
            None => None,
        };
        let mut disabled = settings();
        disabled.schedule_enabled = false;
        w.state
            .db
            .dashboard()
            .save_settings(DEFAULT_TENANT, "u1", &disabled, now())
            .await
            .expect("schedule disabled");
        let fake = crate::test_support::pr_hygiene::FakePrHygiene::start(
            "2026-11-18T09:00:00Z",
            Vec::new(),
        )
        .await;
        super::plugin::with_plugin(&mut w, &fake, Default::default());
        let key_dir = tempfile::tempdir().unwrap();
        Arc::get_mut(&mut w.state.config)
            .unwrap()
            .github
            .private_key_file = key_dir.path().join("absent.pem");
        let count = w.gate.requests();
        w.state.spawn_background();
        w.summary(ALICE).await;
        assert!(!w.state.plugins.active_users().is_empty());
        tokio::time::sleep(Duration::from_millis(250)).await;
        let after = w
            .summary(ALICE)
            .await
            .latest_refresh
            .map(|status| status.id);
        let requests = w.gate.requests();
        w.state.stopping.send(true).expect("stop scheduler");
        assert_eq!(after, before, "opening My work must not start a scan");
        assert_eq!(requests, count, "opening My work must not read GitHub");
    }
}

#[tokio::test]
async fn item_live_only_with_webhook_and_installation() {
    let mut w = world().await;
    w.refreshed().await;
    let repository = repo("acme", "node-client");
    let read_live = |items: ItemPage| {
        items
            .items
            .into_iter()
            .find(|item| item.repo.as_ref() == Some(&repository))
            .expect("GitHub item")
            .live
    };
    assert_eq!(read_live(w.items("").await), Some(false));

    installed(&w.state.db, repository.clone(), 42, now()).await;
    assert_eq!(
        read_live(w.items("").await),
        Some(false),
        "webhook is disabled"
    );
    let revision = w.summary(ALICE).await.revision;
    Arc::get_mut(&mut w.state.config)
        .expect("config")
        .github_webhook_secret = Some(SecretString::from("test webhook secret at least 32 bytes"));
    assert_eq!(read_live(w.items("").await), Some(true));
    let summary = w.summary(ALICE).await;
    assert_ne!(summary.revision, revision);
    assert_eq!(summary.live_repositories, 1);
    assert!(summary.not_live_repositories > 0);
    assert_eq!(
        summary.last_event_at, None,
        "an installation delivery is not a dashboard update"
    );

    let (refresh, mut snapshot) = super::plugin::stored(&w).await;
    snapshot.last_event_at = Some(now());
    super::plugin::write(&w, refresh, &snapshot).await;
    assert_eq!(w.summary(ALICE).await.last_event_at, Some(now()));
    w.refreshed().await;
    assert_eq!(w.summary(ALICE).await.last_event_at, Some(now()));

    let id = Uuid::new_v4();
    w.state
        .db
        .github()
        .record(
            &NewDelivery {
                id,
                hash: &format!("{:064x}", id.as_u128()),
                event: "installation",
                action: Some("suspend"),
                installation_id: Some(42),
                effect: Effect::Installation(42, InstallationUpdate::Suspend(true)),
            },
            now(),
        )
        .await
        .expect("suspension");
    assert_eq!(read_live(w.items("").await), Some(false));
    assert_eq!(w.summary(ALICE).await.live_repositories, 0);
}

#[tokio::test]
async fn todo_has_no_live() {
    let w = world().await;
    w.refreshed().await;
    let page = w.send("GET", "/api/v1/dashboard/items", ALICE, &[]).await;
    let items = page.body["items"].as_array().expect("items");
    let todo = items
        .iter()
        .find(|item| item["kind"] == "todo")
        .expect("TODO");
    assert!(todo.get("live").is_none());
    assert!(items.iter().any(|item| item["live"] == false));
}

#[tokio::test]
async fn repository_installations_lists_rules_and_app_repos() {
    let mut w = world().await;
    Arc::get_mut(&mut w.state.config)
        .expect("config")
        .github_webhook_secret = Some(SecretString::from("test webhook secret at least 32 bytes"));
    installed(&w.state.db, repo("Acme", "new-app-repo"), 11, now()).await;
    db::repo::analyses::testing::seed_tenant(&w.state.db, "other")
        .await
        .expect("tenant");
    db::repo::tenants::testing::allow_login(&w.state.db, "other", "foreign", 90)
        .await
        .expect("allow");
    w.state
        .db
        .users()
        .upsert_member(
            "other",
            "u3",
            &GithubProfile {
                github_id: 90,
                login: "foreign".to_owned(),
                name: None,
                avatar_url: None,
            },
            Timestamp::now(),
        )
        .await
        .expect("member");
    w.state
        .db
        .dashboard()
        .save_settings(
            "other",
            "u3",
            &DashboardSettings {
                enumerate_owners: vec!["foreign".to_owned()],
                ..DashboardSettings::default()
            },
            now(),
        )
        .await
        .expect("foreign rules");
    installed(&w.state.db, repo("foreign", "private"), 12, now()).await;
    let before = w.gate.requests();
    let answer = w
        .send(
            "GET",
            "/api/v1/settings/repository-installations",
            ALICE,
            &[],
        )
        .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let rows = answer.body["repositories"]
        .as_array()
        .expect("repositories");
    assert!(rows.contains(&json!({"repo":{"owner":"acme","name":"wallet-ui"},"live":false})));
    assert!(rows.contains(&json!({"repo":{"owner":"Acme","name":"new-app-repo"},"live":true})));
    assert!(!rows.iter().any(|row| row["repo"]["owner"] == "foreign"));
    assert_eq!(w.gate.requests(), before);
    assert_eq!(
        w.send("GET", "/api/v1/settings/repository-installations", "", &[])
            .await
            .status,
        StatusCode::UNAUTHORIZED,
    );
}

#[tokio::test]
async fn repository_counts_exclude_unlisted_and_snoozed_repositories() {
    let mut w = world().await;
    Arc::get_mut(&mut w.state.config)
        .unwrap()
        .github_webhook_secret = Some(SecretString::from("test webhook secret at least 32 bytes"));
    w.refreshed().await;
    let live = repo("acme", "node-client");
    installed(&w.state.db, live.clone(), 42, now()).await;
    let items = w.items("?limit=100").await.items;
    let visible: std::collections::HashSet<_> =
        items.iter().filter_map(|item| item.repo.clone()).collect();
    let (id, mut snapshot) = super::plugin::stored(&w).await;
    let template = snapshot
        .pull_requests
        .iter()
        .find(|pr| pr.facts.repo == live)
        .unwrap();
    let mut excluded = template.clone();
    excluded.facts.repo = repo("acme", "excluded-only");
    excluded.facts.author = Some("dependabot[bot]".to_owned());
    snapshot.pull_requests.push(excluded);
    super::plugin::write(&w, id, &snapshot).await;

    for item in items
        .iter()
        .filter(|item| item.repo.as_ref() == Some(&live))
    {
        w.state
            .db
            .dashboard()
            .snooze(
                DEFAULT_TENANT,
                "u1",
                &item.key,
                now(),
                now() + SignedDuration::from_hours(1),
            )
            .await
            .unwrap();
    }
    let summary = w.summary(ALICE).await;
    assert_eq!(summary.live_repositories, 0);
    assert_eq!(
        summary.not_live_repositories,
        u32::try_from(visible.len() - 1).unwrap()
    );
}

const HOOK_SECRET: &str = "synthetic-dashboard-webhook-secret";
const LOCAL_MORNING: &str = "2026-11-17T23:15:00Z";

async fn pull_delivery(w: &World, action: &str, repo: &str, number: u64, author: &str) -> Uuid {
    let repo_id = match repo {
        "acme/wallet-ui" => 42,
        "acme/platform" => 43,
        "acme/consensus" => 44,
        _ => panic!("unknown fixture repository"),
    };
    pull_delivery_with_repo_id(w, action, repo, repo_id, number, author).await
}

async fn pull_delivery_with_repo_id(
    w: &World,
    action: &str,
    repo: &str,
    repo_id: i64,
    number: u64,
    author: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    let body = json!({
        "action": action,
        "repository": {"id": repo_id, "full_name": repo},
        "installation": {"id": 42},
        "number": number,
        "pull_request": {"user": {"login": author}},
        "sender": {"login": "unrelated-merger"},
        "delivery_fixture": id,
    });
    let (parts, bytes) = fake_github::signed_webhook(
        "/webhooks/github",
        w.state.config.github.app_id,
        "pull_request",
        id,
        body.to_string().as_bytes(),
        HOOK_SECRET.as_bytes(),
    )
    .expect("signed request")
    .into_parts();
    let req = Request::from_parts(parts, Body::from(bytes));
    let response = api::webhook_router(crate::webhooks::Webhooks::from_app(&w.state))
        .oneshot(req)
        .await
        .expect("webhook response");
    assert_eq!(response.status(), 202);
    id
}

fn pull_state(w: &World, number: u64, state: &str, closed: Option<&str>, merged: Option<&str>) {
    w.gate.pull_updates.lock().expect("pull updates").insert(
        number,
        json!({"state": state, "closedAt": closed, "mergedAt": merged}),
    );
}

async fn flush_webhooks(w: &World, wall: Timestamp) {
    use crate::webhooks::dispatch::Dispatcher;
    use tokio::time::Instant;

    let mut dispatch = Dispatcher::default();
    let db = w.state.db.github();
    let instant = Instant::now();
    while let Some(delivery) = db
        .next_pending_excluding(wall, &dispatch.in_flight())
        .await
        .expect("pending delivery")
    {
        dispatch
            .enqueue(&w.state, delivery, instant)
            .await
            .expect("route delivery");
    }
    dispatch
        .flush(&w.state, instant + Duration::from_secs(11), wall)
        .await
        .expect("dispatch delivery");
}

#[tokio::test]
async fn pull_webhooks_update_chart_history_for_ranked_and_unranked_prs() {
    let mut w = world().await;
    Arc::get_mut(&mut w.state.config)
        .unwrap()
        .github_webhook_secret = Some(HOOK_SECRET.into());
    let refresh = w.refreshed().await;
    let (id, mut snapshot) = super::plugin::stored(&w).await;
    assert!(
        snapshot
            .pull_requests
            .iter()
            .any(|pr| pr.facts.number == 311)
    );
    snapshot.pull_requests.retain(|pr| pr.facts.number != 1541);
    snapshot
        .history
        .retain(|event| event.item_id != "acme/consensus#1541");
    super::plugin::write(&w, id, &snapshot).await;
    let before = w.summary(ALICE).await;

    pull_state(&w, 311, "MERGED", Some(LOCAL_MORNING), Some(LOCAL_MORNING));
    pull_state(&w, 1541, "CLOSED", Some(LOCAL_MORNING), None);
    pull_state(&w, 1203, "CLOSED", Some(LOCAL_MORNING), None);
    for (repo, number, author) in [
        ("acme/wallet-ui", 311, "alice"),
        ("acme/consensus", 1541, "Alice-Helper"),
        ("acme/platform", 1203, "alice-helper"),
    ] {
        pull_delivery(&w, "closed", repo, number, author).await;
    }
    flush_webhooks(&w, Timestamp::now()).await;

    let after = w.summary(ALICE).await;
    assert_eq!(after.latest_refresh.as_ref().unwrap().id, refresh.id);
    assert_ne!(after.revision, before.revision);
    assert!(after.last_event_at.is_some());
    let local_date = |at: Timestamp| {
        at.to_zoned(crate::dashboard::zone::parse("Europe/Warsaw").unwrap())
            .date()
    };
    let today = local_date(now());
    let changes: Vec<_> = after
        .snapshot
        .unwrap()
        .history
        .into_iter()
        .filter(|event| {
            local_date(event.at) == today
                && [
                    "acme/wallet-ui#311",
                    "acme/consensus#1541",
                    "acme/platform#1203",
                ]
                .contains(&event.item_id.as_str())
        })
        .map(|event| (event.item_id, event.kind, event.at.to_string()))
        .collect();
    assert_eq!(
        changes,
        [
            (
                "acme/consensus#1541".to_owned(),
                HistoryKind::PrClosedUnmerged,
                LOCAL_MORNING.to_owned()
            ),
            (
                "acme/wallet-ui#311".to_owned(),
                HistoryKind::PrMerged,
                LOCAL_MORNING.to_owned()
            ),
        ]
    );
    assert!(
        super::plugin::stored(&w)
            .await
            .1
            .pull_requests
            .iter()
            .all(|pr| { ![311, 1541, 1203].contains(&pr.facts.number) })
    );
}

async fn item_history(w: &World, item: &str) -> Vec<(HistoryKind, Timestamp)> {
    w.summary(ALICE)
        .await
        .snapshot
        .expect("snapshot")
        .history
        .into_iter()
        .filter(|event| event.item_id == item)
        .map(|event| (event.kind, event.at))
        .collect()
}

#[tokio::test]
async fn pull_webhooks_retry_and_read_current_state_after_reopening() {
    let mut w = world().await;
    Arc::get_mut(&mut w.state.config)
        .unwrap()
        .github_webhook_secret = Some(HOOK_SECRET.into());
    w.refreshed().await;
    let item = "acme/consensus#1541";
    let opened = item_history(&w, item).await;
    assert!(
        opened
            .iter()
            .any(|(kind, _)| *kind == HistoryKind::PrOpened)
    );

    // A transient partial GraphQL answer must keep the durable signal retryable.
    let receipt = Timestamp::now();
    pull_state(&w, 1541, "CLOSED", Some(LOCAL_MORNING), None);
    pull_delivery(&w, "closed", "acme/consensus", 1541, "alice-helper").await;
    *w.gate.missing.lock().unwrap() = Some((true, 1));
    flush_webhooks(&w, Timestamp::now()).await;
    assert_eq!(item_history(&w, item).await, opened);
    *w.gate.missing.lock().unwrap() = None;
    flush_webhooks(&w, receipt + SignedDuration::from_secs(30)).await;
    let close = (
        HistoryKind::PrClosedUnmerged,
        LOCAL_MORNING.parse().unwrap(),
    );
    assert_eq!(item_history(&w, item).await, [opened[0], close]);

    // A second independently delivered hint must not duplicate any lifecycle events.
    let revision = w.summary(ALICE).await.revision;
    pull_delivery(&w, "closed", "acme/consensus", 1541, "alice-helper").await;
    flush_webhooks(&w, Timestamp::now()).await;
    assert_eq!(w.summary(ALICE).await.revision, revision);

    // Preserve the stale closedAt in the fake response to prove the current state governs the chart.
    pull_state(&w, 1541, "OPEN", Some(LOCAL_MORNING), None);
    pull_delivery(&w, "reopened", "acme/consensus", 1541, "alice-helper").await;
    flush_webhooks(&w, Timestamp::now()).await;
    assert_eq!(item_history(&w, item).await, opened);
    let reopened = w.summary(ALICE).await.revision;
    pull_delivery(&w, "closed", "acme/consensus", 1541, "alice-helper").await;
    flush_webhooks(&w, Timestamp::now()).await;
    assert_eq!(w.summary(ALICE).await.revision, reopened);

    // A delayed reopen signal cannot erase a merge which happened afterwards.
    let merged = "2026-11-18T07:12:00Z";
    pull_state(&w, 1541, "MERGED", Some(merged), Some(merged));
    pull_delivery(&w, "reopened", "acme/consensus", 1541, "alice-helper").await;
    flush_webhooks(&w, Timestamp::now()).await;
    assert_eq!(
        item_history(&w, item).await,
        [opened[0], (HistoryKind::PrMerged, merged.parse().unwrap())]
    );
}

#[tokio::test]
async fn pull_webhook_during_first_scan_reconciles_the_new_snapshot() {
    use db::repo::github::Destination;

    let mut w = world().await;
    Arc::get_mut(&mut w.state.config)
        .unwrap()
        .github_webhook_secret = Some(HOOK_SECRET.into());
    let refresh = w.start_held().await;
    let id = pull_delivery(&w, "closed", "acme/consensus", 1541, "alice-helper").await;
    let receiving = Timestamp::now();
    let requests = w.gate.requests();
    flush_webhooks(&w, receiving).await;
    let parked = w
        .state
        .db
        .github()
        .parked_target(
            &Destination::Dashboard {
                tenant: DEFAULT_TENANT.to_owned(),
                user: "u1".to_owned(),
            },
            receiving,
        )
        .await
        .unwrap();
    let held_requests = w.gate.requests();
    // Release the fake scan before checking the deferred delivery.
    w.gate.open();
    assert_eq!(w.settle(refresh.id).await.state, RefreshState::Finished);
    assert_eq!(
        parked
            .expect("lifecycle delivery deferred behind the scan")
            .id,
        id
    );
    assert_eq!(
        held_requests, requests,
        "a parked delivery must not read GitHub"
    );

    // The delivery observes the close after the scan took its history reading.
    pull_state(&w, 1541, "CLOSED", Some(LOCAL_MORNING), None);
    flush_webhooks(&w, receiving + SignedDuration::from_secs(30)).await;
    let summary = w.summary(ALICE).await;
    assert_eq!(summary.latest_refresh.unwrap().id, refresh.id);
    assert!(item_history(&w, "acme/consensus#1541").await.contains(&(
        HistoryKind::PrClosedUnmerged,
        LOCAL_MORNING.parse().unwrap()
    )));
}

#[tokio::test]
async fn full_scan_reconciles_rankings_with_newer_history_and_falls_back_to_q2() {
    let w = world().await;
    // The Q2 answer for #311 still says OPEN. Its later Q13 answer observes a merge.
    let merged = json!({"state":"MERGED", "closedAt": LOCAL_MORNING, "mergedAt": LOCAL_MORNING});
    let mut history = w.gate.history_updates.lock().unwrap();
    history.insert("2026-11-10T10:00:00Z".to_owned(), merged);
    // For #476, only Q2 sees the merge; Q13 cannot read the PR.
    history.insert("2026-11-13T12:00:00Z".to_owned(), Value::Null);
    drop(history);
    pull_state(&w, 476, "MERGED", Some(LOCAL_MORNING), Some(LOCAL_MORNING));

    assert_eq!(w.refreshed().await.state, RefreshState::Finished);
    let (_, stored) = super::plugin::stored(&w).await;
    let merge = (HistoryKind::PrMerged, LOCAL_MORNING.parse().unwrap());
    assert!(
        item_history(&w, "acme/wallet-ui#311")
            .await
            .contains(&merge)
    );
    assert!(stored.pull_requests.iter().all(|pr| pr.facts.number != 311));
    assert!(
        item_history(&w, "acme/node-client#476")
            .await
            .contains(&merge)
    );
    assert!(stored.pull_requests.iter().all(|pr| pr.facts.number != 476));
}

#[tokio::test]
async fn last_ranked_pull_close_keeps_its_repository_live_for_reopening() {
    use contracts::api::dashboard::HistoryEvent;

    let mut w = world().await;
    Arc::get_mut(&mut w.state.config)
        .unwrap()
        .github_webhook_secret = Some(HOOK_SECRET.into());
    let wallet = repo("acme", "wallet-ui");
    let mut configured = settings();
    configured.repositories.retain(|row| row.repo != wallet);
    configured.enumerate_owners.clear();
    let github = w.state.db.github();
    let wall = Timestamp::now();
    w.state
        .db
        .dashboard()
        .save_settings(DEFAULT_TENANT, "u1", &configured, wall)
        .await
        .expect("settings");
    w.refreshed().await;
    let (id, mut snapshot) = super::plugin::stored(&w).await;
    assert!(
        snapshot
            .pull_requests
            .iter()
            .any(|pr| pr.facts.number == 311)
    );
    snapshot
        .pull_requests
        .retain(|pr| pr.facts.repo != wallet || pr.facts.number == 311);
    snapshot
        .history
        .retain(|event| !event.item_id.starts_with("acme/wallet-ui#"));
    snapshot.history.extend([
        HistoryEvent {
            kind: HistoryKind::ReviewGiven,
            at: wall,
            item_id: "review-only/code#33".to_owned(),
        },
        HistoryEvent {
            kind: HistoryKind::PrClosedUnmerged,
            at: wall - SignedDuration::from_hours(24 * 90),
            item_id: "expired/code#42".to_owned(),
        },
    ]);
    super::plugin::write(&w, id, &snapshot).await;
    installed(&w.state.db, wallet.clone(), 42, wall).await;

    let at = Timestamp::now();
    pull_state(&w, 311, "CLOSED", Some(&at.to_string()), None);
    pull_delivery(&w, "closed", "acme/wallet-ui", 311, "alice").await;
    flush_webhooks(&w, Timestamp::now()).await;
    let closure = (HistoryKind::PrClosedUnmerged, at);
    let item = "acme/wallet-ui#311";
    assert!(item_history(&w, item).await.contains(&closure));
    assert!(
        super::plugin::stored(&w)
            .await
            .1
            .pull_requests
            .iter()
            .all(|pr| pr.facts.repo != wallet)
    );

    github
        .prune_installations()
        .await
        .expect("prune installations");
    let tracked = github
        .repository_installations(Some(DEFAULT_TENANT))
        .await
        .expect("tracking");
    let still_live = tracked
        .iter()
        .any(|entry| entry.repo == wallet && entry.live);

    pull_state(&w, 311, "OPEN", None, None);
    pull_delivery_with_repo_id(&w, "reopened", "acme/wallet-ui", 99, 311, "alice").await;
    flush_webhooks(&w, Timestamp::now()).await;
    assert!(
        item_history(&w, item).await.contains(&closure),
        "a different repository ID must not update the chart"
    );

    pull_delivery(&w, "reopened", "acme/wallet-ui", 311, "alice").await;
    flush_webhooks(&w, Timestamp::now()).await;
    assert!(!item_history(&w, item).await.contains(&closure));
    assert!(
        still_live,
        "pruning must retain the App installation for recent authored history"
    );
    assert!(
        tracked
            .iter()
            .all(|entry| !["review-only", "expired"].contains(&entry.repo.owner.as_str()))
    );
}

#[tokio::test]
async fn renamed_repo_reopen_replaces_only_the_same_id_history_after_retry() {
    use contracts::api::dashboard::HistoryEvent;

    let mut w = world().await;
    Arc::get_mut(&mut w.state.config)
        .unwrap()
        .github_webhook_secret = Some(HOOK_SECRET.into());
    let wallet = repo("acme", "wallet-ui");
    let old = repo("acme", "wallet-old");
    let wall = Timestamp::now();
    let mut configured = settings();
    configured.repositories.retain(|row| row.repo != wallet);
    configured.enumerate_owners.clear();
    w.state
        .db
        .dashboard()
        .save_settings(DEFAULT_TENANT, "u1", &configured, wall)
        .await
        .expect("settings");
    w.refreshed().await;
    let (id, mut snapshot) = super::plugin::stored(&w).await;
    snapshot.pull_requests.retain(|pr| pr.facts.repo != wallet);
    snapshot
        .history
        .retain(|event| !event.item_id.starts_with("acme/wallet-ui#"));
    let closed = HistoryKind::PrClosedUnmerged;
    for name in ["wallet-old", "other-wallet"] {
        snapshot.history.push(HistoryEvent {
            kind: closed,
            at: wall,
            item_id: format!("acme/{name}#311"),
        });
    }
    super::plugin::write(&w, id, &snapshot).await;
    w.state
        .db
        .repos()
        .upsert(DEFAULT_TENANT, &old, Some(42))
        .await
        .expect("old repo identity");
    w.state
        .db
        .repos()
        .upsert(DEFAULT_TENANT, &repo("acme", "other-wallet"), Some(99))
        .await
        .expect("unrelated repo identity");
    installed(&w.state.db, old, 42, wall).await;

    let set_answer = |answer_id: &str| {
        w.gate.pull_updates.lock().unwrap().insert(
            311,
            json!({
                "state": "OPEN", "closedAt": null, "mergedAt": null,
                "baseRepository": {
                    "nameWithOwner": "acme/wallet-ui", "isPrivate": false,
                    "defaultBranchRef": {"name": "main"}, "fullDatabaseId": answer_id
                },
            }),
        );
    };
    // A queued hint can resolve to a reused name with a different identity before a retry.
    let receipt = Timestamp::now();
    set_answer("99");
    pull_delivery(&w, "reopened", "acme/wallet-ui", 311, "alice").await;
    flush_webhooks(&w, Timestamp::now()).await;
    assert!(
        item_history(&w, "acme/wallet-ui#311").await.is_empty(),
        "an unrelated answer cannot update history"
    );
    assert_eq!(
        item_history(&w, "acme/wallet-old#311").await,
        [(closed, wall)]
    );

    set_answer("42");
    flush_webhooks(&w, receipt + SignedDuration::from_secs(30)).await;
    assert!(item_history(&w, "acme/wallet-old#311").await.is_empty());
    let current = item_history(&w, "acme/wallet-ui#311").await;
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].0, HistoryKind::PrOpened);
    assert_eq!(
        item_history(&w, "acme/other-wallet#311").await,
        [(closed, wall)]
    );
    let github = w.state.db.github();
    github
        .prune_installations()
        .await
        .expect("prune renamed repository");
    let live = github
        .repository_installations(Some(DEFAULT_TENANT))
        .await
        .expect("tracking");
    assert!(live.iter().any(|entry| entry.repo == wallet && entry.live));
}

#[tokio::test]
async fn terminal_q2_is_used_for_history_without_reading_ownership_files() {
    let w = world().await;
    let merged = Some(LOCAL_MORNING);
    pull_state(&w, 1176, "MERGED", merged, merged);
    w.gate
        .history_updates
        .lock()
        .unwrap()
        .insert("2026-11-06T09:00:00Z".to_owned(), Value::Null);
    w.faults(json!([{
        "on": "rest",
        "matching": "/repos/acme/platform/pulls/1176/files",
        "answer": {"kind": "primary_rate_limit",
            "reset": (Timestamp::now() + SignedDuration::from_hours(1)).as_second()}
    }]))
    .await;

    assert_eq!(w.refreshed().await.state, RefreshState::Finished);
    assert!(
        item_history(&w, "acme/platform#1176")
            .await
            .contains(&(HistoryKind::PrMerged, LOCAL_MORNING.parse().unwrap()))
    );
    assert!(
        super::plugin::github_log(&w)
            .await
            .iter()
            .all(|entry| { entry["path"] != "/repos/acme/platform/pulls/1176/files" })
    );
}
