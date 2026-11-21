//! Repository for `documents`: validated agent output and pre-pass results as JSON per analysis.
//! Frozen signatures (cross-WP).

use contracts::ids::AnalysisId;
use contracts::time::Timestamp;
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
    /// Lines the server did not send to the LLM, with the reason (key `plan`, or a flow id).
    Unsent,
    /// The MemCan project profile lookup of the analysis, made once per round (key `""`; body `{"memcan":
    /// "not_checked"}` or `{"memcan": "checked", "text": …}`; a Retry deletes a `not_checked` one).
    MemcanProfile,
    /// The Cargo map and conversion facts of the head commit (key `""`; ADR-021).
    Dependencies,
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
            Self::Unsent => "unsent",
            Self::MemcanProfile => "memcan_profile",
            Self::Dependencies => "dependencies",
        }
    }
}

impl Documents<'_> {
    /// Inserts or replaces a document the server built.
    pub async fn put<T: Serialize + Sync>(
        &self,
        analysis: AnalysisId,
        kind: DocumentKind,
        key: &str,
        body: &T,
        now: Timestamp,
    ) -> Result<(), DbError> {
        self.store(analysis, kind, key, body, None, now).await
    }

    /// Inserts or replaces an accepted agent output with whether its attempt ran with GitHub access.
    pub async fn put_agent_output<T: Serialize + Sync>(
        &self,
        analysis: AnalysisId,
        kind: DocumentKind,
        key: &str,
        body: &T,
        github_access: bool,
        now: Timestamp,
    ) -> Result<(), DbError> {
        self.store(analysis, kind, key, body, Some(github_access), now)
            .await
    }

    /// Whether the stored agent output's attempt ran with GitHub access; `None` when the document is missing or
    /// the server built it.
    pub async fn github_access(
        &self,
        analysis: AnalysisId,
        kind: DocumentKind,
        key: &str,
    ) -> Result<Option<bool>, DbError> {
        let access: Option<Option<bool>> = sqlx::query_scalar(
            "SELECT github_access FROM documents WHERE analysis_id = ? AND kind = ? AND key = ?",
        )
        .bind(analysis.to_string())
        .bind(kind.as_str())
        .bind(key)
        .fetch_optional(self.pool)
        .await?;
        Ok(access.flatten())
    }

    async fn store<T: Serialize + Sync>(
        &self,
        analysis: AnalysisId,
        kind: DocumentKind,
        key: &str,
        body: &T,
        github_access: Option<bool>,
        now: Timestamp,
    ) -> Result<(), DbError> {
        let body = serde_json::to_string(body)?;
        let stored = sqlx::query(
            "INSERT INTO documents (analysis_id, tenant_id, kind, key, body, github_access, created_at)
             SELECT id, tenant_id, ?, ?, ?, ?, ? FROM analyses WHERE id = ?
             ON CONFLICT (analysis_id, kind, key) DO UPDATE SET body = excluded.body,
                 github_access = excluded.github_access, created_at = excluded.created_at",
        )
        .bind(kind.as_str())
        .bind(key)
        .bind(body)
        .bind(github_access)
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

    /// Every document of a kind as stored text, as `(key, body)` ordered by key: the version a caller verified.
    pub async fn list_raw(
        &self,
        analysis: AnalysisId,
        kind: DocumentKind,
    ) -> Result<Vec<(String, String)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT key, body FROM documents WHERE analysis_id = ? AND kind = ? ORDER BY key",
        )
        .bind(analysis.to_string())
        .bind(kind.as_str())
        .fetch_all(self.pool)
        .await?)
    }

    /// Deletes one document, if stored.
    pub async fn delete(
        &self,
        analysis: AnalysisId,
        kind: DocumentKind,
        key: &str,
    ) -> Result<(), DbError> {
        sqlx::query("DELETE FROM documents WHERE analysis_id = ? AND kind = ? AND key = ?")
            .bind(analysis.to_string())
            .bind(kind.as_str())
            .bind(key)
            .execute(self.pool)
            .await?;
        Ok(())
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
    use serde_json::{Value, json};

    use super::*;
    use crate::repo::Db;
    use crate::repo::analyses::testing::seed_analysis;

    fn at(minute: u32) -> Timestamp {
        format!("2026-11-16T12:{minute:02}:00Z")
            .parse::<Timestamp>()
            .expect("time")
    }

    #[tokio::test]
    async fn memcan_profile_lookup_is_stored_per_analysis() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        let docs = db.documents();
        let body = json!({"memcan": "checked", "text": null});
        docs.put(analysis, DocumentKind::MemcanProfile, "", &body, at(1))
            .await
            .expect("put");
        let stored: Option<Value> = docs
            .get(analysis, DocumentKind::MemcanProfile, "")
            .await
            .expect("get");
        assert_eq!(stored, Some(body));
    }

    /// GitHub access is kept with an agent output, replaced with it, and absent for server-built documents.
    #[tokio::test]
    async fn agent_output_keeps_its_github_access() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        let docs = db.documents();
        let body = json!({"summary": "s"});
        assert_eq!(
            docs.github_access(analysis, DocumentKind::PrContext, "")
                .await
                .expect("read"),
            None
        );
        docs.put_agent_output(analysis, DocumentKind::PrContext, "", &body, true, at(1))
            .await
            .expect("put");
        assert_eq!(
            docs.github_access(analysis, DocumentKind::PrContext, "")
                .await
                .expect("read"),
            Some(true)
        );
        docs.put_agent_output(analysis, DocumentKind::PrContext, "", &body, false, at(2))
            .await
            .expect("replace");
        assert_eq!(
            docs.github_access(analysis, DocumentKind::PrContext, "")
                .await
                .expect("read"),
            Some(false)
        );
        docs.put(analysis, DocumentKind::PrContext, "", &body, at(3))
            .await
            .expect("server-built");
        assert_eq!(
            docs.github_access(analysis, DocumentKind::PrContext, "")
                .await
                .expect("read"),
            None
        );
        docs.delete(analysis, DocumentKind::PrContext, "")
            .await
            .expect("delete");
        let gone: Option<Value> = docs
            .get(analysis, DocumentKind::PrContext, "")
            .await
            .expect("read");
        assert_eq!(gone, None);
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
