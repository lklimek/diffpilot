//! Container-owned analytic checkpoints and durable, fenced first-Ask preflight.

use super::{
    Db,
    jobs::{parse_wire, wire_name},
};
use crate::error::DbError;
use contracts::{
    ids::UserId,
    runtime::{AskPreflight, LastContext, PreflightState, ThreadKind},
};
use sqlx::{Row, SqlitePool};

/// Session metadata shared by the pipeline and Ask without exposing SQL.
#[derive(Debug)]
pub struct ContainerSessions<'a> {
    pool: &'a SqlitePool,
}

impl Db {
    /// Per-payer analytic checkpoints and private working seeds.
    pub fn container_sessions(&self) -> ContainerSessions<'_> {
        ContainerSessions { pool: &self.pool }
    }
}

/// Analytic checkpoint; context metadata is valid only for this generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalyticThread {
    /// Provider-local opaque thread id.
    pub thread_id: String,
    /// Server-owned generation, advanced when the analysis head changes.
    pub generation: i64,
    /// Last completed turn's context estimate; absence requires recovery.
    pub last_context: Option<LastContext>,
}

/// Private working seed and its first-Ask state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingThread {
    /// Opaque thread id, scoped to this PR and user.
    pub thread_id: String,
    /// Immutable Ask or verify policy.
    pub kind: ThreadKind,
    /// Seed and generation bound to this working thread.
    pub preflight: AskPreflight,
}

