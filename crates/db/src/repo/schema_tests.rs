//! Schema tests: migration, seeds, tenant columns, CHECK lists against the contract enums, queue invariants.

use contracts::agent::JobKind;
use contracts::api::analyses::AnalysisState;
use contracts::api::dashboard::{
    RankingState, RefreshFailureCode, RefreshState, RefreshTrigger, ScheduledOutcome,
};
use contracts::api::feedback::FeedbackValue;
use contracts::api::questions::AskState;
use contracts::common::{LineSide, LlmLanguage};
use contracts::job::result::JobFailureKind;
use contracts::job::spec::ReasoningEffort;
use contracts::job::state::JobState;
use contracts::time::Timestamp;
use jiff::SignedDuration;
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
    let current: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pragma_table_info('jobs') WHERE name='payer_id'")
            .fetch_one(&db.pool)
            .await?;
    if current != 0 {
        sqlx::query("INSERT OR IGNORE INTO users(id,github_id,login,created_at,updated_at) SELECT ?,COALESCE(MAX(github_id),0)+1,?,'t','t' FROM users")
            .bind(credential).bind(credential).execute(&db.pool).await?;
    }
    let sql = if current == 0 {
        "INSERT INTO jobs(id,tenant_id,analysis_id,kind,priority,state,spec,credential_id,created_at,stage) VALUES(?,'default',?,'analyse_flow',1,?,'{}',?,'t',?)"
    } else {
        "INSERT INTO jobs(id,tenant_id,analysis_id,kind,priority,state,spec,payer_id,pull_request_id,created_at,stage) VALUES(?,'default',?,'analyse_flow',1,?,'{}',?,1,'t',?)"
    };
    sqlx::query(sql)
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
    let path = dir.path().join("diffpilot.sqlite");
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

/// Migration 0024 came after 0025 and 0026: a database that already applied those gets it in place on the next start.
#[tokio::test]
async fn gone_items_migration_applies_after_later_migrations() {
    let options = sqlx::sqlite::SqliteConnectOptions::new().in_memory(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .min_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(options)
        .await
        .expect("pool");
    let without_0024 = MIGRATIONS
        .iter()
        .filter(|(version, _, _)| *version != 24 && *version <= 29)
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
    Migrator::with_migrations(without_0024)
        .run(&pool)
        .await
        .expect("through 0026 without 0024");
    migrate(&pool, migration_count_through(29))
        .await
        .expect("0024 later");
    let column: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('comment_items') WHERE name = 'gone_at'",
    )
    .fetch_one(&pool)
    .await
    .expect("columns");
    assert_eq!(column, 1);
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

    migrate(&db.pool, migration_count_through(29))
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
        MIGRATIONS
            .iter()
            .take(migration_count_through(29))
            .map(|(version, ..)| *version)
            .collect::<Vec<_>>()
    );
    // Running the migrator again changes nothing (the checksum of 0001 still matches).
    migrate(&db.pool, migration_count_through(29))
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

async fn allowlist_rows(pool: &sqlx::SqlitePool) -> Vec<(String, Option<i64>)> {
    sqlx::query_as(
        "SELECT login, github_id FROM tenant_allowed_logins WHERE tenant_id = 'default' ORDER BY login",
    )
    .fetch_all(pool)
    .await
    .expect("allowlist")
}

/// ADR-007 addendum: a fresh database allows nobody until `SR_ALLOWED_LOGINS` is applied.
#[tokio::test]
async fn fresh_allowlist_is_empty() {
    let db = db().await;
    assert_eq!(allowlist_rows(&db.pool).await, []);
}

/// Migration 0016 drops the logins 0001 seeded only while nobody signed in through them; a bound entry and
/// logins the deployment added stay.
#[tokio::test]
async fn unbound_seed_logins_migration_keeps_bound_and_added_entries() {
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
    let before = MIGRATIONS
        .iter()
        .position(|(version, ..)| *version == 16)
        .expect("migration 0016");
    migrate(&pool, before).await.expect("up to 0016");
    assert_eq!(
        allowlist_rows(&pool).await,
        [
            ("Claudius-Maginificent".to_owned(), None),
            ("lklimek".to_owned(), None)
        ],
        "0001 seeds both"
    );
    sqlx::raw_sql(
        "UPDATE tenant_allowed_logins SET github_id = 7, bound_at = 't' WHERE login = 'LKLIMEK';
         INSERT INTO tenant_allowed_logins (tenant_id, login) VALUES ('default', 'alice');",
    )
    .execute(&pool)
    .await
    .expect("existing install");
    migrate(&pool, migration_count_through(29))
        .await
        .expect("0016");
    assert_eq!(
        allowlist_rows(&pool).await,
        [("alice".to_owned(), None), ("lklimek".to_owned(), Some(7))]
    );
}

