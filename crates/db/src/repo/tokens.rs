//! Repository for `user_tokens`: GitHub user access and refresh tokens as AES-GCM ciphertext. `version` is the
//! compare-and-swap counter that keeps a refresh from overwriting a newer pair.

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

use super::Db;
use super::sessions::{parse_timestamp, timestamp};
use crate::error::DbError;

/// Access to stored user tokens.
#[derive(Debug)]
pub struct UserTokens<'a> {
    pool: &'a SqlitePool,
}

impl Db {
    /// Encrypted GitHub user tokens.
    pub fn user_tokens(&self) -> UserTokens<'_> {
        UserTokens { pool: &self.pool }
    }
}

/// An encrypted access and refresh token pair with expiries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptedTokens {
    /// Ciphertext of the access token.
    pub access_ct: Vec<u8>,
    /// Expiry of the access token.
    pub access_expires_at: DateTime<Utc>,
    /// Ciphertext of the refresh token.
    pub refresh_ct: Vec<u8>,
    /// Expiry of the refresh token.
    pub refresh_expires_at: DateTime<Utc>,
}

/// A stored pair and its version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredTokens {
    /// The pair.
    pub tokens: EncryptedTokens,
    /// Compare-and-swap counter; changes with every write.
    pub version: i64,
}

impl UserTokens<'_> {
    /// Stores the pair of a sign-in, replacing any previous pair (the version still advances).
    pub async fn put(
        &self,
        user_id: &str,
        tenant_id: &str,
        tokens: &EncryptedTokens,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO user_tokens
                 (user_id, tenant_id, access_ct, access_expires_at, refresh_ct, refresh_expires_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (user_id) DO UPDATE SET tenant_id = excluded.tenant_id,
                 access_ct = excluded.access_ct, access_expires_at = excluded.access_expires_at,
                 refresh_ct = excluded.refresh_ct, refresh_expires_at = excluded.refresh_expires_at,
                 version = user_tokens.version + 1, updated_at = excluded.updated_at",
        )
        .bind(user_id)
        .bind(tenant_id)
        .bind(&tokens.access_ct)
        .bind(timestamp(tokens.access_expires_at))
        .bind(&tokens.refresh_ct)
        .bind(timestamp(tokens.refresh_expires_at))
        .bind(timestamp(now))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// The user's stored pair, if any.
    pub async fn get(&self, user_id: &str) -> Result<Option<StoredTokens>, DbError> {
        let row = sqlx::query(
            "SELECT access_ct, access_expires_at, refresh_ct, refresh_expires_at, version
             FROM user_tokens WHERE user_id = ?",
        )
        .bind(user_id)
        .fetch_optional(self.pool)
        .await?;
        row.map(|row| {
            let access_expires_at: String = row.try_get("access_expires_at")?;
            let refresh_expires_at: String = row.try_get("refresh_expires_at")?;
            Ok(StoredTokens {
                tokens: EncryptedTokens {
                    access_ct: row.try_get("access_ct")?,
                    access_expires_at: parse_timestamp(
                        &access_expires_at,
                        "user_tokens.access_expires_at",
                    )?,
                    refresh_ct: row.try_get("refresh_ct")?,
                    refresh_expires_at: parse_timestamp(
                        &refresh_expires_at,
                        "user_tokens.refresh_expires_at",
                    )?,
                },
                version: row.try_get("version")?,
            })
        })
        .transpose()
    }

    /// Replaces both tokens in one statement if the stored version is still `expected_version`; false if
    /// another write came first (the stored pair is then left unchanged).
    pub async fn replace(
        &self,
        user_id: &str,
        expected_version: i64,
        tokens: &EncryptedTokens,
        now: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let updated = sqlx::query(
            "UPDATE user_tokens SET access_ct = ?, access_expires_at = ?, refresh_ct = ?,
                 refresh_expires_at = ?, version = version + 1, updated_at = ?
             WHERE user_id = ? AND version = ?",
        )
        .bind(&tokens.access_ct)
        .bind(timestamp(tokens.access_expires_at))
        .bind(&tokens.refresh_ct)
        .bind(timestamp(tokens.refresh_expires_at))
        .bind(timestamp(now))
        .bind(user_id)
        .bind(expected_version)
        .execute(self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    /// Deletes the user's pair; false if there was none.
    pub async fn delete(&self, user_id: &str) -> Result<bool, DbError> {
        let deleted = sqlx::query("DELETE FROM user_tokens WHERE user_id = ?")
            .bind(user_id)
            .execute(self.pool)
            .await?;
        Ok(deleted.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::*;
    use crate::repo::DEFAULT_TENANT;
    use crate::repo::users::GithubProfile;

    fn pair(tag: u8) -> EncryptedTokens {
        let at = DateTime::parse_from_rfc3339("2026-11-16T10:00:00Z")
            .expect("time")
            .with_timezone(&Utc);
        EncryptedTokens {
            access_ct: vec![tag; 4],
            access_expires_at: at + TimeDelta::hours(8),
            refresh_ct: vec![tag; 8],
            refresh_expires_at: at + TimeDelta::days(180),
        }
    }

    #[tokio::test]
    async fn replace_is_a_compare_and_swap_on_the_version() {
        let db = Db::in_memory().await.expect("db");
        let profile = GithubProfile {
            github_id: 42,
            login: "lklimek".to_owned(),
            name: None,
            avatar_url: None,
        };
        db.users()
            .upsert_member(DEFAULT_TENANT, "u-1", &profile, Utc::now())
            .await
            .expect("user");
        let tokens = db.user_tokens();
        assert_eq!(tokens.get("u-1").await.expect("get"), None);
        tokens
            .put("u-1", DEFAULT_TENANT, &pair(1), Utc::now())
            .await
            .expect("put");
        let stored = tokens.get("u-1").await.expect("get").expect("stored");
        assert_eq!(stored.tokens, pair(1));
        assert!(
            tokens
                .replace("u-1", stored.version, &pair(2), Utc::now())
                .await
                .expect("replace")
        );
        assert!(
            !tokens
                .replace("u-1", stored.version, &pair(3), Utc::now())
                .await
                .expect("stale replace"),
            "a stale version must not overwrite the newer pair"
        );
        let current = tokens.get("u-1").await.expect("get").expect("stored");
        assert_eq!(current.tokens, pair(2));
        assert_eq!(current.version, stored.version + 1);
        tokens
            .put("u-1", DEFAULT_TENANT, &pair(4), Utc::now())
            .await
            .expect("put again");
        assert_eq!(
            tokens
                .get("u-1")
                .await
                .expect("get")
                .expect("stored")
                .version,
            current.version + 1
        );
        assert!(tokens.delete("u-1").await.expect("delete"));
        assert_eq!(tokens.get("u-1").await.expect("get"), None);
    }
}
