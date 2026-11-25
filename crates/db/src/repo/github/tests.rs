use serde_json::json;
use sqlx::SqliteConnection;

use super::*;

fn repo(id: i64, owner: &str, name: &str) -> Repository {
    Repository {
        id: Some(id),
        name: RepoRef {
            owner: owner.to_owned(),
            name: name.to_owned(),
        },
    }
}

async fn test_db() -> Db {
    let db = Db::in_memory().await.unwrap();
    super::super::analyses::testing::seed_analysis(&db)
        .await
        .unwrap();
    sqlx::raw_sql(
        "INSERT INTO memberships VALUES ('default', 'u1', 'member', '2026-11-16T00:00:00Z');
         UPDATE repos SET github_id = 7;",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    db
}

async fn is_tracked(conn: &mut SqliteConnection, repo: &Repository) -> Result<bool, DbError> {
    let resolved = tracking::resolve(conn, std::slice::from_ref(repo), 1).await?;
    match resolved.first() {
        Some(tracked) => tracked.bind(conn).await,
        None => Ok(false),
    }
}

async fn apply_installation(
    conn: &mut SqliteConnection,
    id: i64,
    update: InstallationUpdate<'_>,
    now: Timestamp,
) -> Result<bool, DbError> {
    match installations::prepare(conn, id, update).await? {
        Some(update) => Ok(installations::apply(conn, id, update, now).await?.is_some()),
        None => Ok(false),
    }
}

#[tokio::test]
async fn repo_tracked_via_rules_watch_analysis_or_snapshot_processed() {
    let db = test_db().await;
    let now = Timestamp::now();
    let repository = repo(7, "acme", "node-client");
    sqlx::raw_sql(
        "INSERT INTO pr_watches (user_id, pull_request_id, tenant_id, since, state)
         VALUES ('u1', 1, 'default', '2026-11-16T00:00:00Z', 'active');",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let rules = json!([{"repo":{"owner":"acme","name":"rule"}}]);
    sqlx::query("INSERT INTO dashboard_settings (user_id,tenant_id,repositories,updated_at) VALUES ('u1','default',?,?)")
        .bind(rules.to_string()).bind(ts(now)).execute(&db.pool).await.unwrap();
    let snapshot = json!({"issues":[{"facts":{"repo":{"owner":"acme","name":"snapshot"}}}]});
    sqlx::query("INSERT INTO dashboard_refreshes (id,user_id,tenant_id,trigger,state,ranking_state,snapshot,started_at)
                 VALUES ('fixture','u1','default','manual','finished','finished',?,?)")
        .bind(snapshot.to_string()).bind(ts(now)).execute(&db.pool).await.unwrap();
    for tracked in [
        &repository,
        &repo(8, "acme", "rule"),
        &repo(9, "acme", "snapshot"),
    ] {
        assert!(
            is_tracked(&mut db.pool.acquire().await.unwrap(), tracked)
                .await
                .unwrap(),
            "{tracked:?}"
        );
    }
    sqlx::query("DELETE FROM analyses")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        is_tracked(&mut db.pool.acquire().await.unwrap(), &repository)
            .await
            .unwrap(),
        "watch"
    );
    sqlx::query("DELETE FROM pr_watches")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        !is_tracked(&mut db.pool.acquire().await.unwrap(), &repository)
            .await
            .unwrap()
    );
    super::super::analyses::testing::seed_analysis(&db)
        .await
        .unwrap();
    assert!(
        is_tracked(&mut db.pool.acquire().await.unwrap(), &repository)
            .await
            .unwrap(),
        "analysis"
    );
}

#[tokio::test]
async fn tracked_repo_matched_by_numeric_id_after_rename() {
    let db = test_db().await;
    let mut conn = db.pool.acquire().await.unwrap();
    assert!(
        is_tracked(&mut conn, &repo(7, "other-owner", "new-name"))
            .await
            .unwrap()
    );
    assert!(
        !is_tracked(&mut conn, &repo(8, "acme", "node-client"))
            .await
            .unwrap()
    );
    let mut no_id = repo(7, "acme", "node-client");
    no_id.id = None;
    assert!(is_tracked(&mut conn, &no_id).await.unwrap());
}

#[tokio::test]
async fn renamed_repo_updates_row_and_frees_old_name() {
    let db = test_db().await;
    let mut tx = write_tx(&db.pool).await.unwrap();
    let now = Timestamp::now();
    let original = repo(7, "acme", "node-client");
    assert!(
        apply_installation(&mut tx, 42, InstallationUpdate::Add(&[original]), now)
            .await
            .unwrap()
    );
    assert!(
        apply_installation(
            &mut tx,
            43,
            InstallationUpdate::Add(&[repo(7, "other-owner", "renamed")]),
            now
        )
        .await
        .unwrap()
    );
    let rows: Vec<(String, String, i64, i64)> =
        sqlx::query_as("SELECT owner, name, repository_id, installation_id FROM app_repositories")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        rows,
        [("other-owner".to_owned(), "renamed".to_owned(), 7, 43)]
    );
    assert!(
        !apply_installation(
            &mut tx,
            44,
            InstallationUpdate::Add(&[repo(8, "acme", "node-client")]),
            now
        )
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn installation_events_store_only_tracked_repos() {
    let db = test_db().await;
    let mut tx = write_tx(&db.pool).await.unwrap();
    let now = Timestamp::now();
    let tracked = repo(7, "acme", "node-client");
    let untracked = repo(8, "other-owner", "untracked");
    assert!(
        apply_installation(
            &mut tx,
            42,
            InstallationUpdate::Add(&[tracked.clone(), untracked]),
            now
        )
        .await
        .unwrap()
    );
    let rows: Vec<i64> = sqlx::query_scalar("SELECT repository_id FROM app_repositories")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(rows, [7]);
    assert!(
        apply_installation(&mut tx, 42, InstallationUpdate::Suspend(true), now)
            .await
            .unwrap()
    );
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT suspended FROM app_repositories")
            .fetch_one(&mut *tx)
            .await
            .unwrap()
    );
    assert!(
        apply_installation(&mut tx, 42, InstallationUpdate::Suspend(false), now)
            .await
            .unwrap()
    );
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT suspended FROM app_repositories")
            .fetch_one(&mut *tx)
            .await
            .unwrap()
    );
    assert!(
        apply_installation(&mut tx, 42, InstallationUpdate::Remove(&[tracked]), now)
            .await
            .unwrap()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM app_repositories")
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn app_repositories_capped() {
    let db = test_db().await;
    let mut tx = write_tx(&db.pool).await.unwrap();
    let now = Timestamp::now();
    sqlx::query(
        "WITH RECURSIVE ids(id) AS (SELECT 100 UNION ALL SELECT id+1 FROM ids WHERE id<2099)
         INSERT INTO app_repositories SELECT 'acme', CAST(id AS TEXT), id, 42, 0, ? FROM ids",
    )
    .bind(ts(now))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let repos = [repo(7, "acme", "node-client")];
    let id = Uuid::new_v4();
    let receipt = NewDelivery {
        id,
        hash: &"0".repeat(64),
        event: "installation",
        action: Some("created"),
        installation_id: Some(42),
        effect: Effect::Installation(42, InstallationUpdate::Add(&repos)),
    };
    assert_eq!(
        db.github().record(&receipt, now).await.unwrap(),
        Outcome::InstallationCapped
    );
    let error: String =
        sqlx::query_scalar("SELECT error FROM github_deliveries WHERE delivery_id = ?")
            .bind(id.to_string())
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(error, "installation_cap");
    let mut tx = read_tx(&db.pool).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM app_repositories")
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        2_000
    );
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM app_repositories WHERE repository_id=7)"
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn failed_delivery_reprocessed_on_redelivery() {
    let db = test_db().await;
    let github = db.github();
    let repository = repo(7, "acme", "node-client");
    let stored = "a".repeat(64);
    let delivery = NewDelivery {
        id: Uuid::new_v4(),
        hash: &stored,
        event: "pull_request",
        action: Some("opened"),
        installation_id: Some(42),
        effect: Effect::Signal(&repository, &[]),
    };
    let now = Timestamp::now();
    assert_eq!(
        github.record(&delivery, now).await.unwrap(),
        Outcome::Queued
    );
    for (attempt, seconds) in [10, 60, 600, 0].into_iter().enumerate() {
        let at = now + SignedDuration::from_secs([0, 10, 70, 670][attempt]);
        let pending = github.next_pending(at).await.unwrap().unwrap();
        assert_eq!(pending.attempts, i64::try_from(attempt).unwrap());
        let retry = (seconds > 0).then(|| at + SignedDuration::from_secs(seconds));
        assert!(
            github
                .fail(&pending, DeliveryError::Dispatch, retry, at)
                .await
                .unwrap()
        );
        assert!(github.next_pending(at).await.unwrap().is_none());
    }
    let at = now + SignedDuration::from_secs(671);
    assert_eq!(github.record(&delivery, at).await.unwrap(), Outcome::Queued);
    let pending = github.next_pending(at).await.unwrap().unwrap();
    assert_eq!(pending.attempts, 0);
    github.finish(&pending, at).await.unwrap();
    assert_eq!(
        github.record(&delivery, at).await.unwrap(),
        Outcome::Duplicate
    );
}

#[tokio::test]
async fn webhook_startup_resumes_recent_failed_only() {
    let db = test_db().await;
    let github = db.github();
    let now = Timestamp::now();
    let old = Uuid::new_v4();
    let recent = Uuid::new_v4();
    for (id, days, hash) in [(old, 4, "a".repeat(64)), (recent, 2, "b".repeat(64))] {
        sqlx::query("INSERT INTO github_deliveries (delivery_id,body_sha256,event,state,changes,received_at)
                     VALUES (?,?,'status','failed','[]',?)")
            .bind(id.to_string()).bind(hash).bind(ts(now - SignedDuration::from_hours(days * 24)))
            .execute(&db.pool).await.unwrap();
    }
    github.resume(now).await.unwrap();
    let pending = github.next_pending(now).await.unwrap().unwrap();
    assert_eq!(pending.id, recent);
    github.finish(&pending, now).await.unwrap();
    assert!(github.next_pending(now).await.unwrap().is_none());
}

#[tokio::test]
async fn name_rule_does_not_move_to_new_repository_after_transfer() {
    let db = test_db().await;
    let now = Timestamp::now();
    sqlx::query(
        "INSERT INTO dashboard_settings (user_id,tenant_id,repositories,updated_at)
         VALUES ('u1','default','[{\"repo\":{\"owner\":\"acme\",\"name\":\"configured\"}}]',?)",
    )
    .bind(ts(now))
    .execute(&db.pool)
    .await
    .unwrap();
    let mut tx = write_tx(&db.pool).await.unwrap();
    for (id, repo) in [
        (42, repo(20, "acme", "configured")),
        (43, repo(20, "other-owner", "renamed")),
    ] {
        assert!(
            apply_installation(&mut tx, id, InstallationUpdate::Add(&[repo]), now)
                .await
                .unwrap()
        );
    }
    assert!(
        !is_tracked(&mut tx, &repo(21, "acme", "configured"))
            .await
            .unwrap()
    );
    assert!(
        is_tracked(&mut tx, &repo(20, "other-owner", "renamed"))
            .await
            .unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_receipts_of_one_body_queue_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("dedupe.db")).await.unwrap();
    let hash = "c".repeat(64);
    let shared_id = Uuid::new_v4();
    for same_guid in [false, true] {
        let hash = if same_guid {
            "d".repeat(64)
        } else {
            hash.clone()
        };
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let db = db.clone();
            let hash = hash.clone();
            let id = if same_guid { shared_id } else { Uuid::new_v4() };
            tasks.spawn(async move {
                let delivery = NewDelivery {
                    id,
                    hash: &hash,
                    event: "ping",
                    action: None,
                    installation_id: None,
                    effect: Effect::Ignore,
                };
                db.github()
                    .record(&delivery, Timestamp::now())
                    .await
                    .unwrap()
            });
        }
        let outcomes = tasks.join_all().await;
        assert_eq!(
            outcomes.iter().filter(|o| **o == Outcome::Ignored).count(),
            1,
            "same_guid={same_guid}: {outcomes:?}"
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| **o == Outcome::Duplicate)
                .count(),
            15
        );
        let rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM github_deliveries WHERE body_sha256 = ?")
                .bind(&hash)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(rows, 1);
    }
}