#[tokio::test]
async fn allowlist_binds_each_github_id_once_per_tenant() {
    let db = db().await;
    sqlx::query(
        "INSERT INTO tenant_allowed_logins (tenant_id, login) VALUES ('default', 'lklimek'), ('default', 'Claudius-Maginificent')",
    )
    .execute(&db.pool)
    .await
    .expect("allow");
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
    // User-wide settings, installation-wide infrastructure and decision-owned origins have their own scope.
    let db = db().await;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
           AND name NOT IN ('_sqlx_migrations', 'tenants', 'users', 'llm_selection',
               'assessment_config', 'assessment_daily_usage',
               'github_deliveries', 'app_installations_allowed', 'app_repositories', 'comment_decision_origins') ORDER BY name",
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
    let languages = [LlmLanguage::En, LlmLanguage::Pl, LlmLanguage::Ru].map(wire_name);
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
            "questions",
            in_list([LineSide::Head, LineSide::Base].map(wire_name)),
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
            in_list([
                "linking".to_owned(),
                "linked".to_owned(),
                "relink_needed".to_owned(),
            ]),
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
                            JobFailureKind::WorkerUnavailable,
                            JobFailureKind::WorkerLost,
                            JobFailureKind::CredentialMissing,
                            JobFailureKind::CredentialRelink,
                            JobFailureKind::ModelUnavailable,
                            JobFailureKind::UpstreamRejected,
                            JobFailureKind::ReportVersionUnsupported,
                            JobFailureKind::InvalidJsonSchema,
                            JobFailureKind::AskCompactionUnconfirmed,
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
            in_list(
                [
                    RefreshTrigger::Manual,
                    RefreshTrigger::Scheduled,
                    RefreshTrigger::Automatic,
                ]
                .map(wire_name),
            ),
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
                    RankingState::Cancelled,
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
        (
            "llm_selection",
            format!(
                "ask_effort IN ({})",
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
async fn one_running_job_per_analysis_and_payer() {
    let db = db().await;
    seed(&db).await;
    insert_job(&db, "j1", "a1", "c1", "running")
        .await
        .expect("first running job");
    insert_job(&db, "j2", "a1", "c2", "queued")
        .await
        .expect("queued jobs may wait");
    let second = insert_job(&db, "j3", "a1", "c1", "claimed").await;
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
            "INSERT INTO jobs (id, tenant_id, analysis_id, kind, priority, state, spec, payer_id, pull_request_id, created_at, stage)
             VALUES (?, 'default', 'a1', ?, 1, ?, '{}', 'u1', 1, 't', ?)",
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
            "INSERT INTO jobs (id, tenant_id, analysis_id, kind, priority, state, spec, payer_id, pull_request_id, created_at,
                               stage, generation)
             VALUES (?, 'default', 'a1', 'analyse_flow', 1, ?, '{}', 'u1', 1, 't', 'flow:f1', ?)",
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
    let now = "2026-11-16T18:00:00Z".parse::<Timestamp>().expect("time");
    let jobs = db.jobs();
    jobs.record_github_token(job, 1, b"ciphertext-1", now + SignedDuration::from_hours(1))
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
    jobs.record_github_token(job, 2, b"ciphertext-2", now + SignedDuration::from_hours(2))
        .await
        .expect("record second");
    let due = jobs.github_tokens_to_revoke().await.expect("list");
    assert_eq!(due.len(), 1, "{due:?}");
    assert_eq!((due[0].job, due[0].attempt), (job, 1));
    assert_eq!(due[0].ciphertext, b"ciphertext-1".to_vec());
    assert_eq!(due[0].expires_at, now + SignedDuration::from_hours(1));
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
async fn distinct_payers_can_run_on_one_pull_request() {
    let db = db().await;
    seed(&db).await;
    insert_job(&db, "j1", "a1", "cred-1", "running")
        .await
        .unwrap();
    insert_job(&db, "j2", "a2", "cred-2", "claimed")
        .await
        .unwrap();
    insert_job(&db, "j3", "a2", "cred-2", "queued")
        .await
        .unwrap();
    assert!(
        sqlx::query("UPDATE jobs SET state='claimed' WHERE id='j3'")
            .execute(&db.pool)
            .await
            .is_err()
    );
}

/// Migration 0006 upgrades a database in place: the stored selection keeps its model, efforts and change pair and
/// gains the default Ask effort `medium`.
#[tokio::test]
async fn ask_effort_migration_upgrades_a_stored_selection() {
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
    migrate(&pool, 5).await.expect("schema before 0006");
    sqlx::raw_sql(
        "INSERT INTO llm_selection (singleton, model, reasoning_effort, judge_effort, changed_by, changed_at)
           VALUES (1, 'gpt-6-astra', 'high', 'medium', 'lklimek', 't');",
    )
    .execute(&pool)
    .await
    .expect("stored row");
    migrate(&pool, migration_count_through(29))
        .await
        .expect("upgrade in place");
    let row: (String, String, String, String, String) = sqlx::query_as(
        "SELECT model, reasoning_effort, judge_effort, ask_effort, changed_by FROM llm_selection",
    )
    .fetch_one(&pool)
    .await
    .expect("selection");
    assert_eq!(
        row,
        (
            "gpt-6-astra".to_owned(),
            "high".to_owned(),
            "medium".to_owned(),
            "medium".to_owned(),
            "lklimek".to_owned()
        )
    );
    let invalid = sqlx::query("UPDATE llm_selection SET ask_effort = 'ultra'")
        .execute(&pool)
        .await;
    assert!(invalid.is_err(), "ask_effort is low, medium or high");
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
    migrate(&pool, migration_count_through(29))
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
        let ranked = matches!(state, "ranking" | "finished");
        sqlx::query(
            "INSERT INTO dashboard_refreshes (id, tenant_id, user_id, trigger, state, started_at, snapshot, ranking_state)
             VALUES (?, 'default', ?, 'manual', ?, 't', ?, ?)",
        )
        .bind(id)
        .bind(user)
        .bind(state)
        .bind(ranked.then_some("{}"))
        .bind(match state {
            "ranking" => Some("running"),
            "finished" => Some("finished"),
            _ => None,
        })
        .execute(&db.pool)
    };
    start("r1", "u1", "scanning").await.expect("first");
    let second = start("r2", "u1", "ranking").await.map_err(DbError::from);
    assert!(matches!(second, Err(DbError::Conflict)), "{second:?}");
    start("r3", "u2", "ranking").await.expect("another user");
    start("r4", "u1", "cancelled")
        .await
        .expect("ended ones are not live");
    start("r6", "u1", "finished")
        .await
        .expect("ended ones are not live");
    let failed_without_code = start("r5", "u1", "failed").await;
    assert!(
        failed_without_code.is_err(),
        "a failed refresh names its failure"
    );
}

/// Refresh states of http-api.md "Refresh states": only `ranking` and `finished` hold a snapshot and a ranking
/// state; `ranking` has a live ranking state, `finished` an ending one (including `cancelled`).
#[tokio::test]
async fn refresh_rows_follow_the_state_table() {
    let db = db().await;
    sqlx::query(
        "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES ('u1', 1, 'alice', 't', 't')",
    )
    .execute(&db.pool)
    .await
    .expect("user");
    let insert = |id: &'static str,
                  state: &'static str,
                  snapshot: Option<&'static str>,
                  ranking: Option<&'static str>| {
        sqlx::query(
            "INSERT INTO dashboard_refreshes (id, tenant_id, user_id, trigger, state, started_at, snapshot, ranking_state,
                 failure_code, failure_message)
             VALUES (?, 'default', 'u1', 'scheduled', ?, 't', ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(state)
        .bind(snapshot)
        .bind(ranking)
        .bind((state == "failed").then_some("interrupted"))
        .bind((state == "failed").then_some("A server restart interrupted the scan."))
        .execute(&db.pool)
    };
    let allowed = [
        ("a1", "finished", Some("{}"), Some("finished")),
        ("a2", "finished", Some("{}"), Some("cancelled")),
        ("a3", "finished", Some("{}"), Some("interrupted")),
        ("a4", "failed", None, None),
        ("a5", "cancelled", None, None),
    ];
    for (id, state, snapshot, ranking) in allowed {
        insert(id, state, snapshot, ranking)
            .await
            .unwrap_or_else(|e| panic!("{id}: {e}"));
    }
    let rejected = [
        ("b1", "cancelled", Some("{}"), None),
        ("b2", "failed", Some("{}"), None),
        ("b3", "finished", None, Some("finished")),
        ("b4", "finished", Some("{}"), None),
        ("b5", "finished", Some("{}"), Some("running")),
        ("b6", "scanning", None, Some("running")),
        ("b7", "scanning", Some("{}"), None),
        ("b8", "ranking", Some("{}"), Some("cancelled")),
    ];
    for (id, state, snapshot, ranking) in rejected {
        assert!(
            insert(id, state, snapshot, ranking).await.is_err(),
            "{id} {state} accepted"
        );
    }
    // Checked last: a live row would also reject the scanning cases through the one-live index.
    insert("a6", "ranking", Some("{}"), Some("waiting_for_analysis"))
        .await
        .expect("a ranking refresh");
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

/// Migration 0007 rebuilds the job tables in place on a populated v1 database: every job, event and pending token
/// revocation is kept, stored specs move into `target`, and the children reference the new `jobs` table.
#[tokio::test]
async fn jobs_targets_migration_applies_to_a_v1_database() {
    use contracts::ids::AnalysisId;
    use contracts::job::spec::{JobTarget, PullRequestTarget};

    use crate::repo::analyses::testing::{job_spec, judge_spec, seed_ranking_refresh};

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
    migrate(&pool, 6).await.expect("schema before 0007");
    let db = Db { pool };
    seed(&db).await;
    let analysis: AnalysisId =
        serde_json::from_value(serde_json::json!("00000000-0000-4000-8000-0000000000a1"))
            .expect("id");
    sqlx::query("UPDATE analyses SET id = ? WHERE id = 'a1'")
        .bind(analysis.to_string())
        .execute(&db.pool)
        .await
        .expect("uuid analysis id");
    let current = job_spec(analysis, JobKind::AnalyseFlow);
    let mut v1 = serde_json::to_value(&current).expect("spec");
    let target = v1
        .as_object_mut()
        .and_then(|spec| spec.remove("target"))
        .expect("target");
    for field in ["analysis_id", "pull_request_id", "repo"] {
        v1[field] = target[field].clone();
    }
    v1["pull_request_id"] = serde_json::json!(1);
    sqlx::query(
        "INSERT INTO jobs (id, tenant_id, analysis_id, kind, priority, state, spec, credential_id, created_at, stage,
             attempt, result, claimed_at, finished_at)
         VALUES (?1, 'default', ?2, 'analyse_flow', 1, 'succeeded', ?3, 'c1', ?5, 'flow:f1', 1, ?4, ?5, ?5)",
    )
    .bind(current.job_id.to_string())
    .bind(analysis.to_string())
    .bind(v1.to_string())
    .bind(
        serde_json::to_string(&contracts::job::result::JobResult {
            usage: Some(contracts::job::result::Usage {
                input_tokens: 7,
                cached_input_tokens: 0,
                output_tokens: 1,
                requests: None,
            }),
            ..crate::repo::jobs::server_result(
                current.job_id,
                1,
                contracts::job::result::JobOutcome::Cancelled,
            )
        })
        .expect("result"),
    )
    .bind("2026-11-16T00:00:00.000000Z")
    .execute(&db.pool)
    .await
    .expect("v1 job");
    for sql in [
        "INSERT INTO job_events (job_id, tenant_id, attempt, seq, at, body) VALUES (?1, 'default', 1, 1, 't', '{}')",
        "INSERT INTO job_events (job_id, tenant_id, attempt, seq, at, body) VALUES (?1, 'default', 1, 2, 't', '{}')",
        "INSERT INTO installation_tokens (job_id, tenant_id, attempt, ciphertext, expires_at)
           VALUES (?1, 'default', 1, x'00', 't')",
    ] {
        sqlx::query(sql)
            .bind(current.job_id.to_string())
            .execute(&db.pool)
            .await
            .expect("child row");
    }

    migrate(&db.pool, migration_count_through(29))
        .await
        .expect("upgrade in place");

    let record = db.jobs().get(current.job_id).await.expect("job reads back");
    assert_eq!(record.analysis_id, Some(analysis));
    assert_eq!(record.refresh_id, None);
    assert_eq!(
        record.spec.target,
        JobTarget::PullRequest(PullRequestTarget {
            analysis_id: analysis,
            pull_request_id: 1,
            repo: current
                .target
                .pull_request()
                .expect("pull request")
                .repo
                .clone(),
        })
    );
    assert_eq!(record.spec.prompt, current.prompt);
    assert_eq!(record.state, JobState::Succeeded);
    let kept: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM job_events), (SELECT COUNT(*) FROM installation_tokens),
             (SELECT json_extract(result, '$.usage.input_tokens') FROM jobs)",
    )
    .fetch_one(&db.pool)
    .await
    .expect("children");
    assert_eq!(kept, (2, 1, 7));
    for child in ["job_events", "installation_tokens"] {
        let parents: Vec<String> =
            sqlx::query_scalar("SELECT \"table\" FROM pragma_foreign_key_list(?)")
                .bind(child)
                .fetch_all(&db.pool)
                .await
                .expect("foreign keys");
        assert!(parents.contains(&"jobs".to_owned()), "{child}: {parents:?}");
    }
    sqlx::query("DELETE FROM installation_tokens")
        .execute(&db.pool)
        .await
        .expect("revoked");
    sqlx::query("DELETE FROM jobs")
        .execute(&db.pool)
        .await
        .expect("delete job");
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM job_events")
        .fetch_one(&db.pool)
        .await
        .expect("events");
    assert_eq!(events, 0, "events still cascade with their job");

    let refresh = seed_ranking_refresh(&db).await.expect("refresh");
    let judge = judge_spec(refresh);
    let insert = |analysis: Option<String>,
                  refresh: Option<String>,
                  kind: &'static str,
                  stage: &'static str| {
        sqlx::query(
            "INSERT INTO jobs (id, tenant_id, analysis_id, refresh_id, kind, priority, state, spec, credential_id,
                 created_at, stage)
             VALUES (?, 'default', ?, ?, ?, 0, 'queued', '{}', 'c1', 't', ?)",
        )
        .bind(contracts::ids::JobId::new_random().to_string())
        .bind(analysis)
        .bind(refresh)
        .bind(kind)
        .bind(stage)
        .execute(&db.pool)
    };
    let (a, r) = (Some(analysis.to_string()), Some(refresh.to_string()));
    assert!(
        insert(None, r.clone(), "judge_work", "judge:x:1")
            .await
            .is_ok()
    );
    assert!(
        insert(a.clone(), r.clone(), "judge_work", "judge:x:2")
            .await
            .is_err(),
        "both targets"
    );
    assert!(
        insert(None, None, "judge_work", "judge:x:3").await.is_err(),
        "no target"
    );
    assert!(
        insert(a.clone(), None, "judge_work", "judge:x:4")
            .await
            .is_err(),
        "judge job of an analysis"
    );
    assert!(
        insert(None, r.clone(), "plan", "plan:1").await.is_err(),
        "analysis kind on a refresh"
    );
    assert!(
        insert(None, r, "judge_work", "flow:f1").await.is_err(),
        "stage of another kind"
    );
    assert_eq!(judge.kind, JobKind::JudgeWork);
}

