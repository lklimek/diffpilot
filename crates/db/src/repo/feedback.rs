//! Repository for `feedback`. Frozen signatures (cross-WP: WP3-REVIEW records `missed_by_llm`, WP3-MEMCAN the
//! rest); bodies are filled in by WP3-MEMCAN.

use chrono::{DateTime, Utc};
use contracts::api::feedback::{Feedback, FeedbackTarget, FeedbackValue};
use contracts::ids::{AnalysisId, FeedbackId};
use sqlx::SqlitePool;

use crate::error::DbError;

/// Access to feedback.
#[derive(Debug)]
pub struct FeedbackRepo<'a> {
    #[expect(dead_code, reason = "used once WP3-MEMCAN implements the operations")]
    pub(crate) pool: &'a SqlitePool,
}

/// Feedback to record; replaces the user's earlier entry for the same target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewFeedback {
    /// Feedback id (kept when replacing).
    pub id: FeedbackId,
    /// Analysis.
    pub analysis_id: AnalysisId,
    /// User.
    pub user_id: String,
    /// Target.
    pub target: FeedbackTarget,
    /// Value.
    pub value: FeedbackValue,
    /// Optional note.
    pub note: Option<String>,
}

/// State of the MemCan write of a feedback entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemcanState {
    /// Not written yet.
    Pending,
    /// Written.
    Written,
    /// The write failed; retried later.
    Failed,
}

#[expect(
    unused_variables,
    reason = "signatures frozen by WP1-A; bodies are filled in by WP3-MEMCAN"
)]
impl FeedbackRepo<'_> {
    /// Inserts or replaces the user's feedback for the target; resets the MemCan state to pending.
    pub async fn upsert(
        &self,
        feedback: NewFeedback,
        now: DateTime<Utc>,
    ) -> Result<Feedback, DbError> {
        Err(DbError::NotImplemented {
            operation: "feedback::upsert",
        })
    }

    /// The user's feedback on an analysis.
    pub async fn list(
        &self,
        analysis: AnalysisId,
        user_id: &str,
    ) -> Result<Vec<Feedback>, DbError> {
        Err(DbError::NotImplemented {
            operation: "feedback::list",
        })
    }

    /// Entries not yet written to MemCan (pending or failed), oldest first.
    pub async fn pending_memcan(&self, limit: u32) -> Result<Vec<(AnalysisId, Feedback)>, DbError> {
        Err(DbError::NotImplemented {
            operation: "feedback::pending_memcan",
        })
    }

    /// Records the outcome of the MemCan write.
    pub async fn set_memcan_state(
        &self,
        id: FeedbackId,
        state: MemcanState,
    ) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "feedback::set_memcan_state",
        })
    }
}