#[tokio::test]
async fn untracked_burst_does_not_take_write_lock_or_store_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::repo::testing::file_backed_with_busy_timeout(
        dir.path(),
        std::time::Duration::from_millis(100),
    )
    .await
    .unwrap();
    let writer = write_tx(&db.pool).await.unwrap();
    let foreign = repo(99, "foreign", "repo");
    let now = Timestamp::now();
    for index in 0..100 {
        let hash = format!("{index:064x}");
        let delivery = NewDelivery {
            id: Uuid::new_v4(),
            hash: &hash,
            event: "status",
            action: None,
            installation_id: Some(11),
            effect: Effect::Signal(&foreign, &[]),
        };
        assert_eq!(
            db.github().record(&delivery, now).await.unwrap(),
            Outcome::NotAllowed
        );
    }
    let mut listed: Vec<_> = (100..20_100)
        .map(|id| repo(id, "foreign", &format!("repo-{id}")))
        .collect();
    let installation = NewDelivery {
        id: Uuid::new_v4(),
        hash: &"f".repeat(64),
        event: "installation",
        action: Some("created"),
        installation_id: Some(11),
        effect: Effect::Installation(11, InstallationUpdate::Add(&listed)),
    };
    assert_eq!(
        db.github().record(&installation, now).await.unwrap(),
        Outcome::NotAllowed
    );
    writer.commit().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM github_deliveries")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(db.github().last_delivery_at().await.unwrap(), None);

    super::super::analyses::testing::seed_analysis(&db)
        .await
        .unwrap();
    sqlx::raw_sql(
        "INSERT INTO memberships VALUES ('default','u1','member','2026-11-16T00:00:00Z');
                   UPDATE repos SET github_id = 7;",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    listed.push(repo(7, "acme", "node-client"));
    let installation = NewDelivery {
        effect: Effect::Installation(11, InstallationUpdate::Add(&listed)),
        id: Uuid::new_v4(),
        hash: &"e".repeat(64),
        event: "installation",
        action: Some("created"),
        installation_id: Some(11),
    };
    assert_eq!(
        db.github().record(&installation, now).await.unwrap(),
        Outcome::Queued
    );
    let stored: Vec<i64> = sqlx::query_scalar("SELECT repository_id FROM app_repositories")
        .fetch_all(&db.pool)
        .await
        .unwrap();
    assert_eq!(stored, [7]);
}

