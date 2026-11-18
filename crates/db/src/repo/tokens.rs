//! Repository for `user_tokens`: GitHub user access and refresh tokens as AES-GCM ciphertext. `version` is the
//! compare-and-swap counter that keeps a refresh from overwriting a newer pair: it starts at a random value for
//! every new row and grows by one per write, so a version read before the row was deleted and re-created does
//! not match the new row.

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

use super::Db;
use super::sessions::{
    Liveness, NewSession, insert_session, live_session, pair_kept_by_schedule, parse_timestamp,
    timestamp,
};
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
    /// Compare-and-swap counter; changes with every write and is not reused by a re-created row.
    pub version: i64,
}

/// Which stored pair [`UserTokens::end_sign_in`] applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairCondition {
    /// Whatever is stored (explicit sign-out).
    Any,
    /// Only the pair with this version (a refresh of that pair was rejected).
    Version(i64),
    /// Only when no pair is stored (sessions left without tokens).
    Absent,
}

/// Upsert of a sign-in pair; a new row starts at a random version below 2^62, an existing one advances by one.
fn put_query<'q>(
    user_id: &'q str,
    tenant_id: &'q str,
    tokens: &'q EncryptedTokens,
    now: DateTime<Utc>,
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments> {
    sqlx::query(
        "INSERT INTO user_tokens
             (user_id, tenant_id, access_ct, access_expires_at, refresh_ct, refresh_expires_at, version,
              updated_at)
         VALUES (?, ?, ?, ?, ?, ?, random() & 0x3FFFFFFFFFFFFFFF, ?)
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
}

impl UserTokens<'_> {
    /// Stores the pair of a sign-in, replacing any previous pair (the version still advances; a new row starts
    /// at a random version below 2^62).
    pub async fn put(
        &self,
        user_id: &str,
        tenant_id: &str,
        tokens: &EncryptedTokens,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        put_query(user_id, tenant_id, tokens, now)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// Stores a new session and the pair of its sign-in (replacing the previous pair) in one transaction: on
    /// failure, or when the caller is cancelled before the commit, neither is written.
    pub async fn put_with_session(
        &self,
        tenant_id: &str,
        tokens: &EncryptedTokens,
        session: &NewSession<'_>,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        insert_session(session, now).execute(&mut *tx).await?;
        put_query(session.user_id, tenant_id, tokens, now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
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

    /// Ends the user's sign-in in one transaction: deletes the token pair and every session of the user if the
    /// stored pair matches `condition`; false (nothing changed) otherwise.
    pub async fn end_sign_in(
        &self,
        user_id: &str,
        condition: PairCondition,
    ) -> Result<bool, DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let matched = match condition {
            PairCondition::Any => {
                sqlx::query("DELETE FROM user_tokens WHERE user_id = ?")
                    .bind(user_id)
                    .execute(&mut *tx)
                    .await?;
                true
            }
            PairCondition::Version(version) => {
                sqlx::query("DELETE FROM user_tokens WHERE user_id = ? AND version = ?")
                    .bind(user_id)
                    .bind(version)
                    .execute(&mut *tx)
                    .await?
                    .rows_affected()
                    == 1
            }
            PairCondition::Absent => {
                let stored: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM user_tokens WHERE user_id = ?)",
                )
                .bind(user_id)
                .fetch_one(&mut *tx)
                .await?;
                !stored
            }
        };
        if !matched {
            return Ok(false);
        }
        // The guard keeps a pair stored meanwhile (Absent) together with its sessions.
        sqlx::query(
            "DELETE FROM sessions WHERE user_id = ?
             AND (? OR NOT EXISTS (SELECT 1 FROM user_tokens WHERE user_id = ?))",
        )
        .bind(user_id)
        .bind(condition != PairCondition::Absent)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Ends the user's session `token_hash`; when the user has no other live session left (see [`Liveness`]) and no
    /// dashboard schedule keeps the pair (ADR-020), deletes it in the same transaction. True if the pair was deleted.
    pub async fn end_session(
        &self,
        user_id: &str,
        token_hash: &[u8],
        live: Liveness,
    ) -> Result<bool, DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        sqlx::query("DELETE FROM sessions WHERE token_hash = ? AND user_id = ?")
            .bind(token_hash)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        let deleted = sqlx::query(concat!(
            "DELETE FROM user_tokens WHERE user_id = ?3
             AND NOT EXISTS (SELECT 1 FROM sessions WHERE sessions.user_id = ?3 AND ",
            live_session!(),
            ") AND NOT ",
            pair_kept_by_schedule!()
        ))
        .bind(timestamp(live.now))
        .bind(timestamp(live.seen_after))
        .bind(user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(deleted == 1)
    }

    /// Marks the access token of pair `version` expired at `now` (GitHub rejected it), so its next use refreshes
    /// the pair; false if another write came first.
    pub async fn expire_access(
        &self,
        user_id: &str,
        version: i64,
        now: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let now = timestamp(now);
        let updated = sqlx::query(
            "UPDATE user_tokens SET access_expires_at = ?, version = version + 1, updated_at = ?
             WHERE user_id = ? AND version = ?",
        )
        .bind(&now)
        .bind(&now)
        .bind(user_id)
        .bind(version)
        .execute(self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }
}

/// Failure injection for tests (`test-util` feature); never in release builds.
#[cfg(any(test, feature = "test-util"))]
pub mod testing {
    use super::*;

    /// Makes every write of a token pair fail until [`allow_token_writes`].
    pub async fn fail_token_writes(db: &Db) -> Result<(), DbError> {
        sqlx::raw_sql(
            "CREATE TRIGGER fail_token_insert BEFORE INSERT ON user_tokens
                 BEGIN SELECT RAISE(ABORT, 'injected token write failure'); END;
             CREATE TRIGGER fail_token_update BEFORE UPDATE ON user_tokens
                 BEGIN SELECT RAISE(ABORT, 'injected token write failure'); END;",
        )
        .execute(&db.pool)
        .await?;
        Ok(())
    }

    /// Undoes [`fail_token_writes`].
    pub async fn allow_token_writes(db: &Db) -> Result<(), DbError> {
        sqlx::raw_sql(
            "DROP TRIGGER IF EXISTS fail_token_insert; DROP TRIGGER IF EXISTS fail_token_update;",
        )
        .execute(&db.pool)
        .await?;
        Ok(())
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
        assert!(
            tokens
                .end_sign_in("u-1", PairCondition::Any)
                .await
                .expect("end")
        );
        assert_eq!(tokens.get("u-1").await.expect("get"), None);
    }

    async fn db_with_signed_in_user() -> Db {
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
        db.user_tokens()
            .put("u-1", DEFAULT_TENANT, &pair(1), Utc::now())
            .await
            .expect("put");
        db.sessions()
            .create(
                &crate::repo::sessions::NewSession {
                    token_hash: b"h",
                    tenant_id: DEFAULT_TENANT,
                    user_id: "u-1",
                    csrf_token: "c",
                    expires_at: Utc::now() + TimeDelta::hours(1),
                },
                Utc::now(),
            )
            .await
            .expect("session");
        db
    }

    async fn session_count(db: &Db) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE user_id = 'u-1'")
            .fetch_one(&db.pool)
            .await
            .expect("count")
    }

    #[tokio::test]
    async fn stale_version_does_not_match_a_recreated_row() {
        let db = db_with_signed_in_user().await;
        let tokens = db.user_tokens();
        let stale = tokens
            .get("u-1")
            .await
            .expect("get")
            .expect("stored")
            .version;
        assert!(
            tokens
                .end_sign_in("u-1", PairCondition::Any)
                .await
                .expect("end")
        );
        tokens
            .put("u-1", DEFAULT_TENANT, &pair(5), Utc::now())
            .await
            .expect("new sign-in");
        assert!(
            !tokens
                .replace("u-1", stale, &pair(6), Utc::now())
                .await
                .expect("stale replace"),
            "a version read before the row was re-created must not match"
        );
        assert!(
            !tokens
                .end_sign_in("u-1", PairCondition::Version(stale))
                .await
                .expect("stale end"),
        );
        assert_eq!(
            tokens.get("u-1").await.expect("get").expect("kept").tokens,
            pair(5)
        );
    }

    #[tokio::test]
    async fn end_sign_in_applies_only_to_its_pair() {
        let db = db_with_signed_in_user().await;
        let tokens = db.user_tokens();
        let version = tokens
            .get("u-1")
            .await
            .expect("get")
            .expect("stored")
            .version;
        assert!(
            !tokens
                .end_sign_in("u-1", PairCondition::Absent)
                .await
                .expect("absent")
        );
        assert!(
            !tokens
                .end_sign_in("u-1", PairCondition::Version(version + 1))
                .await
                .expect("other version")
        );
        assert_eq!(session_count(&db).await, 1, "nothing ended");
        assert!(
            tokens
                .end_sign_in("u-1", PairCondition::Version(version))
                .await
                .expect("own version")
        );
        assert_eq!(tokens.get("u-1").await.expect("get"), None);
        assert_eq!(session_count(&db).await, 0);
    }

    #[tokio::test]
    async fn absent_pair_ends_leftover_sessions() {
        let db = db_with_signed_in_user().await;
        sqlx::query("DELETE FROM user_tokens WHERE user_id = 'u-1'")
            .execute(&db.pool)
            .await
            .expect("simulate a pair lost before its sessions");
        assert!(
            db.user_tokens()
                .end_sign_in("u-1", PairCondition::Absent)
                .await
                .expect("absent")
        );
        assert_eq!(session_count(&db).await, 0);
    }

    fn live_at(now: DateTime<Utc>) -> Liveness {
        Liveness {
            now,
            seen_after: now - TimeDelta::hours(1),
        }
    }

    #[tokio::test]
    async fn end_session_deletes_the_pair_with_the_last_live_session() {
        let db = db_with_signed_in_user().await;
        let tokens = db.user_tokens();
        let now = Utc::now();
        crate::repo::tenants::testing::allow_login(&db, DEFAULT_TENANT, "lklimek", 42)
            .await
            .expect("allowed");
        for (hash, expires_at) in [
            (&b"second"[..], now + TimeDelta::hours(1)),
            (&b"expired"[..], now - TimeDelta::seconds(1)),
        ] {
            db.sessions()
                .create(
                    &crate::repo::sessions::NewSession {
                        token_hash: hash,
                        tenant_id: DEFAULT_TENANT,
                        user_id: "u-1",
                        csrf_token: "c",
                        expires_at,
                    },
                    now,
                )
                .await
                .expect("session");
        }
        assert!(
            !tokens
                .end_session("u-1", b"h", live_at(now))
                .await
                .expect("end"),
            "another live session keeps the pair"
        );
        assert!(tokens.get("u-1").await.expect("get").is_some());
        assert_eq!(session_count(&db).await, 2);
        assert!(
            !tokens
                .end_session("u-2", b"second", live_at(now))
                .await
                .expect("end"),
            "only the user's own session"
        );
        assert_eq!(session_count(&db).await, 2);
        assert!(
            tokens
                .end_session("u-1", b"second", live_at(now))
                .await
                .expect("end"),
            "the last live session takes the pair with it"
        );
        assert_eq!(tokens.get("u-1").await.expect("get"), None);
    }

    /// ADR-020: sign-out of the last session keeps the pair while the schedule is on; sign-out everywhere deletes it.
    #[tokio::test]
    async fn last_sign_out_keeps_the_pair_while_schedule_on() {
        let db = db_with_signed_in_user().await;
        let tokens = db.user_tokens();
        let now = Utc::now();
        crate::repo::tenants::testing::allow_login(&db, DEFAULT_TENANT, "lklimek", 42)
            .await
            .expect("allowed");
        let settings = contracts::api::dashboard::DashboardSettings {
            time_zone: Some("Europe/Warsaw".to_owned()),
            ..Default::default()
        };
        assert!(settings.schedule_enabled, "on by default");
        db.dashboard()
            .save_settings(DEFAULT_TENANT, "u-1", &settings, now)
            .await
            .expect("settings");
        assert!(
            !tokens
                .end_session("u-1", b"h", live_at(now))
                .await
                .expect("end"),
            "the schedule keeps the pair"
        );
        assert!(tokens.get("u-1").await.expect("get").is_some());
        assert_eq!(session_count(&db).await, 0);
        assert!(
            tokens
                .end_sign_in("u-1", PairCondition::Any)
                .await
                .expect("everywhere")
        );
        assert_eq!(
            tokens.get("u-1").await.expect("get"),
            None,
            "sign-out everywhere deletes it"
        );
    }

    #[tokio::test]
    async fn expire_access_is_a_compare_and_swap() {
        let db = db_with_signed_in_user().await;
        let tokens = db.user_tokens();
        let stored = tokens.get("u-1").await.expect("get").expect("stored");
        let now = pair(1).access_expires_at - TimeDelta::hours(1);
        assert!(
            !tokens
                .expire_access("u-1", stored.version + 1, now)
                .await
                .expect("stale")
        );
        assert!(
            tokens
                .expire_access("u-1", stored.version, now)
                .await
                .expect("expire")
        );
        let expired = tokens.get("u-1").await.expect("get").expect("stored");
        assert_eq!(expired.tokens.access_expires_at, now);
        assert_eq!(expired.tokens.refresh_ct, stored.tokens.refresh_ct);
        assert_eq!(expired.version, stored.version + 1);
    }

    #[tokio::test]
    async fn sign_in_writes_session_and_pair_in_one_transaction() {
        let db = db_with_signed_in_user().await;
        let now = Utc::now();
        let session = crate::repo::sessions::NewSession {
            token_hash: b"new",
            tenant_id: DEFAULT_TENANT,
            user_id: "u-1",
            csrf_token: "c",
            expires_at: now + TimeDelta::hours(1),
        };
        testing::fail_token_writes(&db)
            .await
            .expect("inject failure");
        let result = db
            .user_tokens()
            .put_with_session(DEFAULT_TENANT, &pair(7), &session, now)
            .await;
        assert!(result.is_err(), "{result:?}");
        assert_eq!(
            session_count(&db).await,
            1,
            "the session is rolled back with the pair"
        );
        testing::allow_token_writes(&db)
            .await
            .expect("remove failure");
        db.user_tokens()
            .put_with_session(DEFAULT_TENANT, &pair(7), &session, now)
            .await
            .expect("sign-in");
        assert_eq!(session_count(&db).await, 2);
        assert_eq!(
            db.user_tokens()
                .get("u-1")
                .await
                .expect("get")
                .expect("pair")
                .tokens,
            pair(7)
        );
    }

    #[tokio::test]
    async fn end_sign_in_is_one_transaction() {
        let db = db_with_signed_in_user().await;
        sqlx::query(
            "CREATE TRIGGER fail_session_delete BEFORE DELETE ON sessions
             BEGIN SELECT RAISE(ABORT, 'injected failure'); END",
        )
        .execute(&db.pool)
        .await
        .expect("inject failure");
        let result = db
            .user_tokens()
            .end_sign_in("u-1", PairCondition::Any)
            .await;
        assert!(result.is_err(), "{result:?}");
        assert!(
            db.user_tokens().get("u-1").await.expect("get").is_some(),
            "the token delete is rolled back with the failed session delete"
        );
        assert_eq!(session_count(&db).await, 1);
    }
}
