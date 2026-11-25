//! Repository API, one module per aggregate, behind the [`Db`] handle.
//!
//! Aggregates used by more than one work package (`jobs`, `analyses`, `documents`, `feedback`,
//! `llm_credentials`) have frozen signatures; the other modules are filled in by their owning work package.

use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use sqlx::migrate::{Migration, MigrationType, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{SqlSafeStr, Sqlite, Transaction};

use crate::error::DbError;

pub mod access_tokens;
pub mod agent_sessions;
pub mod analyses;
mod analysis_progress;
mod assessment_primary;
pub mod assessments;
pub mod comment_sources;
pub mod comments;
pub mod container_sessions;
pub mod dashboard;
pub mod documents;
pub mod feedback;
pub mod flow_work;
pub mod jobs;
pub mod llm_credentials;
pub mod pull_requests;
pub mod questions;
pub mod repos;
pub mod review_loop;
pub mod review_state;
pub mod sessions;
pub mod settings;
pub mod tenants;
pub mod threads;
pub mod tokens;
pub mod users;

#[cfg(test)]
mod concurrency_tests;
#[cfg(test)]
mod containers_tests;
#[cfg(test)]
mod migration_0030_tests;
#[cfg(test)]
mod schema_tests;

/// Test helpers shared by dependent crates.
#[cfg(any(test, feature = "test-util"))]
pub mod testing {
    use std::path::Path;
    use std::time::Duration;

    use sqlx::{Sqlite, Transaction};

    use super::{BUSY_TIMEOUT, Db};
    use crate::error::DbError;

    /// A file-backed WAL database in `dir` with the production pool (several connections), for tests where
    /// connections run concurrently; [`Db::in_memory`] has a single connection and never sees lock contention.
    pub async fn file_backed(dir: &Path) -> Result<Db, DbError> {
        file_backed_with_busy_timeout(dir, BUSY_TIMEOUT).await
    }

    /// [`file_backed`] with a shorter busy timeout, so tests of a held write lock fail fast.
    pub async fn file_backed_with_busy_timeout(
        dir: &Path,
        busy_timeout: Duration,
    ) -> Result<Db, DbError> {
        Db::open_with(&dir.join("diffpilot.sqlite"), busy_timeout).await
    }

    /// Creates a populated pre-0030 database with synthetic auth, for migration/startup tests only.
    pub async fn pre_container_database(path: &Path) -> Result<(), DbError> {
        let options = super::SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(super::SqliteJournalMode::Wal)
            .foreign_keys(true);
        let pool = super::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        super::migrate(&pool, super::migration_count_through(29)).await?;
        sqlx::raw_sql(include_str!("../../tests/fixtures/pre_0030.sql"))
            .execute(&pool)
            .await?;
        // Keep the synthetic browser session live regardless of when the fixture is used.
        sqlx::query("UPDATE sessions SET last_seen_at = ?")
            .bind(super::jobs::ts(contracts::time::Timestamp::now()))
            .execute(&pool)
            .await?;
        pool.close().await;
        Ok(())
    }

    /// Injects a malformed saved read timestamp to test error isolation.
    pub async fn corrupt_comment_read_timestamp(db: &Db, pr: i64) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE comment_prs SET read_at = 'invalid timestamp' WHERE pull_request_id = ?",
        )
        .bind(pr)
        .execute(&db.pool)
        .await?;
        Ok(())
    }

    /// Makes the next `count` writes of a pass's job link change nothing (so each reports a conflict), as links
    /// lost while the database is busy right after a job was submitted; later links are written.
    pub async fn lose_pass_job_links(db: &Db, count: u32) -> Result<(), DbError> {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE TABLE lost_pass_job_links (lost INTEGER NOT NULL);
             CREATE TRIGGER lose_pass_job_links BEFORE UPDATE OF job_id ON comment_passes
             WHEN (SELECT COUNT(*) FROM lost_pass_job_links) < {count}
             BEGIN
                 INSERT INTO lost_pass_job_links (lost) VALUES (1);
                 SELECT RAISE(IGNORE);
             END;"
        )))
        .execute(&db.pool)
        .await?;
        Ok(())
    }

    /// Makes every later write of an in-memory `db` fail while reads still work, as on a full disk
    /// ([`Db::in_memory`] has one connection, which keeps the setting).
    pub async fn refuse_writes(db: &Db) -> Result<(), DbError> {
        sqlx::query("PRAGMA query_only = ON")
            .execute(&db.pool)
            .await?;
        Ok(())
    }

    /// The write lock held on its own connection until dropped (rolled back), as by a concurrent writer.
    #[derive(Debug)]
    pub struct WriteLock(
        #[expect(dead_code, reason = "held for its drop")] Transaction<'static, Sqlite>,
    );

    /// Takes the write lock of `db` on a connection of its own; waits on the busy timeout like any writer.
    pub async fn hold_write_lock(db: &Db) -> Result<WriteLock, DbError> {
        // `BEGIN IMMEDIATE` takes the write lock itself.
        Ok(WriteLock(super::write_tx(&db.pool).await?))
    }
}

