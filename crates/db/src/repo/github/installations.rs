//! Filters and applies installation coverage updates.

use std::collections::HashSet;

use contracts::common::RepoRef;
use contracts::time::Timestamp;
use sqlx::SqliteConnection;

use super::{
    INSTALLATION_CAP, InstallationUpdate, Repository,
    tracking::{self, Tracked},
};
use crate::error::DbError;
use crate::repo::jobs::ts;

pub(super) const UPDATE_CAP: i64 = 500;

pub(super) struct Applied {
    pub(super) capped: bool,
    pub(super) removed: String,
}

impl Applied {
    fn coverage(capped: bool) -> Self {
        Self {
            capped,
            removed: "[]".to_owned(),
        }
    }
}

pub(super) enum Prepared<'a> {
    Add(Vec<Tracked<'a>>),
    Remove(Vec<Repository>),
    Delete,
    Suspend(bool),
}

pub(super) async fn prepare<'a>(
    conn: &mut SqliteConnection,
    installation: i64,
    update: InstallationUpdate<'a>,
) -> Result<Option<Prepared<'a>>, DbError> {
    let prepared = match update {
        InstallationUpdate::Add(repos) => {
            let tracked = tracking::resolve(conn, repos, UPDATE_CAP + 1).await?;
            if tracked.is_empty() {
                return Ok(None);
            }
            Prepared::Add(tracked)
        }
        InstallationUpdate::Remove(repos) => {
            let matched: Vec<(String, String, Option<i64>)> = sqlx::query_as(
                "SELECT ar.owner, ar.name, MIN(offered.value ->> '$.id')
                 FROM app_repositories ar JOIN json_each(?2) offered
                   ON ar.repository_id = offered.value ->> '$.id'
                      OR (ar.repository_id IS NULL AND ar.owner = offered.value ->> '$.name.owner'
                          AND ar.name = offered.value ->> '$.name.name')
                 WHERE ar.installation_id = ?1
                 GROUP BY ar.owner, ar.name ORDER BY ar.owner, ar.name LIMIT ?3",
            )
            .bind(installation)
            .bind(serde_json::to_string(repos)?)
            .bind(UPDATE_CAP + 1)
            .fetch_all(&mut *conn)
            .await?;
            let mut removed: Vec<_> = matched
                .into_iter()
                .map(|(owner, name, id)| Repository {
                    name: RepoRef { owner, name },
                    id,
                })
                .collect();
            // A removal can race a lookup before the repository's first row exists.
            if removed.len() as i64 <= UPDATE_CAP {
                let mut names: HashSet<_> = removed
                    .iter()
                    .map(|repo| {
                        (
                            repo.name.owner.to_ascii_lowercase(),
                            repo.name.name.to_ascii_lowercase(),
                        )
                    })
                    .collect();
                for tracked in tracking::resolve(conn, repos, UPDATE_CAP + 1).await? {
                    let repo = tracked.repo;
                    if names.insert((
                        repo.name.owner.to_ascii_lowercase(),
                        repo.name.name.to_ascii_lowercase(),
                    )) {
                        removed.push(repo.clone());
                    }
                    if removed.len() as i64 > UPDATE_CAP {
                        break;
                    }
                }
            }
            if removed.is_empty() {
                return Ok(None);
            }
            Prepared::Remove(removed)
        }
        InstallationUpdate::Delete | InstallationUpdate::Suspend(_) => {
            if !known(conn, installation).await? {
                return Ok(None);
            }
            match update {
                InstallationUpdate::Suspend(value) => Prepared::Suspend(value),
                _ => Prepared::Delete,
            }
        }
    };
    Ok(Some(prepared))
}

pub(super) async fn known(conn: &mut SqliteConnection, installation: i64) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM app_repositories WHERE installation_id = ?)",
    )
    .bind(installation)
    .fetch_one(conn)
    .await?)
}

