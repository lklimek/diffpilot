//! Job claiming for runners: job tokens, lease, credentials and session drops (`runner-protocol.md` Claim).

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, TimeDelta, Utc};
use contracts::job::access::{AgentAccess, Secret};
use contracts::job::protocol::{Claim, ClaimRequest, ClaimResponse, LeaseTiming};
use db::error::DbError;
use db::repo::jobs::ClaimParams;
use db::repo::{DEFAULT_TENANT, Db};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::jobs::queue::expire_leases;

/// Random bytes of a job token.
const TOKEN_BYTES: usize = 32;

/// A claim could not be served.
#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    /// The job queue's storage failed.
    #[error("the job queue is not available")]
    Db(#[from] DbError),
    /// The operating system's random source failed.
    #[error("could not generate a job token")]
    Random(#[source] getrandom::Error),
}

/// SHA-256 of a bearer value; only this hash of a job token is stored.
pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Compares two token hashes in constant time.
pub fn hashes_match(stored: &[u8], presented: &[u8]) -> bool {
    stored.ct_eq(presented).into()
}

/// A new random job token and its hash.
fn mint_token() -> Result<(Secret, Vec<u8>), ClaimError> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(ClaimError::Random)?;
    let token = URL_SAFE_NO_PAD.encode(bytes);
    let hash = token_hash(&token);
    Ok((Secret::new(token), hash))
}

/// The server-side lease end for a request received at `now` ([`LeaseTiming::V1`]).
pub fn lease_until(now: DateTime<Utc>) -> DateTime<Utc> {
    now + TimeDelta::seconds(i64::from(LeaseTiming::V1.lease_secs))
}

/// Serves one runner claim: housekeeping, credential registration, then the next runnable job, if any.
pub async fn claim(
    db: &Db,
    request: &ClaimRequest,
    now: DateTime<Utc>,
) -> Result<ClaimResponse, ClaimError> {
    expire_leases(db, now).await?;
    let resumed = db.jobs().resume_paused(now).await?;
    if !resumed.is_empty() {
        tracing::info!(
            jobs = resumed.len(),
            "paused jobs queued again after the usage limit reset"
        );
    }
    db.analyses()
        .ack_session_drops(&request.dropped_sessions)
        .await?;
    let mut usable = Vec::with_capacity(request.credentials.len());
    for status in &request.credentials {
        db.llm_credentials()
            .register(DEFAULT_TENANT, &status.credential, status.usable, now)
            .await?;
        if status.usable {
            usable.push(status.credential.id.clone());
        }
    }
    let drop_sessions = db.analyses().pending_session_drops().await?;
    let (token, hash) = mint_token()?;
    let lease_expires_at = lease_until(now);
    let claimed = db
        .jobs()
        .claim(ClaimParams {
            credential_ids: &usable,
            job_token_hash: &hash,
            lease_until: lease_expires_at,
            now,
        })
        .await?;
    let claim = claimed.map(|job| {
        tracing::info!(
            job = %job.id,
            attempt = job.attempt,
            runner = %request.runner_id,
            "runner claimed a job"
        );
        let mut spec = job.spec;
        // TODO(WP3-APPTOKEN): mint the read-only installation token for the PR repository here, store it with
        // `Jobs::record_github_token` and set `access.github`; the stage-2 slice runs without it.
        // TODO(WP3-RUN): fill `access.memcan` from `Config::memcan`; the stage-2 slice runs without MemCan.
        spec.access = AgentAccess::default();
        Claim {
            job_token: token,
            lease_expires_at,
            spec,
        }
    });
    Ok(ClaimResponse {
        claim,
        drop_sessions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_and_only_their_hash_matches() {
        let (first, first_hash) = mint_token().expect("token");
        let (second, _) = mint_token().expect("token");
        assert_ne!(first.expose(), second.expose());
        assert_eq!(
            first.expose().len(),
            43,
            "32 bytes, base64url without padding"
        );
        assert_eq!(first_hash, token_hash(first.expose()));
        assert!(hashes_match(&first_hash, &token_hash(first.expose())));
        assert!(!hashes_match(&first_hash, &token_hash(second.expose())));
        assert!(!hashes_match(&first_hash, &[]));
    }

    #[test]
    fn lease_uses_the_v1_timing() {
        let now = Utc::now();
        assert_eq!(
            lease_until(now) - now,
            TimeDelta::seconds(i64::from(LeaseTiming::V1.lease_secs))
        );
    }
}
