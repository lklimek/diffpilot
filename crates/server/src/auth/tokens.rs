//! GitHub user tokens at rest (AES-GCM, bound to their row) and their serialised refresh (ADR-007, S4).
//!
//! A refresh rotates both tokens and kills the old pair at once. Every change of a user's sign-in state (sign-in
//! store and session creation, refresh, end of sign-in) runs under that user's [`UserLock`] (in-process: one
//! server process owns the database); the rotated pair is stored with a version compare-and-swap before the new
//! access token is used, and a rejected refresh ends only the pair it was made for.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use tokio::sync::OwnedMutexGuard;

use chrono::{DateTime, TimeDelta, Utc};
use contracts::api::error::ApiErrorCode;
use db::error::DbError;
use db::repo::tokens::{EncryptedTokens, PairCondition, StoredTokens};
use secrecy::{ExposeSecret, SecretString};

use crate::api::error::ApiErrorSource;
use crate::github::oauth::{self, OauthError, TokenGrant};
use crate::secrets::{Aad, SecretError, TokenCipher};
use crate::state::AppState;

/// An access token expiring within this margin is refreshed before use.
const REFRESH_MARGIN: TimeDelta = TimeDelta::minutes(5);
const TABLE: &str = "user_tokens";
/// OAuth error code for a refresh token that is unknown, used or expired.
const BAD_REFRESH_TOKEN: &str = "bad_refresh_token";

/// Per-user locks; one entry per user since start (a handful of users).
static USER_LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Mutex::default);

/// Proof that the caller holds the user's sign-in lock; released on drop.
pub struct UserLock {
    user_id: String,
    _guard: OwnedMutexGuard<()>,
}

impl std::fmt::Debug for UserLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserLock")
            .field("user_id", &self.user_id)
            .finish_non_exhaustive()
    }
}

/// Waits for the user's sign-in lock. Never call [`end_sign_in`] or [`user_access_token`] while holding it.
pub async fn lock_user(user_id: &str) -> UserLock {
    let lock = {
        let mut locks = USER_LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(locks.entry(user_id.to_owned()).or_default())
    };
    UserLock {
        user_id: user_id.to_owned(),
        _guard: lock.lock_owned().await,
    }
}

fn access_aad(user_id: &str) -> Aad<'_> {
    Aad {
        table: TABLE,
        column: "access_ct",
        row_id: user_id,
    }
}

fn refresh_aad(user_id: &str) -> Aad<'_> {
    Aad {
        table: TABLE,
        column: "refresh_ct",
        row_id: user_id,
    }
}

/// The user's GitHub token could not be provided.
#[derive(Debug, thiserror::Error)]
pub enum UserTokenError {
    /// No usable token: never stored, refresh token expired or rejected; the user must sign in again.
    #[error("the GitHub sign-in has ended; the user must sign in again")]
    SignInRequired,
    /// Another write replaced the token pair during this refresh.
    #[error("the GitHub token pair changed during the refresh")]
    Superseded,
    /// GitHub did not complete the refresh.
    #[error("refreshing the GitHub user token failed")]
    Github(#[from] OauthError),
    /// Encryption or decryption failed.
    #[error("the stored GitHub token could not be read or written")]
    Secret(#[from] SecretError),
    /// The token store failed.
    #[error("the GitHub token store failed")]
    Db(#[from] DbError),
}

impl ApiErrorSource for UserTokenError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::SignInRequired => ApiErrorCode::Unauthenticated,
            Self::Superseded => ApiErrorCode::Conflict,
            Self::Github(_) => ApiErrorCode::GithubUnavailable,
            Self::Secret(_) => ApiErrorCode::Internal,
            Self::Db(error) => error.code(),
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::SignInRequired => Cow::Borrowed("Your GitHub sign-in has ended. Sign in again."),
            Self::Superseded => Cow::Borrowed("Your GitHub sign-in changed meanwhile. Try again."),
            Self::Github(_) => Cow::Borrowed("GitHub did not answer as expected. Try again later."),
            Self::Secret(_) => {
                Cow::Borrowed("Something went wrong on the server. Try again later.")
            }
            Self::Db(error) => error.public_message(),
        }
    }
}

