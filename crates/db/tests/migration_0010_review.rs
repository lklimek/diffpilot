//! Review repro for migration 0010 (`language_ru`): a database built by the real migrator through 0009, seeded
//! in every user/analysis child table, upgraded by `Db::open` (the production path).
#![expect(clippy::expect_used, reason = "test helpers outside #[test] functions")]

use std::path::Path;

use contracts::common::LlmLanguage;
use contracts::time::Timestamp;
use db::repo::Db;
use sqlx::migrate::{Migration, MigrationType, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{AssertSqlSafe, SqlSafeStr};

const MIGRATIONS: [(i64, &str, &str); 10] = [
    (1, "init", include_str!("../migrations/0001_init.sql")),
    (
        2,
        "flow_auto_retries",
        include_str!("../migrations/0002_flow_auto_retries.sql"),
    ),
    (
        3,
        "dashboard",
        include_str!("../migrations/0003_dashboard.sql"),
    ),
    (
        4,
        "access_tokens",
        include_str!("../migrations/0004_access_tokens.sql"),
    ),
    (
        5,
        "ranking_attempt",
        include_str!("../migrations/0005_ranking_attempt.sql"),
    ),
    (
        6,
        "ask_effort",
        include_str!("../migrations/0006_ask_effort.sql"),
    ),
    (
        7,
        "jobs_targets",
        include_str!("../migrations/0007_jobs_targets.sql"),
    ),
    (
        8,
        "ranking_sent",
        include_str!("../migrations/0008_ranking_sent.sql"),
    ),
    (
        9,
        "known_bots",
        include_str!("../migrations/0009_known_bots.sql"),
    ),
    (
        10,
        "language_ru",
        include_str!("../migrations/0010_language_ru.sql"),
    ),
];

/// Same construction as `db::repo::migrate`.
async fn migrate(pool: &SqlitePool, count: usize) -> Result<(), sqlx::migrate::MigrateError> {
    let migrations = MIGRATIONS
        .iter()
        .take(count)
        .map(|(version, description, sql)| {
            Migration::new(
                *version,
                (*description).into(),
                MigrationType::Simple,
                sql.into_sql_str(),
                false,
            )
        })
        .collect();
    Migrator::with_migrations(migrations).run(pool).await
}

/// Production connection options (`Db::open_with`).
async fn pool(path: &Path, max: u32) -> SqlitePool {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true);
    SqlitePoolOptions::new()
        .max_connections(max)
        .connect_with(options)
        .await
        .expect("pool")
}

const SEED: &str = "
INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES ('u1', 1, 'lklimek', 't', 't');
INSERT INTO users (id, github_id, login, language, created_at, updated_at) VALUES ('u2', 2, 'bob', 'pl', 't', 't');
INSERT INTO memberships (tenant_id, user_id, created_at) VALUES ('default', 'u1', 't'), ('default', 'u2', 't');
INSERT INTO sessions (token_hash, tenant_id, user_id, csrf_token, created_at, last_seen_at, expires_at)
  VALUES (x'01', 'default', 'u1', 'c', 't', 't', 't');
INSERT INTO user_tokens (user_id, tenant_id, access_ct, access_expires_at, refresh_ct, refresh_expires_at, updated_at)
  VALUES ('u1', 'default', x'02', 't', x'03', 't', 't');
INSERT INTO repos (id, tenant_id, owner, name) VALUES (1, 'default', 'acme', 'node-client');
INSERT INTO pull_requests (id, tenant_id, repo_id, number, title, author, html_url, base_sha, head_sha, fetched_at)
  VALUES (1, 'default', 1, 482, 'Заголовок ż', 'alice', 'u', 'b', 'h', 't');
INSERT INTO analyses (id, tenant_id, pull_request_id, base_sha, head_sha, language, state, created_by, created_at,
    background_analysis)
  VALUES ('a1', 'default', 1, 'b', 'h1', 'en', 'running', 'u1', 't', 0),
         ('a2', 'default', 1, 'b', 'h1', 'pl', 'finished', 'u2', 't', 1);
INSERT INTO documents (analysis_id, tenant_id, kind, key, body, created_at)
  VALUES ('a1', 'default', 'plan', '', '{\"x\":\"текст\"}', 't');