/// Migration 0010 widens the language lists of `users` and `analyses` in place: stored rows, their children and
/// every index survive, Russian is accepted and an unknown language is still refused.
#[tokio::test]
async fn russian_migration_keeps_the_data_and_accepts_ru() {
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
    migrate(&pool, 9).await.expect("schema before 0010");
    let db = Db { pool };
    let analysis = crate::repo::analyses::testing::seed_analysis(&db)
        .await
        .expect("analysis");
    migrate(&db.pool, migration_count_through(29))
        .await
        .expect("upgrade in place");
    let kept: (String, String) =
        sqlx::query_as("SELECT a.language, u.login FROM analyses a JOIN users u ON u.id = a.created_by WHERE a.id = ?")
            .bind(analysis.to_string())
            .fetch_one(&db.pool)
            .await
            .expect("kept");
    assert_eq!(kept, ("en".to_owned(), "lklimek".to_owned()));
    sqlx::query("UPDATE users SET language = 'ru' WHERE id = 'u1'")
        .execute(&db.pool)
        .await
        .expect("ru accepted");
    sqlx::query("UPDATE analyses SET language = 'ru' WHERE id = ?")
        .bind(analysis.to_string())
        .execute(&db.pool)
        .await
        .expect("ru accepted");
    assert!(
        sqlx::query("UPDATE users SET language = 'de' WHERE id = 'u1'")
            .execute(&db.pool)
            .await
            .is_err(),
        "de is refused"
    );
    let check: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&db.pool)
        .await
        .expect("check");
    assert_eq!(check, "ok");
    let unique: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_schema WHERE name = 'analyses_one_live_per_head'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("index");
    assert_eq!(unique, 1);
}

/// Migration 0015 widens the document kinds in place: stored documents stay, `dependencies` is accepted and an unknown
/// kind is still refused.
#[tokio::test]
async fn dependencies_migration_keeps_documents_and_accepts_the_new_kind() {
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
    migrate(&pool, 14).await.expect("schema before 0015");
    let db = Db { pool };
    let analysis = crate::repo::analyses::testing::seed_analysis(&db)
        .await
        .expect("analysis");
    db.documents()
        .put(
            analysis,
            crate::repo::documents::DocumentKind::Plan,
            "",
            &serde_json::json!({"flows": []}),
            Timestamp::now(),
        )
        .await
        .expect("plan stored");
    let refused = sqlx::query(
        "INSERT INTO documents (analysis_id, tenant_id, kind, key, body, created_at) VALUES (?, 'default', 'dependencies', '', '{}', 't')",
    )
    .bind(analysis.to_string())
    .execute(&db.pool)
    .await;
    assert!(refused.is_err(), "not a kind before 0015");
    migrate(&db.pool, migration_count_through(29))
        .await
        .expect("upgrade in place");
    db.documents()
        .put(
            analysis,
            crate::repo::documents::DocumentKind::Dependencies,
            "",
            &serde_json::json!({"status": "not_rust"}),
            Timestamp::now(),
        )
        .await
        .expect("dependencies accepted");
    let kinds: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM documents WHERE analysis_id = ? ORDER BY kind")
            .bind(analysis.to_string())
            .fetch_all(&db.pool)
            .await
            .expect("kinds");
    assert_eq!(kinds, ["dependencies", "plan"]);
    let junk = sqlx::query(
        "INSERT INTO documents (analysis_id, tenant_id, kind, key, body, created_at) VALUES (?, 'default', 'junk', '', '{}', 't')",
    )
    .bind(analysis.to_string())
    .execute(&db.pool)
    .await;
    assert!(junk.is_err(), "an unknown kind is refused");
    let check: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&db.pool)
        .await
        .expect("check");
    assert_eq!(check, "ok");
}

/// Migration 0012 rebuilds `questions` with anchors: every stored question stays, as a step anchor.
#[tokio::test]
async fn question_anchors_migration_keeps_every_question_as_a_step_anchor() {
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
    let before_anchors = MIGRATIONS
        .iter()
        .position(|(_, name, _)| *name == "question_anchors")
        .expect("migration 0012");
    migrate(&pool, before_anchors)
        .await
        .expect("schema before 0012");
    let db = Db { pool };
    seed(&db).await;
    sqlx::raw_sql(
        "INSERT INTO questions (id, tenant_id, analysis_id, step_id, user_id, text, state, answer, job_id, created_at,
             answered_at)
           VALUES ('q1', 'default', 'a1', 'f1.1', 'u1', 'why?', 'answered', '{\"answer\":\"a\",\"evidence\":[]}',
                   'j9', 't1', 't2'),
                  ('q2', 'default', 'a2', 'f2.3', 'u1', 'how?', 'pending', NULL, NULL, 't3', NULL);",
    )
    .execute(&db.pool)
    .await
    .expect("questions of the old schema");
    migrate(&db.pool, migration_count_through(29))
        .await
        .expect("upgrade in place");
    type Row = (
        String,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, step_id, path, state, answer, answered_at FROM questions ORDER BY rowid",
    )
    .fetch_all(&db.pool)
    .await
    .expect("questions");
    assert_eq!(
        rows,
        [
            (
                "q1".to_owned(),
                Some("f1.1".to_owned()),
                None,
                "answered".to_owned(),
                Some(r#"{"answer":"a","evidence":[]}"#.to_owned()),
                Some("t2".to_owned())
            ),
            (
                "q2".to_owned(),
                Some("f2.3".to_owned()),
                None,
                "pending".to_owned(),
                None,
                None
            ),
        ]
    );
    let index: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_schema WHERE name = 'questions_by_analysis'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("index");
    assert_eq!(index, 1);
    let check: String = sqlx::query_scalar("PRAGMA foreign_key_check")
        .fetch_optional(&db.pool)
        .await
        .expect("check")
        .unwrap_or_default();
    assert_eq!(check, "");
}

