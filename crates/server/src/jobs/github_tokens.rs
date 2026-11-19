//! Lifecycle of minted installation tokens (ADR-005, threat model T3b): AES-GCM copy per (job, attempt) until
//! GitHub confirms the revocation, the single revocation worker, and the lookup for the server-side scrub (T10).

use std::sync::Arc;
use std::time::Duration;

use contracts::ids::JobId;
use contracts::job::access::InstallationToken;
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::Db;
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::Notify;

use crate::config::Config;
use crate::github::app_token::{self, AppTokenError};
use crate::jobs::queue::expire_leases;
use crate::secrets::{Aad, SecretError, TokenCipher};

/// Sweep interval without failures; the first sweep runs at start.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
/// First retry delay after a failed revocation; doubled per failed sweep up to [`MAX_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// Longest wait honoured from GitHub's rate-limit headers.
const MAX_RATE_LIMIT_WAIT: Duration = Duration::from_secs(15 * 60);

/// Wakes the worker; a nudge never waits for a sweep.
static NUDGE: Notify = Notify::const_new();

/// A stored installation token could not be written or read.
#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    /// Encryption or decryption failed.
    #[error("an installation token could not be encrypted or decrypted")]
    Secret(#[from] SecretError),
    /// The queue's storage failed.
    #[error("the installation token store is not available")]
    Db(#[from] DbError),
}

fn row_id(job: JobId, attempt: u32) -> String {
    format!("{job}:{attempt}")
}

/// Associated data binding a ciphertext to its `installation_tokens` row.
fn aad(row_id: &str) -> Aad<'_> {
    Aad {
        table: "installation_tokens",
        column: "ciphertext",
        row_id,
    }
}

/// Stores the token minted for `attempt`, encrypted, until its revocation is confirmed. `token.expires_at` is
/// the revocation deadline: after it the row is only confirmed.
pub async fn store(
    db: &Db,
    config: &Config,
    job: JobId,
    attempt: u32,
    token: &InstallationToken,
) -> Result<(), TokenStoreError> {
    let row = row_id(job, attempt);
    let ciphertext =
        TokenCipher::new(&config.secret_key).encrypt(aad(&row), token.token.expose())?;
    db.jobs()
        .record_github_token(job, attempt, &ciphertext, token.expires_at)
        .await?;
    Ok(())
}

/// The token stored for `attempt`, decrypted; for scrubbing runner data (T10).
pub async fn load(
    db: &Db,
    config: &Config,
    job: JobId,
    attempt: u32,
) -> Result<Option<SecretString>, TokenStoreError> {
    let Some(ciphertext) = db.jobs().github_token(job, attempt).await? else {
        return Ok(None);
    };
    let row = row_id(job, attempt);
    Ok(Some(
        TokenCipher::new(&config.secret_key).decrypt(aad(&row), &ciphertext)?,
    ))
}

/// Asks the worker for a sweep soon (after a result or an expired lease) without waiting for it.
pub fn nudge() {
    NUDGE.notify_waiters();
}

/// Outcome of one sweep.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sweep {
    /// Tokens revoked or, being expired, confirmed.
    pub confirmed: usize,
    /// Revocations that failed and stay listed.
    pub failed: usize,
    /// GitHub limited the rate; the sweep stopped early. The wait GitHub asked for, if it said.
    pub rate_limited: Option<Option<Duration>>,
}

/// One pass: ends overdue leases (no runner, follower or claim may be left to do it), then revokes the tokens of
/// attempts that are no longer claimed or running, soonest expiry first; an expired token is only confirmed.
/// Stops at a rate limit. Each token is retried by later sweeps until confirmed or expired.
pub async fn sweep(db: &Db, config: &Config, now: Timestamp) -> Result<Sweep, DbError> {
    expire_leases(db, now).await?;
    let pending = db.jobs().github_tokens_to_revoke().await?;
    let mut outcome = Sweep::default();
    if pending.is_empty() {
        return Ok(outcome);
    }
    let cipher = TokenCipher::new(&config.secret_key);
    for stale in pending {
        if stale.expires_at > now {
            let row = row_id(stale.job, stale.attempt);
            let token = match cipher.decrypt(aad(&row), &stale.ciphertext) {
                Ok(token) => token,
                Err(error) => {
                    tracing::error!(
                        job = %stale.job,
                        attempt = stale.attempt,
                        %error,
                        "stored installation token unreadable; it stays listed until it expires"
                    );
                    continue;
                }
            };
            match app_token::revoke(&config.github, token.expose_secret()).await {
                Ok(()) => {}
                Err(AppTokenError::RateLimited { retry_after }) => {
                    tracing::warn!(
                        job = %stale.job,
                        attempt = stale.attempt,
                        "GitHub rate-limited installation token revocation; the worker waits before retrying"
                    );
                    outcome.failed += 1;
                    outcome.rate_limited = Some(retry_after);
                    break;
                }
                Err(error) => {
                    tracing::warn!(
                        job = %stale.job,
                        attempt = stale.attempt,
                        %error,
                        "installation token revocation failed; the worker retries it"
                    );
                    outcome.failed += 1;
                    continue;
                }
            }
        }
        db.jobs()
            .confirm_github_token_revoked(stale.job, stale.attempt)
            .await?;
        outcome.confirmed += 1;
    }
    tracing::info!(
        confirmed = outcome.confirmed,
        failed = outcome.failed,
        "installation token revocation sweep finished"
    );
    Ok(outcome)
}