#[tokio::test]
async fn webhook_prune_keeps_the_exact_replay_boundary() {
    let db = Db::in_memory().await.unwrap();
    let now: Timestamp = "2026-11-25T12:00:00Z".parse().unwrap();
    let boundary = now - RETENTION;
    for at in [boundary - SignedDuration::from_secs(1), boundary, now] {
        let id = Uuid::new_v4();
        let hash = format!("{:064x}", id.as_u128());
        let receipt = NewDelivery {
            id,
            hash: &hash,
            event: "ping",
            action: None,
            installation_id: None,
            effect: Effect::Ignore,
        };
        db.github().record(&receipt, at).await.unwrap();
    }
    db.github().prune_deliveries(now).await.unwrap();
    let kept: Vec<String> =
        sqlx::query_scalar("SELECT received_at FROM github_deliveries ORDER BY received_at")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert_eq!(kept, [ts(boundary), ts(now)]);
}

#[tokio::test]
async fn webhook_audits_typed_payload_error_without_queuing() {
    let db = Db::in_memory().await.unwrap();
    let id = Uuid::new_v4();
    let now = Timestamp::now();
    let receipt = NewDelivery {
        id,
        hash: &"1".repeat(64),
        event: "issues",
        action: Some("opened"),
        installation_id: None,
        effect: Effect::Reject(None, DeliveryError::InvalidPayload),
    };
    assert_eq!(
        db.github().record(&receipt, now).await.unwrap(),
        Outcome::Ignored
    );
    let stored: (String, String, String) =
        sqlx::query_as("SELECT state, error, changes FROM github_deliveries WHERE delivery_id = ?")
            .bind(id.to_string())
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        stored,
        (
            "ignored".to_owned(),
            "invalid_payload".to_owned(),
            "[]".to_owned()
        )
    );
    assert_eq!(
        db.github().record(&receipt, now).await.unwrap(),
        Outcome::Duplicate
    );
    let foreign = repo(99, "foreign", "repo");
    let dropped = NewDelivery {
        id: Uuid::new_v4(),
        hash: &"2".repeat(64),
        effect: Effect::IgnoreRepository(&foreign),
        ..receipt
    };
    assert_eq!(
        db.github().record(&dropped, now).await.unwrap(),
        Outcome::NotAllowed
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM github_deliveries")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
}

