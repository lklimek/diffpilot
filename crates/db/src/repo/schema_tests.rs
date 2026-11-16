//! Schema tests: migration, seeds, tenant columns, CHECK lists against the contract enums, queue invariants.

use contracts::agent::JobKind;
use contracts::api::analyses::AnalysisState;
use contracts::api::feedback::FeedbackValue;
use contracts::api::questions::AskState;
use contracts::common::LlmLanguage;
use contracts::job::spec::CredentialKind;
use contracts::job::state::JobState;
use serde::Serialize;

use super::*;

async fn db() -> Db {
    Db::in_memory().await.expect("in-memory database")
}

fn wire_name<T: Serialize>(value: T) -> String {
    match serde_json::to_value(value).expect("serialises") {
        serde_json::Value::String(name) => name,
        serde_json::Value::Object(map) => map["state"].as_str().expect("tag").to_owned(),
        other => panic!("unexpected wire form {other}"),
    }
}

fn in_list(names: impl IntoIterator<Item = String>) -> String {
    let quoted: Vec<String> = names.into_iter().map(|name| format!("'{name}'")).collect();
    format!("IN ({})", quoted.join(", "))
}

async fn table_sql(db: &Db, table: &str) -> String {
    sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?")
        .bind(table)
        .fetch_one(&db.pool)
        .await
        .expect("table exists")
}

/// A user, repository, pull request and analysis `a1`/`a2`, for queue tests.
async fn seed(db: &Db) {
    sqlx::raw_sql(
        "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES ('u1', 1, 'lklimek', 't', 't');
         INSERT INTO repos (id, tenant_id, owner, name) VALUES (1, 'default', 'acme', 'node-client');
         INSERT INTO pull_requests (id, tenant_id, repo_id, number, title, author, html_url, base_sha, head_sha, fetched_at)
           VALUES (1, 'default', 1, 482, 't', 'alice', 'u', 'b', 'h', 't');
         INSERT INTO analyses (id, tenant_id, pull_request_id, base_sha, head_sha, language, state, created_by, created_at)
           VALUES ('a1', 'default', 1, 'b', 'h1', 'en', 'running', 'u1', 't'),
                  ('a2', 'default', 1, 'b', 'h2', 'en', 'running', 'u1', 't');",
    )
    .execute(&db.pool)
    .await
    .expect("seed");
}

async fn insert_job(
    db: &Db,
    id: &str,
    analysis: &str,
    credential: &str,
    state: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO jobs (id, tenant_id, analysis_id, kind, priority, state, spec, credential_id, created_at)
         VALUES (?, 'default', ?, 'analyse_flow', 1, ?, '{}', ?, 't')",
    )
    .bind(id)
    .bind(analysis)
    .bind(state)
    .bind(credential)
    .execute(&db.pool)
    .await?;
    Ok(())
}

#[tokio::test]
async fn migrations_apply_to_an_empty_file_and_reopen() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("smart-review.sqlite");
    Db::open(&path)
        .await
        .expect("first open creates the schema");
    let db = Db::open(&path).await.expect("reopen applies nothing new");
    let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&db.pool)
        .await
        .expect("migration table");
    assert_eq!(applied, 1);
}

#[tokio::test]
async fn tenant_default_seeded() {
    let db = db().await;
    let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM tenants")
        .fetch_all(&db.pool)
        .await
        .expect("tenants");
    assert_eq!(ids, [DEFAULT_TENANT]);
}

#[tokio::test]
async fn default_allowlist_seeded_unbound_and_case_insensitive() {
    let db = db().await;
    let rows: Vec<(String, Option<i64>)> =
        sqlx::query_as("SELECT login, github_id FROM tenant_allowed_logins WHERE tenant_id = 'default' ORDER BY login")
            .fetch_all(&db.pool)
            .await
            .expect("allowlist");
    assert_eq!(
        rows,
        [
            ("Claudius-Maginificent".to_owned(), None),
            ("lklimek".to_owned(), None)
        ]
    );
    let found: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tenant_allowed_logins WHERE login = 'LKLIMEK'")
            .fetch_one(&db.pool)
            .await
            .expect("lookup");
    assert_eq!(found, 1);
}

