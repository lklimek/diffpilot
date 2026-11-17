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
        sqlx::query_scalar("SELECT analytic_generation FROM pull_requests WHERE id = ?")
            .bind(pr)
            .fetch_optional(self.pool)
            .await?
            .ok_or(DbError::NotFound)
    }

    /// The caller's thread only when it was forked from the current analytic generation.
    pub async fn working(&self, pr: i64, user: &str) -> Result<Option<String>, DbError> {
        Ok(sqlx::query_scalar("SELECT w.agent_thread_id FROM working_sessions w JOIN pull_requests p ON p.id = w.pull_request_id
            WHERE w.pull_request_id = ? AND w.user_id = ? AND w.analytic_generation = p.analytic_generation
            AND p.session_state IN ('none', 'active')")
            .bind(pr).bind(user).fetch_optional(self.pool).await?)
    }
}
