//! Repository for `comment_requests`: idempotency of pending-review comment creation (ADR-006). The comments
//! themselves live only on GitHub.

use contracts::ids::AnalysisId;
use contracts::time::Timestamp;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::{Row, SqlitePool};

use super::Db;
use super::jobs::ts;
use crate::error::DbError;

/// Access to comment requests.
#[derive(Debug)]
pub struct CommentRequests<'a> {
    pool: &'a SqlitePool,
}

impl Db {
    /// Idempotency records of comment creation.
    pub fn comment_requests(&self) -> CommentRequests<'_> {
        CommentRequests { pool: &self.pool }
    }
}

/// A comment creation request, identified by (user, idempotency key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommentRequestKey<'a> {
    /// The requesting user.
    pub user_id: &'a str,
    /// Client-generated idempotency key.
    pub idempotency_key: &'a str,
}

/// How a comment request may proceed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommentRequestStart<T> {
    /// First attempt, or a retry after a definite failure: the caller performs the write.
    Started,
    /// The same request already succeeded: its stored response.
    Done(T),
    /// The same request is running or its outcome is unknown (GitHub did not answer the write).
    InProgress,
    /// The key was used for another request (other analysis or request hash).
    Mismatch,
}

impl CommentRequests<'_> {
    /// Claims `key` for the request `request_hash` on `analysis`; see [`CommentRequestStart`].
    pub async fn begin<T: DeserializeOwned>(
        &self,
        key: CommentRequestKey<'_>,
        analysis: AnalysisId,
        request_hash: &[u8],
        now: Timestamp,
    ) -> Result<CommentRequestStart<T>, DbError> {
        let analysis = analysis.to_string();
        let inserted = sqlx::query(
            "INSERT INTO comment_requests (user_id, idempotency_key, tenant_id, analysis_id, request_hash, state,
                 created_at)
             SELECT ?, ?, tenant_id, id, ?, 'in_progress', ? FROM analyses WHERE id = ?
             ON CONFLICT (user_id, idempotency_key) DO NOTHING",
        )
        .bind(key.user_id)
        .bind(key.idempotency_key)
        .bind(request_hash)
        .bind(ts(now))
        .bind(&analysis)
        .execute(self.pool)
        .await?;
        if inserted.rows_affected() == 1 {
            return Ok(CommentRequestStart::Started);
        }
        let retried = sqlx::query(
            "UPDATE comment_requests SET state = 'in_progress'
             WHERE user_id = ? AND idempotency_key = ? AND analysis_id = ? AND request_hash = ?
                 AND state = 'failed'",
        )
        .bind(key.user_id)
        .bind(key.idempotency_key)
        .bind(&analysis)
        .bind(request_hash)
        .execute(self.pool)
        .await?;
        if retried.rows_affected() == 1 {
            return Ok(CommentRequestStart::Started);
        }
        let Some(row) = sqlx::query(
            "SELECT analysis_id, request_hash, state, response FROM comment_requests
             WHERE user_id = ? AND idempotency_key = ?",
        )
        .bind(key.user_id)
        .bind(key.idempotency_key)
        .fetch_optional(self.pool)
        .await?
        else {
            // Nothing inserted and no row: the analysis does not exist.
            return Err(DbError::NotFound);
        };
        let stored_analysis: String = row.try_get("analysis_id")?;
        let stored_hash: Vec<u8> = row.try_get("request_hash")?;
        if stored_analysis != analysis || stored_hash != request_hash {
            return Ok(CommentRequestStart::Mismatch);
        }
        let state: String = row.try_get("state")?;
        let response: Option<String> = row.try_get("response")?;
        match (state.as_str(), response) {
            ("done", Some(response)) => {
                Ok(CommentRequestStart::Done(serde_json::from_str(&response)?))
            }
            ("in_progress", _) => Ok(CommentRequestStart::InProgress),
            _ => Err(DbError::Corrupt {
                column: "comment_requests.state",
            }),
        }
    }

    /// Stores the response of a started request; later [`CommentRequests::begin`] calls return it.
    pub async fn complete<T: Serialize + Sync>(
        &self,
        key: CommentRequestKey<'_>,
        response: &T,
    ) -> Result<(), DbError> {
        let response = serde_json::to_string(response)?;
        self.finish(key, "done", Some(response)).await
    }

    /// Marks a started request as definitely failed (nothing was written); a retry with the same key proceeds.
    pub async fn fail(&self, key: CommentRequestKey<'_>) -> Result<(), DbError> {
        self.finish(key, "failed", None).await
    }

    async fn finish(
        &self,
        key: CommentRequestKey<'_>,
        state: &str,
        response: Option<String>,
    ) -> Result<(), DbError> {
        let updated = sqlx::query(
            "UPDATE comment_requests SET state = ?, response = ?
             WHERE user_id = ? AND idempotency_key = ? AND state = 'in_progress'",
        )
        .bind(state)
        .bind(response)
        .bind(key.user_id)
        .bind(key.idempotency_key)
        .execute(self.pool)
        .await?;
        match updated.rows_affected() {
            1 => Ok(()),
            _ => Err(DbError::Conflict),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::repo::analyses::testing::seed_analysis;

    const KEY: CommentRequestKey<'static> = CommentRequestKey {
        user_id: "u1",
        idempotency_key: "4f6c5a5e-1f43-4d6b-9e4f-6f0d1c2b3a49",
    };

    async fn begin(db: &Db, analysis: AnalysisId, hash: &[u8]) -> CommentRequestStart<Value> {
        db.comment_requests()
            .begin(KEY, analysis, hash, Timestamp::now())
            .await
            .expect("begin")
    }

    #[tokio::test]
    async fn completed_request_returns_its_stored_response() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        assert_eq!(
            begin(&db, analysis, b"hash").await,
            CommentRequestStart::Started
        );
        assert_eq!(
            begin(&db, analysis, b"hash").await,
            CommentRequestStart::InProgress,
            "a repeat before completion does not write again"
        );
        let response = json!({"comment": {"id": "PRRC_1"}});
        db.comment_requests()
            .complete(KEY, &response)
            .await
            .expect("complete");
        assert_eq!(
            begin(&db, analysis, b"hash").await,
            CommentRequestStart::Done(response)
        );
    }

    #[tokio::test]
    async fn key_reused_for_another_request_is_a_mismatch() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        let other = seed_analysis(&db).await.expect("seed");
        assert_eq!(
            begin(&db, analysis, b"hash").await,
            CommentRequestStart::Started
        );
        assert_eq!(
            begin(&db, analysis, b"other").await,
            CommentRequestStart::Mismatch
        );
        assert_eq!(
            begin(&db, other, b"hash").await,
            CommentRequestStart::Mismatch
        );
    }

    #[tokio::test]
    async fn failed_request_can_be_retried_with_the_same_key() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        assert_eq!(
            begin(&db, analysis, b"hash").await,
            CommentRequestStart::Started
        );
        db.comment_requests().fail(KEY).await.expect("fail");
        assert_eq!(
            begin(&db, analysis, b"other").await,
            CommentRequestStart::Mismatch,
            "a failed key stays bound to its request"
        );
        assert_eq!(
            begin(&db, analysis, b"hash").await,
            CommentRequestStart::Started
        );
    }

    #[tokio::test]
    async fn finishing_a_request_that_is_not_in_progress_conflicts() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        assert!(matches!(
            db.comment_requests().fail(KEY).await,
            Err(DbError::Conflict)
        ));
        begin(&db, analysis, b"hash").await;
        db.comment_requests()
            .complete(KEY, &json!({}))
            .await
            .expect("complete");
        assert!(matches!(
            db.comment_requests().fail(KEY).await,
            Err(DbError::Conflict)
        ));
    }

    #[tokio::test]
    async fn unknown_analysis_is_not_found() {
        let db = Db::in_memory().await.expect("db");
        seed_analysis(&db).await.expect("seed user");
        let result = db
            .comment_requests()
            .begin::<Value>(KEY, AnalysisId::new_random(), b"hash", Timestamp::now())
            .await;
        assert!(matches!(result, Err(DbError::NotFound)), "{result:?}");
    }
}