#[tokio::test]
async fn allowlist_binds_each_github_id_once_per_tenant() {
    let db = db().await;
    sqlx::query(
        "UPDATE tenant_allowed_logins SET github_id = 7, bound_at = 't' WHERE login = 'lklimek'",
    )
    .execute(&db.pool)
    .await
    .expect("bind first login");
    let second = sqlx::query(
        "UPDATE tenant_allowed_logins SET github_id = 7, bound_at = 't' WHERE login = 'Claudius-Maginificent'",
    )
    .execute(&db.pool)
    .await
    .map_err(DbError::from);
    assert!(matches!(second, Err(DbError::Conflict)), "{second:?}");
    let half_bound = sqlx::query(
        "UPDATE tenant_allowed_logins SET github_id = 8 WHERE login = 'Claudius-Maginificent'",
    )
    .execute(&db.pool)
    .await;
    assert!(
        half_bound.is_err(),
        "github_id without bound_at must be rejected"
    );
}

#[tokio::test]
async fn tenant_id_on_every_table() {
    let db = db().await;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
           AND name NOT IN ('_sqlx_migrations', 'tenants', 'users') ORDER BY name",
    )
    .fetch_all(&db.pool)
    .await
    .expect("tables");
    assert!(tables.len() >= 15, "{tables:?}");
    for table in tables {
        let not_null: Option<i64> = sqlx::query_scalar(
            "SELECT \"notnull\" FROM pragma_table_info(?) WHERE name = 'tenant_id'",
        )
        .bind(&table)
        .fetch_optional(&db.pool)
        .await
        .expect("table info");
        assert_eq!(not_null, Some(1), "{table} lacks a NOT NULL tenant_id");
    }
}

#[tokio::test]
async fn check_lists_match_contract_enums() {
    let db = db().await;
    let analysis_states = [
        AnalysisState::Planning,
        AnalysisState::Running { done: 0, total: 0 },
        AnalysisState::Paused { resumes_at: None },
        AnalysisState::Failed {
            flow: None,
            message: String::new(),
        },
        AnalysisState::Finished { total: 0 },
        AnalysisState::Cancelled,
    ];
    let languages = [LlmLanguage::En, LlmLanguage::Pl].map(wire_name);
    let cases = [
        (
            "jobs",
            in_list(JobState::ALL.map(|state| state.as_str().to_owned())),
        ),
        (
            "jobs",
            in_list(JobKind::ALL.map(|kind| kind.as_str().to_owned())),
        ),
        ("analyses", in_list(analysis_states.map(wire_name))),
        ("analyses", in_list(languages.clone())),
        ("users", in_list(languages)),
        (
            "questions",
            in_list(
                [
                    AskState::Pending,
                    AskState::Answering,
                    AskState::Answered,
                    AskState::Failed,
                ]
                .map(wire_name),
            ),
        ),
        (
            "feedback",
            in_list(
                [
                    FeedbackValue::Correct,
                    FeedbackValue::Wrong,
                    FeedbackValue::Irrelevant,
                    FeedbackValue::Overrated,
                    FeedbackValue::Underrated,
                    FeedbackValue::MissedByLlm,
                ]
                .map(wire_name),
            ),
        ),
        (
            "llm_credentials",
            in_list([wire_name(CredentialKind::CodexChatgpt)]),
        ),
    ];
    for (table, list) in cases {
        let sql = table_sql(&db, table).await;
        assert!(sql.contains(&list), "{table}: expected {list}");
    }
}

#[tokio::test]
async fn one_running_job_per_analysis() {
    let db = db().await;
    seed(&db).await;
    insert_job(&db, "j1", "a1", "c1", "running")
        .await
        .expect("first running job");
    insert_job(&db, "j2", "a1", "c2", "queued")
        .await
        .expect("queued jobs may wait");
    let second = insert_job(&db, "j3", "a1", "c3", "claimed").await;
    assert!(matches!(second, Err(DbError::Conflict)), "{second:?}");
}

