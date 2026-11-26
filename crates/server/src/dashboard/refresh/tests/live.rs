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
