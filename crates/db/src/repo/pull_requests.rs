//! Repository for `pull_requests`: the latest snapshot of each resolved pull request.

use chrono::{DateTime, Utc};
use contracts::api::pr::PrSummary;
use sqlx::SqlitePool;

use super::Db;
use super::jobs::ts;
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
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use contracts::common::RepoRef;
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
}
