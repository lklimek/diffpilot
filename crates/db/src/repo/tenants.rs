//! Repository for `tenants` and the sign-in allowlist `tenant_allowed_logins` (ADR-007).

use contracts::time::Timestamp;
use sqlx::SqlitePool;

use super::Db;
use super::sessions::timestamp;
use crate::error::DbError;

/// Access to tenants and their sign-in allowlist.
#[derive(Debug)]
pub struct Tenants<'a> {
    pool: &'a SqlitePool,
}

impl Db {
    /// Tenants and their sign-in allowlist.
    pub fn tenants(&self) -> Tenants<'_> {
        Tenants { pool: &self.pool }
    }
}

/// Outcome of [`Tenants::admit_login`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginAdmission {
    /// The login is allowed and bound to this GitHub user id.
    Allowed,
    /// The login is not in the tenant's allowlist.
    NotListed,
    /// The login is listed but bound to another GitHub user id, or the id is bound to another login.
    IdMismatch,
}

/// SQL condition on a row of user `?2` in tenant `?1`, the user found by the GitHub id an allowlist entry was bound to.
macro_rules! of_user {
    () => {
        "tenant_id = ?1 AND user_id IN (SELECT id FROM users WHERE github_id = ?2)"
    };
}

/// Outcome of [`Tenants::reconcile_logins`]; logins as stored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AllowlistChange {
    /// Logins added, unbound.
    pub added: Vec<String>,
    /// Logins removed.
    pub removed: Vec<String>,
}

impl Tenants<'_> {
    /// Makes the tenant's allowlist equal `logins` (case-insensitive) in one transaction: adds missing logins
    /// unbound, keeps listed entries with their binding, and removes the others. A removed bound entry also ends
    /// its user's access in the tenant: sessions and GitHub token pair deleted, access tokens revoked at `now`, so
    /// listing the login again revives none of them (ADR-007).
    pub async fn reconcile_logins(
        &self,
        tenant_id: &str,
        logins: &[String],
        now: Timestamp,
    ) -> Result<AllowlistChange, DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let stored: Vec<(String, Option<i64>)> = sqlx::query_as(
            "SELECT login, github_id FROM tenant_allowed_logins WHERE tenant_id = ? ORDER BY login",
        )
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await?;
        let mut change = AllowlistChange::default();
        // `COLLATE NOCASE` folds ASCII only, as `eq_ignore_ascii_case`.
        for (login, github_id) in stored {
            if logins
                .iter()
                .any(|wanted| wanted.eq_ignore_ascii_case(&login))
            {
                continue;
            }
            if let Some(github_id) = github_id {
                for sql in [
                    concat!("DELETE FROM sessions WHERE ", of_user!()),
                    concat!("DELETE FROM user_tokens WHERE ", of_user!()),
                ] {
                    sqlx::query(sql)
                        .bind(tenant_id)
                        .bind(github_id)
                        .execute(&mut *tx)
                        .await?;
                }
                sqlx::query(concat!(
                    "UPDATE access_tokens SET revoked_at = ?3 WHERE revoked_at IS NULL AND ",
                    of_user!()
                ))
                .bind(tenant_id)
                .bind(github_id)
                .bind(timestamp(now))
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query("DELETE FROM tenant_allowed_logins WHERE tenant_id = ? AND login = ?")
                .bind(tenant_id)
                .bind(&login)
                .execute(&mut *tx)
                .await?;
            change.removed.push(login);
        }
        for login in logins {
            let inserted = sqlx::query(
                "INSERT INTO tenant_allowed_logins (tenant_id, login) VALUES (?, ?)
                 ON CONFLICT (tenant_id, login) DO NOTHING",
            )
            .bind(tenant_id)
            .bind(login)
            .execute(&mut *tx)
            .await?;
            if inserted.rows_affected() == 1 {
                change.added.push(login.clone());
            }
        }
        tx.commit().await?;
        Ok(change)
    }

    /// Checks `login` (case-insensitive) against the tenant's allowlist; the first admission binds the entry to
    /// `github_id`, later admissions must present the same id.
    pub async fn admit_login(
        &self,
        tenant_id: &str,
        login: &str,
        github_id: i64,
        now: Timestamp,
    ) -> Result<LoginAdmission, DbError> {
        let bound = sqlx::query(
            "UPDATE tenant_allowed_logins SET github_id = ?, bound_at = ?
             WHERE tenant_id = ? AND login = ? AND github_id IS NULL",
        )
        .bind(github_id)
        .bind(timestamp(now))
        .bind(tenant_id)
        .bind(login)
        .execute(self.pool)
        .await
        .map_err(DbError::from);
        match bound {
            Ok(_) => {}
            // The id is already bound to another login of the tenant.
            Err(DbError::Conflict) => return Ok(LoginAdmission::IdMismatch),
            Err(error) => return Err(error),
        }
        let entry: Option<Option<i64>> = sqlx::query_scalar(
            "SELECT github_id FROM tenant_allowed_logins WHERE tenant_id = ? AND login = ?",
        )
        .bind(tenant_id)
        .bind(login)
        .fetch_optional(self.pool)
        .await?;
        Ok(match entry {
            None => LoginAdmission::NotListed,
            Some(Some(bound_id)) if bound_id == github_id => LoginAdmission::Allowed,
            Some(_) => LoginAdmission::IdMismatch,
        })
    }
}

