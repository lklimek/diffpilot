//! Repository for `access_tokens`: personal access tokens (ADR-019). Only the SHA-256 of a token is stored.

use std::fmt;

use chrono::{DateTime, TimeDelta, Utc};
use contracts::api::access_tokens::{AccessTokenId, CreateAccessToken, TokenScope};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqlitePool};

use super::Db;
use super::sessions::{parse_timestamp, timestamp};
use crate::error::DbError;

/// [`AccessTokens::touch`] records a use only when the last recorded one is at least this old.
pub const TOUCH_INTERVAL: TimeDelta = TimeDelta::minutes(1);

/// Access to personal access tokens.
#[derive(Debug)]
pub struct AccessTokens<'a> {
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

/// Columns of an [`AccessTokenRecord`] (read by name in [`record_from_row`]).
macro_rules! columns {
    () => {
        "id, tenant_id, user_id, name, scopes, created_at, expires_at, last_used_at, revoked_at"
    };
}

impl AccessTokens<'_> {
    /// Stores a new token after checking the name and the live-token limit in one write transaction.
    pub async fn create(
        &self,
        tenant_id: &str,
        user_id: &str,
        token: &NewAccessToken,
        now: DateTime<Utc>,
    ) -> Result<CreateOutcome, DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let name_taken: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM access_tokens
                            WHERE user_id = ? AND name = ? AND revoked_at IS NULL)",
        )
        .bind(user_id)
        .bind(&token.name)
        .fetch_one(&mut *tx)
        .await?;
        if name_taken {
            return Ok(CreateOutcome::NameTaken);
        }
        let live: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM access_tokens
             WHERE user_id = ? AND revoked_at IS NULL AND expires_at > ?",
        )
        .bind(user_id)
        .bind(timestamp(now))
        .fetch_one(&mut *tx)
        .await?;
        if live >= i64::from(CreateAccessToken::MAX_LIVE_PER_USER) {
            return Ok(CreateOutcome::LimitReached);
        }
        let row = sqlx::query(concat!(
            "INSERT INTO access_tokens (id, tenant_id, user_id, name, token_hash, scopes, created_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING ",
            columns!()
        ))
        .bind(&token.id.0)
        .bind(tenant_id)
        .bind(user_id)
        .bind(&token.name)
        .bind(token.token_hash.as_slice())
        .bind(serde_json::to_string(&token.scopes)?)
        .bind(timestamp(now))
        .bind(timestamp(token.expires_at))
        .fetch_one(&mut *tx)
        .await?;
        let record = record_from_row(&row)?;
        tx.commit().await?;
        Ok(CreateOutcome::Created(record))
    }

    /// The user's tokens that are not revoked, newest first (expired ones included until revoked).
    pub async fn list(
        &self,
        tenant_id: &str,
        user_id: &str,
    ) -> Result<Vec<AccessTokenRecord>, DbError> {
        let rows = sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM access_tokens WHERE tenant_id = ? AND user_id = ? AND revoked_at IS NULL
             ORDER BY created_at DESC, id DESC"
        ))
        .bind(tenant_id)
        .bind(user_id)
        .fetch_all(self.pool)
        .await?;
        rows.iter().map(record_from_row).collect()
    }

    /// Server-internal (request check): the token with this public id in any state, with its hash; `None` also
    /// when its user is no longer on the sign-in allow-list of the token's tenant (as for sessions).
    pub async fn credential(&self, id: &AccessTokenId) -> Result<Option<TokenCredential>, DbError> {
        let row = sqlx::query(concat!(
            "SELECT ",
            columns!(),
            ", token_hash FROM access_tokens
             WHERE id = ? AND EXISTS (
                 SELECT 1 FROM users JOIN tenant_allowed_logins AS allowed ON allowed.github_id = users.github_id
                 WHERE users.id = access_tokens.user_id AND allowed.tenant_id = access_tokens.tenant_id)"
        ))
        .bind(&id.0)
        .fetch_optional(self.pool)
        .await?;
        row.map(|row| {
            let hash: Vec<u8> = row.try_get("token_hash")?;
            Ok(TokenCredential {
                record: record_from_row(&row)?,
                token_hash: hash.try_into().map_err(|_| DbError::Corrupt {
                    column: "access_tokens.token_hash",
                })?,
            })
        })
        .transpose()
    }

    /// Records an accepted use; writes only when the stored `last_used_at` is a minute or more before `now`.
    pub async fn touch(&self, id: &AccessTokenId, now: DateTime<Utc>) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE access_tokens SET last_used_at = ?
             WHERE id = ? AND revoked_at IS NULL AND (last_used_at IS NULL OR last_used_at <= ?)",
        )
        .bind(timestamp(now))
        .bind(&id.0)
        .bind(timestamp(now - TOUCH_INTERVAL))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Revokes one of the user's tokens; `NotFound` for an unknown, revoked or another user's token.
    pub async fn revoke(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: &AccessTokenId,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        let revoked = sqlx::query(
            "UPDATE access_tokens SET revoked_at = ?
             WHERE tenant_id = ? AND user_id = ? AND id = ? AND revoked_at IS NULL",
        )
        .bind(timestamp(now))
        .bind(tenant_id)
        .bind(user_id)
        .bind(&id.0)
        .execute(self.pool)
        .await?;
        if revoked.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Revokes every token of the user ("sign out everywhere"); returns how many were live.
    pub async fn revoke_all(
        &self,
        tenant_id: &str,
        user_id: &str,
        now: DateTime<Utc>,
    ) -> Result<u64, DbError> {
        let now = timestamp(now);
        let expiries: Vec<String> = sqlx::query_scalar(
            "UPDATE access_tokens SET revoked_at = ?
             WHERE tenant_id = ? AND user_id = ? AND revoked_at IS NULL RETURNING expires_at",
        )
        .bind(&now)
        .bind(tenant_id)
        .bind(user_id)
        .fetch_all(self.pool)
        .await?;
        Ok(expiries
            .iter()
            .map(|expires| u64::from(*expires > now))
            .sum())
    }
}