async fn lifecycle(db: &Db, action: &str, update: InstallationUpdate<'_>, at: Timestamp) {
    let id = Uuid::new_v4();
    let names = ["deleted", "suspend", "unsuspend"];
    assert_eq!(
        db.github()
            .record(
                &NewDelivery {
                    id,
                    hash: &format!("{:064x}", id.as_u128()),
                    event: if names.contains(&action) {
                        "installation"
                    } else {
                        "installation_repositories"
                    },
                    action: Some(action),
                    installation_id: Some(42),
                    effect: Effect::Installation(42, update),
                },
                at
            )
            .await
            .unwrap(),
        Outcome::Queued
    );
}

async fn lookup_row(db: &Db, name: &RepoRef) -> Option<(Option<i64>, i64, bool)> {
    sqlx::query_as("SELECT repository_id, installation_id, suspended FROM app_repositories WHERE owner = ? AND name = ?")
        .bind(&name.owner).bind(&name.name).fetch_optional(&db.pool).await.unwrap()
}

#[tokio::test]
async fn lookup_older_than_installation_event_does_not_reinsert() {
    for (delete, known) in [(false, true), (true, true), (false, false)] {
        let db = test_db().await;
        let now = Timestamp::now();
        let tracked = repo(7, "acme", "node-client");
        let github = db.github();
        if known {
            github
                .record_installation_lookup(&tracked.name, Some((42, false)), now)
                .await
                .unwrap();
        }
        let (action, update) = if delete {
            ("deleted", InstallationUpdate::Delete)
        } else {
            (
                "removed",
                InstallationUpdate::Remove(std::slice::from_ref(&tracked)),
            )
        };
        let received_at = now + SignedDuration::from_secs(2);
        lifecycle(&db, action, update, received_at).await;
        assert!(
            github
                .next_pending(received_at)
                .await
                .unwrap()
                .unwrap()
                .changes
                .is_empty()
        );
        github
            .record_installation_lookup(
                &tracked.name,
                Some((42, false)),
                now + SignedDuration::from_secs(1),
            )
            .await
            .unwrap();
        assert_eq!(lookup_row(&db, &tracked.name).await, None);
    }
}

