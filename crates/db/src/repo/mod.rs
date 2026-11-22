//! Repository API, one module per aggregate, behind the [`Db`] handle.
//!
//! Aggregates used by more than one work package (`jobs`, `analyses`, `documents`, `feedback`,
//! `llm_credentials`) have frozen signatures; the other modules are filled in by their owning work package.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use sqlx::migrate::{Migration, MigrationType, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{SqlSafeStr, Sqlite, Transaction};

use crate::error::DbError;

pub mod access_tokens;
pub mod agent_sessions;
pub mod analyses;
mod analysis_progress;
pub mod comments;
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
const MIGRATIONS: [(i64, &str, &str); 19] = [
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
        Ok(Self { pool })
    }

    /// The job queue.
    pub fn jobs(&self) -> jobs::Jobs<'_> {
        jobs::Jobs { pool: &self.pool }
    }

    /// Analyses.
    pub fn analyses(&self) -> analyses::Analyses<'_> {
        analyses::Analyses { pool: &self.pool }
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