/// How long the worker waits after a sweep, and whether a nudge may cut the wait short (not while backing off).
fn next_wait(backoff: &mut Option<Duration>, outcome: &Result<Sweep, DbError>) -> (Duration, bool) {
    let failed = match outcome {
        Ok(sweep) => sweep.failed > 0,
        Err(_) => true,
    };
    if !failed {
        *backoff = None;
        return (SWEEP_INTERVAL, true);
    }
    let delay = backoff.map_or(FIRST_BACKOFF, |previous| (previous * 2).min(MAX_BACKOFF));
    *backoff = Some(delay);
    let asked = match outcome {
        Ok(Sweep {
            rate_limited: Some(asked),
            ..
        }) => asked.unwrap_or(MAX_BACKOFF).min(MAX_RATE_LIMIT_WAIT),
        _ => Duration::ZERO,
    };
    (delay.max(asked), false)
}

/// The single revocation worker: sweeps at start, when nudged and every minute; after failures it backs off
/// exponentially and honours GitHub's rate-limit wait. Claims and results only nudge it.
pub async fn run_worker(db: Db, config: Arc<Config>) {
    let mut backoff = None;
    loop {
        let nudged = NUDGE.notified();
        tokio::pin!(nudged);
        // Registered before the sweep, so a nudge sent during it is not lost.
        nudged.as_mut().enable();
        let outcome = sweep(&db, &config, Timestamp::now()).await;
        if let Err(error) = &outcome {
            tracing::warn!(%error, "installation token sweep could not read the queue; the worker retries");
        }
        let (wait, interruptible) = next_wait(&mut backoff, &outcome);
        if interruptible {
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = &mut nudged => {}
            }
        } else {
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(rate_limited: Option<Option<Duration>>) -> Result<Sweep, DbError> {
        Ok(Sweep {
            confirmed: 0,
            failed: 1,
            rate_limited,
        })
    }

    #[test]
    fn worker_backs_off_after_failures_and_honours_rate_limits() {
        let mut backoff = None;
        assert_eq!(
            next_wait(&mut backoff, &Ok(Sweep::default())),
            (SWEEP_INTERVAL, true)
        );
        let waits: Vec<(Duration, bool)> = (0..8)
            .map(|_| next_wait(&mut backoff, &failed(None)))
            .collect();
        assert_eq!(
            waits[0],
            (FIRST_BACKOFF, false),
            "nudges ignored while backing off"
        );
        assert_eq!(waits[1].0, FIRST_BACKOFF * 2);
        assert_eq!(waits[7].0, MAX_BACKOFF, "bounded");
        assert_eq!(
            next_wait(&mut backoff, &Err(DbError::Conflict)).0,
            MAX_BACKOFF
        );
        assert_eq!(
            next_wait(&mut backoff, &Ok(Sweep::default())),
            (SWEEP_INTERVAL, true),
            "success resets the backoff"
        );
        assert_eq!(
            next_wait(&mut backoff, &failed(Some(Some(Duration::from_secs(120))))).0,
            Duration::from_secs(120),
            "GitHub's retry-after wins over a shorter backoff"
        );
        assert_eq!(
            next_wait(
                &mut backoff,
                &failed(Some(Some(Duration::from_secs(86_400))))
            )
            .0,
            MAX_RATE_LIMIT_WAIT,
            "but is capped"
        );
        assert_eq!(
            next_wait(&mut backoff, &failed(Some(None))).0,
            MAX_BACKOFF,
            "a rate limit without a hint waits the longest backoff"
        );
    }
}
