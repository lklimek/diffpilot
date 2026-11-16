//! Repository for `analyses`. Frozen signatures (cross-WP); bodies are filled in by WP2-JOBS / WP2-INGEST.

use chrono::{DateTime, Utc};
use contracts::api::analyses::AnalysisState;
use contracts::common::LlmLanguage;
use contracts::ids::{AnalysisId, CommitSha};
use sqlx::SqlitePool;

use crate::error::DbError;

/// Access to analyses.
#[derive(Debug)]
pub struct Analyses<'a> {
    #[expect(
        dead_code,
        reason = "used once the owning work package implements the operations"
    )]
    pub(crate) pool: &'a SqlitePool,
}

/// A new analysis in state `planning`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAnalysis {
    /// Analysis id.
    pub id: AnalysisId,
    /// Tenant.
    pub tenant_id: String,
    /// The stored pull request.
    pub pull_request_id: i64,
    /// Base commit.
    pub base_sha: CommitSha,
    /// Head commit.
    pub head_sha: CommitSha,
    /// Language of the LLM texts.
    pub language: LlmLanguage,
    /// User who requested it.
    pub created_by: String,
}

/// A stored analysis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisRecord {
    /// Analysis id.
    pub id: AnalysisId,
    /// Tenant.
    pub tenant_id: String,
    /// The stored pull request.
    pub pull_request_id: i64,
    /// Base commit.
    pub base_sha: CommitSha,
    /// Head commit.
    pub head_sha: CommitSha,
    /// Language of the LLM texts.
    pub language: LlmLanguage,
    /// State and progress.
    pub state: AnalysisState,
    /// Agent session (Codex thread) id, once a job reported one.
    pub agent_thread_id: Option<String>,
    /// Lifecycle of the session files on the runner.
    pub session_state: SessionState,
    /// claudius commit whose skills the agent used.
    pub skills_commit: Option<String>,
    /// User who requested it.
    pub created_by: String,
    /// When it was requested.
    pub created_at: DateTime<Utc>,
    /// When it finished, failed or was cancelled.
    pub finished_at: Option<DateTime<Utc>>,
}

/// Lifecycle of an analysis's agent-session files on the runner (ADR-004).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionState {
    /// No job has run yet.
    None,
    /// The session exists and is resumed by later jobs.
    Active,
    /// Cancelled or superseded: the runner is asked to delete the files.
    DropRequested,
    /// The runner confirmed the deletion.
    Dropped,
}

#[expect(
    unused_variables,
    reason = "signatures frozen by WP1-A; bodies are filled in by the owning work package"
)]
impl Analyses<'_> {
    /// Creates an analysis in state `planning`; `Conflict` if a live one exists for the head and language.
    pub async fn create(
        &self,
        analysis: NewAnalysis,
        now: DateTime<Utc>,
    ) -> Result<AnalysisRecord, DbError> {
        Err(DbError::NotImplemented {
            operation: "analyses::create",
        })
    }

    /// An analysis.
    pub async fn get(&self, id: AnalysisId) -> Result<AnalysisRecord, DbError> {
        Err(DbError::NotImplemented {
            operation: "analyses::get",
        })
    }

    /// The newest analysis of a pull request in a language, any head and state.
    pub async fn latest_for_pr(
        &self,
        pull_request_id: i64,
        language: LlmLanguage,
    ) -> Result<Option<AnalysisRecord>, DbError> {
        Err(DbError::NotImplemented {
            operation: "analyses::latest_for_pr",
        })
    }

    /// Stores a new state (progress included); terminal states set `finished_at`.
    pub async fn set_state(
        &self,
        id: AnalysisId,
        state: &AnalysisState,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "analyses::set_state",
        })
    }

    /// Records the agent session reported by a job and marks it active.
    pub async fn set_session(
        &self,
        id: AnalysisId,
        thread_id: &str,
        skills_commit: Option<&str>,
    ) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "analyses::set_session",
        })
    }

    /// Asks the runner to delete the session files (analysis cancelled or superseded).
    pub async fn request_session_drop(&self, id: AnalysisId) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "analyses::request_session_drop",
        })
    }

    /// Analyses whose session files the runner must delete.
    pub async fn pending_session_drops(&self) -> Result<Vec<AnalysisId>, DbError> {
        Err(DbError::NotImplemented {
            operation: "analyses::pending_session_drops",
        })
    }

    /// Marks session files as deleted, as acknowledged by a runner.
    pub async fn ack_session_drops(&self, ids: &[AnalysisId]) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "analyses::ack_session_drops",
        })
    }
}
