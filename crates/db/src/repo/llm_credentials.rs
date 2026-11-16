//! Repository for `llm_credentials`: credentials runners hold, their pause and usability, and the last LLM
//! call outcome shown in Settings. Frozen signatures (cross-WP); bodies are filled in by WP2-JOBS.

use chrono::{DateTime, Utc};
use contracts::job::result::LlmCallOutcome;
use contracts::job::spec::LlmCredentialRef;
use sqlx::SqlitePool;

use crate::error::DbError;

/// Access to LLM credentials.
#[derive(Debug)]
pub struct LlmCredentials<'a> {
    #[expect(dead_code, reason = "used once WP2-JOBS implements the operations")]
    pub(crate) pool: &'a SqlitePool,
}

/// A stored credential reference with its status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialRecord {
    /// The credential.
    pub credential: LlmCredentialRef,
    /// Paused by a usage limit until then.
    pub paused_until: Option<DateTime<Utc>>,
    /// Unusable since then (refresh failed; the owner must sign in again).
    pub unusable_since: Option<DateTime<Utc>>,
    /// Outcome of the last call made with it.
    pub last_call: Option<LlmCallOutcome>,
}

#[expect(
    unused_variables,
    reason = "signatures frozen by WP1-A; bodies are filled in by WP2-JOBS"
)]
impl LlmCredentials<'_> {
    /// Registers a credential reported by a runner claim, or updates its usability.
    pub async fn register(
        &self,
        tenant_id: &str,
        credential: &LlmCredentialRef,
        usable: bool,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "llm_credentials::register",
        })
    }

    /// A credential.
    pub async fn get(&self, id: &str) -> Result<CredentialRecord, DbError> {
        Err(DbError::NotImplemented {
            operation: "llm_credentials::get",
        })
    }

    /// Records the outcome of a call made with the credential.
    pub async fn record_call(&self, id: &str, outcome: &LlmCallOutcome) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "llm_credentials::record_call",
        })
    }

    /// The most recent call outcome of the tenant over all credentials.
    pub async fn last_call(&self, tenant_id: &str) -> Result<Option<LlmCallOutcome>, DbError> {
        Err(DbError::NotImplemented {
            operation: "llm_credentials::last_call",
        })
    }

    /// Pauses the credential until the usage limit resets.
    pub async fn pause(&self, id: &str, until: DateTime<Utc>) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "llm_credentials::pause",
        })
    }

    /// Marks the credential unusable.
    pub async fn mark_unusable(&self, id: &str, now: DateTime<Utc>) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "llm_credentials::mark_unusable",
        })
    }
}
