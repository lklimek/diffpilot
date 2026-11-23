//! Admission checks against safe gateway status; usage pauses retain queued jobs.
use contracts::gateway::CredentialState;
use db::{error::DbError, repo::Db};

/// A job requires a linked ChatGPT plan belonging to its payer.
#[derive(Debug, thiserror::Error)]
pub enum PayerError {
    /// No usable connection is stored for this user.
    #[error("Link your Codex account in Settings")]
    Missing,
    /// The connection status could not be read.
    #[error("the payer's connection status could not be read")]
    Db(#[from] DbError),
}

/// Pauses affect dispatch, not admission; missing, linking and relink states reject admission.
pub async fn require_linked(db: &Db, user: &str) -> Result<(), PayerError> {
    if db
        .llm_credentials()
        .status(user)
        .await?
        .is_some_and(|s| s.state == CredentialState::Linked)
    {
        Ok(())
    } else {
        Err(PayerError::Missing)
    }
}
