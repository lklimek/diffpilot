//! Resolves a batch of offered repositories against one read snapshot.

use std::collections::BTreeMap;

use sqlx::SqliteConnection;

use super::Repository;
use crate::error::DbError;

#[derive(Default)]
pub(super) struct Evidence {
    direct: bool,
    unbound: Vec<(String, String, String)>,
}

pub(super) struct Tracked<'a> {
    pub(super) repo: &'a Repository,
    evidence: Evidence,
}

#[derive(sqlx::FromRow)]
struct Match {
    position: i64,
    tenant_id: Option<String>,
    owner: Option<String>,
    name: Option<String>,
}

pub(super) async fn resolve<'a>(
    conn: &mut SqliteConnection,
    repos: &'a [Repository],
    limit: i64,
) -> Result<Vec<Tracked<'a>>, DbError> {
    let rows: Vec<Match> = sqlx::query_as(
        "WITH offered AS MATERIALIZED (
           SELECT CAST(key AS INTEGER) AS position, value ->> '$.id' AS id,
                  value ->> '$.name.owner' AS owner, value ->> '$.name.name' AS name
           FROM json_each(?1)
         ), owners AS (
           SELECT DISTINCT owner.value AS name
           FROM dashboard_settings ds JOIN memberships m USING (tenant_id, user_id),
                json_each(ds.enumerate_owners) owner
         ), pr_repos AS MATERIALIZED (
           SELECT DISTINCT r.github_id, r.owner, r.name
           FROM repos r JOIN pull_requests pr ON pr.repo_id = r.id
           WHERE EXISTS (SELECT 1 FROM pr_watches w JOIN memberships m USING (tenant_id, user_id)
                         WHERE w.pull_request_id = pr.id AND w.tenant_id = r.tenant_id AND w.state != 'ended')
              OR EXISTS (SELECT 1 FROM analyses a JOIN memberships m
                         ON m.tenant_id = a.tenant_id AND m.user_id = a.created_by
                         WHERE a.pull_request_id = pr.id AND a.tenant_id = r.tenant_id)
         ), named_tracking AS (
           SELECT ds.tenant_id,
                  rule.value ->> '$.repo.owner' AS owner, rule.value ->> '$.repo.name' AS name
           FROM dashboard_settings ds JOIN memberships m USING (tenant_id, user_id),
                json_each(ds.repositories) rule
           UNION ALL
           SELECT m.tenant_id, item.value ->> '$.facts.repo.owner', item.value ->> '$.facts.repo.name'
           FROM memberships m JOIN dashboard_refreshes d ON d.id = (
             SELECT id FROM dashboard_refreshes
             WHERE tenant_id = m.tenant_id AND user_id = m.user_id AND state IN ('ranking', 'finished')
             ORDER BY started_at DESC, rowid DESC LIMIT 1
           ), json_each(json_array(d.snapshot -> '$.pull_requests', d.snapshot -> '$.issues')) items,
              json_each(items.value) item
         ), names AS MATERIALIZED (
           SELECT DISTINCT t.tenant_id, t.owner, t.name, r.github_id
           FROM named_tracking t LEFT JOIN repos r
             ON r.tenant_id = t.tenant_id AND r.owner = t.owner AND r.name = t.name
           WHERE t.owner IS NOT NULL AND t.name IS NOT NULL
         ), matches AS MATERIALIZED (
           SELECT o.position, NULL AS tenant_id, NULL AS owner, NULL AS name
           FROM offered o JOIN owners p ON p.name = o.owner COLLATE NOCASE
           UNION
           SELECT o.position, NULL, NULL, NULL FROM offered o JOIN pr_repos r ON r.github_id = o.id
           UNION
           SELECT o.position, NULL, NULL, NULL FROM offered o JOIN pr_repos r
             ON r.owner = o.owner COLLATE NOCASE AND r.name = o.name COLLATE NOCASE WHERE o.id IS NULL
           UNION
           SELECT o.position, NULL, NULL, NULL FROM offered o JOIN names n ON n.github_id = o.id
           UNION
           SELECT o.position, NULL, NULL, NULL FROM offered o JOIN names n
             ON n.owner = o.owner COLLATE NOCASE AND n.name = o.name COLLATE NOCASE WHERE o.id IS NULL
           UNION
           SELECT o.position, n.tenant_id, n.owner, n.name FROM offered o JOIN names n
             ON n.owner = o.owner COLLATE NOCASE AND n.name = o.name COLLATE NOCASE
           WHERE o.id IS NOT NULL AND n.github_id IS NULL
             AND NOT EXISTS (SELECT 1 FROM app_repositories ar
                             WHERE ar.owner = o.owner AND ar.name = o.name AND ar.repository_id != o.id)
         ), eligible AS (
           SELECT DISTINCT position FROM matches ORDER BY position LIMIT ?2
         )
         SELECT m.* FROM eligible JOIN matches m USING(position)",
    )
    .bind(serde_json::to_string(repos)?)
    .bind(limit)
    .fetch_all(conn)
    .await?;

    let mut matched: BTreeMap<i64, Evidence> = BTreeMap::new();
    for row in rows {
        let proof = matched.entry(row.position).or_default();
        match (row.tenant_id, row.owner, row.name) {
            (Some(tenant), Some(owner), Some(name)) => proof.unbound.push((tenant, owner, name)),
            _ => proof.direct = true,
        }
    }
    matched
        .into_iter()
        .map(|(position, evidence)| {
            let repo = usize::try_from(position)
                .ok()
                .and_then(|position| repos.get(position))
                .ok_or(DbError::Corrupt { column: "position" })?;
            Ok(Tracked { repo, evidence })
        })
        .collect()
}

impl Tracked<'_> {
    /// A formerly unbound name may have acquired an ID while this receipt waited for the writer.
    pub(super) async fn bind(&self, conn: &mut SqliteConnection) -> Result<bool, DbError> {
        let mut allowed = self.evidence.direct;
        if let Some(id) = self.repo.id {
            for (tenant, owner, name) in &self.evidence.unbound {
                let bound = sqlx::query(
                    "INSERT INTO repos (tenant_id, owner, name, github_id)
                     SELECT ?1, ?2, ?3, ?4 WHERE NOT EXISTS (
                       SELECT 1 FROM app_repositories
                       WHERE owner = ?2 AND name = ?3 AND repository_id != ?4
                     )
                     ON CONFLICT(tenant_id, owner, name) DO UPDATE SET github_id = excluded.github_id
                       WHERE repos.github_id IS NULL OR repos.github_id = excluded.github_id",
                ).bind(tenant).bind(owner).bind(name).bind(id).execute(&mut *conn).await?;
                allowed |= bound.rows_affected() != 0;
            }
        }
        Ok(allowed)
    }
}