impl ContainerSessions<'_> {
    /// Reads an active analytic checkpoint for this payer alone.
    pub async fn analytic(
        &self,
        pr: i64,
        user: &UserId,
    ) -> Result<Option<AnalyticThread>, DbError> {
        let row = sqlx::query("SELECT thread_id, generation, last_context_tokens, last_context_generation FROM analytic_threads WHERE pull_request_id=? AND user_id=? AND state='active'")
            .bind(pr).bind(&user.0).fetch_optional(self.pool).await?;
        row.map(|r| {
            Ok(AnalyticThread {
                thread_id: r.try_get("thread_id")?,
                generation: r.try_get("generation")?,
                last_context: r
                    .try_get::<Option<i64>, _>("last_context_tokens")?
                    .map(|tokens| {
                        Ok::<_, DbError>(LastContext {
                            tokens: u64::try_from(tokens).map_err(|_| DbError::Corrupt {
                                column: "analytic_threads.last_context_tokens",
                            })?,
                            generation: r.try_get("last_context_generation")?,
                        })
                    })
                    .transpose()?,
            })
        })
        .transpose()
    }

    /// Publishes a checkpoint with generation CAS; retirement and stale writers cannot resurrect it.
    pub async fn save_analytic(
        &self,
        pr: i64,
        user: &UserId,
        expected: i64,
        thread: &AnalyticThread,
    ) -> Result<(), DbError> {
        if thread.generation < expected
            || thread
                .last_context
                .as_ref()
                .is_some_and(|c| c.generation != thread.generation)
        {
            return Err(DbError::Conflict);
        }
        let mut tx = super::write_tx(self.pool).await?;
        let changed = sqlx::query("INSERT INTO analytic_threads (tenant_id,pull_request_id,user_id,thread_id,generation,state,last_context_tokens,last_context_generation)
            SELECT (SELECT tenant_id FROM pull_requests WHERE id=?),?,?,?,?,'active',?,? WHERE ?=0 OR EXISTS(SELECT 1 FROM analytic_threads WHERE pull_request_id=? AND user_id=?)
            ON CONFLICT(pull_request_id,user_id) DO UPDATE SET thread_id=excluded.thread_id,generation=excluded.generation,state='active',last_context_tokens=excluded.last_context_tokens,last_context_generation=excluded.last_context_generation
            WHERE analytic_threads.generation=? AND analytic_threads.state IN ('none','active')")
            .bind(pr).bind(pr).bind(&user.0).bind(&thread.thread_id).bind(thread.generation)
            .bind(thread.last_context.as_ref().map(|c| i64::try_from(c.tokens)).transpose().map_err(|_| DbError::Conflict)?)
            .bind(thread.last_context.as_ref().map(|c| c.generation)).bind(expected).bind(pr).bind(&user.0).bind(expected)
            .execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(DbError::Conflict);
        }
        tx.commit().await?;
        Ok(())
    }

    /// Reads a private seed; callers compare its generation with the chosen analytic checkpoint.
    pub async fn working(
        &self,
        pr: i64,
        user: &UserId,
        kind: ThreadKind,
    ) -> Result<Option<WorkingThread>, DbError> {
        let row = sqlx::query("SELECT agent_thread_id,thread_kind,seed,seed_generation,preflight_state FROM working_sessions WHERE pull_request_id=? AND user_id=? AND thread_kind=?")
            .bind(pr).bind(&user.0).bind(wire_name(kind)?).fetch_optional(self.pool).await?;
        row.map(|r| {
            Ok(WorkingThread {
                thread_id: r.try_get("agent_thread_id")?,
                kind: parse_wire(
                    &r.try_get::<String, _>("thread_kind")?,
                    "working_sessions.thread_kind",
                )?,
                preflight: AskPreflight {
                    seed: parse_wire(&r.try_get::<String, _>("seed")?, "working_sessions.seed")?,
                    generation: r.try_get("seed_generation")?,
                    state: parse_wire(
                        &r.try_get::<String, _>("preflight_state")?,
                        "working_sessions.preflight_state",
                    )?,
                },
            })
        })
        .transpose()
    }

    /// Installs a new working seed; the container lease must be held through preflight and answer.
    pub async fn seed_working(
        &self,
        pr: i64,
        user: &UserId,
        previous_thread: Option<&str>,
        thread: &WorkingThread,
    ) -> Result<(), DbError> {
        if !matches!(thread.kind, ThreadKind::Ask | ThreadKind::Verify) {
            return Err(DbError::Conflict);
        }
        let mut tx = super::write_tx(self.pool).await?;
        let changed = sqlx::query("INSERT INTO working_sessions (tenant_id,pull_request_id,user_id,agent_thread_id,analytic_generation,seed,thread_kind,preflight_state,seed_generation)
            SELECT tenant_id,id,?,?,?,?,?,?,? FROM pull_requests WHERE id=? AND (? IS NULL OR EXISTS(SELECT 1 FROM working_sessions WHERE pull_request_id=? AND user_id=? AND thread_kind=? AND agent_thread_id=?))
            ON CONFLICT(pull_request_id,user_id,thread_kind) DO UPDATE SET agent_thread_id=excluded.agent_thread_id,analytic_generation=excluded.analytic_generation,
                seed=excluded.seed,thread_kind=excluded.thread_kind,
                preflight_state=CASE WHEN working_sessions.agent_thread_id=excluded.agent_thread_id THEN working_sessions.preflight_state ELSE excluded.preflight_state END,
                seed_generation=excluded.seed_generation
            WHERE working_sessions.agent_thread_id=? AND working_sessions.analytic_generation<=excluded.analytic_generation
                AND (working_sessions.agent_thread_id<>excluded.agent_thread_id OR
                    (working_sessions.thread_kind=excluded.thread_kind AND working_sessions.seed=excluded.seed AND working_sessions.seed_generation=excluded.seed_generation))")
            .bind(&user.0).bind(&thread.thread_id).bind(thread.preflight.generation).bind(wire_name(thread.preflight.seed)?)
            .bind(wire_name(thread.kind)?).bind(wire_name(thread.preflight.state)?).bind(thread.preflight.generation).bind(pr)
            .bind(previous_thread).bind(pr).bind(&user.0).bind(wire_name(thread.kind)?).bind(previous_thread).bind(previous_thread)
            .execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(DbError::Conflict);
        }
        tx.commit().await?;
        Ok(())
    }

    /// Acknowledges compaction only for the still-current thread, seed and generation.
    pub async fn complete_preflight(
        &self,
        pr: i64,
        user: &UserId,
        thread: &WorkingThread,
    ) -> Result<(), DbError> {
        if thread.preflight.state != PreflightState::Pending {
            return Err(DbError::Conflict);
        }
        let mut tx = super::write_tx(self.pool).await?;
        let changed = sqlx::query("UPDATE working_sessions SET preflight_state='done' WHERE pull_request_id=? AND user_id=? AND agent_thread_id=? AND seed=? AND seed_generation=? AND thread_kind=? AND preflight_state='pending'")
            .bind(pr).bind(&user.0).bind(&thread.thread_id).bind(wire_name(thread.preflight.seed)?).bind(thread.preflight.generation).bind(wire_name(thread.kind)?)
            .execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(DbError::Conflict);
        }
        tx.commit().await?;
        Ok(())
    }
}