/// Migration 0014 rebuilds `questions` for threads and wider anchors: every stored question stays with the kind of
/// its anchor and no parent; the new checks hold.
#[tokio::test]
async fn question_threads_migration_keeps_every_question_with_its_kind() {
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
    let before_threads = MIGRATIONS
        .iter()
        .position(|(_, name, _)| *name == "question_threads")
        .expect("migration 0014");
    migrate(&pool, before_threads)
        .await
        .expect("schema before 0014");
    let db = Db { pool };
    seed(&db).await;
    sqlx::raw_sql(
        "INSERT INTO questions (id, tenant_id, analysis_id, step_id, path, side, start_line, end_line, user_id, text,
             state, answer, job_id, created_at, answered_at, touched_steps)
           VALUES ('q1', 'default', 'a1', 'f1.1', NULL, NULL, 3, 4, 'u1', 'why?', 'answered',
                   '{\"answer\":\"a\",\"evidence\":[]}', 'j9', 't1', 't2', NULL),
                  ('q2', 'default', 'a2', NULL, 'src/a.rs', 'base', NULL, NULL, 'u1', 'how?', 'pending', NULL, NULL,
                   't3', NULL, '[\"f1.2\"]');",
    )
    .execute(&db.pool)
    .await
    .expect("questions of the old schema");
    // An id that sorts first but was stored last: the fallback history orders by rowid, which must survive.
    sqlx::raw_sql(
        "INSERT INTO questions (id, tenant_id, analysis_id, step_id, user_id, text, state, created_at)
           VALUES ('q0', 'default', 'a1', 'f1.2', 'u1', 'last?', 'pending', 't0');",
    )
    .execute(&db.pool)
    .await
    .expect("a later question");
    let order = || async {
        sqlx::query_as::<_, (i64, String)>("SELECT rowid, id FROM questions ORDER BY rowid")
            .fetch_all(&db.pool)
            .await
            .expect("rowids")
    };
    let schema = || async {
        sqlx::query_as::<_, (String, String, Option<String>)>(
            "SELECT type, name, sql FROM sqlite_schema WHERE tbl_name = 'questions' AND type IN ('index', 'trigger')
             ORDER BY name",
        )
        .fetch_all(&db.pool)
        .await
        .expect("schema")
    };
    let (order_before, schema_before) = (order().await, schema().await);
    // Up to 0014 only: later migrations add their own indexes (0018: comment-item questions).
    migrate(&db.pool, before_threads + 1)
        .await
        .expect("upgrade in place");
    assert_eq!(
        order().await,
        order_before,
        "rowids and their order are kept"
    );
    let schema_after = schema().await;
    for entry in &schema_before {
        assert!(
            schema_after.contains(entry),
            "{entry:?} is kept: {schema_after:?}"
        );
    }
    assert_eq!(
        schema_after.len(),
        schema_before.len() + 1,
        "only questions_by_parent is new: {schema_after:?}"
    );
    type Row = (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, kind, step_id, path, start_line, touched_steps, flow_id, parent_id FROM questions
         WHERE id IN ('q1', 'q2') ORDER BY rowid",
    )
    .fetch_all(&db.pool)
    .await
    .expect("questions");
    assert_eq!(
        rows,
        [
            (
                "q1".to_owned(),
                "step".to_owned(),
                Some("f1.1".to_owned()),
                None,
                Some(3),
                None,
                None,
                None
            ),
            (
                "q2".to_owned(),
                "lines".to_owned(),
                None,
                Some("src/a.rs".to_owned()),
                None,
                Some(r#"["f1.2"]"#.to_owned()),
                None,
                None
            ),
        ]
    );
    // A flow anchor needs its flow and no lines; a pull-request anchor names nothing; a follow-up's parent exists.
    let insert = |values: &'static str| {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "INSERT INTO questions (id, tenant_id, analysis_id, step_id, path, side, start_line, end_line, user_id,
                 text, state, created_at, kind, flow_id, parent_id) VALUES {values}"
        )))
        .execute(&db.pool)
    };
    insert("('q3', 'default', 'a1', NULL, NULL, NULL, NULL, NULL, 'u1', 'f?', 'pending', 't4', 'flow', 'f2', NULL)")
        .await
        .expect("flow anchor");
    insert("('q4', 'default', 'a1', NULL, NULL, NULL, NULL, NULL, 'u1', 'p?', 'pending', 't5', 'pr', NULL, 'q3')")
        .await
        .expect("a pull-request follow-up");
    for bad in [
        "('x1', 'default', 'a1', NULL, NULL, NULL, NULL, NULL, 'u1', 'x', 'pending', 't6', 'flow', NULL, NULL)",
        "('x2', 'default', 'a1', NULL, NULL, NULL, 1, 2, 'u1', 'x', 'pending', 't6', 'pr', NULL, NULL)",
        "('x3', 'default', 'a1', 'f1.1', NULL, NULL, NULL, NULL, 'u1', 'x', 'pending', 't6', 'pr', NULL, NULL)",
        "('x4', 'default', 'a1', NULL, NULL, NULL, NULL, NULL, 'u1', 'x', 'pending', 't6', 'pr', NULL, 'missing')",
        "('x5', 'default', 'a1', NULL, NULL, NULL, NULL, NULL, 'u1', 'x', 'pending', 't6', 'pr', NULL, 'x5')",
    ] {
        assert!(insert(bad).await.is_err(), "{bad}");
    }
    let indexes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_schema WHERE name IN ('questions_by_analysis', 'questions_by_parent')",
    )
    .fetch_one(&db.pool)
    .await
    .expect("indexes");
    assert_eq!(indexes, 2);
    let check: String = sqlx::query_scalar("PRAGMA foreign_key_check")
        .fetch_optional(&db.pool)
        .await
        .expect("check")
        .unwrap_or_default();
    assert_eq!(check, "");
}

/// Another connection that read the old definitions before migration 0010 accepts Russian afterwards.
#[tokio::test]
async fn russian_migration_reaches_other_open_connections() {
    let dir = tempfile::tempdir().expect("dir");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(dir.path().join("db.sqlite"))
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .expect("pool");
    migrate(&pool, 9).await.expect("schema before 0010");
    let mut other = pool.acquire().await.expect("connection");
    sqlx::raw_sql(
        "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES ('u1', 1, 'a', 't', 't')",
    )
    .execute(&mut *other)
    .await
    .expect("user");
    migrate(&pool, migration_count_through(29))
        .await
        .expect("upgrade in place");
    sqlx::query("UPDATE users SET language = 'ru' WHERE id = 'u1'")
        .execute(&mut *other)
        .await
        .expect("ru accepted on the older connection");
}

/// Every in-memory connection is its own empty database, so the test pool must never replace its one connection.
/// A caller cancelled while acquiring it (a `tokio::time::timeout`, an aborted task) must leave the schema in place.
#[tokio::test]
async fn cancelled_acquire_keeps_the_in_memory_database() {
    let db = Db::in_memory().await.expect("db");
    for _ in 0..50 {
        // The previous query's connection is back in the pool (its return runs as a spawned task).
        while db.pool.num_idle() == 0 {
            tokio::task::yield_now().await;
        }
        {
            // Polled once, then dropped: cancelled at its first await, typically while acquiring the connection.
            let users = db.users();
            let mut query = std::pin::pin!(users.get("nobody"));
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            let _ = query.as_mut().poll(&mut cx);
        }
        assert!(
            matches!(db.users().get("nobody").await, Err(DbError::NotFound)),
            "the schema is still there"
        );
    }
}

/// A pool migrated up to (not including) migration 0016: a database of an existing v1.1 install.
async fn pre_0016_pool() -> sqlx::SqlitePool {
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
    let before = MIGRATIONS
        .iter()
        .position(|(version, ..)| *version == 16)
        .expect("migration 0016");
    migrate(&pool, before).await.expect("up to 0016");
    pool
}

