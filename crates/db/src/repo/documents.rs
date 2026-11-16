//! Repository for `documents`: validated agent output and pre-pass results as JSON per analysis.
//! Frozen signatures (cross-WP); bodies are filled in by WP2-INGEST.

use chrono::{DateTime, Utc};
use contracts::ids::AnalysisId;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::SqlitePool;

use crate::error::DbError;

/// Access to analysis documents.
#[derive(Debug)]
pub struct Documents<'a> {
    #[expect(dead_code, reason = "used once WP2-INGEST implements the operations")]
    pub(crate) pool: &'a SqlitePool,
}

/// Kind of a stored document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DocumentKind {
    /// Deterministic pre-pass result (key `""`).
    Prepass,
    /// Verified `plan` output (key `""`).
    Plan,
    /// Verified `pr_context` output (key `""`).
    PrContext,
    /// Verified `analyse_flow` output (key = flow id).
    Flow,
    /// Threads and checks fetched at ingest (key `""`).
    GithubSnapshot,
}

#[expect(
    unused_variables,
    reason = "signatures frozen by WP1-A; bodies are filled in by WP2-INGEST"
)]
impl Documents<'_> {
    /// Inserts or replaces a document.
    pub async fn put<T: Serialize + Sync>(
        &self,
        analysis: AnalysisId,
        kind: DocumentKind,
        key: &str,
        body: &T,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "documents::put",
        })
    }

    /// A document, if stored.
    pub async fn get<T: DeserializeOwned>(
        &self,
        analysis: AnalysisId,
        kind: DocumentKind,
        key: &str,
    ) -> Result<Option<T>, DbError> {
        Err(DbError::NotImplemented {
            operation: "documents::get",
        })
    }

    /// Every document of a kind, as `(key, body)` ordered by key.
    pub async fn list<T: DeserializeOwned>(
        &self,
        analysis: AnalysisId,
        kind: DocumentKind,
    ) -> Result<Vec<(String, T)>, DbError> {
        Err(DbError::NotImplemented {
            operation: "documents::list",
        })
    }

    /// Deletes every document of an analysis (cancel discards partial results); returns the count.
    pub async fn delete_for_analysis(&self, analysis: AnalysisId) -> Result<u64, DbError> {
        Err(DbError::NotImplemented {
            operation: "documents::delete_for_analysis",
        })
    }
}
