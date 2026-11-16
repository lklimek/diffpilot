//! Repository API, one module per aggregate, behind the [`Db`] handle.
//!
//! Aggregates used by more than one work package (`jobs`, `analyses`, `documents`, `feedback`,
//! `llm_credentials`) have frozen signatures; the other modules are filled in by their owning work package.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use sqlx::SqlSafeStr;
use sqlx::migrate::{Migration, MigrationType, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::error::DbError;

pub mod analyses;
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
mod schema_tests;

/// The single schema file, rewritten in place until deployment.
const INIT_SQL: &str = include_str!("../../migrations/0001_init.sql");

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
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
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