fn encrypt(
    cipher: &TokenCipher,
    user_id: &str,
    grant: &TokenGrant,
    now: DateTime<Utc>,
) -> Result<EncryptedTokens, UserTokenError> {
    Ok(EncryptedTokens {
        access_ct: cipher.encrypt(access_aad(user_id), grant.access_token.expose_secret())?,
        access_expires_at: now + TimeDelta::seconds(grant.expires_in),
        refresh_ct: cipher.encrypt(refresh_aad(user_id), grant.refresh_token.expose_secret())?,
        refresh_expires_at: now + TimeDelta::seconds(grant.refresh_token_expires_in),
    })
}

/// Stores the token pair of a sign-in for the locked user, replacing the previous pair.
pub async fn store_sign_in_tokens(
    state: &AppState,
    lock: &UserLock,
    tenant_id: &str,
    grant: &TokenGrant,
    now: DateTime<Utc>,
) -> Result<(), UserTokenError> {
    let cipher = TokenCipher::new(&state.config.secret_key);
    let tokens = encrypt(&cipher, &lock.user_id, grant, now)?;
    state
        .db
        .user_tokens()
        .put(&lock.user_id, tenant_id, &tokens, now)
        .await?;
    Ok(())
}

/// Ends the user's sign-in (sign-out everywhere): deletes the token pair and every session in one transaction.
pub async fn end_sign_in(state: &AppState, user_id: &str) -> Result<(), DbError> {
    let _lock = lock_user(user_id).await;
    state
        .db
        .user_tokens()
        .end_sign_in(user_id, PairCondition::Any)
        .await?;
    Ok(())
}

fn fresh_access(
    cipher: &TokenCipher,
    user_id: &str,
    stored: &StoredTokens,
    now: DateTime<Utc>,
) -> Result<Option<SecretString>, UserTokenError> {
    if stored.tokens.access_expires_at - now > REFRESH_MARGIN {
        Ok(Some(
            cipher.decrypt(access_aad(user_id), &stored.tokens.access_ct)?,
        ))
    } else {
        Ok(None)
    }
}

/// The user's GitHub access token, refreshed first when it expires within five minutes. A rejected or
/// expired refresh token ends the sign-in ([`UserTokenError::SignInRequired`]).
pub async fn user_access_token(
    state: &AppState,
    user_id: &str,
) -> Result<SecretString, UserTokenError> {
    let cipher = TokenCipher::new(&state.config.secret_key);
    let tokens = state.db.user_tokens();
    if let Some(stored) = tokens.get(user_id).await?
        && let Some(access) = fresh_access(&cipher, user_id, &stored, Utc::now())?
    {
        return Ok(access);
    }

    let _lock = lock_user(user_id).await;
    // Another task may have refreshed or ended the sign-in while this one waited.
    let Some(stored) = tokens.get(user_id).await? else {
        // Sessions left without tokens (e.g. by an interrupted sign-out) end here.
        tokens.end_sign_in(user_id, PairCondition::Absent).await?;
        return Err(UserTokenError::SignInRequired);
    };
    let now = Utc::now();
    if let Some(access) = fresh_access(&cipher, user_id, &stored, now)? {
        return Ok(access);
    }
    let this_pair = PairCondition::Version(stored.version);
    if stored.tokens.refresh_expires_at <= now {
        tracing::info!(
            user_id,
            "GitHub refresh token expired; the user must sign in again"
        );
        tokens.end_sign_in(user_id, this_pair).await?;
        return Err(UserTokenError::SignInRequired);
    }
    let refresh_token = cipher.decrypt(refresh_aad(user_id), &stored.tokens.refresh_ct)?;
    let grant = match oauth::refresh(&state.config.github, &refresh_token).await {
        Ok(grant) => grant,
        Err(OauthError::Rejected { error }) if error == BAD_REFRESH_TOKEN => {
            tracing::warn!(
                user_id,
                "GitHub rejected the refresh token; ending the sign-in"
            );
            tokens.end_sign_in(user_id, this_pair).await?;
            return Err(UserTokenError::SignInRequired);
        }
        Err(error) => return Err(error.into()),
    };
    let now = Utc::now();
    let rotated = encrypt(&cipher, user_id, &grant, now)?;
    // GitHub has already invalidated the old pair: store the new one before using it.
    if !tokens
        .replace(user_id, stored.version, &rotated, now)
        .await?
    {
        return Err(UserTokenError::Superseded);
    }
    tracing::debug!(user_id, "GitHub user token refreshed");
    Ok(grant.access_token)
}
