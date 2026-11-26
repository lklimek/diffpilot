//! Resolves webhook hints against member snapshots and linked GitHub logins.

use contracts::common::RepoRef;
use sqlx::FromRow;

use super::{Github, WebhookChange};
use crate::error::DbError;

/// A current dashboard item or a candidate for a targeted read.
#[derive(Debug)]
pub struct DashboardTarget {
    /// Position of the triggering hint in the delivery.
    pub change: usize,
    /// Tenant that owns the snapshot.
    pub tenant_id: String,
    /// Member that owns the snapshot and GitHub token.
    pub user_id: String,
    /// Item's repository.
    pub repo: RepoRef,
    /// PR or issue number.
    pub number: u32,
    /// The item is absent and may qualify for admission to the ranking.
    pub candidate: bool,
}

#[derive(FromRow)]
struct Target {
    change: i64,
    tenant_id: String,
    user_id: String,
    owner: String,
    name: String,
    number: u32,
    candidate: bool,
}

impl Github<'_> {
    /// Finds watched PRs and whether their review or comments changed.
    pub async fn watch_targets(
        &self,
        changes: &[WebhookChange],
    ) -> Result<Vec<(i64, bool)>, DbError> {
        Ok(sqlx::query_as(
            "WITH signals AS (
               SELECT value ->> '$.kind' = 'pull_request_review' AS review,
                      value ->> '$.repository.id' AS repo_id,
                      value ->> '$.repository.name.owner' AS owner,
                      value ->> '$.repository.name.name' AS name,
                      value ->> '$.number' AS number
               FROM json_each(?1)
               WHERE value ->> '$.kind' IN ('pull_request','pull_request_review')
             )
             SELECT p.id, MAX(s.review) FROM signals s JOIN repos r ON r.github_id = s.repo_id
               OR ((s.repo_id IS NULL OR r.github_id IS NULL)
                   AND r.owner = s.owner COLLATE NOCASE AND r.name = s.name COLLATE NOCASE)
             JOIN pull_requests p ON p.repo_id = r.id AND p.number = s.number
             JOIN pr_watches w ON w.pull_request_id = p.id AND w.state != 'ended'
             JOIN memberships m ON m.tenant_id = w.tenant_id AND m.user_id = w.user_id
             GROUP BY p.id",
        )
        .bind(serde_json::to_string(changes)?)
        .fetch_all(self.pool)
        .await?)
    }

    /// Maps hints by ranked item, check identity or a login tied to the dashboard.
    /// History reads can wait for an initial scan to produce its first snapshot.
    pub async fn dashboard_targets(
        &self,
        changes: &[WebhookChange],
        snapshot_version: u32,
    ) -> Result<Vec<DashboardTarget>, DbError> {
        let rows: Vec<Target> = sqlx::query_as(
            "WITH signals AS MATERIALIZED (
               SELECT CAST(key AS INTEGER) AS position, value ->> '$.kind' AS kind,
                      value ->> '$.repository.id' AS repo_id,
                      value ->> '$.repository.name.owner' AS owner,
                      value ->> '$.repository.name.name' AS name,
                      value ->> '$.base_repository.id' AS base_id,
                      value ->> '$.base_repository.name.owner' AS base_owner,
                      value ->> '$.base_repository.name.name' AS base_name,
                      value ->> '$.number' AS number, value ->> '$.head_sha' AS sha,
                      value ->> '$.candidate_login' AS login,
                      COALESCE(value ->> '$.action' IN ('closed', 'reopened'), 0) AS lifecycle
               FROM json_each(?1)
             ), snapshots AS MATERIALIZED (
               SELECT m.tenant_id, m.user_id, u.login, d.snapshot
               FROM memberships m JOIN users u ON u.id = m.user_id
               LEFT JOIN dashboard_refreshes d ON d.id = (
                 SELECT id FROM dashboard_refreshes
                 WHERE tenant_id = m.tenant_id AND user_id = m.user_id AND state IN ('ranking', 'finished')
                 ORDER BY started_at DESC, rowid DESC LIMIT 1
               ) AND d.snapshot ->> '$.version' = ?2
               WHERE d.id IS NOT NULL OR (
                 EXISTS (SELECT 1 FROM signals WHERE kind = 'pull_request' AND lifecycle)
                 AND EXISTS (
                   SELECT 1 FROM dashboard_refreshes
                   WHERE tenant_id = m.tenant_id AND user_id = m.user_id AND state = 'scanning'
                 )
               )
             ), items AS MATERIALIZED (
               SELECT d.tenant_id, d.user_id, lists.key = 1 AS issue,
                      item.value ->> '$.facts.repo.owner' AS owner,
                      item.value ->> '$.facts.repo.name' AS name,
                      item.value ->> '$.facts.number' AS number,
                      item.value ->> '$.head_sha' AS sha,
                      item.value ->> '$.head_repository.owner' AS head_owner,
                      item.value ->> '$.head_repository.name' AS head_name
               FROM snapshots d, json_each(json_array(
                  d.snapshot -> '$.pull_requests', d.snapshot -> '$.issues')) lists,
                  json_each(lists.value) item
             )
             SELECT s.position AS change, i.tenant_id, i.user_id, i.owner, i.name, i.number, 0 AS candidate
             FROM signals s JOIN items i
             LEFT JOIN repos r ON r.tenant_id = i.tenant_id AND r.owner = i.owner AND r.name = i.name
             WHERE (
                 s.kind != 'check' AND i.issue = (s.kind = 'issue') AND i.number = s.number
                 AND ((s.owner = i.owner COLLATE NOCASE AND s.name = i.name COLLATE NOCASE
                       AND (r.github_id IS NULL OR s.repo_id IS NULL)) OR r.github_id = s.repo_id)
             ) OR (
                 s.kind = 'check' AND NOT i.issue
                 AND (((s.owner = i.owner COLLATE NOCASE AND s.name = i.name COLLATE NOCASE)
                       AND (r.github_id IS NULL OR s.repo_id IS NULL)) OR r.github_id = s.repo_id
                      OR (s.owner = i.head_owner COLLATE NOCASE AND s.name = i.head_name COLLATE NOCASE))
                 AND (
                   (s.number = i.number AND s.base_owner IS NOT NULL
                     AND (r.github_id = s.base_id OR (
                       i.owner = s.base_owner COLLATE NOCASE AND i.name = s.base_name COLLATE NOCASE
                       AND (r.github_id IS NULL OR s.base_id IS NULL))))
                   OR (s.base_owner IS NULL AND i.sha = s.sha COLLATE NOCASE
                       AND (s.number IS NULL OR (s.number = i.number
                         AND i.owner = s.owner COLLATE NOCASE AND i.name = s.name COLLATE NOCASE)))
                 )
             )
             UNION ALL
             SELECT s.position, d.tenant_id, d.user_id, s.owner, s.name, s.number, NOT s.lifecycle
             FROM signals s JOIN snapshots d
             LEFT JOIN dashboard_settings settings USING (tenant_id, user_id)
             WHERE s.kind = 'pull_request' AND s.login IS NOT NULL AND s.number IS NOT NULL
               AND (s.lifecycle OR d.snapshot IS NOT NULL)
               AND (NOT s.lifecycle OR NOT EXISTS (
                 SELECT 1 FROM repos r
                 WHERE r.tenant_id = d.tenant_id AND r.owner = s.owner COLLATE NOCASE
                   AND r.name = s.name COLLATE NOCASE AND r.github_id != s.repo_id
               ))
               AND (d.login = s.login COLLATE NOCASE OR EXISTS (
                    SELECT 1 FROM json_each(settings.linked_logins)
                    WHERE value = s.login COLLATE NOCASE
               ) OR (s.lifecycle AND (
                    d.snapshot ->> '$.own_login' = s.login COLLATE NOCASE OR EXISTS (
                        SELECT 1 FROM json_each(d.snapshot -> '$.settings.linked_logins')
                        WHERE value = s.login COLLATE NOCASE
                    )
               )))
               AND NOT EXISTS (SELECT 1 FROM items i WHERE i.tenant_id = d.tenant_id AND i.user_id = d.user_id
                   AND NOT i.issue AND i.number = s.number
                   AND i.owner = s.owner COLLATE NOCASE AND i.name = s.name COLLATE NOCASE)",
        )
        .bind(serde_json::to_string(changes)?)
        .bind(snapshot_version)
        .fetch_all(self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(DashboardTarget {
                    change: usize::try_from(row.change)
                        .map_err(|_| DbError::Corrupt { column: "change" })?,
                    tenant_id: row.tenant_id,
                    user_id: row.user_id,
                    repo: RepoRef {
                        owner: row.owner,
                        name: row.name,
                    },
                    number: row.number,
                    candidate: row.candidate,
                })
            })
            .collect()
    }
}
