//! Repository for `tenants` and the sign-in allowlist `tenant_allowed_logins` (ADR-007).

use chrono::{DateTime, Utc};
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

impl Tenants<'_> {
    /// Checks `login` (case-insensitive) against the tenant's allowlist; the first admission binds the entry to
    /// `github_id`, later admissions must present the same id.
    pub async fn admit_login(
        &self,
        tenant_id: &str,
        login: &str,
        github_id: i64,
        now: DateTime<Utc>,
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
        .bind(timestamp(Utc::now()))
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

    #[tokio::test]
    async fn first_admission_binds_the_id_and_later_ones_must_match() {
        let db = Db::in_memory().await.expect("db");
        let now = Utc::now();
        let tenants = db.tenants();
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
}
