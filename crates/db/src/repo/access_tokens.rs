//! Repository for `access_tokens`: personal access tokens (ADR-019). Only the SHA-256 of a token is stored.
//! Signatures frozen by WP11-A; bodies are filled in by WP11-TOKENS.

use std::fmt;

use chrono::{DateTime, Utc};
use contracts::api::access_tokens::{AccessTokenId, TokenScope};
use sqlx::SqlitePool;

use super::Db;
use crate::error::DbError;

/// Access to personal access tokens.
#[derive(Debug)]
pub struct AccessTokens<'a> {
    #[expect(dead_code, reason = "used once WP11-TOKENS implements the operations")]
    pool: &'a SqlitePool,
}

impl Db {
    /// Personal access tokens.
    pub fn access_tokens(&self) -> AccessTokens<'_> {
        AccessTokens { pool: &self.pool }
    }
}

/// A token to store; `Debug` never shows the hash.
#[derive(Clone, PartialEq, Eq)]
pub struct NewAccessToken {
    /// Public id (8 base62 characters).
    pub id: AccessTokenId,
    /// The user's name for it.
    pub name: String,
    /// SHA-256 of the whole token `srp_<id>_<secret>`.
    pub token_hash: [u8; 32],
    /// Scopes.
    pub scopes: Vec<TokenScope>,
    /// Expiry.
    pub expires_at: DateTime<Utc>,
}

/// A stored token, without its hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessTokenRecord {
    /// Public id.
    pub id: AccessTokenId,
    /// Tenant.
    pub tenant_id: String,
    /// Owner.
    pub user_id: String,
    /// Name.
    pub name: String,
    /// Scopes.
    pub scopes: Vec<TokenScope>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Expiry.
    pub expires_at: DateTime<Utc>,
    /// Last accepted use.
    pub last_used_at: Option<DateTime<Utc>>,
    /// Revocation time.
    pub revoked_at: Option<DateTime<Utc>>,
}

/// A stored token with its hash, for the request check (constant-time comparison by the caller); `Debug` never
/// shows the hash.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenCredential {
    /// The token.
    pub record: AccessTokenRecord,
    /// SHA-256 of the whole token.
    pub token_hash: [u8; 32],
}

impl fmt::Debug for NewAccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewAccessToken")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("token_hash", &"[redacted]")
            .field("scopes", &self.scopes)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl fmt::Debug for TokenCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenCredential")
            .field("record", &self.record)
            .field("token_hash", &"[redacted]")
            .finish()
    }
}

/// Result of [`AccessTokens::create`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreateOutcome {
    /// Stored.
    Created(AccessTokenRecord),
    /// The user has a token of this name that is not revoked.
    NameTaken,
    /// The user has `CreateAccessToken::MAX_LIVE_PER_USER` live (not revoked, not expired) tokens.
    LimitReached,
}

#[expect(
    unused_variables,
    reason = "signatures frozen by WP11-A; bodies are filled in by WP11-TOKENS"
)]
impl AccessTokens<'_> {
    /// Stores a new token after checking the name and the live-token limit in one write transaction.
    pub async fn create(
        &self,
        tenant_id: &str,
        user_id: &str,
        token: &NewAccessToken,
        now: DateTime<Utc>,
    ) -> Result<CreateOutcome, DbError> {
        Err(DbError::NotImplemented {
            operation: "access_tokens::create",
        })
    }

    /// The user's tokens that are not revoked, newest first (expired ones included until revoked).
    pub async fn list(
        &self,
        tenant_id: &str,
        user_id: &str,
    ) -> Result<Vec<AccessTokenRecord>, DbError> {
        Err(DbError::NotImplemented {
            operation: "access_tokens::list",
        })
    }

    /// Server-internal (request check): the token with this public id in any state, with its hash.
    pub async fn credential(&self, id: &AccessTokenId) -> Result<Option<TokenCredential>, DbError> {
        Err(DbError::NotImplemented {
            operation: "access_tokens::credential",
        })
    }

    /// Records an accepted use; writes only when the stored `last_used_at` is a minute or more before `now`.
    pub async fn touch(&self, id: &AccessTokenId, now: DateTime<Utc>) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "access_tokens::touch",
        })
    }

    /// Revokes one of the user's tokens; `NotFound` for an unknown, revoked or another user's token.
    pub async fn revoke(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: &AccessTokenId,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "access_tokens::revoke",
        })
    }

    /// Revokes every token of the user ("sign out everywhere"); returns how many were live.
    pub async fn revoke_all(
        &self,
        tenant_id: &str,
        user_id: &str,
        now: DateTime<Utc>,
    ) -> Result<u64, DbError> {
        Err(DbError::NotImplemented {
            operation: "access_tokens::revoke_all",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Threat model T20: no token hash in a `{:?}` log line.
    #[test]
    fn debug_never_shows_the_token_hash() {
        let hash = [0xab; 32];
        let new = NewAccessToken {
            id: AccessTokenId("Ab3dE5gH".to_owned()),
            name: "agent".to_owned(),
            token_hash: hash,
            scopes: vec![TokenScope::Read],
            expires_at: DateTime::<Utc>::UNIX_EPOCH,
        };
        let credential = TokenCredential {
            record: AccessTokenRecord {
                id: new.id.clone(),
                tenant_id: "default".to_owned(),
                user_id: "u1".to_owned(),
                name: new.name.clone(),
                scopes: new.scopes.clone(),
                created_at: DateTime::<Utc>::UNIX_EPOCH,
                expires_at: new.expires_at,
                last_used_at: None,
                revoked_at: None,
            },
            token_hash: hash,
        };
        for text in [
            format!("{new:?}"),
            format!("{credential:?}"),
            format!("{credential:#?}"),
        ] {
            assert!(
                text.contains("[redacted]") && text.contains("Ab3dE5gH"),
                "{text}"
            );
            assert!(!text.contains("171, 171"), "hash shown: {text}");
        }
    }
}
