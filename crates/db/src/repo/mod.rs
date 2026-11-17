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

pub mod analyses;
mod analysis_progress;
pub mod comments;
pub mod documents;
pub mod feedback;
pub mod jobs;
pub mod llm_credentials;
pub mod pull_requests;
pub mod questions;
pub mod repos;
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
        Db::open_with(&dir.join("smart-review.sqlite"), busy_timeout).await
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

/// The single schema file, rewritten in place until deployment.
const INIT_SQL: &str = include_str!("../../migrations/0001_init.sql");

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
        // One connection that never idles out: every in-memory connection is its own database.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await?;
        Self::migrated(pool).await
    }

    async fn migrated(pool: SqlitePool) -> Result<Self, DbError> {
        let init = Migration::new(
            1,
            "init".into(),
            MigrationType::Simple,
            INIT_SQL.into_sql_str(),
            false,
        );
        Migrator::with_migrations(vec![init]).run(&pool).await?;
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
