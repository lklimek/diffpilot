//! Resolves webhook hints against member snapshots and linked GitHub logins.

use contracts::common::RepoRef;
use contracts::time::Timestamp;
use sqlx::{FromRow, SqliteConnection};

use super::{Github, WebhookChange, parse_ts, ts};
use crate::error::DbError;
use crate::repo::write_tx;

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

// Match Comments PRs by the same repository identity used by watch routing.
pub(super) async fn invalidate_comments(
    conn: &mut SqliteConnection,
    hints: &str,
    now: Timestamp,
    only_pr: Option<i64>,
) -> Result<(), DbError> {
    sqlx::query(
        "WITH signals AS (
           SELECT value ->> '$.repository.id' AS repo_id,
                  value ->> '$.repository.name.owner' AS owner,
                  value ->> '$.repository.name.name' AS name,
                  value ->> '$.number' AS number,
                  CASE WHEN value ->> '$.action' = 'synchronize'
                         AND value ->> '$.kind' = 'pull_request'
                       THEN value ->> '$.head_sha' END AS head,
                  value ->> '$.synchronize_at' AS head_at,
                  value ->> '$.synchronize_before' AS prior_head
           FROM json_each(?1)
           WHERE value ->> '$.kind' IN ('pull_request','pull_request_review')
         ), affected AS (
           SELECT p.id, s.head, s.head_at, s.prior_head
           FROM signals s JOIN repos r ON r.github_id = s.repo_id
             OR ((s.repo_id IS NULL OR r.github_id IS NULL)
                 AND r.owner = s.owner COLLATE NOCASE AND r.name = s.name COLLATE NOCASE)
           JOIN pull_requests p ON p.repo_id = r.id AND p.number = s.number
           WHERE (?3 IS NULL OR p.id = ?3) AND EXISTS (
             SELECT 1 FROM app_repositories ar WHERE ar.suspended = 0
               AND (ar.repository_id = r.github_id OR (
                 ar.owner = r.owner COLLATE NOCASE AND ar.name = r.name COLLATE NOCASE
                 AND (ar.repository_id IS NULL OR r.github_id IS NULL)))
           )
         )
         UPDATE comment_prs AS c SET github_dirty_at = MAX(COALESCE(c.github_dirty_at, ?2), ?2),
           (github_head, github_head_at, github_head_before) = (
             SELECT CASE WHEN fresh THEN a.head ELSE c.github_head END,
                    CASE WHEN fresh THEN a.head_at ELSE c.github_head_at END,
                    CASE WHEN fresh THEN COALESCE(a.prior_head, c.github_snapshot ->> '$.head')
                         ELSE c.github_head_before END
             FROM (SELECT a.head IS NOT NULL AND (
               julianday(a.head_at) > latest
               OR (COALESCE(c.github_dirty_at, ?2) <= ?2
                   AND a.prior_head = current_head COLLATE NOCASE
                   AND (julianday(a.head_at) = latest
                        OR (a.head_at IS NULL AND c.github_head_at IS NULL)))
             ) AS fresh
             FROM (SELECT MAX(COALESCE(julianday(c.github_head_at), 0),
                              COALESCE(julianday(c.github_snapshot ->> '$.updated_at'), 0)) AS latest,
                          COALESCE(c.github_head, c.github_snapshot ->> '$.head') AS current_head))
           )
         FROM affected a WHERE c.pull_request_id = a.id",
    )
    .bind(hints)
    .bind(ts(now))
    .bind(only_pr)
    .execute(conn)
    .await?;
    Ok(())
}

impl Github<'_> {
    /// Applies admitted changes received during the first GitHub read, before the PR had a local row.
    pub async fn reconcile_first_comments_read(
        &self,
        pr: i64,
        started_at: Timestamp,
    ) -> Result<(), DbError> {
        let mut tx = write_tx(self.pool).await?;
        let receipts: Vec<(String, String)> = sqlx::query_as(
            "SELECT d.comment_hints, d.received_at
             FROM pull_requests p JOIN repos r ON r.id = p.repo_id
             JOIN github_deliveries d ON d.received_at >= ?2 AND d.comment_hints IS NOT NULL
             WHERE p.id = ?1 AND EXISTS (
               SELECT 1 FROM json_each(d.comment_hints) h
               WHERE h.value ->> '$.number' = p.number
                 AND (h.value ->> '$.repository.id' = r.github_id
                   OR (h.value ->> '$.repository.id' IS NULL
                     AND h.value ->> '$.repository.name.owner' = r.owner COLLATE NOCASE
                     AND h.value ->> '$.repository.name.name' = r.name COLLATE NOCASE))
             )
             ORDER BY d.received_at, d.rowid",
        )
        .bind(pr)
        .bind(ts(started_at))
        .fetch_all(&mut *tx)
        .await?;
        for (hints, received_at) in receipts {
            invalidate_comments(
                &mut tx,
                &hints,
                parse_ts(&received_at, "received_at")?,
                Some(pr),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Finds watched or previously read PRs and whether their review or comments changed.
    pub async fn review_targets(
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
             WHERE EXISTS (SELECT 1 FROM comment_prs c WHERE c.pull_request_id = p.id
                           AND (c.read_at IS NOT NULL OR c.github_dirty_at IS NOT NULL))
                OR EXISTS (SELECT 1 FROM pr_watches w JOIN memberships m USING (tenant_id, user_id)
                           WHERE w.pull_request_id = p.id AND w.state != 'ended')
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
             SELECT s.position AS change, i.tenant_id, i.user_id,
                    CASE WHEN s.kind = 'pull_request' AND s.lifecycle THEN s.owner ELSE i.owner END AS owner,
                    CASE WHEN s.kind = 'pull_request' AND s.lifecycle THEN s.name ELSE i.name END AS name,
                    i.number, 0 AS candidate
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