/// How long a writer waits for the write lock before failing with `SQLITE_BUSY`.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The schema migrations in order, as `(version, description, SQL)`: the initial schema, then additive
/// changes that upgrade an existing database in place.
const MIGRATIONS: [(i64, &str, &str); 31] = [
    (1, "init", include_str!("../../migrations/0001_init.sql")),
    (
        2,
        "flow_auto_retries",
        include_str!("../../migrations/0002_flow_auto_retries.sql"),
    ),
    (
        3,
        "dashboard",
        include_str!("../../migrations/0003_dashboard.sql"),
    ),
    (
        4,
        "access_tokens",
        include_str!("../../migrations/0004_access_tokens.sql"),
    ),
    (
        5,
        "ranking_attempt",
        include_str!("../../migrations/0005_ranking_attempt.sql"),
    ),
    (
        6,
        "ask_effort",
        include_str!("../../migrations/0006_ask_effort.sql"),
    ),
    (
        7,
        "jobs_targets",
        include_str!("../../migrations/0007_jobs_targets.sql"),
    ),
    (
        8,
        "ranking_sent",
        include_str!("../../migrations/0008_ranking_sent.sql"),
    ),
    (
        9,
        "known_bots",
        include_str!("../../migrations/0009_known_bots.sql"),
    ),
    (
        10,
        "language_ru",
        include_str!("../../migrations/0010_language_ru.sql"),
    ),
    (
        11,
        "hidden_mentions",
        include_str!("../../migrations/0011_hidden_mentions.sql"),
    ),
    (
        12,
        "question_anchors",
        include_str!("../../migrations/0012_question_anchors.sql"),
    ),
    (
        13,
        "question_touched_steps",
        include_str!("../../migrations/0013_question_touched_steps.sql"),
    ),
    (
        14,
        "question_threads",
        include_str!("../../migrations/0014_question_threads.sql"),
    ),
    (
        15,
        "dependencies",
        include_str!("../../migrations/0015_dependencies.sql"),
    ),
    (
        16,
        "unbound_seed_logins",
        include_str!("../../migrations/0016_unbound_seed_logins.sql"),
    ),
    (
        17,
        "review_loop",
        include_str!("../../migrations/0017_review_loop.sql"),
    ),
    (
        18,
        "verify_comments_jobs",
        include_str!("../../migrations/0018_verify_comments_jobs.sql"),
    ),
    (
        19,
        "review_loop_actions",
        include_str!("../../migrations/0019_review_loop_actions.sql"),
    ),
    (
        21,
        "dashboard_snoozes",
        include_str!("../../migrations/0021_dashboard_snoozes.sql"),
    ),
    (
        23,
        "watch_passes",
        include_str!("../../migrations/0023_watch_passes.sql"),
    ),
    (
        24,
        "comment_items_gone",
        include_str!("../../migrations/0024_comment_items_gone.sql"),
    ),
    (
        25,
        "feed_delivery",
        include_str!("../../migrations/0025_feed_delivery.sql"),
    ),
    (
        26,
        "dashboard_snooze_ends",
        include_str!("../../migrations/0026_dashboard_snooze_ends.sql"),
    ),
    (
        27,
        "slack_names",
        include_str!("../../migrations/0027_slack_names.sql"),
    ),
    (
        29,
        "dashboard_trigger_automatic",
        include_str!("../../migrations/0029_dashboard_trigger_automatic.sql"),
    ),
    (
        30,
        "v13_containers",
        include_str!("../../migrations/0030_v13_containers.sql"),
    ),
    (
        31,
        "github_webhooks",
        include_str!("../../migrations/0031_github_webhooks.sql"),
    ),
    (
        32,
        "ranking_v14",
        include_str!("../../migrations/0032_ranking_v14.sql"),
    ),
    (
        33,
        "review_loop_v14",
        include_str!("../../migrations/0033_review_loop_v14.sql"),
    ),
    (
        34,
        "pr_history",
        include_str!("../../migrations/0034_pr_history.sql"),
    ),
];

