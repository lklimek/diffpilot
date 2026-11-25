use serde_json::json;

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
    assert!(
        apply_installation(
            &mut tx,
            42,
            InstallationUpdate::Add(&[repo(7, "acme", "node-client")]),
            now
        )
        .await
        .unwrap()
    );
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
