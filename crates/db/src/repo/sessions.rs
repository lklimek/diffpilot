//! Repository for cookie `sessions` (only the hash of the session token is stored) and the OAuth logins in
//! progress (`oauth_logins`: state and encrypted PKCE verifier).

use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::{Row, SqlitePool};

use super::Db;
use crate::error::DbError;

/// Access to sessions and OAuth logins in progress.
#[derive(Debug)]
pub struct Sessions<'a> {
    pool: &'a SqlitePool,
}

impl Db {
    /// Cookie sessions and OAuth logins in progress.
    pub fn sessions(&self) -> Sessions<'_> {
        Sessions { pool: &self.pool }
    }
}

/// A session to create.
#[derive(Clone, Debug)]
pub struct NewSession<'a> {
    /// Hash of the session token held by the browser.
    pub token_hash: &'a [u8],
    /// Tenant.
    pub tenant_id: &'a str,
    /// Signed-in user.
    pub user_id: &'a str,
    /// CSRF token for mutating requests.
    pub csrf_token: &'a str,
    /// End of the session.
    pub expires_at: DateTime<Utc>,
}

/// A live session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRecord {
    /// Tenant.
    pub tenant_id: String,
    /// Signed-in user.
    pub user_id: String,
    /// CSRF token for mutating requests.
    pub csrf_token: String,
    /// End of the session.
    pub expires_at: DateTime<Utc>,
}

/// An OAuth login taken by its callback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OauthLoginRecord {
    /// Tenant the login was started for.
    pub tenant_id: String,
    /// Encrypted PKCE code verifier.
    pub code_verifier_ct: Vec<u8>,
}

/// Fixed-width RFC 3339 UTC text, so stored timestamps compare correctly as text in SQL.
pub(super) fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Parses a stored timestamp of `column`.
pub(super) fn parse_timestamp(text: &str, column: &'static str) -> Result<DateTime<Utc>, DbError> {
    DateTime::parse_from_rfc3339(text)
        .map(|at| at.with_timezone(&Utc))
        .map_err(|_| DbError::Corrupt { column })
}

