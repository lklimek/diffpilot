//! Schema tests: migration, seeds, tenant columns, CHECK lists against the contract enums, queue invariants.

use contracts::agent::JobKind;
use contracts::api::analyses::AnalysisState;
use contracts::api::dashboard::{
    RankingState, RefreshFailureCode, RefreshState, RefreshTrigger, ScheduledOutcome,
};
use contracts::api::feedback::FeedbackValue;
use contracts::api::questions::AskState;
use contracts::common::LlmLanguage;
use contracts::job::result::JobFailureKind;
use contracts::job::spec::{CredentialKind, ReasoningEffort};
use contracts::job::state::JobState;
use serde::Serialize;

use crate::repo::settings::CallFailure;

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
         INSERT INTO analyses (id, tenant_id, pull_request_id, base_sha, head_sha, language, state, created_by, created_at,
             background_analysis)
           VALUES ('a1', 'default', 1, 'b', 'h1', 'en', 'running', 'u1', 't', 0),
                  ('a2', 'default', 1, 'b', 'h2', 'en', 'running', 'u1', 't', 0);",
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
        "INSERT INTO jobs (id, tenant_id, analysis_id, kind, priority, state, spec, credential_id, created_at, stage)
         VALUES (?, 'default', ?, 'analyse_flow', 1, ?, '{}', ?, 't', ?)",
    )
    .bind(id)
    .bind(analysis)
    .bind(state)
    .bind(credential)
    .bind(format!("flow:{id}"))
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
    assert_eq!(applied, i64::try_from(MIGRATIONS.len()).expect("count"));
}