#[tokio::test]
async fn one_running_job_per_credential() {
    let db = db().await;
    seed(&db).await;
    insert_job(&db, "j1", "a1", "c1", "claimed")
        .await
        .expect("first claimed job");
    insert_job(&db, "j2", "a2", "c1", "succeeded")
        .await
        .expect("finished jobs do not count");
    let second = insert_job(&db, "j3", "a2", "c1", "running").await;
    assert!(matches!(second, Err(DbError::Conflict)), "{second:?}");
}

#[tokio::test]
async fn one_live_analysis_per_head_and_language() {
    let db = db().await;
    seed(&db).await;
    let insert = |id: &'static str, state: &'static str| {
        sqlx::query(
            "INSERT INTO analyses (id, tenant_id, pull_request_id, base_sha, head_sha, language, state, created_by, created_at)
             VALUES (?, 'default', 1, 'b', 'h1', 'en', ?, 'u1', 't')",
        )
        .bind(id)
        .bind(state)
        .execute(&db.pool)
    };
    let duplicate = insert("a3", "planning").await.map_err(DbError::from);
    assert!(matches!(duplicate, Err(DbError::Conflict)), "{duplicate:?}");
    sqlx::query("UPDATE analyses SET state = 'cancelled' WHERE id = 'a1'")
        .execute(&db.pool)
        .await
        .expect("cancel");
    insert("a3", "planning")
        .await
        .expect("a cancelled analysis may be analysed again");
}

#[tokio::test]
async fn ready_output_is_json_with_its_attempt() {
    let db = db().await;
    seed(&db).await;
    insert_job(&db, "j1", "a1", "c1", "running")
        .await
        .expect("job");
    let set = |output: &'static str, attempt: Option<i64>| {
        sqlx::query("UPDATE jobs SET ready_output = ?, ready_output_attempt = ? WHERE id = 'j1'")
            .bind(output)
            .bind(attempt)
            .execute(&db.pool)
    };
    assert!(set("not json", Some(1)).await.is_err(), "invalid JSON");
    assert!(set("{}", None).await.is_err(), "output without attempt");
    set("{\"answer\": \"x\"}", Some(1))
        .await
        .expect("output with attempt");
}

#[tokio::test]
async fn feedback_rows_start_at_revision_one_and_never_below() {
    let db = db().await;
    seed(&db).await;
    let insert = |id: &'static str, key: &'static str, revision: Option<i64>| {
        let sql = match revision {
            Some(_) => "INSERT INTO feedback (id, tenant_id, analysis_id, user_id, target_kind, target_key, value,
                        created_at, updated_at, revision) VALUES (?, 'default', 'a1', 'u1', 'step', ?, 'wrong', 't', 't', ?)",
            None => "INSERT INTO feedback (id, tenant_id, analysis_id, user_id, target_kind, target_key, value,
                     created_at, updated_at) VALUES (?, 'default', 'a1', 'u1', 'step', ?, 'wrong', 't', 't')",
        };
        sqlx::query(sql)
            .bind(id)
            .bind(key)
            .bind(revision)
            .execute(&db.pool)
    };
    insert("fb1", "f1.1", None).await.expect("default revision");
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM feedback WHERE id = 'fb1'")
        .fetch_one(&db.pool)
        .await
        .expect("revision");
    assert_eq!(revision, 1);
    assert!(
        insert("fb2", "f1.2", Some(0)).await.is_err(),
        "revision 0 rejected"
    );
}

#[tokio::test]
async fn foreign_keys_are_enforced() {
    let db = db().await;
    let orphan = insert_job(&db, "j1", "missing", "c1", "queued").await;
    assert!(matches!(orphan, Err(DbError::Storage(_))), "{orphan:?}");
}

#[tokio::test]
async fn storage_errors_hide_sqlx_details_in_display() {
    let db = db().await;
    let error = sqlx::query("SELECT * FROM no_such_table")
        .execute(&db.pool)
        .await
        .map_err(DbError::from)
        .expect_err("missing table");
    assert_eq!(error.to_string(), "the database failed");
    let source = std::error::Error::source(&error).expect("storage error");
    assert_eq!(source.to_string(), "SQLite query failed");
    assert!(
        std::error::Error::source(source).is_some(),
        "the sqlx error is kept as the source"
    );
}