impl Sessions<'_> {
    /// Stores a new session.
    pub async fn create(
        &self,
        session: &NewSession<'_>,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        let now = timestamp(now);
        sqlx::query(
            "INSERT INTO sessions (token_hash, tenant_id, user_id, csrf_token, created_at, last_seen_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(session.token_hash)
        .bind(session.tenant_id)
        .bind(session.user_id)
        .bind(session.csrf_token)
        .bind(&now)
        .bind(&now)
        .bind(timestamp(session.expires_at))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// The unexpired session with this token hash, marked as seen at `now`; `None` if absent or expired.
    pub async fn touch(
        &self,
        token_hash: &[u8],
        now: DateTime<Utc>,
    ) -> Result<Option<SessionRecord>, DbError> {
        let now = timestamp(now);
        let row = sqlx::query(
            "UPDATE sessions SET last_seen_at = ? WHERE token_hash = ? AND expires_at > ?
             RETURNING tenant_id, user_id, csrf_token, expires_at",
        )
        .bind(&now)
        .bind(token_hash)
        .bind(&now)
        .fetch_optional(self.pool)
        .await?;
        row.map(|row| {
            let expires_at: String = row.try_get("expires_at")?;
            Ok(SessionRecord {
                tenant_id: row.try_get("tenant_id")?,
                user_id: row.try_get("user_id")?,
                csrf_token: row.try_get("csrf_token")?,
                expires_at: parse_timestamp(&expires_at, "sessions.expires_at")?,
            })
        })
        .transpose()
    }

    /// Deletes one session; false if it did not exist.
    pub async fn delete(&self, token_hash: &[u8]) -> Result<bool, DbError> {
        let deleted = sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(token_hash)
            .execute(self.pool)
            .await?;
        Ok(deleted.rows_affected() > 0)
    }

    /// Deletes every session of the user; returns how many.
    pub async fn delete_for_user(&self, user_id: &str) -> Result<u64, DbError> {
        let deleted = sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(user_id)
            .execute(self.pool)
            .await?;
        Ok(deleted.rows_affected())
    }

    /// Stores an OAuth login in progress under its `state`.
    pub async fn create_oauth_login(
        &self,
        state: &str,
        tenant_id: &str,
        code_verifier_ct: &[u8],
        now: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO oauth_logins (state, tenant_id, code_verifier_ct, created_at, expires_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(state)
        .bind(tenant_id)
        .bind(code_verifier_ct)
        .bind(timestamp(now))
        .bind(timestamp(expires_at))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Removes the OAuth login of `state` and returns it if it had not expired; a state is usable once.
    pub async fn take_oauth_login(
        &self,
        state: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<OauthLoginRecord>, DbError> {
        let row = sqlx::query(
            "DELETE FROM oauth_logins WHERE state = ? RETURNING tenant_id, code_verifier_ct, expires_at",
        )
        .bind(state)
        .fetch_optional(self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let expires_at: String = row.try_get("expires_at")?;
        if parse_timestamp(&expires_at, "oauth_logins.expires_at")? <= now {
            return Ok(None);
        }
        Ok(Some(OauthLoginRecord {
            tenant_id: row.try_get("tenant_id")?,
            code_verifier_ct: row.try_get("code_verifier_ct")?,
        }))
    }

    /// Deletes expired sessions and OAuth logins.
    pub async fn delete_expired(&self, now: DateTime<Utc>) -> Result<(), DbError> {
        let now = timestamp(now);
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM oauth_logins WHERE expires_at <= ?")
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::*;
    use crate::repo::DEFAULT_TENANT;
    use crate::repo::users::GithubProfile;

    async fn db_with_user() -> Db {
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
        db
    }

    fn session(hash: &[u8], expires_at: DateTime<Utc>) -> NewSession<'_> {
        NewSession {
            token_hash: hash,
            tenant_id: DEFAULT_TENANT,
            user_id: "u-1",
            csrf_token: "csrf",
            expires_at,
        }
    }

    #[tokio::test]
    async fn sessions_are_found_by_hash_until_they_expire_or_are_deleted() {
        let db = db_with_user().await;
        let sessions = db.sessions();
        let now = Utc::now();
        sessions
            .create(&session(b"live", now + TimeDelta::hours(1)), now)
            .await
            .expect("create");
        sessions
            .create(&session(b"old", now - TimeDelta::seconds(1)), now)
            .await
            .expect("create");
        let live = sessions
            .touch(b"live", now)
            .await
            .expect("touch")
            .expect("live");
        assert_eq!(live.user_id, "u-1");
        assert_eq!(live.csrf_token, "csrf");
        assert_eq!(sessions.touch(b"old", now).await.expect("touch"), None);
        assert_eq!(sessions.touch(b"unknown", now).await.expect("touch"), None);
        sessions.delete_expired(now).await.expect("cleanup");
        assert!(
            !sessions.delete(b"old").await.expect("delete"),
            "cleaned up"
        );
        assert!(sessions.delete(b"live").await.expect("delete"));
        assert_eq!(sessions.touch(b"live", now).await.expect("touch"), None);
    }

    #[tokio::test]
    async fn delete_for_user_ends_every_session() {
        let db = db_with_user().await;
        let now = Utc::now();
        for hash in [b"a", b"b"] {
            db.sessions()
                .create(&session(hash, now + TimeDelta::hours(1)), now)
                .await
                .expect("create");
        }
        assert_eq!(
            db.sessions().delete_for_user("u-1").await.expect("delete"),
            2
        );
    }

    #[tokio::test]
    async fn oauth_login_is_single_use_and_expires() {
        let db = Db::in_memory().await.expect("db");
        let sessions = db.sessions();
        let now = Utc::now();
        sessions
            .create_oauth_login(
                "s1",
                DEFAULT_TENANT,
                b"ct",
                now,
                now + TimeDelta::minutes(10),
            )
            .await
            .expect("create");
        sessions
            .create_oauth_login(
                "s2",
                DEFAULT_TENANT,
                b"ct",
                now,
                now - TimeDelta::seconds(1),
            )
            .await
            .expect("create");
        let taken = sessions.take_oauth_login("s1", now).await.expect("take");
        assert_eq!(
            taken,
            Some(OauthLoginRecord {
                tenant_id: DEFAULT_TENANT.to_owned(),
                code_verifier_ct: b"ct".to_vec()
            })
        );
        assert_eq!(
            sessions.take_oauth_login("s1", now).await.expect("take"),
            None
        );
        assert_eq!(
            sessions.take_oauth_login("s2", now).await.expect("take"),
            None
        );
    }

    #[test]
    fn timestamps_are_fixed_width() {
        let whole = DateTime::parse_from_rfc3339("2026-11-16T10:00:00Z")
            .expect("time")
            .with_timezone(&Utc);
        assert_eq!(timestamp(whole), "2026-11-16T10:00:00.000000Z");
        assert_eq!(
            parse_timestamp(&timestamp(whole), "t").expect("parse"),
            whole
        );
    }
}
