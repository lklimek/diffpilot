//! Repository for `pull_requests`: the latest snapshot of each resolved pull request.

use chrono::{DateTime, Utc};
use contracts::api::pr::PrSummary;
use contracts::common::RepoRef;
use sqlx::SqlitePool;

use super::jobs::ts;
use super::{Db, read_tx};
use crate::error::DbError;

/// Access to pull-request snapshots.
#[derive(Debug)]
pub struct PullRequests<'a> {
    pool: &'a SqlitePool,
}

impl Db {
    /// Pull-request snapshots.
    pub fn pull_requests(&self) -> PullRequests<'_> {
        PullRequests { pool: &self.pool }
    }
}

impl PullRequests<'_> {
    /// Stores the pull request as GitHub reports it now (title, author, base and head); returns its row id.
    pub async fn upsert(
        &self,
        tenant_id: &str,
        repo_id: i64,
        pr: &PrSummary,
        now: DateTime<Utc>,
    ) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar(
            "INSERT INTO pull_requests (tenant_id, repo_id, number, title, author, html_url, base_sha, head_sha,
                 fetched_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (repo_id, number) DO UPDATE SET title = excluded.title, author = excluded.author,
                 html_url = excluded.html_url, base_sha = excluded.base_sha, head_sha = excluded.head_sha,
                 fetched_at = excluded.fetched_at
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(repo_id)
        .bind(pr.number)
        .bind(&pr.title)
        .bind(&pr.author)
        .bind(&pr.html_url)
        .bind(&pr.base_sha.0)
        .bind(&pr.head_sha.0)
        .bind(ts(now))
        .fetch_one(self.pool)
        .await?)
    }

    /// The row id of the tenant's stored pull request `repo#number` (owner and name case-insensitive); none when
    /// it was never resolved. Read-only.
    pub async fn find(
        &self,
        tenant_id: &str,
        repo: &RepoRef,
        number: u32,
    ) -> Result<Option<i64>, DbError> {
        let mut tx = read_tx(self.pool).await?;
        let id = sqlx::query_scalar(
            "SELECT p.id FROM pull_requests p JOIN repos r ON r.id = p.repo_id
             WHERE r.tenant_id = ?1 AND p.tenant_id = ?1 AND r.owner = ?2 AND r.name = ?3 AND p.number = ?4",
        )
        .bind(tenant_id)
        .bind(&repo.owner)
        .bind(&repo.name)
        .bind(number)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use contracts::ids::CommitSha;

    use super::*;
    use crate::repo::DEFAULT_TENANT;

    fn pr(head: &str) -> PrSummary {
        PrSummary {
            repo: RepoRef {
                owner: "acme".to_owned(),
                name: "node-client".to_owned(),
            },
            number: 482,
            title: "feat(rpc): retry".to_owned(),
            author: "dana".to_owned(),
            html_url: "https://github.com/acme/node-client/pull/482".to_owned(),
            base_sha: CommitSha("1".repeat(40)),
            head_sha: CommitSha(head.repeat(40)),
        }
    }

    #[tokio::test]
    async fn upsert_keeps_one_row_per_number_with_the_latest_head() {
        let db = Db::in_memory().await.expect("db");
        let now = Utc
            .with_ymd_and_hms(2026, 11, 16, 12, 0, 0)
            .single()
            .unwrap_or_default();
        let repo = db
            .repos()
            .upsert(DEFAULT_TENANT, &pr("a").repo, None)
            .await
            .expect("repo");
        let first = db
            .pull_requests()
            .upsert(DEFAULT_TENANT, repo.id, &pr("a"), now)
            .await
            .expect("insert");
        let second = db
            .pull_requests()
            .upsert(DEFAULT_TENANT, repo.id, &pr("b"), now)
            .await
            .expect("update");
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn find_is_tenant_scoped_case_insensitive_and_read_only() {
        let db = Db::in_memory().await.expect("db");
        crate::repo::analyses::testing::seed_tenant(&db, "other")
            .await
            .expect("tenant");
        let now = Utc
            .with_ymd_and_hms(2026, 11, 16, 12, 0, 0)
            .single()
            .unwrap_or_default();
        let repo = db
            .repos()
            .upsert(DEFAULT_TENANT, &pr("a").repo, None)
            .await
            .expect("repo");
        let id = db
            .pull_requests()
            .upsert(DEFAULT_TENANT, repo.id, &pr("a"), now)
            .await
            .expect("insert");
        let upper = RepoRef {
            owner: "ACME".to_owned(),
            name: "Node-Client".to_owned(),
        };
        let prs = db.pull_requests();
        assert_eq!(
            prs.find(DEFAULT_TENANT, &upper, 482).await.expect("find"),
            Some(id)
        );
        assert_eq!(
            prs.find(DEFAULT_TENANT, &upper, 483).await.expect("find"),
            None
        );
        assert_eq!(prs.find("other", &upper, 482).await.expect("find"), None);
        let unknown = RepoRef {
            owner: "acme".to_owned(),
            name: "wallet-ui".to_owned(),
        };
        assert_eq!(
            prs.find(DEFAULT_TENANT, &unknown, 482).await.expect("find"),
            None
        );
        assert_eq!(
            db.repos()
                .upsert(DEFAULT_TENANT, &unknown, None)
                .await
                .expect("repo")
                .id,
            repo.id + 1,
            "find created no repository row"
        );
    }
}
