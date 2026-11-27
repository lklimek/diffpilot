//! Repository for `repos`: the repositories a tenant has resolved pull requests of.

use contracts::common::RepoRef;
use sqlx::{Row, SqlitePool};

use super::Db;
use crate::error::DbError;

/// Access to known repositories.
#[derive(Debug)]
pub struct Repos<'a> {
    pool: &'a SqlitePool,
}

impl Db {
    /// Known repositories.
    pub fn repos(&self) -> Repos<'_> {
        Repos { pool: &self.pool }
    }
}

/// A stored repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoRecord {
    /// Row id.
    pub id: i64,
    /// Tenant.
    pub tenant_id: String,
    /// Owner and name as GitHub spells them.
    pub repo: RepoRef,
    /// GitHub repository id.
    pub github_id: Option<i64>,
}

impl Repos<'_> {
    /// Finds tenant-bound repository names for the supplied GitHub IDs.
    pub async fn bound_names(
        &self,
        tenant: &str,
        ids: &[i64],
    ) -> Result<Vec<(i64, RepoRef)>, DbError> {
        let rows: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT github_id, owner, name FROM repos
             WHERE tenant_id = ?1 AND github_id IN (SELECT value FROM json_each(?2))",
        )
        .bind(tenant)
        .bind(serde_json::to_string(ids)?)
        .fetch_all(self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, owner, name)| (id, RepoRef { owner, name }))
            .collect())
    }

    /// Inserts the repository or updates its spelling and GitHub id (owner and name match case-insensitively).
    pub async fn upsert(
        &self,
        tenant_id: &str,
        repo: &RepoRef,
        github_id: Option<i64>,
    ) -> Result<RepoRecord, DbError> {
        let row = sqlx::query(
            "INSERT INTO repos (tenant_id, owner, name, github_id) VALUES (?, ?, ?, ?)
             ON CONFLICT (tenant_id, owner, name) DO UPDATE SET owner = excluded.owner, name = excluded.name,
                 github_id = COALESCE(excluded.github_id, github_id)
             RETURNING id, tenant_id, owner, name, github_id",
        )
        .bind(tenant_id)
        .bind(&repo.owner)
        .bind(&repo.name)
        .bind(github_id)
        .fetch_one(self.pool)
        .await?;
        Ok(RepoRecord {
            id: row.try_get("id")?,
            tenant_id: row.try_get("tenant_id")?,
            repo: RepoRef {
                owner: row.try_get("owner")?,
                name: row.try_get("name")?,
            },
            github_id: row.try_get("github_id")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::DEFAULT_TENANT;

    fn repo(owner: &str, name: &str) -> RepoRef {
        RepoRef {
            owner: owner.to_owned(),
            name: name.to_owned(),
        }
    }

    #[tokio::test]
    async fn upsert_matches_case_insensitively_and_keeps_one_row() {
        let db = Db::in_memory().await.expect("db");
        let first = db
            .repos()
            .upsert(DEFAULT_TENANT, &repo("acme", "node-client"), None)
            .await
            .expect("insert");
        let second = db
            .repos()
            .upsert(DEFAULT_TENANT, &repo("Acme", "Node-Client"), Some(42))
            .await
            .expect("update");
        assert_eq!(first.id, second.id);
        assert_eq!(second.repo, repo("Acme", "Node-Client"));
        assert_eq!(second.github_id, Some(42));
        let third = db
            .repos()
            .upsert(DEFAULT_TENANT, &repo("acme", "node-client"), None)
            .await
            .expect("again");
        assert_eq!(
            third.github_id,
            Some(42),
            "a missing id keeps the known one"
        );
    }
}