/// Signs `user` (GitHub id `github_id`) in through the bound entry `login`: a user row, the binding, one session,
/// one GitHub token pair and one access token, all in tenant `default`.
async fn signed_in_v11_user(pool: &sqlx::SqlitePool, user: &str, login: &str, github_id: i64) {
    let now = sessions::timestamp(Timestamp::now());
    let later = sessions::timestamp(Timestamp::now() + SignedDuration::from_hours(24));
    sqlx::query(
        "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
    )
    .bind(user)
    .bind(github_id)
    .bind(login)
    .bind(&now)
    .execute(pool)
    .await
    .expect("user");
    sqlx::query(
        "INSERT INTO memberships (tenant_id, user_id, created_at) VALUES ('default', ?, ?)",
    )
    .bind(user)
    .bind(&now)
    .execute(pool)
    .await
    .expect("membership");
    sqlx::query(
        "INSERT INTO tenant_allowed_logins (tenant_id, login, github_id, bound_at) VALUES ('default', ?1, ?2, ?3)
         ON CONFLICT (tenant_id, login) DO UPDATE SET github_id = excluded.github_id, bound_at = excluded.bound_at",
    )
    .bind(login)
    .bind(github_id)
    .bind(&now)
    .execute(pool)
    .await
    .expect("binding");
    sqlx::query(
        "INSERT INTO sessions (token_hash, tenant_id, user_id, csrf_token, created_at, last_seen_at, expires_at)
         VALUES (?1, 'default', ?2, 'csrf', ?3, ?3, ?4)",
    )
    .bind(user.as_bytes())
    .bind(user)
    .bind(&now)
    .bind(&later)
    .execute(pool)
    .await
    .expect("session");
    sqlx::query(
        "INSERT INTO user_tokens (user_id, tenant_id, access_ct, access_expires_at, refresh_ct, refresh_expires_at,
                                  updated_at)
         VALUES (?1, 'default', x'01', ?3, x'02', ?3, ?2)",
    )
    .bind(user)
    .bind(&now)
    .bind(&later)
    .execute(pool)
    .await
    .expect("token pair");
    let mut hash = [0u8; 32];
    hash[..user.len()].copy_from_slice(user.as_bytes());
    sqlx::query(
        "INSERT INTO access_tokens (id, tenant_id, user_id, name, token_hash, scopes, created_at, expires_at)
         VALUES (?1, 'default', ?2, 'agent', ?3, '[\"read\"]', ?4, ?5)",
    )
    .bind(format!("{user:0>8}"))
    .bind(user)
    .bind(&hash[..])
    .bind(&now)
    .bind(&later)
    .execute(pool)
    .await
    .expect("access token");
}

async fn live_credentials(pool: &sqlx::SqlitePool, user: &str) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM sessions WHERE user_id = ?1),
                (SELECT COUNT(*) FROM user_tokens WHERE user_id = ?1),
                (SELECT COUNT(*) FROM access_tokens WHERE user_id = ?1 AND revoked_at IS NULL)",
    )
    .bind(user)
    .fetch_one(pool)
    .await
    .expect("credentials")
}

/// ADR-007 addendum, first v1.2 start of an existing install: `lklimek` bound, `Claudius-Maginificent` still
/// unbound, and an `SR_ALLOWED_LOGINS` without `lklimek`. Migration 0016 drops only the unbound seed; the start's
/// reconcile then removes the bound seed with all its credentials and adds the listed logins unbound.
#[tokio::test]
async fn first_v12_start_on_a_partially_bound_seed_ends_the_unlisted_maintainer() {
    let pool = pre_0016_pool().await;
    signed_in_v11_user(&pool, "u-lk", "lklimek", 7).await;
    signed_in_v11_user(&pool, "u-owner", "owner", 9).await;
    assert_eq!(live_credentials(&pool, "u-lk").await, (1, 1, 1));

    migrate(&pool, migration_count_through(29))
        .await
        .expect("0016");
    assert_eq!(
        allowlist_rows(&pool).await,
        [
            ("lklimek".to_owned(), Some(7)),
            ("owner".to_owned(), Some(9))
        ],
        "0016 keeps the bound seed and the deployment's own login"
    );
    let db = Db { pool };
    let change = db
        .tenants()
        .reconcile_logins(
            DEFAULT_TENANT,
            &["OWNER".to_owned(), "Claudius-Maginificent".to_owned()],
            Timestamp::now(),
        )
        .await
        .expect("reconcile");
    assert_eq!(change.removed, ["lklimek"]);
    assert_eq!(change.added, ["Claudius-Maginificent"]);
    assert_eq!(
        allowlist_rows(&db.pool).await,
        [
            ("Claudius-Maginificent".to_owned(), None),
            ("owner".to_owned(), Some(9))
        ],
        "the re-listed owner keeps spelling and binding"
    );
    assert_eq!(
        live_credentials(&db.pool, "u-lk").await,
        (0, 0, 0),
        "lklimek's access ended"
    );
    assert_eq!(
        live_credentials(&db.pool, "u-owner").await,
        (1, 1, 1),
        "the owner keeps hers"
    );
}

/// A fresh install whose `SR_ALLOWED_LOGINS` names a seeded login: 0001 seeds it, 0016 drops it, the start adds it
/// back unbound, so the operator's own list wins and nothing is left bound from the seed.
#[tokio::test]
async fn fresh_install_listing_a_seeded_login_gets_it_unbound() {
    let db = db().await;
    let change = db
        .tenants()
        .reconcile_logins(DEFAULT_TENANT, &["LKLIMEK".to_owned()], Timestamp::now())
        .await
        .expect("reconcile");
    assert_eq!(change.added, ["LKLIMEK"]);
    assert_eq!(change.removed, Vec::<String>::new());
    assert_eq!(
        allowlist_rows(&db.pool).await,
        [("LKLIMEK".to_owned(), None)]
    );
}

