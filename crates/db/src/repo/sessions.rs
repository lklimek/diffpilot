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
    /// Last recorded use (see [`Sessions::touch`]).
    pub last_seen_at: DateTime<Utc>,
}

/// When a session counts as live: unexpired at `now`, seen after `seen_after` (idle timeout), and its user's
/// GitHub id bound to an allowlist entry of the session's tenant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Liveness {
    /// Current time.
    pub now: DateTime<Utc>,
    /// Sessions last seen at or before this time are idle.
    pub seen_after: DateTime<Utc>,
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

/// SQL condition on a `sessions` row for [`Liveness`]; binds `?1` = now and `?2` = seen after.
macro_rules! live_session {
    () => {
        "(sessions.expires_at > ?1 AND sessions.last_seen_at > ?2
          AND EXISTS (SELECT 1 FROM users JOIN tenant_allowed_logins AS allowed ON allowed.github_id = users.github_id
                      WHERE users.id = sessions.user_id AND allowed.tenant_id = sessions.tenant_id))"
    };
}
pub(super) use live_session;

/// Insert of a new session (also used by the sign-in transaction of `tokens`).
pub(super) fn insert_session<'q>(
    session: &'q NewSession<'q>,
    now: DateTime<Utc>,
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments> {
    let now = timestamp(now);
    sqlx::query(
        "INSERT INTO sessions (token_hash, tenant_id, user_id, csrf_token, created_at, last_seen_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(session.token_hash)
    .bind(session.tenant_id)
    .bind(session.user_id)
    .bind(session.csrf_token)
    .bind(now.clone())
    .bind(now)
    .bind(timestamp(session.expires_at))
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
        insert_session(session, now).execute(self.pool).await?;
        Ok(())
    }

    /// The live session with this token hash (see [`Liveness`]); `None` if absent or not live. Reads only.
    pub async fn live(
        &self,
        token_hash: &[u8],
        live: Liveness,
    ) -> Result<Option<SessionRecord>, DbError> {
        let row = sqlx::query(concat!(
            "SELECT tenant_id, user_id, csrf_token, expires_at, last_seen_at FROM sessions WHERE token_hash = ?3 AND ",
            live_session!()
        ))
        .bind(timestamp(live.now))
        .bind(timestamp(live.seen_after))
        .bind(token_hash)
        .fetch_optional(self.pool)
        .await?;
        row.map(|row| {
            let expires_at: String = row.try_get("expires_at")?;
            let last_seen_at: String = row.try_get("last_seen_at")?;
            Ok(SessionRecord {
                tenant_id: row.try_get("tenant_id")?,
                user_id: row.try_get("user_id")?,
                csrf_token: row.try_get("csrf_token")?,
                expires_at: parse_timestamp(&expires_at, "sessions.expires_at")?,
                last_seen_at: parse_timestamp(&last_seen_at, "sessions.last_seen_at")?,
            })
        })
        .transpose()
    }

    /// Records a use at `live.now` of the session if it is still live and was last seen before `seen_before`; a
    /// deleted or ended session is never revived or extended. True if recorded.
    pub async fn touch(
        &self,
        token_hash: &[u8],
        live: Liveness,
        seen_before: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let touched = sqlx::query(concat!(
            "UPDATE sessions SET last_seen_at = ?1 WHERE token_hash = ?3 AND last_seen_at < ?4 AND ",
            live_session!()
        ))
        .bind(timestamp(live.now))
        .bind(timestamp(live.seen_after))
        .bind(token_hash)
        .bind(timestamp(seen_before))
        .execute(self.pool)
        .await?;
        Ok(touched.rows_affected() > 0)
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

    /// Deletes sessions that are not live (see [`Liveness`]), the GitHub token pairs of users left without any
    /// session, and expired OAuth logins, in one transaction.
    pub async fn delete_expired(&self, live: Liveness) -> Result<(), DbError> {
        let now = timestamp(live.now);
        let mut tx = super::write_tx(self.pool).await?;
        sqlx::query(concat!("DELETE FROM sessions WHERE NOT ", live_session!()))
            .bind(&now)
            .bind(timestamp(live.seen_after))
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "DELETE FROM user_tokens
             WHERE NOT EXISTS (SELECT 1 FROM sessions WHERE sessions.user_id = user_tokens.user_id)",
        )
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
        with_user(Db::in_memory().await.expect("db")).await
    }

    async fn with_user(db: Db) -> Db {
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
        crate::repo::tenants::testing::allow_login(&db, DEFAULT_TENANT, "lklimek", 42)
            .await
            .expect("allowed");
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

    fn live_at(now: DateTime<Utc>) -> Liveness {
        Liveness {
            now,
            seen_after: now - TimeDelta::hours(1),
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
            .live(b"live", live_at(now))
            .await
            .expect("read")
            .expect("live");
        assert_eq!(live.user_id, "u-1");
        assert_eq!(live.csrf_token, "csrf");
        assert_eq!(
            sessions.live(b"old", live_at(now)).await.expect("read"),
            None
        );
        assert_eq!(
            sessions.live(b"unknown", live_at(now)).await.expect("read"),
            None
        );
        sessions
            .delete_expired(live_at(now))
            .await
            .expect("cleanup");
        assert!(
            !sessions.delete(b"old").await.expect("delete"),
            "cleaned up"
        );
        assert!(sessions.delete(b"live").await.expect("delete"));
        assert_eq!(
            sessions.live(b"live", live_at(now)).await.expect("read"),
            None
        );
    }

    #[tokio::test]
    async fn idle_session_is_not_live() {
        let db = db_with_user().await;
        let sessions = db.sessions();
        let created = Utc::now();
        sessions
            .create(&session(b"s", created + TimeDelta::days(7)), created)
            .await
            .expect("create");
        let later = created + TimeDelta::minutes(30);
        assert!(
            sessions
                .live(b"s", live_at(later))
                .await
                .expect("read")
                .is_some(),
            "seen within the idle window"
        );
        let idle = later + TimeDelta::hours(2);
        assert_eq!(
            sessions.live(b"s", live_at(idle)).await.expect("read"),
            None,
            "unseen for longer than the idle window"
        );
        sessions
            .delete_expired(live_at(idle))
            .await
            .expect("cleanup");
        assert!(
            !sessions.delete(b"s").await.expect("delete"),
            "idle sessions are cleaned up"
        );
    }

    #[tokio::test]
    async fn session_of_a_removed_or_rebound_login_is_not_live() {
        let db = db_with_user().await;
        let sessions = db.sessions();
        let now = Utc::now();
        sessions
            .create(&session(b"s", now + TimeDelta::hours(1)), now)
            .await
            .expect("create");
        let live = |db: &Db| {
            let db = db.clone();
            async move {
                db.sessions()
                    .live(b"s", live_at(now))
                    .await
                    .expect("read")
                    .is_some()
            }
        };
        assert!(live(&db).await, "bound login");
        crate::repo::tenants::testing::allow_login(&db, DEFAULT_TENANT, "lklimek", 43)
            .await
            .expect("rebind");
        assert!(!live(&db).await, "entry bound to another GitHub id");
        crate::repo::tenants::testing::allow_login(&db, DEFAULT_TENANT, "lklimek", 42)
            .await
            .expect("bind");
        assert!(live(&db).await);
        crate::repo::tenants::testing::remove_login(&db, DEFAULT_TENANT, "lklimek")
            .await
            .expect("remove");
        assert!(!live(&db).await, "login removed from the allowlist");
        sessions
            .delete_expired(live_at(now))
            .await
            .expect("cleanup");
        assert!(
            !sessions.delete(b"s").await.expect("delete"),
            "sessions of removed logins are cleaned up"
        );
    }

    #[tokio::test]
    async fn session_is_live_only_in_a_tenant_that_allows_its_user() {
        let db = db_with_user().await;
        crate::repo::analyses::testing::seed_tenant(&db, "other")
            .await
            .expect("tenant");
        let now = Utc::now();
        db.sessions()
            .create(
                &NewSession {
                    tenant_id: "other",
                    ..session(b"other", now + TimeDelta::hours(1))
                },
                now,
            )
            .await
            .expect("create");
        assert_eq!(
            db.sessions()
                .live(b"other", live_at(now))
                .await
                .expect("read"),
            None
        );
    }

    #[tokio::test]
    async fn touch_records_use_only_of_a_live_session_last_seen_before_the_bound() {
        let db = db_with_user().await;
        let sessions = db.sessions();
        // Stored timestamps keep microseconds.
        let created = chrono::SubsecRound::trunc_subsecs(Utc::now(), 6) - TimeDelta::minutes(10);
        let expires = created + TimeDelta::minutes(20);
        sessions
            .create(&session(b"s", expires), created)
            .await
            .expect("create");
        let seen = |at| {
            let sessions = db.sessions();
            async move {
                sessions
                    .live(b"s", live_at(at))
                    .await
                    .expect("read")
                    .map(|record| record.last_seen_at)
            }
        };
        let now = created + TimeDelta::minutes(5);
        assert!(
            !sessions
                .touch(b"s", live_at(now), created)
                .await
                .expect("touch"),
            "seen at the bound: not written again"
        );
        assert_eq!(seen(now).await, Some(created));
        assert!(
            sessions
                .touch(b"s", live_at(now), now - TimeDelta::minutes(1))
                .await
                .expect("touch")
        );
        assert_eq!(seen(now).await, Some(now));
        let ended = expires + TimeDelta::seconds(1);
        assert!(
            !sessions
                .touch(b"s", live_at(ended), ended)
                .await
                .expect("touch"),
            "past its expiry"
        );
        let idle = now + TimeDelta::hours(1);
        assert!(
            !sessions
                .touch(
                    b"s",
                    Liveness {
                        now: idle,
                        seen_after: now,
                    },
                    idle
                )
                .await
                .expect("touch"),
            "idle at its last recorded use"
        );
        crate::repo::tenants::testing::remove_login(&db, DEFAULT_TENANT, "lklimek")
            .await
            .expect("remove");
        assert!(
            !sessions
                .touch(b"s", live_at(now), now + TimeDelta::minutes(1))
                .await
                .expect("touch"),
            "login removed from the allowlist"
        );
        assert!(sessions.delete(b"s").await.expect("delete"));
        assert!(
            !sessions
                .touch(b"s", live_at(now), now + TimeDelta::minutes(1))
                .await
                .expect("touch"),
            "signed out"
        );
        assert!(!sessions.delete(b"s").await.expect("delete"), "not revived");
    }

    #[tokio::test]
    async fn live_session_reads_while_another_connection_holds_the_write_lock() {
        let dir = tempfile::tempdir().expect("temp dir");
        let busy_timeout = std::time::Duration::from_millis(100);
        let db = crate::repo::testing::file_backed_with_busy_timeout(dir.path(), busy_timeout)
            .await
            .expect("db");
        let db = with_user(db).await;
        let now = Utc::now();
        let sessions = db.sessions();
        sessions
            .create(&session(b"s", now + TimeDelta::hours(1)), now)
            .await
            .expect("create");
        let _lock = crate::repo::testing::hold_write_lock(&db)
            .await
            .expect("write lock");
        let later = now + TimeDelta::minutes(5);
        assert!(
            sessions
                .live(b"s", live_at(later))
                .await
                .expect("read under a held write lock")
                .is_some()
        );
        let error = sessions
            .touch(b"s", live_at(later), later)
            .await
            .expect_err("the write lock is held past the busy timeout");
        assert_eq!(error.sqlite_code().as_deref(), Some("5"), "{error:?}");
        assert!(error.is_busy());
        assert!(!DbError::NotFound.is_busy());
    }

    #[tokio::test]
    async fn cleanup_deletes_token_pairs_left_without_sessions() {
        let db = db_with_user().await;
        let now = Utc::now();
        let pair = crate::repo::tokens::EncryptedTokens {
            access_ct: vec![1],
            access_expires_at: now + TimeDelta::hours(8),
            refresh_ct: vec![2],
            refresh_expires_at: now + TimeDelta::days(180),
        };
        db.user_tokens()
            .put("u-1", DEFAULT_TENANT, &pair, now)
            .await
            .expect("pair");
        db.sessions()
            .create(&session(b"s", now + TimeDelta::hours(1)), now)
            .await
            .expect("create");
        db.sessions()
            .delete_expired(live_at(now))
            .await
            .expect("cleanup");
        assert!(
            db.user_tokens().get("u-1").await.expect("get").is_some(),
            "a live session keeps its pair"
        );
        let later = now + TimeDelta::hours(2);
        db.sessions()
            .delete_expired(live_at(later))
            .await
            .expect("cleanup");
        assert_eq!(
            db.user_tokens().get("u-1").await.expect("get"),
            None,
            "the pair ends with the user's last session"
        );
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