/// A binary that knows fewer migrations than the database has applied does not open it.
#[tokio::test]
async fn older_binary_refuses_a_newer_database() {
    let options = sqlx::sqlite::SqliteConnectOptions::new().in_memory(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .min_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(options)
        .await
        .expect("pool");
    migrate(&pool, MIGRATIONS.len()).await.expect("current");
    let older = migrate(&pool, MIGRATIONS.len() - 1).await;
    assert!(matches!(older, Err(DbError::Storage(_))), "{older:?}");
}

/// Migration 0002 upgrades a database made by 0001 in place: analyses, jobs, flow work and questions keep
/// their rows, and the new counter starts at 0 with its constraint.
#[tokio::test]
async fn flow_auto_retries_migration_keeps_the_rows_of_an_existing_database() {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .in_memory(true)
        .foreign_keys(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .min_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(options)
        .await
        .expect("pool");
    migrate(&pool, 1).await.expect("0001");
    let db = Db { pool };
    seed(&db).await;
    insert_job(&db, "j1", "a1", "c1", "succeeded")
        .await
        .expect("job");
    sqlx::raw_sql(
        "INSERT INTO flow_work (tenant_id, analysis_id, flow_id, requested_at, error)
           VALUES ('default', 'a1', 'f1', '2026-11-17T10:00:00.000000Z', NULL),
                  ('default', 'a1', 'f2', NULL, 'The flow analysis failed. Retry this flow.');
         INSERT INTO questions (id, tenant_id, analysis_id, step_id, user_id, text, state, created_at)
           VALUES ('q1', 'default', 'a1', 'f1.1', 'u1', 'why?', 'pending', 't');",
    )
    .execute(&db.pool)
    .await
    .expect("rows of the old schema");
    let counts =
        "SELECT (SELECT COUNT(*) FROM analyses) || '/' || (SELECT COUNT(*) FROM jobs) || '/' ||
        (SELECT COUNT(*) FROM flow_work) || '/' || (SELECT COUNT(*) FROM questions)";
    let before: String = sqlx::query_scalar(counts)
        .fetch_one(&db.pool)
        .await
        .expect("counts");
    assert_eq!(before, "2/1/2/1");

    migrate(&db.pool, MIGRATIONS.len())
        .await
        .expect("upgrade in place");

    let after: String = sqlx::query_scalar(counts)
        .fetch_one(&db.pool)
        .await
        .expect("counts");
    assert_eq!(after, before, "no row is lost");
    let flows: Vec<(String, Option<String>, Option<String>, i64)> = sqlx::query_as(
        "SELECT flow_id, requested_at, error, auto_retries FROM flow_work ORDER BY flow_id",
    )
    .fetch_all(&db.pool)
    .await
    .expect("flow work");
    assert_eq!(
        flows,
        [
            (
                "f1".to_owned(),
                Some("2026-11-17T10:00:00.000000Z".to_owned()),
                None,
                0
            ),
            (
                "f2".to_owned(),
                None,
                Some("The flow analysis failed. Retry this flow.".to_owned()),
                0
            )
        ]
    );
    let negative = sqlx::query("UPDATE flow_work SET auto_retries = -1 WHERE flow_id = 'f1'")
        .execute(&db.pool)
        .await;
    assert!(negative.is_err(), "the counter cannot go below zero");
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
            .fetch_all(&db.pool)
            .await
            .expect("versions");
    assert_eq!(
        versions,
        (1..=i64::try_from(MIGRATIONS.len()).expect("count")).collect::<Vec<_>>()
    );
    // Running the migrator again changes nothing (the checksum of 0001 still matches).
    migrate(&db.pool, MIGRATIONS.len())
        .await
        .expect("idempotent");
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
           AND name NOT IN ('_sqlx_migrations', 'tenants', 'users', 'llm_selection') ORDER BY name",
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
        AnalysisState::Planning { stage: None },
        AnalysisState::Running {
            done: 0,
            total: 0,
            idle: false,
            stage: None,
        },
        AnalysisState::Paused {
            reason: Some(contracts::api::analyses::PauseReason::UsageLimit),
            paused_by: None,
            paused_at: None,
            resumes_at: None,
            done: 0,
            total: None,
            stage: None,
        },
        AnalysisState::Failed {
            reason: None,
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
        (
            "llm_credentials",
            in_list(
                std::iter::once(CallFailure::UsageLimited)
                    .chain(
                        [
                            JobFailureKind::ProviderFailed,
                            JobFailureKind::OutputNotJson,
                            JobFailureKind::CredentialUnusable,
                            JobFailureKind::CheckoutFailed,
                            JobFailureKind::Timeout,
                            JobFailureKind::Internal,
                            JobFailureKind::Configuration,
                        ]
                        .map(CallFailure::Failed),
                    )
                    .map(|failure| {
                        if let CallFailure::Failed(kind) = failure {
                            assert_eq!(failure.wire(), wire_name(kind));
                        }
                        failure.wire().to_owned()
                    }),
            ),
        ),
    ];
    let dashboard_cases = [
        (
            "dashboard_refreshes",
            in_list([RefreshTrigger::Manual, RefreshTrigger::Scheduled].map(wire_name)),
        ),
        (
            "dashboard_refreshes",
            in_list(
                [
                    RefreshState::Scanning,
                    RefreshState::Ranking,
                    RefreshState::Finished,
                    RefreshState::Failed,
                    RefreshState::Cancelled,
                ]
                .map(wire_name),
            ),
        ),
        (
            "dashboard_refreshes",
            in_list(
                [
                    RankingState::WaitingForAnalysis,
                    RankingState::Running,
                    RankingState::Paused,
                    RankingState::Failed,
                    RankingState::Interrupted,
                    RankingState::Finished,
                ]
                .map(wire_name),
            ),
        ),
        (
            "dashboard_refreshes",
            in_list(
                [
                    RefreshFailureCode::GithubRateLimited,
                    RefreshFailureCode::GithubUnavailable,
                    RefreshFailureCode::SignInRequired,
                    RefreshFailureCode::TimedOut,
                    RefreshFailureCode::Interrupted,
                    RefreshFailureCode::Internal,
                ]
                .map(wire_name),
            ),
        ),
        (
            "dashboard_settings",
            in_list(
                [
                    ScheduledOutcome::Started,
                    ScheduledOutcome::Skipped,
                    ScheduledOutcome::NeedsSignIn,
                    ScheduledOutcome::Failed,
                ]
                .map(wire_name),
            ),
        ),
        (
            "llm_selection",
            format!(
                "judge_effort IN ({})",
                [
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High
                ]
                .map(|effort| format!("'{}'", wire_name(effort)))
                .join(", ")
            ),
        ),
    ];
    for (table, list) in cases.into_iter().chain(dashboard_cases) {
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
            "INSERT INTO analyses (id, tenant_id, pull_request_id, base_sha, head_sha, language, state, created_by, created_at,
                 background_analysis)
             VALUES (?, 'default', 1, 'b', 'h1', 'en', ?, 'u1', 't', 0)",
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
async fn one_live_job_per_pipeline_stage() {
    let db = db().await;
    seed(&db).await;
    let insert = |id: &'static str,
                  kind: &'static str,
                  stage: &'static str,
                  state: &'static str| {
        sqlx::query(
            "INSERT INTO jobs (id, tenant_id, analysis_id, kind, priority, state, spec, credential_id, created_at, stage)
             VALUES (?, 'default', 'a1', ?, 1, ?, '{}', 'c1', 't', ?)",
        )
        .bind(id)
        .bind(kind)
        .bind(state)
        .bind(stage)
        .execute(&db.pool)
    };
    insert("j1", "analyse_flow", "flow:f1", "failed")
        .await
        .expect("failed job");
    insert("j2", "analyse_flow", "flow:f1", "queued")
        .await
        .expect("a failed stage may be retried");
    let duplicate = insert("j3", "analyse_flow", "flow:f1", "queued")
        .await
        .map_err(DbError::from);
    assert!(matches!(duplicate, Err(DbError::Conflict)), "{duplicate:?}");
    insert("j4", "analyse_flow", "flow:f2", "queued")
        .await
        .expect("another flow");
    insert("j5", "plan", "plan:1", "succeeded")
        .await
        .expect("plan");
    let replan = insert("j6", "plan", "plan:1", "queued")
        .await
        .map_err(DbError::from);
    assert!(
        matches!(replan, Err(DbError::Conflict)),
        "a succeeded stage is not submitted again: {replan:?}"
    );
    insert("j7", "plan", "plan:2", "queued")
        .await
        .expect("another plan batch");
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

#[tokio::test]
async fn repair_runs_are_new_generations_of_a_stage() {
    let db = db().await;
    seed(&db).await;
    let insert = |id: &'static str, generation: i64, state: &'static str| {
        sqlx::query(
            "INSERT INTO jobs (id, tenant_id, analysis_id, kind, priority, state, spec, credential_id, created_at,
                               stage, generation)
             VALUES (?, 'default', 'a1', 'analyse_flow', 1, ?, '{}', 'c1', 't', 'flow:f1', ?)",
        )
        .bind(id)
        .bind(state)
        .bind(generation)
        .execute(&db.pool)
    };
    // The first run succeeded but verification rejected its output: a repair is generation 1.
    insert("j0", 0, "succeeded").await.expect("first run");
    insert("j1", 1, "queued")
        .await
        .expect("repair run 1 of a rejected output");
    // A restart submits the same repair again: deduplicated.
    let again = insert("j1b", 1, "queued").await.map_err(DbError::from);
    assert!(matches!(again, Err(DbError::Conflict)), "{again:?}");
    insert("j2", 2, "queued").await.expect("repair run 2");
    assert!(
        insert("j3", 3, "queued").await.is_err(),
        "at most MAX_REPAIR_RUNS repairs"
    );
    assert_eq!(jobs::MAX_REPAIR_RUNS, 2);
}

/// Installation tokens are read without being removed and cleared only after a confirmed revocation, so a
/// crash between reading and revoking leaves them listed for recovery.
#[tokio::test]
async fn installation_tokens_survive_until_revocation_is_confirmed() {
    use chrono::{Duration, TimeZone, Utc};
    use contracts::ids::JobId;

    let db = db().await;
    seed(&db).await;
    let job = JobId::new_random();
    let job_key = job.to_string();
    insert_job(&db, &job_key, "a1", "c1", "running")
        .await
        .expect("job");
    sqlx::query("UPDATE jobs SET attempt = 1 WHERE id = ?")
        .bind(&job_key)
        .execute(&db.pool)
        .await
        .expect("attempt");
    let now = Utc
        .with_ymd_and_hms(2026, 11, 16, 18, 0, 0)
        .single()
        .expect("time");
    let jobs = db.jobs();
    jobs.record_github_token(job, 1, b"ciphertext-1", now + Duration::hours(1))
        .await
        .expect("record");
    assert_eq!(
        jobs.github_token(job, 1).await.expect("read"),
        Some(b"ciphertext-1".to_vec())
    );
    assert_eq!(
        jobs.github_token(job, 1).await.expect("read again"),
        Some(b"ciphertext-1".to_vec()),
        "reading does not remove"
    );
    assert!(
        jobs.github_tokens_to_revoke()
            .await
            .expect("list")
            .is_empty(),
        "the token of the running attempt is in use"
    );
    // The attempt ends (lease expired, re-queued) and is claimed again with a new token before the old one
    // was revoked (crash): both stay, only the old one is due.
    sqlx::query("UPDATE jobs SET state = 'running', attempt = 2 WHERE id = ?")
        .bind(&job_key)
        .execute(&db.pool)
        .await
        .expect("re-claim");
    jobs.record_github_token(job, 2, b"ciphertext-2", now + Duration::hours(2))
        .await
        .expect("record second");
    let due = jobs.github_tokens_to_revoke().await.expect("list");
    assert_eq!(due.len(), 1, "{due:?}");
    assert_eq!((due[0].job, due[0].attempt), (job, 1));
    assert_eq!(due[0].ciphertext, b"ciphertext-1".to_vec());
    assert_eq!(due[0].expires_at, now + Duration::hours(1));
    assert!(
        jobs.confirm_github_token_revoked(job, 1)
            .await
            .expect("confirm")
    );
    assert!(
        !jobs
            .confirm_github_token_revoked(job, 1)
            .await
            .expect("confirm twice"),
        "idempotent"
    );
    sqlx::query("UPDATE jobs SET state = 'succeeded' WHERE id = ?")
        .bind(&job_key)
        .execute(&db.pool)
        .await
        .expect("finish");
    let due = jobs.github_tokens_to_revoke().await.expect("list");
    assert_eq!(due.iter().map(|t| t.attempt).collect::<Vec<_>>(), [2]);
    assert_eq!(
        jobs.github_token(job, 2).await.expect("read"),
        Some(b"ciphertext-2".to_vec())
    );
}

#[tokio::test]
async fn one_running_job_per_pull_request_across_credentials() {
    let db = db().await;
    seed(&db).await;
    insert_job(&db, "j1", "a1", "cred-1", "running")
        .await
        .unwrap();
    assert!(
        insert_job(&db, "j2", "a2", "cred-2", "claimed")
            .await
            .is_err()
    );
    insert_job(&db, "j3", "a2", "cred-2", "queued")
        .await
        .unwrap();
    assert!(
        sqlx::query("UPDATE jobs SET state = 'claimed' WHERE id = 'j3'")
            .execute(&db.pool)
            .await
            .is_err()
    );
}

/// Migrations 0003 and 0004 upgrade a v1 database in place: the stored model choice is kept and gains the
/// default ranking effort `low`; the new tables start empty (plan §D.1, OD8 keeps them append-only).
#[tokio::test]
async fn dashboard_migrations_upgrade_a_v1_database() {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .in_memory(true)
        .foreign_keys(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .min_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(options)
        .await
        .expect("pool");
    migrate(&pool, 2).await.expect("v1 schema");
    sqlx::raw_sql(
        "INSERT INTO llm_selection (singleton, model, reasoning_effort, changed_by, changed_at)
           VALUES (1, 'gpt-6-astra', 'medium', 'lklimek', 't');",
    )
    .execute(&pool)
    .await
    .expect("v1 row");
    migrate(&pool, MIGRATIONS.len())
        .await
        .expect("upgrade in place");
    let row: (String, String, String) =
        sqlx::query_as("SELECT model, reasoning_effort, judge_effort FROM llm_selection")
            .fetch_one(&pool)
            .await
            .expect("selection");
    assert_eq!(
        row,
        (
            "gpt-6-astra".to_owned(),
            "medium".to_owned(),
            "low".to_owned()
        )
    );
    let invalid = sqlx::query("UPDATE llm_selection SET judge_effort = 'ultra'")
        .execute(&pool)
        .await;
    assert!(invalid.is_err(), "judge_effort is low, medium or high");
    let rows: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM dashboard_settings) + (SELECT COUNT(*) FROM dashboard_refreshes)
           + (SELECT COUNT(*) FROM dashboard_judgements) + (SELECT COUNT(*) FROM access_tokens)",
    )
    .fetch_one(&pool)
    .await
    .expect("new tables");
    assert_eq!(rows, 0);
}

/// A defaults row needs only its keys; a refresh is live while scanning or ranking, at most one per user.
#[tokio::test]
async fn one_live_dashboard_refresh_per_user() {
    let db = db().await;
    seed(&db).await;
    sqlx::raw_sql(
        "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES ('u2', 2, 'bob', 't', 't');
         INSERT INTO dashboard_settings (user_id, tenant_id, updated_at) VALUES ('u1', 'default', 't');",
    )
    .execute(&db.pool)
    .await
    .expect("users");
    let defaults: (String, i64, i64, String) = sqlx::query_as(
        "SELECT skip_authors, include_todos, schedule_enabled, schedule_time FROM dashboard_settings",
    )
    .fetch_one(&db.pool)
    .await
    .expect("defaults");
    assert_eq!(
        defaults,
        (
            r#"["dependabot","dependabot[bot]"]"#.to_owned(),
            1,
            1,
            "06:00".to_owned()
        )
    );
    let start = |id: &'static str, user: &'static str, state: &'static str| {
        sqlx::query(
            "INSERT INTO dashboard_refreshes (id, tenant_id, user_id, trigger, state, started_at)
             VALUES (?, 'default', ?, 'manual', ?, 't')",
        )
        .bind(id)
        .bind(user)
        .bind(state)
        .execute(&db.pool)
    };
    start("r1", "u1", "scanning").await.expect("first");
    let second = start("r2", "u1", "ranking").await.map_err(DbError::from);
    assert!(matches!(second, Err(DbError::Conflict)), "{second:?}");
    start("r3", "u2", "scanning").await.expect("another user");
    start("r4", "u1", "cancelled")
        .await
        .expect("ended ones are not live");
    let failed_without_code = start("r5", "u1", "failed").await;
    assert!(
        failed_without_code.is_err(),
        "a failed refresh names its failure"
    );
}