/// Migration 0018 rebuilds `questions`, `comment_passes` and `jobs` in place: every row keeps its rowid, a follow-up
/// keeps its parent (no drop cascades into copied rows), questions gain their pull request, children still cascade,
/// and the new owner rules hold (a `verify_comments` job belongs to a pass; a question without an analysis is a
/// comment-item question of a pull request).
#[tokio::test]
async fn verify_comments_jobs_migration_keeps_rows_and_adds_the_pass_owner() {
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
    let before = MIGRATIONS
        .iter()
        .position(|(_, name, _)| *name == "verify_comments_jobs")
        .expect("migration 0018");
    migrate(&pool, before).await.expect("schema before 0018");
    let db = Db { pool };
    seed(&db).await;
    sqlx::raw_sql(
        "INSERT INTO questions (id, tenant_id, analysis_id, step_id, user_id, text, state, created_at, kind)
           VALUES ('q9', 'default', 'a1', 'f1.1', 'u1', 'first?', 'answered', 't1', 'step');
         INSERT INTO questions (id, tenant_id, analysis_id, user_id, text, state, created_at, kind, parent_id)
           VALUES ('q1', 'default', 'a1', 'u1', 'and?', 'pending', 't2', 'pr', 'q9');
         INSERT INTO comment_prs (pull_request_id, tenant_id, created_at) VALUES (1, 'default', 't');
         INSERT INTO comment_passes (id, tenant_id, pull_request_id, user_id, cause, head, items, state, created_at)
           VALUES ('p1', 'default', 1, 'u1', 'opened', 'h', 2, 'queued', 't');",
    )
    .execute(&db.pool)
    .await
    .expect("rows of the old schema");
    insert_job(&db, "j2", "a1", "c1", "succeeded")
        .await
        .expect("job");
    insert_job(&db, "j1", "a2", "c1", "queued")
        .await
        .expect("job");
    sqlx::raw_sql(
        "INSERT INTO job_events (job_id, tenant_id, attempt, seq, at, body) VALUES ('j2', 'default', 1, 1, 't', '{}');
         INSERT INTO installation_tokens (job_id, tenant_id, attempt, ciphertext, expires_at)
           VALUES ('j2', 'default', 1, x'00', 't');",
    )
    .execute(&db.pool)
    .await
    .expect("children");
    let rowids = || async {
        sqlx::query_as::<_, (String, i64)>(
            "SELECT 'q:' || id, rowid FROM questions UNION ALL SELECT 'j:' || id, rowid FROM jobs
             UNION ALL SELECT 'p:' || id, rowid FROM comment_passes ORDER BY 1",
        )
        .fetch_all(&db.pool)
        .await
        .expect("rowids")
    };
    let kept = rowids().await;
    migrate(&db.pool, migration_count_through(29))
        .await
        .expect("upgrade in place");
    assert_eq!(rowids().await, kept, "rows and rowids are kept");
    let follow_up: (Option<String>, Option<i64>) =
        sqlx::query_as("SELECT parent_id, pull_request_id FROM questions WHERE id = 'q1'")
            .fetch_one(&db.pool)
            .await
            .expect("follow-up");
    assert_eq!(follow_up, (Some("q9".to_owned()), Some(1)));
    let pass: (Option<String>, String, Option<String>) =
        sqlx::query_as("SELECT user_id, sent, question_id FROM comment_passes WHERE id = 'p1'")
            .fetch_one(&db.pool)
            .await
            .expect("pass");
    assert_eq!(pass, (Some("u1".to_owned()), "[]".to_owned(), None));
    let children: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM job_events), (SELECT COUNT(*) FROM installation_tokens)",
    )
    .fetch_one(&db.pool)
    .await
    .expect("children");
    assert_eq!(children, (1, 1));
    for (child, parent) in [
        ("job_events", "jobs"),
        ("installation_tokens", "jobs"),
        ("jobs", "comment_passes"),
        ("comment_passes", "questions"),
        ("questions", "comment_items"),
    ] {
        let parents: Vec<String> =
            sqlx::query_scalar("SELECT \"table\" FROM pragma_foreign_key_list(?)")
                .bind(child)
                .fetch_all(&db.pool)
                .await
                .expect("foreign keys");
        assert!(parents.contains(&parent.to_owned()), "{child}: {parents:?}");
    }
    let insert_job_of = |analysis: Option<&'static str>,
                         pass: Option<&'static str>,
                         kind: &'static str| {
        sqlx::query(
            "INSERT INTO jobs (id, tenant_id, analysis_id, pass_id, kind, priority, state, spec, credential_id,
                 created_at, stage)
             VALUES (?, 'default', ?, ?, ?, 0, 'queued', '{}', 'c1', 't', ?)",
        )
        .bind(contracts::ids::JobId::new_random().to_string())
        .bind(analysis)
        .bind(pass)
        .bind(kind)
        .bind(if kind == "verify_comments" { "verify:p1" } else { "plan:1" })
        .execute(&db.pool)
    };
    assert!(
        insert_job_of(None, Some("p1"), "verify_comments")
            .await
            .is_ok()
    );
    assert!(
        insert_job_of(Some("a1"), Some("p1"), "verify_comments")
            .await
            .is_err(),
        "two owners"
    );
    assert!(
        insert_job_of(None, Some("p1"), "plan").await.is_err(),
        "analysis kind on a pass"
    );
    assert!(
        insert_job_of(Some("a1"), None, "verify_comments")
            .await
            .is_err(),
        "verify job of an analysis"
    );
    let no_analysis = sqlx::raw_sql(
        "INSERT INTO questions (id, tenant_id, pull_request_id, user_id, text, state, created_at, kind)
           VALUES ('x1', 'default', 1, 'u1', 'x', 'pending', 't', 'pr')",
    )
    .execute(&db.pool)
    .await;
    assert!(
        no_analysis.is_err(),
        "only comment-item questions go without an analysis"
    );
    sqlx::query("DELETE FROM installation_tokens")
        .execute(&db.pool)
        .await
        .expect("revoked");
    sqlx::query("DELETE FROM jobs WHERE id = 'j2'")
        .execute(&db.pool)
        .await
        .expect("delete job");
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM job_events")
        .fetch_one(&db.pool)
        .await
        .expect("events");
    assert_eq!(events, 0, "events still cascade with their job");
    let check: Option<String> = sqlx::query_scalar("PRAGMA foreign_key_check")
        .fetch_optional(&db.pool)
        .await
        .expect("check");
    assert_eq!(check, None);
}

/// 0026 rebuilds `dashboard_snoozes` so ended snoozes can be kept: every existing snooze is kept as not ended, and
/// an ended one stays beside a new snooze of the same item.
#[tokio::test]
async fn snooze_ends_migration_keeps_every_snooze_as_not_ended() {
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
    let before = MIGRATIONS
        .iter()
        .position(|(_, name, _)| *name == "dashboard_snooze_ends")
        .expect("migration 0026");
    migrate(&pool, before).await.expect("schema before 0026");
    let db = Db { pool };
    seed(&db).await;
    sqlx::raw_sql(
        "INSERT INTO dashboard_snoozes (tenant_id, user_id, item_key, snoozed_at, until) VALUES
           ('default', 'u1', 'acme~platform~pr~1203', '2026-11-18T06:00:00.000000Z', '2026-11-21T06:00:00.000000Z'),
           ('default', 'u1', 'acme~platform~issue~7', '2026-11-18T06:00:00.000000Z', '2026-11-19T06:00:00.000000Z');",
    )
    .execute(&db.pool)
    .await
    .expect("snoozes of the old schema");
    migrate(&db.pool, migration_count_through(29))
        .await
        .expect("upgrade in place");
    type Row = (String, String, String, Option<String>, Option<String>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT item_key, snoozed_at, until, ended_at, end_reason FROM dashboard_snoozes ORDER BY id",
    )
    .fetch_all(&db.pool)
    .await
    .expect("rows");
    let kept = |key: &str, until: &str| {
        (
            key.to_owned(),
            "2026-11-18T06:00:00.000000Z".to_owned(),
            until.to_owned(),
            None,
            None,
        )
    };
    assert_eq!(
        rows,
        [
            kept("acme~platform~pr~1203", "2026-11-21T06:00:00.000000Z"),
            kept("acme~platform~issue~7", "2026-11-19T06:00:00.000000Z"),
        ]
    );
    let dash = db.dashboard();
    let key = contracts::api::dashboard::ItemKey::parse("acme~platform~issue~7").expect("key");
    let woken_at = "2026-11-18T08:00:00Z".parse::<Timestamp>().expect("time");
    dash.wake(
        "default",
        "u1",
        std::slice::from_ref(&key),
        dashboard::SnoozeEnd::Woken,
        woken_at,
    )
    .await
    .expect("wake");
    dash.snooze(
        "default",
        "u1",
        &key,
        woken_at,
        woken_at + SignedDuration::from_hours(24),
    )
    .await
    .expect("snooze again");
    let issue_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dashboard_snoozes WHERE item_key = 'acme~platform~issue~7'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("count");
    assert_eq!(issue_rows, 2, "the ended snooze stays beside the new one");
    let duplicate = sqlx::query(
        "INSERT INTO dashboard_snoozes (tenant_id, user_id, item_key, snoozed_at, until)
         VALUES ('default', 'u1', 'acme~platform~pr~1203', 't0', 't1')",
    )
    .execute(&db.pool)
    .await
    .map_err(DbError::from);
    assert!(
        matches!(duplicate, Err(DbError::Conflict)),
        "one snooze per item has not ended: {duplicate:?}"
    );
}

/// 0027 adds the Slack names table to `dashboard_settings` in place: an existing row keeps its settings and gains an
/// empty table; the column takes only a JSON array.
#[tokio::test]
async fn slack_names_migration_gives_existing_settings_an_empty_table() {
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
    let before = MIGRATIONS
        .iter()
        .position(|(_, name, _)| *name == "slack_names")
        .expect("migration 0027");
    migrate(&pool, before).await.expect("schema before 0027");
    let db = Db { pool };
    seed(&db).await;
    sqlx::raw_sql(
        "INSERT INTO dashboard_settings (user_id, tenant_id, ready_label, updated_at)
         VALUES ('u1', 'default', 'ready', 't');",
    )
    .execute(&db.pool)
    .await
    .expect("settings of the old schema");
    migrate(&db.pool, migration_count_through(29))
        .await
        .expect("upgrade in place");
    let row: (String, String) =
        sqlx::query_as("SELECT ready_label, slack_names FROM dashboard_settings")
            .fetch_one(&db.pool)
            .await
            .expect("row");
    assert_eq!(row, ("ready".to_owned(), "[]".to_owned()));
    let refused = sqlx::query("UPDATE dashboard_settings SET slack_names = '{}'")
        .execute(&db.pool)
        .await;
    assert!(refused.is_err(), "an object is not a Slack names table");
}

