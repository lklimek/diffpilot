//! Repository for `users` and their tenant `memberships`.

use chrono::{DateTime, Utc};
use contracts::common::LlmLanguage;
use serde_json::Value;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqlitePool};

use super::Db;
use super::sessions::timestamp;
use crate::error::DbError;

/// Access to users.
#[derive(Debug)]
pub struct Users<'a> {
    pool: &'a SqlitePool,
}

impl Db {
    /// Users and their memberships.
    pub fn users(&self) -> Users<'_> {
        Users { pool: &self.pool }
    }
}

/// GitHub profile data of a signed-in user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubProfile {
    /// Numeric GitHub user id.
    pub github_id: i64,
    /// GitHub login.
    pub login: String,
    /// Display name.
    pub name: Option<String>,
    /// Avatar URL.
    pub avatar_url: Option<String>,
}

/// A stored user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserRecord {
    /// Server user id.
    pub id: String,
    /// GitHub profile as of the last sign-in.
    pub profile: GithubProfile,
    /// Language of LLM texts.
    pub language: LlmLanguage,
}

impl Users<'_> {
    /// Creates the user with id `new_id`, or updates the profile of the user with the same GitHub id, and makes
    /// it a member of `tenant_id`; returns the stored user.
    pub async fn upsert_member(
        &self,
        tenant_id: &str,
        new_id: &str,
        profile: &GithubProfile,
        now: DateTime<Utc>,
    ) -> Result<UserRecord, DbError> {
        let now = timestamp(now);
        let mut tx = super::write_tx(self.pool).await?;
        let row = sqlx::query(
            "INSERT INTO users (id, github_id, login, name, avatar_url, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (github_id) DO UPDATE SET login = excluded.login, name = excluded.name,
                 avatar_url = excluded.avatar_url, updated_at = excluded.updated_at
             RETURNING id, github_id, login, name, avatar_url, language",
        )
        .bind(new_id)
        .bind(profile.github_id)
        .bind(&profile.login)
        .bind(&profile.name)
        .bind(&profile.avatar_url)
        .bind(&now)
        .bind(&now)
        .fetch_one(&mut *tx)
        .await?;
        let user = user_from_row(&row)?;
        sqlx::query(
            "INSERT INTO memberships (tenant_id, user_id, created_at) VALUES (?, ?, ?)
             ON CONFLICT (tenant_id, user_id) DO NOTHING",
        )
        .bind(tenant_id)
        .bind(&user.id)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(user)
    }

    /// The user with this id; `NotFound` if absent.
    pub async fn get(&self, id: &str) -> Result<UserRecord, DbError> {
        let row = sqlx::query(
            "SELECT id, github_id, login, name, avatar_url, language FROM users WHERE id = ?",
        )
        .bind(id)
        .fetch_one(self.pool)
        .await?;
        user_from_row(&row)
    }

    /// The user with this GitHub id, if known.
    pub async fn find_by_github_id(&self, github_id: i64) -> Result<Option<UserRecord>, DbError> {
        let row = sqlx::query(
            "SELECT id, github_id, login, name, avatar_url, language FROM users WHERE github_id = ?",
        )
        .bind(github_id)
        .fetch_optional(self.pool)
        .await?;
        row.as_ref().map(user_from_row).transpose()
    }
}

fn user_from_row(row: &SqliteRow) -> Result<UserRecord, DbError> {
    let language: String = row.try_get("language")?;
    Ok(UserRecord {
        id: row.try_get("id")?,
        profile: GithubProfile {
            github_id: row.try_get("github_id")?,
            login: row.try_get("login")?,
            name: row.try_get("name")?,
            avatar_url: row.try_get("avatar_url")?,
        },
        language: serde_json::from_value(Value::String(language)).map_err(|_| {
            DbError::Corrupt {
                column: "users.language",
            }
        })?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::DEFAULT_TENANT;

    fn profile(login: &str, name: Option<&str>) -> GithubProfile {
        GithubProfile {
            github_id: 42,
            login: login.to_owned(),
            name: name.map(str::to_owned),
            avatar_url: Some("https://avatars.example.test/42".to_owned()),
        }
    }

    #[tokio::test]
    async fn upsert_keeps_the_id_of_a_known_github_user_and_updates_the_profile() {
        let db = Db::in_memory().await.expect("db");
        let users = db.users();
        let first = users
            .upsert_member(DEFAULT_TENANT, "u-1", &profile("lklimek", None), Utc::now())
            .await
            .expect("insert");
        assert_eq!(first.id, "u-1");
        assert_eq!(first.language, LlmLanguage::En);
        let second = users
            .upsert_member(
                DEFAULT_TENANT,
                "u-2",
                &profile("lklimek", Some("Lukasz")),
                Utc::now(),
            )
            .await
            .expect("update");
        assert_eq!(second.id, "u-1");
        assert_eq!(second.profile.name.as_deref(), Some("Lukasz"));
        assert_eq!(users.get("u-1").await.expect("get"), second);
        assert!(matches!(users.get("u-2").await, Err(DbError::NotFound)));
        assert_eq!(
            users.find_by_github_id(42).await.expect("find"),
            Some(second.clone())
        );
        assert_eq!(users.find_by_github_id(43).await.expect("find"), None);
        let members: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM memberships WHERE user_id = 'u-1'")
                .fetch_one(&db.pool)
                .await
                .expect("count");
        assert_eq!(members, 1);
    }
}