fn record_from_row(row: &SqliteRow) -> Result<AccessTokenRecord, DbError> {
    let scopes: String = row.try_get("scopes")?;
    let created_at: String = row.try_get("created_at")?;
    let expires_at: String = row.try_get("expires_at")?;
    let last_used_at: Option<String> = row.try_get("last_used_at")?;
    let revoked_at: Option<String> = row.try_get("revoked_at")?;
    Ok(AccessTokenRecord {
        id: AccessTokenId(row.try_get("id")?),
        tenant_id: row.try_get("tenant_id")?,
        user_id: row.try_get("user_id")?,
        name: row.try_get("name")?,
        scopes: serde_json::from_str(&scopes)?,
        created_at: parse_timestamp(&created_at, "access_tokens.created_at")?,
        expires_at: parse_timestamp(&expires_at, "access_tokens.expires_at")?,
        last_used_at: last_used_at
            .map(|at| parse_timestamp(&at, "access_tokens.last_used_at"))
            .transpose()?,
        revoked_at: revoked_at
            .map(|at| parse_timestamp(&at, "access_tokens.revoked_at"))
            .transpose()?,
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::repo::DEFAULT_TENANT;
    use crate::repo::tenants::testing::{allow_login, remove_login};
    use crate::repo::users::GithubProfile;

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 18, 10, minute, 0)
            .single()
            .expect("time")
    }

    /// A database with users `u1` (`lklimek`) and `u2` (`colleague`), both allowed in the default tenant.
    async fn db() -> Db {
        let db = Db::in_memory().await.expect("db");
        for (id, login, github_id) in [("u1", "lklimek", 1), ("u2", "colleague", 2)] {
            allow_login(&db, DEFAULT_TENANT, login, github_id)
                .await
                .expect("allow");
            db.users()
                .upsert_member(
                    DEFAULT_TENANT,
                    id,
                    &GithubProfile {
                        github_id,
                        login: login.to_owned(),
                        name: None,
                        avatar_url: None,
                    },
                    at(0),
                )
                .await
                .expect("user");
        }
        db
    }

    fn token(id: &str, name: &str, expires_at: DateTime<Utc>) -> NewAccessToken {
        let mut hash = [0u8; 32];
        hash[..id.len()].copy_from_slice(id.as_bytes());
        NewAccessToken {
            id: AccessTokenId(id.to_owned()),
            name: name.to_owned(),
            token_hash: hash,
            scopes: vec![TokenScope::Read],
            expires_at,
        }
    }

    async fn create(
        db: &Db,
        user: &str,
        new: &NewAccessToken,
        now: DateTime<Utc>,
    ) -> CreateOutcome {
        db.access_tokens()
            .create(DEFAULT_TENANT, user, new, now)
            .await
            .expect("create")
    }

    #[tokio::test]
    async fn created_token_is_listed_and_found_with_its_hash() {
        let db = db().await;
        let new = token("Ab3dE5gH", "agent", at(0) + TimeDelta::days(90));
        let CreateOutcome::Created(record) = create(&db, "u1", &new, at(0)).await else {
            panic!("created");
        };
        assert_eq!(
            (
                record.id.clone(),
                record.user_id.as_str(),
                record.name.as_str()
            ),
            (new.id.clone(), "u1", "agent")
        );
        assert_eq!(
            (
                record.created_at,
                record.expires_at,
                record.last_used_at,
                record.revoked_at
            ),
            (at(0), new.expires_at, None, None)
        );
        assert_eq!(record.scopes, [TokenScope::Read]);
        let tokens = db.access_tokens();
        assert_eq!(
            tokens.list(DEFAULT_TENANT, "u1").await.expect("list"),
            std::slice::from_ref(&record)
        );
        assert!(
            tokens
                .list(DEFAULT_TENANT, "u2")
                .await
                .expect("list")
                .is_empty()
        );
        let credential = tokens
            .credential(&new.id)
            .await
            .expect("read")
            .expect("found");
        assert_eq!(
            (credential.record, credential.token_hash),
            (record, new.token_hash)
        );
        assert!(
            tokens
                .credential(&AccessTokenId("Zz9yX8wV".to_owned()))
                .await
                .expect("read")
                .is_none()
        );
    }

    #[tokio::test]
    async fn list_is_newest_first_and_skips_revoked_tokens() {
        let db = db().await;
        let far = at(0) + TimeDelta::days(30);
        for (minute, id) in [(1, "AAAAAAA1"), (2, "AAAAAAA2"), (3, "AAAAAAA3")] {
            create(&db, "u1", &token(id, id, far), at(minute)).await;
        }
        let tokens = db.access_tokens();
        tokens
            .revoke(
                DEFAULT_TENANT,
                "u1",
                &AccessTokenId("AAAAAAA2".to_owned()),
                at(4),
            )
            .await
            .expect("revoke");
        let ids: Vec<String> = tokens
            .list(DEFAULT_TENANT, "u1")
            .await
            .expect("list")
            .into_iter()
            .map(|record| record.id.0)
            .collect();
        assert_eq!(ids, ["AAAAAAA3", "AAAAAAA1"]);
        let revoked = tokens
            .credential(&AccessTokenId("AAAAAAA2".to_owned()))
            .await
            .expect("read")
            .expect("revoked tokens stay readable for the request check");
        assert_eq!(revoked.record.revoked_at, Some(at(4)));
    }

    /// A name is unique among the user's tokens that are not revoked; other users and revoked tokens do not count.
    #[tokio::test]
    async fn names_are_unique_per_user_until_revoked() {
        let db = db().await;
        let far = at(0) + TimeDelta::days(30);
        create(&db, "u1", &token("AAAAAAA1", "agent", far), at(0)).await;
        assert_eq!(
            create(&db, "u1", &token("AAAAAAA2", "agent", far), at(1)).await,
            CreateOutcome::NameTaken
        );
        assert!(matches!(
            create(&db, "u2", &token("AAAAAAA3", "agent", far), at(1)).await,
            CreateOutcome::Created(_)
        ));
        db.access_tokens()
            .revoke(
                DEFAULT_TENANT,
                "u1",
                &AccessTokenId("AAAAAAA1".to_owned()),
                at(2),
            )
            .await
            .expect("revoke");
        assert!(matches!(
            create(&db, "u1", &token("AAAAAAA4", "agent", far), at(3)).await,
            CreateOutcome::Created(_)
        ));
    }

    /// At most ten live tokens: revoked and expired ones free a place.
    #[tokio::test]
    async fn live_tokens_are_limited_per_user() {
        let db = db().await;
        let far = at(0) + TimeDelta::days(30);
        for n in 0..10 {
            let id = format!("AAAAAA{n:02}");
            let expires = if n == 0 { at(5) } else { far };
            assert!(matches!(
                create(&db, "u1", &token(&id, &id, expires), at(0)).await,
                CreateOutcome::Created(_)
            ));
        }
        assert_eq!(
            create(&db, "u1", &token("BBBBBB01", "eleventh", far), at(1)).await,
            CreateOutcome::LimitReached
        );
        assert!(
            matches!(
                create(&db, "u2", &token("BBBBBB02", "other", far), at(1)).await,
                CreateOutcome::Created(_)
            ),
            "the limit is per user"
        );
        assert!(
            matches!(
                create(&db, "u1", &token("BBBBBB03", "eleventh", far), at(5)).await,
                CreateOutcome::Created(_)
            ),
            "an expired token frees its place"
        );
        assert_eq!(
            create(&db, "u1", &token("BBBBBB04", "twelfth", far), at(6)).await,
            CreateOutcome::LimitReached
        );
        db.access_tokens()
            .revoke(
                DEFAULT_TENANT,
                "u1",
                &AccessTokenId("AAAAAA05".to_owned()),
                at(7),
            )
            .await
            .expect("revoke");
        assert!(matches!(
            create(&db, "u1", &token("BBBBBB04", "twelfth", far), at(8)).await,
            CreateOutcome::Created(_)
        ));
    }

    #[tokio::test]
    async fn revoke_is_scoped_to_the_owner_and_live_tokens() {
        let db = db().await;
        let id = AccessTokenId("AAAAAAA1".to_owned());
        create(
            &db,
            "u1",
            &token(&id.0, "agent", at(0) + TimeDelta::days(30)),
            at(0),
        )
        .await;
        let tokens = db.access_tokens();
        for (tenant, user) in [(DEFAULT_TENANT, "u2"), ("other", "u1")] {
            assert!(
                matches!(
                    tokens.revoke(tenant, user, &id, at(1)).await,
                    Err(DbError::NotFound)
                ),
                "{tenant}/{user}"
            );
        }
        assert!(matches!(
            tokens
                .revoke(
                    DEFAULT_TENANT,
                    "u1",
                    &AccessTokenId("Zz9yX8wV".to_owned()),
                    at(1)
                )
                .await,
            Err(DbError::NotFound)
        ));
        tokens
            .revoke(DEFAULT_TENANT, "u1", &id, at(1))
            .await
            .expect("revoke");
        assert!(matches!(
            tokens.revoke(DEFAULT_TENANT, "u1", &id, at(2)).await,
            Err(DbError::NotFound)
        ));
    }

    /// "Sign out everywhere" revokes every token of the user, live or expired, and counts the live ones.
    #[tokio::test]
    async fn revoke_all_revokes_every_token_of_the_user_only() {
        let db = db().await;
        let far = at(0) + TimeDelta::days(30);
        create(&db, "u1", &token("AAAAAAA1", "a", far), at(0)).await;
        create(&db, "u1", &token("AAAAAAA2", "b", at(5)), at(0)).await;
        create(&db, "u2", &token("AAAAAAA3", "c", far), at(0)).await;
        let tokens = db.access_tokens();
        assert_eq!(
            tokens
                .revoke_all(DEFAULT_TENANT, "u1", at(10))
                .await
                .expect("revoke"),
            1
        );
        assert!(
            tokens
                .list(DEFAULT_TENANT, "u1")
                .await
                .expect("list")
                .is_empty()
        );
        assert_eq!(
            tokens.list(DEFAULT_TENANT, "u2").await.expect("list").len(),
            1
        );
        assert_eq!(
            tokens
                .revoke_all(DEFAULT_TENANT, "u1", at(11))
                .await
                .expect("again"),
            0
        );
    }

    /// A use is recorded at most once a minute, and never on a revoked token.
    #[tokio::test]
    async fn touch_writes_at_most_once_a_minute() {
        let db = db().await;
        let id = AccessTokenId("AAAAAAA1".to_owned());
        create(
            &db,
            "u1",
            &token(&id.0, "agent", at(0) + TimeDelta::days(30)),
            at(0),
        )
        .await;
        let tokens = db.access_tokens();
        let last_used = || async {
            tokens
                .credential(&id)
                .await
                .expect("read")
                .expect("found")
                .record
                .last_used_at
        };
        tokens.touch(&id, at(1)).await.expect("touch");
        assert_eq!(last_used().await, Some(at(1)));
        tokens
            .touch(&id, at(1) + TimeDelta::seconds(59))
            .await
            .expect("touch");
        assert_eq!(last_used().await, Some(at(1)), "throttled");
        tokens.touch(&id, at(2)).await.expect("touch");
        assert_eq!(last_used().await, Some(at(2)));
        tokens
            .revoke(DEFAULT_TENANT, "u1", &id, at(3))
            .await
            .expect("revoke");
        tokens.touch(&id, at(9)).await.expect("touch");
        assert_eq!(last_used().await, Some(at(2)), "revoked");
    }

    /// A user removed from the tenant's allow-list loses its tokens at once, as its sessions.
    #[tokio::test]
    async fn credential_of_a_user_off_the_allow_list_is_absent() {
        let db = db().await;
        let id = AccessTokenId("AAAAAAA1".to_owned());
        create(
            &db,
            "u1",
            &token(&id.0, "agent", at(0) + TimeDelta::days(30)),
            at(0),
        )
        .await;
        remove_login(&db, DEFAULT_TENANT, "lklimek")
            .await
            .expect("remove");
        assert!(
            db.access_tokens()
                .credential(&id)
                .await
                .expect("read")
                .is_none()
        );
    }

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
