//! Repository for `documents`: validated agent output and pre-pass results as JSON per analysis.
//! Frozen signatures (cross-WP).

use chrono::{DateTime, Utc};
use contracts::ids::AnalysisId;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::SqlitePool;

use super::jobs::ts;
use crate::error::DbError;

/// Access to analysis documents.
#[derive(Debug)]
pub struct Documents<'a> {
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

impl DocumentKind {
    /// The stored `kind` value.
    fn as_str(self) -> &'static str {
        match self {
            Self::Prepass => "prepass",
            Self::Plan => "plan",
            Self::PrContext => "pr_context",
            Self::Flow => "flow",
            Self::GithubSnapshot => "github_snapshot",
        }
    }
}

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
        let body = serde_json::to_string(body)?;
        let stored = sqlx::query(
            "INSERT INTO documents (analysis_id, tenant_id, kind, key, body, created_at)
             SELECT id, tenant_id, ?, ?, ?, ? FROM analyses WHERE id = ?
             ON CONFLICT (analysis_id, kind, key) DO UPDATE SET body = excluded.body,
                 created_at = excluded.created_at",
        )
        .bind(kind.as_str())
        .bind(key)
        .bind(body)
        .bind(ts(now))
        .bind(analysis.to_string())
        .execute(self.pool)
        .await?;
        if stored.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// A document, if stored.
    pub async fn get<T: DeserializeOwned>(
        &self,
        analysis: AnalysisId,
        kind: DocumentKind,
        key: &str,
    ) -> Result<Option<T>, DbError> {
        let body: Option<String> = sqlx::query_scalar(
            "SELECT body FROM documents WHERE analysis_id = ? AND kind = ? AND key = ?",
        )
        .bind(analysis.to_string())
        .bind(kind.as_str())
        .bind(key)
        .fetch_optional(self.pool)
        .await?;
        Ok(body.map(|body| serde_json::from_str(&body)).transpose()?)
    }

    /// Every document of a kind, as `(key, body)` ordered by key.
    pub async fn list<T: DeserializeOwned>(
        &self,
        analysis: AnalysisId,
        kind: DocumentKind,
    ) -> Result<Vec<(String, T)>, DbError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT key, body FROM documents WHERE analysis_id = ? AND kind = ? ORDER BY key",
        )
        .bind(analysis.to_string())
        .bind(kind.as_str())
        .fetch_all(self.pool)
        .await?;
        rows.into_iter()
            .map(|(key, body)| Ok((key, serde_json::from_str(&body)?)))
            .collect()
    }

    /// Deletes every document of an analysis (cancel discards partial results); returns the count.
    pub async fn delete_for_analysis(&self, analysis: AnalysisId) -> Result<u64, DbError> {
        let deleted = sqlx::query("DELETE FROM documents WHERE analysis_id = ?")
            .bind(analysis.to_string())
            .execute(self.pool)
            .await?;
        Ok(deleted.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::{Value, json};

    use super::*;
    use crate::repo::Db;
    use crate::repo::analyses::testing::seed_analysis;

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 16, 12, minute, 0)
            .single()
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn put_replaces_by_kind_and_key_and_lists_in_key_order() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        let docs = db.documents();
        docs.put(analysis, DocumentKind::Flow, "f2", &json!({"n": 2}), at(1))
            .await
            .expect("put f2");
        docs.put(analysis, DocumentKind::Flow, "f1", &json!({"n": 0}), at(1))
            .await
            .expect("put f1");
        docs.put(analysis, DocumentKind::Flow, "f1", &json!({"n": 1}), at(2))
            .await
            .expect("replace f1");
        docs.put(
            analysis,
            DocumentKind::Plan,
            "",
            &json!({"flows": []}),
            at(2),
        )
        .await
        .expect("put plan");

        let flows: Vec<(String, Value)> =
            docs.list(analysis, DocumentKind::Flow).await.expect("list");
        assert_eq!(
            flows,
            [
                ("f1".to_owned(), json!({"n": 1})),
                ("f2".to_owned(), json!({"n": 2}))
            ]
        );
        let plan: Option<Value> = docs
            .get(analysis, DocumentKind::Plan, "")
            .await
            .expect("get");
        assert_eq!(plan, Some(json!({"flows": []})));
        let missing: Option<Value> = docs
            .get(analysis, DocumentKind::PrContext, "")
            .await
            .expect("get");
        assert_eq!(missing, None);
        assert_eq!(docs.delete_for_analysis(analysis).await.expect("delete"), 3);
        assert!(
            docs.list::<Value>(analysis, DocumentKind::Flow)
                .await
                .expect("list")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn put_for_unknown_analysis_is_not_found() {
        let db = Db::in_memory().await.expect("db");
        let result = db
            .documents()
            .put(
                AnalysisId::new_random(),
                DocumentKind::Plan,
                "",
                &json!({}),
                at(1),
            )
            .await;
        assert!(matches!(result, Err(DbError::NotFound)));
    }
}