INSERT INTO review_states (user_id, analysis_id, tenant_id, updated_at) VALUES ('u1', 'a1', 'default', 't');
INSERT INTO questions (id, tenant_id, analysis_id, step_id, user_id, text, state, created_at)
  VALUES ('q1', 'default', 'a1', 'f1.1', 'u1', 'why?', 'pending', 't');
INSERT INTO feedback (id, tenant_id, analysis_id, user_id, target_kind, target_key, value, created_at, updated_at)
  VALUES ('fb1', 'default', 'a1', 'u1', 'step', 'f1.1', 'wrong', 't', 't');
INSERT INTO comment_requests (user_id, idempotency_key, tenant_id, analysis_id, request_hash, state, created_at)
  VALUES ('u1', 'k1', 'default', 'a1', x'04', 'done', 't');
INSERT INTO working_sessions (tenant_id, pull_request_id, user_id, agent_thread_id, analytic_generation)
  VALUES ('default', 1, 'u1', 'th', 0);
INSERT INTO flow_work (tenant_id, analysis_id, flow_id, requested_at, error) VALUES ('default', 'a1', 'f1', 't', NULL);
INSERT INTO jobs (id, tenant_id, analysis_id, kind, priority, state, spec, credential_id, created_at, stage)
  VALUES ('j1', 'default', 'a1', 'analyse_flow', 1, 'succeeded', '{}', 'c1', 't', 'flow:f1');
INSERT INTO job_events (job_id, tenant_id, attempt, seq, at, body) VALUES ('j1', 'default', 1, 1, 't', '{}');
INSERT INTO installation_tokens (job_id, tenant_id, attempt, ciphertext, expires_at) VALUES ('j1', 'default', 1, x'00', 't');
INSERT INTO dashboard_settings (user_id, tenant_id, updated_at) VALUES ('u1', 'default', 't');
INSERT INTO dashboard_refreshes (id, tenant_id, user_id, trigger, state, started_at, snapshot, ranking_state)
  VALUES ('r1', 'default', 'u1', 'manual', 'finished', 't', '{}', 'finished');
INSERT INTO dashboard_judgements (tenant_id, user_id, item_key, input_fingerprint, input_version, prompt_version,
    body, refresh_id, model, judged_at, last_seen_at)
  VALUES ('default', 'u1', 'pr:acme/node-client#482', 'f', 2, 4, '{}', 'r1', 'm', 't', 't');
INSERT INTO access_tokens (id, tenant_id, user_id, name, token_hash, scopes, created_at, expires_at)
  VALUES ('Ab3dE5gH', 'default', 'u1', 'agent', zeroblob(32), '[\"read\"]', 't', 't');
";

/// Every table's rows as quoted text, and every schema entry except the two edited table definitions.
async fn snapshot(pool: &SqlitePool) -> (Vec<(String, Vec<String>)>, Vec<String>) {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .expect("tables");
    let mut rows = Vec::new();
    for table in tables {
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
                .bind(&table)
                .fetch_all(pool)
                .await
                .expect("columns");
        let expr = columns
            .iter()
            .map(|c| format!("quote(\"{c}\")"))
            .collect::<Vec<_>>()
            .join(" || '|' || ");
        let dump: Vec<String> = sqlx::query_scalar(AssertSqlSafe(format!(
            "SELECT {expr} FROM \"{table}\" ORDER BY 1"
        )))
        .fetch_all(pool)
        .await
        .expect("dump");
        rows.push((table, dump));
    }
    let schema: Vec<String> = sqlx::query_scalar(
        "SELECT type || '|' || name || '|' || tbl_name || '|' || rootpage || '|' || coalesce(sql, '')
         FROM sqlite_schema WHERE NOT (type = 'table' AND name IN ('users', 'analyses')) ORDER BY type, name",
    )
    .fetch_all(pool)
    .await
    .expect("schema");
    (rows, schema)
}

async fn table_sql(pool: &SqlitePool, table: &str) -> String {
    sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1")
        .bind(table)
        .fetch_one(pool)
        .await
        .expect("table sql")
}