/// Applies coverage and records removal hints used to fence in-flight REST lookups.
pub(super) async fn apply(
    conn: &mut SqliteConnection,
    installation: i64,
    update: Prepared<'_>,
    now: Timestamp,
) -> Result<Option<Applied>, DbError> {
    match update {
        Prepared::Add(tracked) => {
            let mut repos = Vec::new();
            let mut ids = HashSet::new();
            let mut names = HashSet::new();
            let capped = tracked.len() as i64 > UPDATE_CAP;
            for matched in tracked.into_iter().take(UPDATE_CAP as usize) {
                let repo = matched.repo;
                let name = (
                    repo.name.owner.to_ascii_lowercase(),
                    repo.name.name.to_ascii_lowercase(),
                );
                if let Some(id) = repo.id
                    && !ids.contains(&id)
                    && !names.contains(&name)
                    && matched.bind(conn).await?
                {
                    ids.insert(id);
                    names.insert(name);
                    repos.push(repo);
                }
            }
            if repos.is_empty() {
                return Ok(None);
            }
            let offered = serde_json::to_string(&repos)?;
            sqlx::query(
                "DELETE FROM app_repositories AS ar WHERE EXISTS (
                   SELECT 1 FROM json_each(?1) offered
                   WHERE ar.owner = offered.value ->> '$.name.owner' AND ar.name = offered.value ->> '$.name.name'
                     AND ar.repository_id IS NOT offered.value ->> '$.id'
                 )",
            )
            .bind(&offered)
            .execute(&mut *conn)
            .await?;
            let applied = sqlx::query(
                "WITH offered AS (
                   SELECT CAST(key AS INTEGER) AS position, value ->> '$.id' AS id,
                          value ->> '$.name.owner' AS owner, value ->> '$.name.name' AS name
                   FROM json_each(?1)
                 ), ranked AS (
                   SELECT offered.*, ar.repository_id AS existing,
                          COUNT(*) FILTER (WHERE ar.repository_id IS NULL) OVER (ORDER BY position) AS new_rank
                   FROM offered LEFT JOIN app_repositories ar ON ar.repository_id = offered.id
                 )
                 INSERT INTO app_repositories (owner, name, repository_id, installation_id, suspended, checked_at)
                   SELECT owner, name, id, ?2, 0, ?3 FROM ranked
                   WHERE existing IS NOT NULL OR new_rank <= ?4 - (SELECT COUNT(*) FROM app_repositories)
                 ON CONFLICT(repository_id) DO UPDATE SET owner = excluded.owner, name = excluded.name,
                   installation_id = excluded.installation_id, suspended = excluded.suspended, checked_at = excluded.checked_at",
            ).bind(offered).bind(installation).bind(ts(now)).bind(INSTALLATION_CAP).execute(conn).await?;
            Ok(Some(Applied::coverage(
                capped || applied.rows_affected() < repos.len() as u64,
            )))
        }
        Prepared::Remove(mut repos) => {
            let capped = repos.len() as i64 > UPDATE_CAP;
            repos.truncate(UPDATE_CAP as usize);
            let removed = serde_json::to_string(&repos)?;
            sqlx::query(
                "DELETE FROM app_repositories AS ar WHERE installation_id = ?1 AND EXISTS (
                   SELECT 1 FROM json_each(?2) removed
                   WHERE ar.repository_id = removed.value ->> '$.id'
                      OR (ar.repository_id IS NULL AND ar.owner = removed.value ->> '$.name.owner'
                          AND ar.name = removed.value ->> '$.name.name')
                 )",
            )
            .bind(installation)
            .bind(&removed)
            .execute(conn)
            .await?;
            Ok(Some(Applied { capped, removed }))
        }
        Prepared::Delete => {
            sqlx::query("DELETE FROM app_repositories WHERE installation_id = ?")
                .bind(installation)
                .execute(conn)
                .await?;
            Ok(Some(Applied::coverage(false)))
        }
        Prepared::Suspend(suspended) => {
            sqlx::query("UPDATE app_repositories SET suspended = ?, checked_at = ? WHERE installation_id = ?")
                .bind(suspended).bind(ts(now)).bind(installation).execute(conn).await?;
            Ok(Some(Applied::coverage(false)))
        }
    }
}