/// Starts a transaction that may write: `BEGIN IMMEDIATE` takes the write lock at once and waits on the busy
/// timeout. A deferred transaction that reads and then writes fails at once with `SQLITE_BUSY` when another
/// connection writes meanwhile, because SQLite cannot upgrade its read snapshot.
pub(crate) async fn write_tx(pool: &SqlitePool) -> Result<Transaction<'static, Sqlite>, DbError> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the one place that starts a write transaction"
    )]
    let tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    Ok(tx)
}

/// Starts a read-only transaction (one consistent snapshot); it must not write, use [`write_tx`] for that.
pub(crate) async fn read_tx(pool: &SqlitePool) -> Result<Transaction<'static, Sqlite>, DbError> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the one place that starts a read-only transaction"
    )]
    let tx = pool.begin_with("BEGIN DEFERRED").await?;
    Ok(tx)
}

#[cfg(any(test, feature = "test-util"))]
fn migration_count_through(version: i64) -> usize {
    MIGRATIONS.partition_point(|(number, ..)| *number <= version)
}

/// Applies the first `count` of [`MIGRATIONS`] that the database does not have yet.
async fn migrate(pool: &SqlitePool, count: usize) -> Result<(), DbError> {
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
    Migrator::with_migrations(migrations).run(pool).await?;
    Ok(())
}

/// The tenant every v1 row belongs to.
pub const DEFAULT_TENANT: &str = "default";

/// Handle to the database; cheap to clone.
#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
    job_changes: Arc<jobs::JobChanges>,
}

impl fmt::Debug for Db {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Db").finish_non_exhaustive()
    }
}

impl Db {
    /// Opens (creating if missing) the database file in WAL mode and applies the schema.
    pub async fn open(path: &Path) -> Result<Self, DbError> {
        Self::open_with(path, BUSY_TIMEOUT).await
    }

    async fn open_with(path: &Path, busy_timeout: Duration) -> Result<Self, DbError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(busy_timeout);
        let pool = SqlitePoolOptions::new().connect_with(options).await?;
        Self::migrated(pool).await
    }

    /// A private in-memory database with the schema applied, for tests.
    pub async fn in_memory() -> Result<Self, DbError> {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true);
        // One connection that is never replaced: every in-memory connection is its own database. No ping before
        // acquire: a caller cancelled during that ping makes sqlx drop the connection (and the database with it).
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .test_before_acquire(false)
            .connect_with(options)
            .await?;
        Self::migrated(pool).await
    }

    async fn migrated(pool: SqlitePool) -> Result<Self, DbError> {
        migrate(&pool, MIGRATIONS.len()).await?;
        Ok(Self::from_pool(pool))
    }

    fn from_pool(pool: SqlitePool) -> Self {
        Self {
            pool,
            job_changes: Arc::default(),
        }
    }

    /// The job queue.
    pub fn jobs(&self) -> jobs::Jobs<'_> {
        jobs::Jobs {
            pool: &self.pool,
            changes: &self.job_changes,
        }
    }

    /// Analyses.
    pub fn analyses(&self) -> analyses::Analyses<'_> {
        analyses::Analyses {
            pool: &self.pool,
            db: self,
        }
    }

    /// JSON documents of an analysis.
    pub fn documents(&self) -> documents::Documents<'_> {
        documents::Documents { pool: &self.pool }
    }

    /// Feedback.
    pub fn feedback(&self) -> feedback::FeedbackRepo<'_> {
        feedback::FeedbackRepo { pool: &self.pool }
    }

    /// LLM credentials and the last LLM call outcome.
    pub fn llm_credentials(&self) -> llm_credentials::LlmCredentials<'_> {
        llm_credentials::LlmCredentials { pool: &self.pool }
    }
}

pub mod usage;

#[cfg(test)]
mod assessments_tests;

#[cfg(test)]
mod private_ask_tests;