/// Allowlist edits for tests (`test-util` feature); v1 has no API for them (ADR-007).
#[cfg(any(test, feature = "test-util"))]
pub mod testing {
    use super::*;

    /// Puts `login` on the tenant's allowlist, bound to `github_id` (replacing an existing entry's binding).
    pub async fn allow_login(
        db: &Db,
        tenant_id: &str,
        login: &str,
        github_id: i64,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO tenant_allowed_logins (tenant_id, login, github_id, bound_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (tenant_id, login) DO UPDATE SET github_id = excluded.github_id,
                 bound_at = excluded.bound_at",
        )
        .bind(tenant_id)
        .bind(login)
        .bind(github_id)
        .bind(timestamp(Timestamp::now()))
        .execute(&db.pool)
        .await?;
        Ok(())
    }

    /// Review repro: every recorded scheduled run of `user_id` fails (persistent per-user database error).
    pub async fn fail_scheduled_run_records(db: &Db, user_id: &str) -> Result<(), DbError> {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE TRIGGER fail_scheduled_run BEFORE UPDATE OF last_scheduled_on ON dashboard_settings
                 WHEN NEW.user_id = '{user_id}'
                 BEGIN SELECT RAISE(ABORT, 'injected scheduled run record failure'); END;"
        )))
        .execute(&db.pool)
        .await?;
        Ok(())
    }

    /// Makes the dashboard settings row of `user_id` undecodable (a repository rule that is not an object).
    pub async fn corrupt_dashboard_settings(db: &Db, user_id: &str) -> Result<(), DbError> {
        sqlx::query("UPDATE dashboard_settings SET repositories = '[1]' WHERE user_id = ?")
            .bind(user_id)
            .execute(&db.pool)
            .await?;
        Ok(())
    }

    /// Removes `login` from the tenant's allowlist, as the owner would by hand.
    pub async fn remove_login(db: &Db, tenant_id: &str, login: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM tenant_allowed_logins WHERE tenant_id = ? AND login = ?")
            .bind(tenant_id)
            .bind(login)
            .execute(&db.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::DEFAULT_TENANT;

    fn logins(list: &[&str]) -> Vec<String> {
        list.iter().map(|login| (*login).to_owned()).collect()
    }

    async fn allowlist(db: &Db) -> Vec<(String, Option<i64>)> {
        sqlx::query_as(
            "SELECT login, github_id FROM tenant_allowed_logins WHERE tenant_id = ? ORDER BY login",
        )
        .bind(DEFAULT_TENANT)
        .fetch_all(&db.pool)
        .await
        .expect("allowlist")
    }

    #[tokio::test]
    async fn first_admission_binds_the_id_and_later_ones_must_match() {
        let db = Db::in_memory().await.expect("db");
        let now = Timestamp::now();
        let tenants = db.tenants();
        tenants
            .reconcile_logins(
                DEFAULT_TENANT,
                &logins(&["lklimek", "Claudius-Maginificent"]),
                Timestamp::now(),
            )
            .await
            .expect("reconcile");
        let admit =
            |login: &'static str, id: i64| tenants.admit_login(DEFAULT_TENANT, login, id, now);
        assert_eq!(
            admit("lklimek", 42).await.expect("admit"),
            LoginAdmission::Allowed
        );
        assert_eq!(
            admit("LKLIMEK", 42).await.expect("admit"),
            LoginAdmission::Allowed,
            "case-insensitive"
        );
        assert_eq!(
            admit("lklimek", 43).await.expect("admit"),
            LoginAdmission::IdMismatch
        );
        assert_eq!(
            admit("Claudius-Maginificent", 42).await.expect("admit"),
            LoginAdmission::IdMismatch,
            "an id bound to one login cannot take over another entry"
        );
        assert_eq!(
            admit("mallory", 44).await.expect("admit"),
            LoginAdmission::NotListed
        );
        assert_eq!(
            tenants
                .admit_login("other-tenant", "lklimek", 42, now)
                .await
                .expect("admit"),
            LoginAdmission::NotListed
        );
    }

    #[tokio::test]
    async fn reconcile_adds_missing_logins_unbound_and_removes_unlisted_ones() {
        let db = Db::in_memory().await.expect("db");
        let tenants = db.tenants();
        assert_eq!(allowlist(&db).await, [], "a fresh database allows nobody");
        let first = tenants
            .reconcile_logins(DEFAULT_TENANT, &logins(&["alice", "Bob"]), Timestamp::now())
            .await
            .expect("first");
        assert_eq!(
            first,
            AllowlistChange {
                added: logins(&["alice", "Bob"]),
                removed: Vec::new()
            }
        );
        testing::allow_login(&db, DEFAULT_TENANT, "alice", 7)
            .await
            .expect("bind alice");

        let second = tenants
            .reconcile_logins(
                DEFAULT_TENANT,
                &logins(&["ALICE", "carol"]),
                Timestamp::now(),
            )
            .await
            .expect("second");
        assert_eq!(
            second,
            AllowlistChange {
                added: logins(&["carol"]),
                removed: logins(&["Bob"])
            },
            "logins compare case-insensitively"
        );
        assert_eq!(
            allowlist(&db).await,
            [("alice".to_owned(), Some(7)), ("carol".to_owned(), None)],
            "a kept entry keeps its spelling and binding"
        );
        let again = tenants
            .reconcile_logins(
                DEFAULT_TENANT,
                &logins(&["ALICE", "carol"]),
                Timestamp::now(),
            )
            .await
            .expect("unchanged");
        assert_eq!(again, AllowlistChange::default());
        let other = tenants
            .reconcile_logins("other-tenant", &[], Timestamp::now())
            .await
            .expect("other tenant");
        assert_eq!(other, AllowlistChange::default());
        assert_eq!(allowlist(&db).await.len(), 2, "other tenants are untouched");
    }

    /// ADR-007 addendum: removing a bound entry ends its user's sessions, GitHub token pair and access tokens, so
    /// adding the login again later revives none of them.
    #[tokio::test]
    async fn reconcile_removing_a_bound_login_ends_its_access_for_good() {
        use crate::repo::access_tokens::NewAccessToken;
        use crate::repo::sessions::{Liveness, NewSession};
        use crate::repo::tokens::EncryptedTokens;
        use crate::repo::users::GithubProfile;
        use contracts::api::access_tokens::{AccessTokenId, TokenScope};
        use jiff::SignedDuration;

        let db = Db::in_memory().await.expect("db");
        let now = Timestamp::now();
        let tenants = db.tenants();
        tenants
            .reconcile_logins(DEFAULT_TENANT, &logins(&["alice", "bob"]), Timestamp::now())
            .await
            .expect("allow");
        for (user, login, github_id) in [("u-alice", "alice", 7), ("u-bob", "bob", 8)] {
            let profile = GithubProfile {
                github_id,
                login: login.to_owned(),
                name: None,
                avatar_url: None,
            };
            db.users()
                .upsert_member(DEFAULT_TENANT, user, &profile, now)
                .await
                .expect("user");
            assert_eq!(
                tenants
                    .admit_login(DEFAULT_TENANT, login, github_id, now)
                    .await
                    .expect("admit"),
                LoginAdmission::Allowed
            );
            db.sessions()
                .create(
                    &NewSession {
                        token_hash: user.as_bytes(),
                        tenant_id: DEFAULT_TENANT,
                        user_id: user,
                        csrf_token: "csrf",
                        expires_at: now + SignedDuration::from_hours(1),
                    },
                    now,
                )
                .await
                .expect("session");
            db.user_tokens()
                .put(
                    user,
                    DEFAULT_TENANT,
                    &EncryptedTokens {
                        access_ct: vec![1],
                        access_expires_at: now + SignedDuration::from_hours(8),
                        refresh_ct: vec![2],
                        refresh_expires_at: now + SignedDuration::from_hours(24),
                    },
                    now,
                )
                .await
                .expect("pair");
            let mut hash = [0u8; 32];
            hash[..user.len()].copy_from_slice(user.as_bytes());
            db.access_tokens()
                .create(
                    DEFAULT_TENANT,
                    user,
                    &NewAccessToken {
                        id: AccessTokenId(format!("{:0>8}", &user[2..])),
                        name: "agent".to_owned(),
                        token_hash: hash,
                        scopes: vec![TokenScope::Read],
                        expires_at: now + SignedDuration::from_hours(24),
                    },
                    now,
                )
                .await
                .expect("access token");
        }

        let change = tenants
            .reconcile_logins(DEFAULT_TENANT, &logins(&["bob"]), Timestamp::now())
            .await
            .expect("remove alice");
        assert_eq!(change.removed, logins(&["alice"]));
        // Back on the list, and alice signs in again with the same GitHub id.
        tenants
            .reconcile_logins(DEFAULT_TENANT, &logins(&["bob", "alice"]), Timestamp::now())
            .await
            .expect("add alice again");
        assert_eq!(
            tenants
                .admit_login(DEFAULT_TENANT, "alice", 7, now)
                .await
                .expect("admit"),
            LoginAdmission::Allowed
        );

        let live = Liveness {
            now,
            seen_after: now - SignedDuration::from_hours(1),
        };
        let sessions = db.sessions();
        assert!(
            sessions
                .live(b"u-alice", live)
                .await
                .expect("read")
                .is_none(),
            "the removed user's session ended"
        );
        assert!(sessions.live(b"u-bob", live).await.expect("read").is_some());
        assert!(
            db.user_tokens()
                .get("u-alice")
                .await
                .expect("read")
                .is_none(),
            "the removed user's GitHub token pair is deleted"
        );
        assert!(db.user_tokens().get("u-bob").await.expect("read").is_some());
        let credential = |id: &str| {
            let id = AccessTokenId(id.to_owned());
            let tokens = db.access_tokens();
            async move { tokens.credential(&id).await.expect("read") }
        };
        let alice_token = credential("000alice").await.expect("listed again");
        assert!(
            alice_token.record.revoked_at.is_some(),
            "the removed user's access tokens are revoked"
        );
        let bob_token = credential("00000bob").await.expect("bob's token");
        assert!(bob_token.record.revoked_at.is_none());
    }
}