/// 0029 rebuilds `dashboard_refreshes` so a refresh can be `automatic` (plan §M): every refresh keeps its row and
/// rowid, the jobs and judgements that reference one keep it (no drop cascades into them), the other CHECKs and both
/// indexes stay, and an unknown trigger is still refused.
#[tokio::test]
async fn automatic_trigger_migration_keeps_refreshes_and_what_references_them() {
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
    let before = MIGRATIONS
        .iter()
        .position(|(_, name, _)| *name == "dashboard_trigger_automatic")
        .expect("migration 0029");
    migrate(&pool, before).await.expect("schema before 0029");
    let db = Db { pool };
    seed(&db).await;
    sqlx::raw_sql(
        "INSERT INTO dashboard_refreshes (rowid, id, tenant_id, user_id, trigger, state, started_at, snapshot,
             ranking_state, judged, sent, ranking_attempt, finished_at)
           VALUES (7, 'r1', 'default', 'u1', 'scheduled', 'finished', 't1', '{\"version\":1}', 'finished', 3, 4, 1, 't2'),
                  (9, 'r2', 'default', 'u1', 'manual', 'scanning', 't3', NULL, NULL, 0, 0, 0, NULL);
         INSERT INTO jobs (id, tenant_id, refresh_id, kind, priority, state, spec, credential_id, created_at, stage)
           VALUES ('j1', 'default', 'r1', 'judge_work', 1, 'succeeded', '{}', 'c1', 't', 'judge:r1:1');
         INSERT INTO comment_prs (pull_request_id, tenant_id, created_at) VALUES (1, 'default', 't');
         INSERT INTO comment_passes (id, tenant_id, pull_request_id, user_id, cause, head, items, state, created_at)
           VALUES ('p1', 'default', 1, 'u1', 'opened', 'h', 2, 'queued', 't');
         INSERT INTO jobs (rowid, id, tenant_id, pass_id, kind, priority, state, spec, credential_id, attempt,
             max_attempts, lease_expires_at, job_token_hash, cancel_requested, resumes_at, ready_output,
             ready_output_attempt, result, stage, generation, round, github_access_granted, flow_id, question_id,
             created_at, claimed_at, finished_at)
           VALUES (19, 'j2', 'default', 'p1', 'verify_comments', 0, 'succeeded', '{\"v\":2}', 'c2', 2,
             4, 'lease', x'1234', 1, 'resume', '{}', 2, '{}', 'verify:p1', 1, 3, 1, 'flow', 'question',
             'created', 'claimed', 'finished');
         INSERT INTO job_events (rowid, job_id, tenant_id, attempt, seq, at, body)
           VALUES (11, 'j1', 'default', 0, 1, 't', '{}'), (13, 'j2', 'default', 2, 7, 'event', '{}');
         INSERT INTO installation_tokens (rowid, job_id, tenant_id, attempt, ciphertext, expires_at)
           VALUES (17, 'j2', 'default', 2, x'1234', 'expires');
         INSERT INTO dashboard_judgements (tenant_id, user_id, item_key, input_fingerprint, input_version,
             prompt_version, body, refresh_id, model, judged_at, last_seen_at)
           VALUES ('default', 'u1', 'acme~platform~pr~1', 'f', 1, 1, '{}', 'r1', 'm', 't', 't');",
    )
    .execute(&db.pool)
    .await
    .expect("rows of the old schema");
    async fn contents(db: &Db) -> Vec<(Vec<(String, String)>, Vec<String>)> {
        let mut contents = Vec::new();
        for table in [
            "dashboard_refreshes",
            "dashboard_judgements",
            "jobs",
            "job_events",
            "installation_tokens",
        ] {
            let columns: Vec<(String, String)> =
                sqlx::query_as("SELECT name, type FROM pragma_table_info(?) ORDER BY name")
                    .bind(table)
                    .fetch_all(&db.pool)
                    .await
                    .expect("columns");
            let expressions: Vec<String> = columns
                .iter()
                .map(|(name, kind)| {
                    if kind == "BLOB" {
                        format!("CASE WHEN {name} IS NULL THEN NULL ELSE hex({name}) END")
                    } else {
                        name.clone()
                    }
                })
                .collect();
            // Both identifiers come only from the fixture schema and the fixed table list.
            let rows = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT json_array(rowid, {}) FROM {table} ORDER BY rowid",
                expressions.join(",")
            )))
            .fetch_all(&db.pool)
            .await
            .expect("all columns");
            contents.push((columns, rows));
        }
        contents
    }
    let original = contents(&db).await;
    migrate(&db.pool, migration_count_through(29))
        .await
        .expect("upgrade in place");
    assert_eq!(
        contents(&db).await,
        original,
        "every column and rowid survives"
    );

    type Row = (i64, String, String, String, i64, i64, i64);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT rowid, id, trigger, state, judged, sent, ranking_attempt FROM dashboard_refreshes ORDER BY rowid",
    )
    .fetch_all(&db.pool)
    .await
    .expect("rows");
    assert_eq!(
        rows,
        [
            (
                7,
                "r1".into(),
                "scheduled".into(),
                "finished".into(),
                3,
                4,
                1
            ),
            (9, "r2".into(), "manual".into(), "scanning".into(), 0, 0, 0),
        ]
    );
    let kept: (i64, i64, Option<String>) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM jobs WHERE refresh_id = 'r1'),
                (SELECT count(*) FROM job_events WHERE job_id = 'j1'),
                (SELECT refresh_id FROM dashboard_judgements)",
    )
    .fetch_one(&db.pool)
    .await
    .expect("references");
    assert_eq!(kept, (1, 1, Some("r1".into())), "nothing cascaded");
    let dangling: Vec<(String,)> = sqlx::query_as("SELECT \"table\" FROM pragma_foreign_key_check")
        .fetch_all(&db.pool)
        .await
        .expect("foreign key check");
    assert!(dangling.is_empty(), "{dangling:?}");

    let insert = |id: &'static str, trigger: &'static str, state: &'static str| {
        sqlx::query(
            "INSERT INTO dashboard_refreshes (id, tenant_id, user_id, trigger, state, started_at)
             VALUES (?, 'default', 'u1', ?, ?, 't4')",
        )
        .bind(id)
        .bind(trigger)
        .bind(state)
        .execute(&db.pool)
    };
    assert!(
        insert("r3", "automatic", "cancelled").await.is_ok(),
        "automatic is a trigger"
    );
    assert!(
        insert("r4", "hourly", "cancelled").await.is_err(),
        "an unknown trigger"
    );
    assert!(
        insert("r5", "automatic", "finished").await.is_err(),
        "a finished refresh still needs its snapshot"
    );
    let second_live = insert("r6", "automatic", "scanning")
        .await
        .map_err(DbError::from);
    assert!(
        matches!(second_live, Err(DbError::Conflict)),
        "one live refresh per user: {second_live:?}"
    );
    let indexes: Vec<(String,)> = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'dashboard_refreshes'
           AND name NOT LIKE 'sqlite_autoindex%' ORDER BY name",
    )
    .fetch_all(&db.pool)
    .await
    .expect("indexes");
    assert_eq!(
        indexes,
        [
            ("dashboard_refreshes_by_user".to_owned(),),
            ("dashboard_refreshes_one_live_per_user".to_owned(),)
        ]
    );
    let parents: Vec<(String, String)> = sqlx::query_as(
        "SELECT 'jobs', \"table\" FROM pragma_foreign_key_list('jobs')
         UNION ALL SELECT 'dashboard_judgements', \"table\" FROM pragma_foreign_key_list('dashboard_judgements')
         UNION ALL SELECT 'job_events', \"table\" FROM pragma_foreign_key_list('job_events')
         UNION ALL SELECT 'installation_tokens', \"table\" FROM pragma_foreign_key_list('installation_tokens')
         ORDER BY 1, 2",
    )
    .fetch_all(&db.pool)
    .await
    .expect("foreign keys");
    let parents: Vec<(&str, &str)> = parents
        .iter()
        .map(|(child, parent)| (child.as_str(), parent.as_str()))
        .collect();
    assert_eq!(
        parents,
        [
            ("dashboard_judgements", "dashboard_refreshes"),
            ("dashboard_judgements", "tenants"),
            ("dashboard_judgements", "users"),
            ("installation_tokens", "jobs"),
            ("installation_tokens", "tenants"),
            ("job_events", "jobs"),
            ("job_events", "tenants"),
            ("jobs", "analyses"),
            ("jobs", "comment_passes"),
            ("jobs", "dashboard_refreshes"),
            ("jobs", "tenants"),
        ],
        "every reference names the final tables"
    );
}