#[tokio::test]
async fn unrelated_repository_delivery_does_not_block_lookup() {
    let db = test_db().await;
    let now = Timestamp::now();
    let other = repo(8, "acme", "other");
    sqlx::query("INSERT INTO dashboard_settings (user_id,tenant_id,enumerate_owners,updated_at) VALUES ('u1','default',?,?)").bind(json!(["acme"]).to_string())
        .bind(ts(now)).execute(&db.pool).await.unwrap();
    let github = db.github();
    github
        .record_installation_lookup(&other.name, Some((42, false)), now)
        .await
        .unwrap();
    lifecycle(
        &db,
        "removed",
        InstallationUpdate::Remove(&[other]),
        now + SignedDuration::from_secs(2),
    )
    .await;
    let tracked = repo(7, "acme", "node-client");
    github
        .record_installation_lookup(
            &tracked.name,
            Some((42, false)),
            now + SignedDuration::from_secs(1),
        )
        .await
        .unwrap();
    assert_eq!(
        lookup_row(&db, &tracked.name).await,
        Some((None, 42, false))
    );
}

#[tokio::test]
async fn lookup_does_not_overwrite_newer_webhook_row() {
    let db = test_db().await;
    let now = Timestamp::now();
    let tracked = repo(7, "acme", "node-client");
    lifecycle(
        &db,
        "added",
        InstallationUpdate::Add(std::slice::from_ref(&tracked)),
        now + SignedDuration::from_secs(2),
    )
    .await;
    db.github()
        .record_installation_lookup(&tracked.name, Some((23, true)), now)
        .await
        .unwrap();
    assert_eq!(
        lookup_row(&db, &tracked.name).await,
        Some((Some(7), 42, false))
    );
}

#[tokio::test]
async fn lookup_not_found_keeps_row_added_after_start() {
    let db = test_db().await;
    let now = Timestamp::now();
    let tracked = repo(7, "acme", "node-client");
    lifecycle(
        &db,
        "added",
        InstallationUpdate::Add(std::slice::from_ref(&tracked)),
        now + SignedDuration::from_secs(2),
    )
    .await;
    db.github()
        .record_installation_lookup(&tracked.name, None, now)
        .await
        .unwrap();
    assert_eq!(
        lookup_row(&db, &tracked.name).await,
        Some((Some(7), 42, false))
    );
}

#[tokio::test]
async fn removed_event_matches_rest_learned_row_by_name() {
    let db = test_db().await;
    let now = Timestamp::now();
    let tracked = repo(7, "ACME", "Node-Client");
    db.github()
        .record_installation_lookup(&repo(7, "acme", "node-client").name, Some((42, false)), now)
        .await
        .unwrap();
    assert_eq!(
        lookup_row(&db, &tracked.name).await,
        Some((None, 42, false))
    );
    lifecycle(
        &db,
        "removed",
        InstallationUpdate::Remove(std::slice::from_ref(&tracked)),
        now + SignedDuration::from_secs(1),
    )
    .await;
    assert_eq!(lookup_row(&db, &tracked.name).await, None);
}

