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
        sqlx::query_scalar("SELECT COALESCE((SELECT t.generation FROM analytic_threads t JOIN analyses a ON a.pull_request_id=t.pull_request_id AND COALESCE(a.payer_id,a.created_by)=t.user_id WHERE a.pull_request_id=p.id ORDER BY a.created_at DESC,a.rowid DESC LIMIT 1),0) FROM pull_requests p WHERE p.id = ?")
            .bind(pr)
            .fetch_optional(self.pool)
            .await?
            .ok_or(DbError::NotFound)
    }

    /// The caller's thread only when it was forked from the current analytic generation.
    pub async fn working(&self, pr: i64, user: &str) -> Result<Option<String>, DbError> {
        Ok(sqlx::query_scalar("SELECT w.agent_thread_id FROM working_sessions w JOIN analytic_threads p ON p.pull_request_id = w.pull_request_id AND p.user_id=w.user_id
            WHERE w.pull_request_id = ? AND w.user_id = ? AND w.analytic_generation = p.generation
            AND w.thread_kind='ask' AND p.state IN ('none', 'active')")
            .bind(pr).bind(user).fetch_optional(self.pool).await?)
    }
}