#[tokio::test]
async fn migrations_upgrade_v13_db_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diffpilot.sqlite");
    testing::pre_container_database(&path).await.unwrap();
    let pool = SqlitePoolOptions::new()
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .foreign_keys(true),
        )
        .await
        .unwrap();
    migrate(&pool, migration_count_through(30)).await.unwrap();
    sqlx::raw_sql(
        "INSERT INTO dashboard_refreshes (id, tenant_id, user_id, trigger, state, started_at, ranking_attempt, sent)
         SELECT 'recent', tenant_id, user_id, 'manual', 'cancelled',
             strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 day'), 2, 7 FROM dashboard_settings
         UNION ALL SELECT 'old', tenant_id, user_id, 'scheduled', 'cancelled',
             strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-3 days'), 1, 4 FROM dashboard_settings
         UNION ALL SELECT 'unsent', tenant_id, user_id, 'manual', 'cancelled',
             strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 0, 0 FROM dashboard_settings;
         INSERT INTO dashboard_judgements (tenant_id, user_id, item_key, input_fingerprint, input_version,
             prompt_version, body, refresh_id, model, judged_at, last_seen_at)
         SELECT tenant_id, user_id, 'item', 'fingerprint', 1, 1, '{}', id, 'model', started_at, started_at
             FROM dashboard_refreshes WHERE id = 'recent';
         INSERT INTO comment_prs (pull_request_id, tenant_id, created_at)
         SELECT id, tenant_id, 't' FROM pull_requests LIMIT 1;
         INSERT INTO comment_items (id, tenant_id, pull_request_id, item_no, kind, stable_key, author, author_kind,
             title, body, body_hash, first_seen_head, github, state, created_at, updated_at)
         SELECT 'v13-item', tenant_id, id, 1, 'pr_comment', 'key', 'bot', 'bot', 'title', 'body', 'hash', head_sha,
             '{}', 'decided', 't', 't' FROM pull_requests LIMIT 1;
         INSERT INTO comment_decisions (id, tenant_id, item_id, user_id, kind, resolve_ticked, via, head, body_hash, decided_at)
         SELECT 'v13-decision', i.tenant_id, i.id, r.user_id, 'reject', 0, 'question', i.first_seen_head, i.body_hash, 't'
             FROM comment_items i JOIN dashboard_refreshes r ON r.id = 'recent' WHERE i.id = 'v13-item';",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    let db = Db::open(&path).await.unwrap();
    db.pool.close().await;
    let db = Db::open(&path).await.unwrap();
    let versions: Vec<i64> = sqlx::query_scalar(
        "SELECT version FROM _sqlx_migrations WHERE version > 30 ORDER BY version",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(versions, [31, 32, 33, 34]);
    let sends: Vec<(String, i64, i64, bool)> = sqlx::query_as(
        "SELECT s.refresh_id, s.attempt, s.items,
                s.sent_at = r.started_at AND s.tenant_id = r.tenant_id AND s.user_id = r.user_id
                AND s.job_id IS NULL AND s.released_at IS NULL
           FROM dashboard_sends s JOIN dashboard_refreshes r ON r.id = s.refresh_id",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(sends, [("recent".into(), 2, 7, true)]);
    let kept: (i64, i64, String, Option<String>, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM dashboard_refreshes),
                (SELECT sent FROM dashboard_refreshes WHERE id = 'recent'),
                (SELECT via FROM comment_decisions WHERE id = 'v13-decision'),
                (SELECT rejudge_requested_at FROM dashboard_judgements WHERE item_key = 'item'),
                (SELECT status_comment FROM comment_items WHERE id = 'v13-item')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(kept, (3, 7, "question".into(), None, 0));
    assert!(
        sqlx::query("UPDATE comment_items SET status_comment = 2 WHERE id = 'v13-item'")
            .execute(&db.pool)
            .await
            .is_err()
    );
    assert!(sqlx::query("INSERT INTO comment_decision_origins (decision_id, origin) VALUES ('v13-decision', 'other')")
        .execute(&db.pool).await.is_err());
    sqlx::query("INSERT INTO comment_decision_origins (decision_id, origin, by_login, recorded_at, evidence_url, quote)
        VALUES ('v13-decision', 'imported_github', 'owner', 't', 'https://github.com/comment', 'quote')")
        .execute(&db.pool).await.unwrap();
    assert!(
        sqlx::query("UPDATE comment_decision_origins SET quote = 'changed'")
            .execute(&db.pool)
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM comment_items WHERE id = 'v13-item'")
        .execute(&db.pool)
        .await
        .unwrap();
    let origins: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM comment_decision_origins")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(origins, 0);
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&db.pool)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn github_webhook_schema_enforces_delivery_and_repository_keys() {
    let db = db().await;
    sqlx::raw_sql(
        "INSERT INTO github_deliveries (delivery_id, body_sha256, event, changes, state, received_at)
         VALUES ('d1', 'hash', 'ping', '[]', 'pending', 't'), ('d2', 'hash', 'ping', '[]', 'ignored', 't');
         INSERT INTO app_installations_allowed (installation_id, account_id, account_login, repository_selection, resolved_at)
         VALUES (1, 2, 'Acme', 'selected', 't');
         INSERT INTO app_repositories (owner, name, installation_id, checked_at)
         VALUES ('Acme', 'Project', 1, 't');",
    ).execute(&db.pool).await.unwrap();
    for invalid in [
        "UPDATE github_deliveries SET delivery_id = 'd1' WHERE delivery_id = 'd2'",
        "UPDATE github_deliveries SET state = 'unknown' WHERE delivery_id = 'd1'",
        "UPDATE github_deliveries SET changes = 'invalid' WHERE delivery_id = 'd1'",
        "UPDATE github_deliveries SET changes = '{}' WHERE delivery_id = 'd1'",
        "UPDATE app_installations_allowed SET repository_selection = 'unknown'",
        "INSERT INTO app_repositories (owner, name, installation_id, checked_at) VALUES ('acme', 'project', 1, 't')",
    ] {
        assert!(
            sqlx::query(invalid).execute(&db.pool).await.is_err(),
            "{invalid}"
        );
    }
    let indexing: Vec<String> = sqlx::query_scalar(
        "SELECT group_concat(name, ',') FROM (
            SELECT indexes.name AS index_name, cols.name
            FROM pragma_index_list('github_deliveries') AS indexes, pragma_index_info(indexes.name) AS cols
            ORDER BY indexes.name, cols.seqno
         ) GROUP BY index_name",
    ).fetch_all(&db.pool).await.unwrap();
    assert!(
        indexing
            .iter()
            .any(|columns| columns == "body_sha256,received_at")
    );
    assert!(indexing.iter().any(|columns| columns == "received_at"));
}

#[tokio::test]
async fn pr_page_openings_belong_to_a_member_and_pr_in_the_same_tenant() {
    let db = db().await;
    seed(&db).await;
    sqlx::raw_sql(
        "INSERT INTO tenants (id, name, created_at) VALUES ('another', 'Another', 't');
         INSERT INTO memberships (tenant_id, user_id, created_at) VALUES ('default', 'u1', 't'), ('another', 'u1', 't');
         INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES ('u2', 2, 'alice', 't', 't');
         INSERT INTO memberships (tenant_id, user_id, created_at) VALUES ('default', 'u2', 't');
         INSERT INTO pr_page_openings (tenant_id, user_id, pull_request_id, last_opened_at)
         VALUES ('default', 'u1', 1, 'first'), ('default', 'u2', 1, 'other')
         ON CONFLICT (tenant_id, user_id, pull_request_id) DO UPDATE SET last_opened_at = excluded.last_opened_at;
         INSERT INTO pr_page_openings (tenant_id, user_id, pull_request_id, last_opened_at)
         VALUES ('default', 'u1', 1, 'latest')
         ON CONFLICT (tenant_id, user_id, pull_request_id) DO UPDATE SET last_opened_at = excluded.last_opened_at;",
    ).execute(&db.pool).await.unwrap();
    let times: Vec<String> =
        sqlx::query_scalar("SELECT last_opened_at FROM pr_page_openings ORDER BY user_id")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert_eq!(times, ["latest", "other"]);
    assert!(
        sqlx::query("INSERT INTO pr_page_openings VALUES ('another', 'u1', 1, 't')")
            .execute(&db.pool)
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM memberships WHERE tenant_id = 'default' AND user_id = 'u1'")
        .execute(&db.pool)
        .await
        .unwrap();
    let times: Vec<String> = sqlx::query_scalar("SELECT last_opened_at FROM pr_page_openings")
        .fetch_all(&db.pool)
        .await
        .unwrap();
    assert_eq!(times, ["other"]);
}