#[tokio::test]
async fn added_event_binds_id_to_rest_learned_row() {
    let db = test_db().await;
    let now = Timestamp::now();
    let tracked = repo(7, "ACME", "Node-Client");
    db.github()
        .record_installation_lookup(&repo(7, "acme", "node-client").name, Some((42, true)), now)
        .await
        .unwrap();
    assert_eq!(lookup_row(&db, &tracked.name).await, Some((None, 42, true)));
    lifecycle(
        &db,
        "added",
        InstallationUpdate::Add(std::slice::from_ref(&tracked)),
        now + SignedDuration::from_secs(1),
    )
    .await;
    assert_eq!(
        lookup_row(&db, &tracked.name).await,
        Some((Some(7), 42, false))
    );
}

#[tokio::test]
async fn prune_installations_keeps_tracked_rows() {
    let db = test_db().await;
    let now = Timestamp::now();
    let tracked = repo(7, "acme", "node-client");
    db.github()
        .record_installation_lookup(&tracked.name, Some((42, false)), now)
        .await
        .unwrap();
    sqlx::query("INSERT INTO app_repositories VALUES ('acme', 'untracked', 8, 42, 0, ?)")
        .bind(ts(now))
        .execute(&db.pool)
        .await
        .unwrap();
    db.github().prune_installations().await.unwrap();
    assert_eq!(
        lookup_row(&db, &tracked.name).await,
        Some((None, 42, false))
    );
    assert_eq!(
        lookup_row(&db, &repo(8, "acme", "untracked").name).await,
        None
    );
}

#[tokio::test]
async fn dashboard_fanout_matches_repository_ids_in_each_tenant() {
    use crate::repo::users::GithubProfile;

    let db = test_db().await;
    let now = Timestamp::now();
    sqlx::query("INSERT INTO tenants (id, name, created_at) VALUES ('other', 'Other', ?)")
        .bind(ts(now))
        .execute(&db.pool)
        .await
        .unwrap();
    db.users()
        .upsert_member(
            "other",
            "u2",
            &GithubProfile {
                github_id: 22,
                login: "bob".into(),
                name: None,
                avatar_url: None,
            },
            now,
        )
        .await
        .unwrap();
    let repository = repo(7, "acme", "node-client");
    db.repos()
        .upsert("other", &repository.name, Some(8))
        .await
        .unwrap();
    let item = json!({"facts": {"repo": repository.name, "number": 1}});
    let snapshot_version = 2;
    let snapshot = json!({"version": snapshot_version, "pull_requests": [item], "issues": []});
    for (tenant, user) in [("default", "u1"), ("other", "u2")] {
        sqlx::query(
            "INSERT INTO dashboard_refreshes (id, user_id, tenant_id, trigger, state, ranking_state, snapshot, started_at)
             VALUES (?, ?, ?, 'manual', 'finished', 'finished', ?, ?)",
        ).bind(tenant).bind(user).bind(tenant).bind(snapshot.to_string()).bind(ts(now))
            .execute(&db.pool).await.unwrap();
    }
    let mut change = WebhookChange {
        kind: ChangeKind::PullRequest,
        repository,
        head_repository: None,
        number: Some(1),
        head_sha: None,
        action: None,
        installation_id: None,
        base_repository: None,
        candidate_login: None,
    };
    let github = db.github();
    assert!(
        github
            .dashboard_targets(std::slice::from_ref(&change), 1)
            .await
            .unwrap()
            .is_empty()
    );
    let targets = github
        .dashboard_targets(std::slice::from_ref(&change), snapshot_version)
        .await
        .unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(
        (&*targets[0].tenant_id, &*targets[0].user_id),
        ("default", "u1")
    );
    change.repository.id = Some(8);
    let targets = github
        .dashboard_targets(std::slice::from_ref(&change), snapshot_version)
        .await
        .unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(
        (&*targets[0].tenant_id, &*targets[0].user_id),
        ("other", "u2")
    );

    // The same GitHub repository may legitimately be tracked by more than one tenant.
    db.repos()
        .upsert("other", &change.repository.name, Some(7))
        .await
        .unwrap();
    change.repository.id = Some(7);
    let targets = github
        .dashboard_targets(&[change], snapshot_version)
        .await
        .unwrap();
    assert_eq!(
        targets
            .into_iter()
            .map(|target| target.tenant_id)
            .collect::<std::collections::BTreeSet<_>>(),
        ["default".into(), "other".into()].into()
    );
}
