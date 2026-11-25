//! Private Ask forks of a pull request's analytic session.

use super::Db;
use crate::error::DbError;
use sqlx::SqlitePool;

/// Session routing facts, read only when the PR's job has been claimed.
#[derive(Debug)]
pub struct AgentSessions<'a> {
    pool: &'a SqlitePool,
}

impl Db {
    /// Analytic generations and private working threads.
    pub fn agent_sessions(&self) -> AgentSessions<'_> {
        AgentSessions { pool: &self.pool }
    }
}

impl AgentSessions<'_> {
    /// Current analytic generation, including successful turns that reused the same thread.
    pub async fn generation(&self, pr: i64) -> Result<i64, DbError> {
        let mut tx = super::read_tx(self.pool).await?;
        let current = generation(&mut tx, pr).await?;
        tx.commit().await?;
        Ok(current)
    }

    /// The caller's thread only when it was forked from the current analytic generation.
    pub async fn working(&self, pr: i64, user: &str) -> Result<Option<String>, DbError> {
        let mut tx = super::read_tx(self.pool).await?;
        let current = generation(&mut tx, pr).await?;
        let thread = sqlx::query_scalar("SELECT w.agent_thread_id FROM working_sessions w
            WHERE w.pull_request_id=? AND w.user_id=? AND w.seed_generation=? AND w.thread_kind='ask'
                AND NOT EXISTS(SELECT 1 FROM analytic_threads t WHERE t.pull_request_id=w.pull_request_id AND t.user_id=w.user_id AND t.state IN ('drop_requested','dropped'))")
            .bind(pr).bind(user).bind(current).fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(thread)
    }
}

/// Current shared analysis generation; a missing checkpoint on the latest payer is not an older payer's history.
pub(super) async fn generation(tx: &mut sqlx::SqliteConnection, pr: i64) -> Result<i64, DbError> {
    sqlx::query_scalar("SELECT COALESCE((SELECT t.generation FROM analyses a LEFT JOIN analytic_threads t ON t.pull_request_id=a.pull_request_id AND t.user_id=COALESCE(a.payer_id,a.created_by) WHERE a.pull_request_id=p.id ORDER BY a.created_at DESC,a.rowid DESC LIMIT 1),0) FROM pull_requests p WHERE p.id=?")
        .bind(pr).fetch_optional(tx).await?.ok_or(DbError::NotFound)
}