/// 0010 through `Db::open` on a seeded 0009 database: data, schema objects and checks intact, `ru` accepted on
/// both tables (new, already-open and other-pool connections), an unknown language still refused.
#[tokio::test]
async fn migration_0010_on_a_seeded_v9_database() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("diffpilot.sqlite");
    let old = pool(&path, 2).await;
    migrate(&old, 9).await.expect("0001..0009");
    sqlx::raw_sql(SEED).execute(&old).await.expect("seed");
    let (rows_before, schema_before) = snapshot(&old).await;
    let empty: Vec<&str> = rows_before
        .iter()
        .filter(|(t, r)| r.is_empty() && !t.starts_with('_'))
        .map(|(t, _)| t.as_str())
        .collect();
    eprintln!("tables without seeded rows: {empty:?}");
    let users_before = table_sql(&old, "users").await;
    let analyses_before = table_sql(&old, "analyses").await;
    let version_before: i64 = sqlx::query_scalar("PRAGMA schema_version")
        .fetch_one(&old)
        .await
        .expect("version");

    // A connection of the old pool and a separate pool (another process's view) with prepared statements.
    let mut held = old.acquire().await.expect("held");
    sqlx::query("UPDATE users SET language = ?1 WHERE id = 'u1'")
        .bind("en")
        .execute(&mut *held)
        .await
        .expect("prepared before");
    let other = pool(&path, 1).await;
    sqlx::query("UPDATE analyses SET language = ?1 WHERE id = 'a1'")
        .bind("en")
        .execute(&other)
        .await
        .expect("prepared before");

    let db = Db::open(&path).await.expect("Db::open applies 0010");

    let version_after: i64 = sqlx::query_scalar("PRAGMA schema_version")
        .fetch_one(&mut *held)
        .await
        .expect("version");
    assert!(
        version_after > version_before,
        "{version_before} -> {version_after}"
    );
    assert_eq!(
        table_sql(&old, "users").await,
        users_before.replace("('en', 'pl')", "('en', 'pl', 'ru')")
    );
    assert_eq!(
        table_sql(&old, "analyses").await,
        analyses_before.replace("('en', 'pl')", "('en', 'pl', 'ru')")
    );
    let (rows_after, schema_after) = snapshot(&old).await;
    // 0018 rebuilds `jobs` (with `job_events` and `installation_tokens`), `questions` and `comment_passes` (new root
    // pages, the `pass_id` owner, comment-item questions; the schema tests check them): their schema entries are left
    // out here and their rows compared without the added columns (`jobs.pass_id`, `questions.pull_request_id` and
    // `questions.item_id`).
    // 0029 rebuilds `dashboard_refreshes` and `dashboard_judgements` (and `jobs` with its children again) for the
    // `automatic` trigger; the schema tests check them, their rows are compared unchanged.
    const REBUILT_0018: [&str; 7] = [
        "jobs",
        "job_events",
        "installation_tokens",
        "questions",
        "comment_passes",
        "dashboard_refreshes",
        "dashboard_judgements",
    ];
    let rebuilt_0018 = |entry: &String| {
        entry
            .split('|')
            .nth(2)
            .is_some_and(|table| REBUILT_0018.contains(&table))
    };
    let schema_before: Vec<_> = schema_before
        .into_iter()
        .filter(|entry| !rebuilt_0018(entry))
        .collect();
    // 0027 adds `dashboard_settings.slack_names` in place (the schema tests check it): its table entry is compared
    // without the added column, its rows without the value (an empty table).
    const SLACK_NAMES_0027: &str = ", slack_names TEXT NOT NULL DEFAULT '[]'
    CHECK (json_valid(slack_names) AND json_type(slack_names) = 'array')";
    let schema_after: Vec<_> = schema_after
        .into_iter()
        .filter(|entry| !rebuilt_0018(entry))
        .map(|entry| entry.replace(SLACK_NAMES_0027, ""))
        .collect();
    let rows_after: Vec<(String, Vec<String>)> = rows_after
        .into_iter()
        .map(|(table, rows)| {
            let added: &[(usize, &str)] = match table.as_str() {
                "jobs" => &[(4, "NULL")],
                "questions" => &[(21, "NULL"), (3, "1")],
                "dashboard_settings" => &[(18, "'[]'")],
                _ => &[],
            };
            let rows = rows
                .into_iter()
                .map(|row| {
                    let mut fields: Vec<&str> = row.split('|').collect();
                    for (index, value) in added {
                        assert_eq!(fields.get(*index), Some(value), "{table}: {row}");
                        fields.remove(*index);
                    }
                    fields.join("|")
                })
                .collect::<Vec<String>>();
            (table, rows)
        })
        .collect();
    // Db::open also applies 0011 (a new empty table and its index) and 0014 (the `questions_by_parent` index): additive
    // to this 0010 preservation check.
    assert_eq!(
        rows_after
            .iter()
            .find(|(t, _)| t == "dashboard_hidden_mentions")
            .map(|(_, rows)| rows.len()),
        Some(0)
    );
    // 0017 and 0019 add the review-loop tables (with their indexes and trigger), 0021 `dashboard_snoozes` (with its
    // index; 0026 rebuilds it) and 0025 `notification_sends`, all empty: left out of this check.
    const ADDED_0017: [&str; 17] = [
        "comment_prs",
        "comment_items",
        "comment_assessments",
        "comment_passes",
        "comment_decisions",
        "comment_effects",
        "comment_effect_reports",
        "comment_carry_events",
        "agent_subscriptions",
        "pr_watches",
        "user_events",
        "notification_channels",
        "notification_deliveries",
        "comment_decision_undos",
        "comment_item_skips",
        "dashboard_snoozes",
        "notification_sends",
    ];
    let added_0017 = |entry: &String| {
        entry
            .split('|')
            .nth(2)
            .is_some_and(|table| ADDED_0017.contains(&table))
    };
    for table in ADDED_0017 {
        let rows = rows_after
            .iter()
            .find(|(t, _)| t == table)
            .map(|(_, rows)| rows.len());
        assert_eq!(rows, Some(0), "{table}");
    }
    let schema_after: Vec<_> = schema_after
        .into_iter()
        .filter(|entry| !added_0017(entry))
        .collect();
    let rows_after: Vec<_> = rows_after
        .into_iter()
        .filter(|(t, _)| !ADDED_0017.contains(&t.as_str()))
        .collect();
    // 0015 adds `controlled_owners` and `controlled_owners_changed` (with their primary-key indexes) and widens the
    // document kinds in place.
    // (0014's `questions_by_parent` is left out with the tables 0018 rebuilds.)
    assert_eq!(schema_after.len(), schema_before.len() + 6);
    let added_0015 = |entry: &String| {
        matches!(
            entry.split('|').nth(2),
            Some("controlled_owners" | "controlled_owners_changed")
        )
    };
    // 0012 rebuilds `questions` with four anchor columns after `step_id`, 0013 appends `touched_steps` and 0014
    // `kind`, `flow_id` and `parent_id`, all empty for existing step questions except `kind`; the repository and
    // schema tests check that table, so here it is compared without them.
    let rebuilt = |entry: &String| entry.split('|').nth(2) == Some("questions");
    let schema_before: Vec<_> = schema_before
        .into_iter()
        .filter(|entry| !rebuilt(entry))
        .map(|entry| entry.replace("'memcan_profile'))", "'memcan_profile', 'dependencies'))"))
        .collect();
    let schema_after: Vec<_> = schema_after
        .into_iter()
        .filter(|entry| {
            entry.split('|').nth(2) != Some("dashboard_hidden_mentions")
                && !rebuilt(entry)
                && !added_0015(entry)
        })
        .collect();
    let rows_after: Vec<_> = rows_after
        .into_iter()
        .filter(|(t, _)| {
            !matches!(
                t.as_str(),
                "_sqlx_migrations"
                    | "dashboard_hidden_mentions"
                    | "controlled_owners"
                    | "controlled_owners_changed"
            )
        })
        .map(|(table, rows)| {
            if table != "questions" {
                return (table, rows);
            }
            let rows = rows
                .into_iter()
                .map(|row| {
                    let fields: Vec<&str> = row.splitn(9, '|').collect();
                    assert_eq!(fields[4..8], ["NULL"; 4], "{row}");
                    let row = [&fields[..4], &fields[8..]].concat().join("|");
                    row.strip_suffix("|NULL|'step'|NULL|NULL")
                        .unwrap_or_else(|| panic!("{row}"))
                        .to_owned()
                })
                .collect();
            (table, rows)
        })
        .collect();
    // 0016 drops the allowlist logins 0001 seeded, still unbound here.
    let rows_before: Vec<_> = rows_before
        .into_iter()
        .filter(|(t, _)| t != "_sqlx_migrations")
        .map(|(table, rows)| {
            if table == "tenant_allowed_logins" {
                (table, Vec::new())
            } else {
                (table, rows)
            }
        })
        .collect();
    assert_eq!(rows_before, rows_after, "row contents changed");
    assert_eq!(schema_before, schema_after, "other schema entries changed");
    let integrity: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(&old)
        .await
        .expect("integrity");
    assert_eq!(integrity, vec!["ok".to_owned()]);
    let fk: Vec<String> = sqlx::query_scalar("SELECT \"table\" FROM pragma_foreign_key_check")
        .fetch_all(&old)
        .await
        .expect("fk");
    assert!(fk.is_empty(), "{fk:?}");

    // The production API on the new pool.
    db.settings()
        .set_language("u2", LlmLanguage::Ru, Timestamp::now())
        .await
        .expect("ru via Db");
    // The connection held across the migration, with its cached statement.
    sqlx::query("UPDATE users SET language = ?1 WHERE id = 'u1'")
        .bind("ru")
        .execute(&mut *held)
        .await
        .expect("ru on the held connection");
    // The other pool, with its cached statement.
    sqlx::query("UPDATE analyses SET language = ?1 WHERE id = 'a1'")
        .bind("ru")
        .execute(&other)
        .await
        .expect("ru on the other pool");
    sqlx::raw_sql(
        "INSERT INTO analyses (id, tenant_id, pull_request_id, base_sha, head_sha, language, state, created_by,
             created_at, background_analysis)
           VALUES ('a3', 'default', 1, 'b', 'h3', 'ru', 'planning', 'u2', 't', 0)",
    )
    .execute(&other)
    .await
    .expect("ru insert");
    for (sql, what) in [
        (
            "UPDATE users SET language = 'de' WHERE id = 'u1'",
            "users de",
        ),
        (
            "UPDATE analyses SET language = 'de' WHERE id = 'a1'",
            "analyses de",
        ),
        (
            "UPDATE users SET language = 'RU' WHERE id = 'u1'",
            "users RU",
        ),
    ] {
        assert!(
            sqlx::query(AssertSqlSafe(sql.to_owned()))
                .execute(&mut *held)
                .await
                .is_err(),
            "{what} accepted"
        );
        assert!(
            sqlx::query(AssertSqlSafe(sql.to_owned()))
                .execute(&other)
                .await
                .is_err(),
            "{what} accepted on the other pool"
        );
    }
}