/// Tokens store a 32-byte hash, unique names among a user's tokens that are not revoked, and go with their user.
#[tokio::test]
async fn access_tokens_keep_hashes_and_live_names_unique() {
    let db = db().await;
    sqlx::query(
        "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES ('u1', 1, 'alice', 't', 't')",
    )
    .execute(&db.pool)
    .await
    .expect("user");
    let insert = |id: &'static str, name: &'static str, hash: Vec<u8>| {
        sqlx::query(
            "INSERT INTO access_tokens (id, tenant_id, user_id, name, token_hash, scopes, created_at, expires_at)
             VALUES (?, 'default', 'u1', ?, ?, '[\"read\"]', 't', 't')",
        )
        .bind(id)
        .bind(name)
        .bind(hash)
        .execute(&db.pool)
    };
    insert("Ab3dE5gH", "agent", vec![1; 32])
        .await
        .expect("token");
    let short = insert("Zq9xW2vB", "other", vec![2; 16]).await;
    assert!(short.is_err(), "the hash is SHA-256");
    let same_name = insert("Zq9xW2vB", "agent", vec![2; 32])
        .await
        .map_err(DbError::from);
    assert!(matches!(same_name, Err(DbError::Conflict)), "{same_name:?}");
    sqlx::query("UPDATE access_tokens SET revoked_at = 't' WHERE id = 'Ab3dE5gH'")
        .execute(&db.pool)
        .await
        .expect("revoke");
    insert("Zq9xW2vB", "agent", vec![2; 32])
        .await
        .expect("a revoked token's name is free");
    sqlx::query("DELETE FROM users WHERE id = 'u1'")
        .execute(&db.pool)
        .await
        .expect("user removed");
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM access_tokens")
        .fetch_one(&db.pool)
        .await
        .expect("count");
    assert_eq!(left, 0);
}
