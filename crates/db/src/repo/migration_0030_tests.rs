//! Cut-over contract: keep identity/configuration bytes; discard old runtime data atomically.

use std::collections::BTreeSet;

use sqlx::AssertSqlSafe;

use super::*;

const KEPT: &[&str] = &[
    "access_tokens",
    "agent_subscriptions",
    "controlled_owners",
    "controlled_owners_changed",
    "dashboard_hidden_mentions",
    "dashboard_settings",
    "dashboard_snoozes",
    "llm_selection",
    "memberships",
    "notification_channels",
    "oauth_logins",
    "pr_watches",
    "repos",
    "sessions",
    "tenant_allowed_logins",
    "tenants",
    "user_tokens",
    "users",
];
const CLEANED: &[&str] = &[
    "analyses",
    "comment_assessments",
    "comment_carry_events",
    "comment_decision_undos",
    "comment_decisions",
    "comment_effect_reports",
    "comment_effects",
    "comment_item_skips",
    "comment_items",
    "comment_passes",
    "comment_prs",
    "comment_requests",
    "dashboard_judgements",
    "dashboard_refreshes",
    "documents",
    "feedback",
    "flow_work",
    "installation_tokens",
    "job_events",
    "jobs",
    "llm_credentials",
    "notification_deliveries",
    "notification_sends",
    "questions",
    "review_states",
    "user_events",
    "working_sessions",
];
const NEW: &[&str] = &[
    "analytic_threads",
    "assessment_attempt_usage",
    "assessment_comparison_items",
    "assessment_config",
    "assessment_daily_usage",
    "assessment_method_runs",
    "assessment_owner_decisions",
    "assessment_passes",
    "comment_sources",
];

async fn tables(pool: &SqlitePool) -> BTreeSet<String> {
    sqlx::query_scalar(
        "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations'",
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .collect()
}

// Include rowid, SQLite type and every byte (including BLOBs and embedded NULs).
async fn snapshot(pool: &SqlitePool, table: &str, excluded: &[&str]) -> (String, Vec<String>) {
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info(?) ORDER BY cid")
            .bind(table)
            .fetch_all(pool)
            .await
            .unwrap();
    let expressions = std::iter::once("quote(rowid)".to_owned())
        .chain(
            columns
                .iter()
                .filter(|c| !excluded.contains(&c.as_str()))
                .map(|c| format!("typeof(\"{c}\") || ':' || hex(CAST(\"{c}\" AS BLOB))")),
        )
        .collect::<Vec<_>>()
        .join(" || '|' || ");
    let rows = sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT {expressions} FROM \"{table}\" ORDER BY rowid"
    )))
    .fetch_all(pool)
    .await
    .unwrap();
    let schema = sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE name=? AND type='table'")
        .bind(table)
        .fetch_one(pool)
        .await
        .unwrap();
    (schema, rows)
}

async fn assert_clean(db: &Db) {
    for table in CLEANED.iter().chain(NEW) {
        let count: i64 = sqlx::query_scalar(AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "{table} must start empty");
    }
    let fk_errors: Vec<String> =
        sqlx::query_scalar("SELECT \"table\" FROM pragma_foreign_key_check")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert!(fk_errors.is_empty(), "{fk_errors:?}");
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
}

#[tokio::test]
async fn migration_0030_cleans_populated_0029_and_preserves_identity_and_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diffpilot.sqlite");
    testing::pre_container_database(&path).await.unwrap();
    let old = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .foreign_keys(true),
        )
        .await
        .unwrap();
    let expected: BTreeSet<_> = KEPT
        .iter()
        .chain(CLEANED)
        .copied()
        .chain(["pull_requests"])
        .map(str::to_owned)
        .collect();
    assert_eq!(
        tables(&old).await,
        expected,
        "classify every pre-0030 table"
    );
    for table in &expected {
        let count: i64 = sqlx::query_scalar(AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(&old)
            .await
            .unwrap();
        assert!(count > 0, "fixture must populate {table}");
    }
    let automatic: String =
        sqlx::query_scalar("SELECT trigger FROM dashboard_refreshes WHERE id='r1'")
            .fetch_one(&old)
            .await
            .unwrap();
    assert_eq!(automatic, "automatic", "exercise the 0029 row shape");
    let mut kept = Vec::new();
    for table in KEPT {
        kept.push((*table, snapshot(&old, table, &[]).await));
    }
    let thread_columns = ["agent_thread_id", "analytic_generation", "session_state"];
    let pr = snapshot(&old, "pull_requests", &thread_columns).await;
    old.close().await;

    // Use the actual startup path, including SQLx checksums and FK-enabled WAL connections.
    let db = Db::open(&path).await.unwrap();
    for (table, before) in &kept {
        assert_eq!(&snapshot(&db.pool, table, &[]).await, before, "{table}");
    }
    assert_eq!(snapshot(&db.pool, "pull_requests", &[]).await.1, pr.1);
    let removed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pragma_table_info('pull_requests') WHERE name IN ('agent_thread_id','analytic_generation','session_state')",
    ).fetch_one(&db.pool).await.unwrap();
    assert_eq!(removed, 0);
    let expected: BTreeSet<_> = expected
        .into_iter()
        .chain(NEW.iter().map(|s| (*s).to_owned()))
        .collect();
    assert_eq!(
        tables(&db.pool).await,
        expected,
        "classify every post-0030 table"
    );
    assert_clean(&db).await;
    db.pool.close().await;
    let reopened = Db::open(&path).await.unwrap();
    assert_clean(&reopened).await;
    for (table, before) in &kept {
        assert_eq!(
            &snapshot(&reopened.pool, table, &[]).await,
            before,
            "{table} on reopen"
        );
    }
}

#[tokio::test]
async fn migration_0030_empty_database_starts_clean() {
    let db = Db::in_memory().await.unwrap();
    assert_clean(&db).await;
    let tenants: i64 = sqlx::query_scalar("SELECT count(*) FROM tenants")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(tenants, 1);
}

#[tokio::test]
async fn migration_0030_cleanup_failure_rolls_back_data_and_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diffpilot.sqlite");
    testing::pre_container_database(&path).await.unwrap();
    let old = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .foreign_keys(true),
        )
        .await
        .unwrap();
    let mut before = Vec::new();
    for table in tables(&old).await {
        before.push((table.clone(), snapshot(&old, &table, &[]).await));
    }
    sqlx::raw_sql("CREATE TRIGGER refuse_cleanup BEFORE DELETE ON analyses BEGIN SELECT RAISE(ABORT, 'injected cleanup failure'); END;")
        .execute(&old).await.unwrap();
    assert!(migrate(&old, MIGRATIONS.len()).await.is_err());
    for (table, data) in before {
        assert_eq!(snapshot(&old, &table, &[]).await, data, "rollback {table}");
    }
    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE version=30")
        .fetch_one(&old)
        .await
        .unwrap();
    assert_eq!(applied, 0);
    let fk: Vec<String> = sqlx::query_scalar("SELECT \"table\" FROM pragma_foreign_key_check")
        .fetch_all(&old)
        .await
        .unwrap();
    assert!(fk.is_empty());
    sqlx::raw_sql("DROP TRIGGER refuse_cleanup;")
        .execute(&old)
        .await
        .unwrap();
    migrate(&old, MIGRATIONS.len()).await.unwrap();
    assert_clean(&Db { pool: old }).await;
}
