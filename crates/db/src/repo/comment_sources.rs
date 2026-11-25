//! Versioned review sources; each cleaned version is assessed at most once.

use super::{
    Db,
    jobs::{parse_ts, parse_wire, ts, wire_name},
};
use crate::error::DbError;
use contracts::{api::review_loop::CommentSource, time::Timestamp};
use sqlx::{Row, SqlitePool};

/// Repository for source versions, independent of how many points the model reads from them.
#[derive(Debug)]
pub struct CommentSources<'a> {
    pool: &'a SqlitePool,
}

/// A stored source version and its stable database key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRecord {
    /// Database source key.
    pub id: i64,
    /// Non-secret source version exposed by the API.
    pub source: CommentSource,
}

impl Db {
    /// Versioned comment sources.
    pub fn comment_sources(&self) -> CommentSources<'_> {
        CommentSources { pool: &self.pool }
    }
}

impl CommentSources<'_> {
    /// Stores a cleaned source version idempotently; observing it again never clears its read marker.
    pub async fn put(&self, tenant: &str, pr: i64, source: &CommentSource) -> Result<i64, DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let id=sqlx::query_scalar("INSERT INTO comment_sources(tenant_id,pull_request_id,source_kind,url,author,body,body_hash,role)
            SELECT ?,?,?,?,?,?,?,? WHERE EXISTS(SELECT 1 FROM comment_prs WHERE pull_request_id=? AND tenant_id=?)
            ON CONFLICT(pull_request_id,url,body_hash) DO UPDATE SET role=excluded.role
            WHERE comment_sources.tenant_id=excluded.tenant_id RETURNING id")
            .bind(tenant).bind(pr).bind(wire_name(source.kind)?).bind(&source.url).bind(&source.author).bind(&source.body)
            .bind(&source.body_hash).bind(wire_name(source.role)?).bind(pr).bind(tenant).fetch_optional(&mut *tx).await?.ok_or(DbError::NotFound)?;
        tx.commit().await?;
        Ok(id)
    }

    /// Records the first successful read; returns false when already read or absent.
    pub async fn mark_read(&self, id: i64, now: Timestamp) -> Result<bool, DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let changed =
            sqlx::query("UPDATE comment_sources SET read_at=? WHERE id=? AND read_at IS NULL")
                .bind(ts(now))
                .bind(id)
                .execute(&mut *tx)
                .await?
                .rows_affected()
                == 1;
        tx.commit().await?;
        Ok(changed)
    }

    /// Lists all versions, including sources from which no points were read.
    pub async fn list(&self, tenant: &str, pr: i64) -> Result<Vec<SourceRecord>, DbError> {
        let mut tx = super::read_tx(self.pool).await?;
        let rows = sqlx::query(
            "SELECT * FROM comment_sources WHERE tenant_id=? AND pull_request_id=? ORDER BY id",
        )
        .bind(tenant)
        .bind(pr)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        rows.into_iter()
            .map(|r| {
                Ok(SourceRecord {
                    id: r.try_get("id")?,
                    source: CommentSource {
                        url: r.try_get("url")?,
                        kind: parse_wire(
                            &r.try_get::<String, _>("source_kind")?,
                            "comment_sources.source_kind",
                        )?,
                        author: r.try_get("author")?,
                        body: r.try_get("body")?,
                        body_hash: r.try_get("body_hash")?,
                        role: parse_wire(&r.try_get::<String, _>("role")?, "comment_sources.role")?,
                        read_at: r
                            .try_get::<Option<String>, _>("read_at")?
                            .map(|t| parse_ts(&t, "comment_sources.read_at"))
                            .transpose()?,
                    },
                })
            })
            .collect()
    }
}