/// After 0010 the migrating connection has schema writing off again.
#[tokio::test]
async fn migration_0010_leaves_writable_schema_off() {
    let dir = tempfile::tempdir().expect("dir");
    let single = pool(&dir.path().join("db.sqlite"), 1).await;
    migrate(&single, 9).await.expect("0009");
    migrate(&single, 10).await.expect("0010");
    let writable: i64 = sqlx::query_scalar("PRAGMA writable_schema")
        .fetch_one(&single)
        .await
        .expect("pragma");
    assert_eq!(writable, 0);
}

/// Contract: 0010 either widens both CHECKs or fails. A stored definition whose text differs from 0001's (here one
/// extra space) must not be recorded as applied while `ru` is still refused.
#[tokio::test]
async fn migration_0010_fails_loudly_when_the_definition_does_not_match() {
    let dir = tempfile::tempdir().expect("dir");
    let single = pool(&dir.path().join("db.sqlite"), 1).await;
    migrate(&single, 9).await.expect("0009");
    sqlx::raw_sql(
        "PRAGMA writable_schema = ON;
         UPDATE sqlite_schema SET sql = replace(sql, 'CHECK (language IN', 'CHECK  (language IN')
           WHERE type = 'table' AND name = 'users';
         PRAGMA writable_schema = RESET;
         CREATE TABLE drift (x INTEGER); DROP TABLE drift;
         INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES ('u1', 1, 'a', 't', 't');",
    )
    .execute(&single)
    .await
    .expect("drifted definition");
    let migrated = migrate(&single, 10).await;
    let ru = sqlx::query("UPDATE users SET language = 'ru' WHERE id = 'u1'")
        .execute(&single)
        .await;
    assert!(
        migrated.is_err() || ru.is_ok(),
        "0010 recorded as applied ({migrated:?}) but ru refused: {ru:?}"
    );
}
