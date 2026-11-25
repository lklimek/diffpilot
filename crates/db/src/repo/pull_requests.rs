//! Repository for `pull_requests`: the latest snapshot of each resolved pull request.

use contracts::api::pr::{PrHistoryAnalysis, PrHistoryItem, PrHistoryWatch, PrSummary};
use contracts::common::{LlmLanguage, PrRef, RepoRef};
use contracts::time::Timestamp;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqlitePool};

use contracts::ids::{AnalysisId, CommitSha};

use super::jobs::{parse_ts, parse_wire, ts, wire_name};
use super::{Db, read_tx, write_tx};
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
        now: Timestamp,
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

    /// Records a member's latest successful PR page opening.
    pub async fn record_opening(
        &self,
        tenant_id: &str,
        user_id: &str,
        pull_request_id: i64,
        at: Timestamp,
    ) -> Result<(), DbError> {
        let mut tx = write_tx(self.pool).await?;
        sqlx::query(
            "INSERT INTO pr_page_openings (tenant_id, user_id, pull_request_id, last_opened_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (tenant_id, user_id, pull_request_id) DO UPDATE
                 SET last_opened_at = MAX(last_opened_at, excluded.last_opened_at)",
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(pull_request_id)
        .bind(ts(at))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The member's PR history newest first, with row ids for keyset pagination.
    pub async fn history(
        &self,
        tenant_id: &str,
        user_id: &str,
        before: Option<(Timestamp, i64)>,
        limit: u32,
    ) -> Result<Vec<(i64, PrHistoryItem)>, DbError> {
        let rows = sqlx::query(
            "WITH analyses_ranked AS (
                 SELECT id, pull_request_id, state, head_sha, created_at,
                     ROW_NUMBER() OVER (PARTITION BY pull_request_id ORDER BY created_at DESC, id DESC) AS n
                 FROM analyses WHERE tenant_id = ?1 AND created_by = ?2
             ), latest_analyses AS (
                 SELECT * FROM analyses_ranked WHERE n = 1
             ), watches AS (
                 SELECT pull_request_id, state, since, ended_at FROM pr_watches
                 WHERE tenant_id = ?1 AND user_id = ?2
             ), openings AS (
                 SELECT pull_request_id, last_opened_at FROM pr_page_openings
                 WHERE tenant_id = ?1 AND user_id = ?2
             ), activity AS (
                 SELECT pull_request_id, created_at AS at FROM latest_analyses
                 UNION ALL SELECT pull_request_id, COALESCE(ended_at, since) FROM watches
                 UNION ALL SELECT pull_request_id, last_opened_at FROM openings
             ), history AS (
                 SELECT pull_request_id, MAX(at) AS last_activity_at FROM activity GROUP BY pull_request_id
             )
             SELECT p.id, p.title, p.number, r.owner, r.name, h.last_activity_at, o.last_opened_at,
                 a.id AS analysis_id, a.state AS analysis_state, a.head_sha AS analysis_head,
                 a.head_sha <> p.head_sha AS head_moved, w.state AS watch_state, w.since, w.ended_at
             FROM history h
             JOIN pull_requests p ON p.id = h.pull_request_id AND p.tenant_id = ?1
             JOIN repos r ON r.id = p.repo_id AND r.tenant_id = ?1
             LEFT JOIN latest_analyses a ON a.pull_request_id = p.id
             LEFT JOIN watches w ON w.pull_request_id = p.id
             LEFT JOIN openings o ON o.pull_request_id = p.id
             WHERE ?3 IS NULL OR (h.last_activity_at, p.id) < (?3, ?4)
             ORDER BY h.last_activity_at DESC, p.id DESC LIMIT ?5",
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(before.map(|(at, _)| ts(at)))
        .bind(before.map(|(_, id)| id))
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        rows.iter().map(history_item).collect()
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

    /// The stored pull request `id` as last resolved; `NotFound` when unknown.
    pub async fn get(&self, id: i64) -> Result<PullRequestRecord, DbError> {
        let row = sqlx::query(
            "SELECT p.tenant_id, r.owner, r.name, p.number, p.title, p.author, p.html_url, p.base_sha, p.head_sha
             FROM pull_requests p JOIN repos r ON r.id = p.repo_id WHERE p.id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool)
        .await?
        .ok_or(DbError::NotFound)?;
        Ok(PullRequestRecord {
            tenant_id: row.try_get("tenant_id")?,
            pr: PrSummary {
                repo: RepoRef {
                    owner: row.try_get("owner")?,
                    name: row.try_get("name")?,
                },
                number: row.try_get("number")?,
                title: row.try_get("title")?,
                author: row.try_get("author")?,
                html_url: row.try_get("html_url")?,
                base_sha: CommitSha(row.try_get("base_sha")?),
                head_sha: CommitSha(row.try_get("head_sha")?),
            },
        })
    }

    /// For each of `items` (repository and number; owner and name case-insensitive) with a stored pull request
    /// of the tenant: the analysis a reader is shown (as [`super::analyses::Analyses::latest_for_pr`]) and
    /// whether `user_id` stored review state for it, in list order. One statement for the whole list. Read-only.
    pub async fn review_links(
        &self,
        tenant_id: &str,
        user_id: &str,
        language: LlmLanguage,
        items: &[(RepoRef, u32)],
    ) -> Result<Vec<PrReviewLink>, DbError> {
        let wanted = serde_json::Value::Array(
            items
                .iter()
                .map(|(repo, number)| serde_json::json!([repo.owner, repo.name, number]))
                .collect(),
        );
        let mut tx = read_tx(self.pool).await?;
        let rows: Vec<(i64, String, String, bool)> = sqlx::query_as(
            "WITH wanted AS (
                 SELECT CAST(j.key AS INTEGER) AS idx, json_extract(j.value, '$[0]') AS owner,
                     json_extract(j.value, '$[1]') AS name, json_extract(j.value, '$[2]') AS number
                 FROM json_each(?1) j)
             SELECT w.idx, a.id, a.head_sha,
                 EXISTS(SELECT 1 FROM review_states s WHERE s.user_id = ?2 AND s.analysis_id = a.id)
             FROM wanted w
             JOIN repos r ON r.tenant_id = ?3 AND r.owner = w.owner AND r.name = w.name
             JOIN pull_requests p ON p.tenant_id = ?3 AND p.repo_id = r.id AND p.number = w.number
             JOIN analyses a ON a.id = (
                 SELECT a2.id FROM analyses a2 WHERE a2.pull_request_id = p.id
                 ORDER BY a2.state <> 'cancelled' DESC, a2.language = ?4 DESC, a2.created_at DESC, a2.rowid DESC
                 LIMIT 1)
             WHERE a.tenant_id = ?3
             ORDER BY w.idx",
        )
        .bind(wanted.to_string())
        .bind(user_id)
        .bind(tenant_id)
        .bind(wire_name(language)?)
        .fetch_all(&mut *tx)
        .await?;
        let mut links = Vec::with_capacity(rows.len());
        for (index, analysis_id, head_sha, has_review_state) in rows {
            links.push(PrReviewLink {
                index: usize::try_from(index).unwrap_or(usize::MAX),
                analysis_id: parse_wire(&analysis_id, "analyses.id")?,
                head_sha: CommitSha(head_sha),
                has_review_state,
            });
        }
        tx.commit().await?;
        Ok(links)
    }
}

fn history_item(row: &SqliteRow) -> Result<(i64, PrHistoryItem), DbError> {
    let optional_time = |column| -> Result<Option<Timestamp>, DbError> {
        row.try_get::<Option<String>, _>(column)?
            .map(|value| parse_ts(&value, column))
            .transpose()
    };
    let analysis = row
        .try_get::<Option<String>, _>("analysis_id")?
        .map(|id| -> Result<_, DbError> {
            Ok(PrHistoryAnalysis {
                id: parse_wire(&id, "analyses.id")?,
                status: parse_wire(row.try_get("analysis_state")?, "analyses.state")?,
                head_sha: CommitSha(row.try_get("analysis_head")?),
                head_moved: row.try_get("head_moved")?,
            })
        })
        .transpose()?;
    let watch = row
        .try_get::<Option<String>, _>("watch_state")?
        .map(|state| -> Result<_, DbError> {
            Ok(PrHistoryWatch {
                state: parse_wire(&state, "pr_watches.state")?,
                since: parse_ts(row.try_get("since")?, "pr_watches.since")?,
                ended_at: optional_time("ended_at")?,
            })
        })
        .transpose()?;
    Ok((
        row.try_get("id")?,
        PrHistoryItem {
            pr: PrRef {
                repo: RepoRef {
                    owner: row.try_get("owner")?,
                    name: row.try_get("name")?,
                },
                number: row.try_get("number")?,
            },
            title: row.try_get("title")?,
            last_activity_at: parse_ts(row.try_get("last_activity_at")?, "last_activity_at")?,
            opened_at: optional_time("last_opened_at")?,
            analysis,
            watch,
        },
    ))
}

/// The analysis shown for one pull request of a [`PullRequests::review_links`] list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrReviewLink {
    /// Position in the requested list.
    pub index: usize,
    /// The analysis.
    pub analysis_id: AnalysisId,
    /// Its analysed head.
    pub head_sha: CommitSha,
    /// The reader stored review state for it.
    pub has_review_state: bool,
}

/// A stored pull request with its tenant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequestRecord {
    /// Tenant.
    pub tenant_id: String,
    /// The pull request as last resolved (title, author, base and head).
    pub pr: PrSummary,
}

/// Fault injection for pull-request repository tests.
#[cfg(any(test, feature = "test-util"))]
pub mod testing {
    use super::*;

    /// Rejects new PR page openings while leaving the rest of the database usable.
    pub async fn reject_openings(db: &Db) -> Result<(), DbError> {
        sqlx::query(
            "CREATE TRIGGER reject_openings BEFORE INSERT ON pr_page_openings
             BEGIN SELECT RAISE(ABORT, 'injected PR opening failure'); END",
        )
        .execute(&db.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {

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
        let now = "2026-11-16T12:00:00Z"
            .parse::<Timestamp>()
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
    async fn review_links_in_one_call_pick_the_shown_analysis_and_the_reader_state() {
        let db = Db::in_memory().await.expect("db");
        let shown = crate::repo::analyses::testing::seed_analysis(&db)
            .await
            .expect("seed");
        let later = crate::repo::analyses::testing::seed_analysis(&db)
            .await
            .expect("seed");
        sqlx::query("UPDATE analyses SET created_at = '2026-11-17T00:00:00.000000Z' WHERE id = ?")
            .bind(later.to_string())
            .execute(&db.pool)
            .await
            .expect("newer");
        sqlx::query("UPDATE analyses SET state = 'cancelled' WHERE id = ?")
            .bind(later.to_string())
            .execute(&db.pool)
            .await
            .expect("cancelled");
        sqlx::query(
            "INSERT INTO review_states (user_id, analysis_id, tenant_id, ticked_steps, updated_at)
             VALUES ('u1', ?, 'default', '[]', '2026-11-16T00:00:00.000000Z')",
        )
        .bind(shown.to_string())
        .execute(&db.pool)
        .await
        .expect("state");
        let repo = |owner: &str, name: &str| RepoRef {
            owner: owner.to_owned(),
            name: name.to_owned(),
        };
        let items = [
            (repo("acme", "wallet-ui"), 482),
            (repo("ACME", "Node-Client"), 482),
            (repo("acme", "node-client"), 483),
        ];
        let prs = db.pull_requests();
        let links = prs
            .review_links(DEFAULT_TENANT, "u1", LlmLanguage::En, &items)
            .await
            .expect("links");
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(links[0].index, 1);
        assert_eq!(links[0].analysis_id, shown, "usable before cancelled");
        assert!(links[0].has_review_state);
        let other = prs
            .review_links(DEFAULT_TENANT, "u2", LlmLanguage::En, &items)
            .await
            .expect("links");
        assert!(!other[0].has_review_state, "another reader's state");
        let foreign = prs
            .review_links("other", "u1", LlmLanguage::En, &items)
            .await
            .expect("links");
        assert!(foreign.is_empty(), "tenant scoped");
        let many: Vec<_> = (0..450).map(|_| items[1].clone()).collect();
        let all = prs
            .review_links(DEFAULT_TENANT, "u1", LlmLanguage::En, &many)
            .await
            .expect("links");
        assert_eq!(all.len(), 450, "a whole page in one statement");
        assert_eq!(all[449].index, 449);
    }

    #[tokio::test]
    async fn find_is_tenant_scoped_case_insensitive_and_read_only() {
        let db = Db::in_memory().await.expect("db");
        crate::repo::analyses::testing::seed_tenant(&db, "other")
            .await
            .expect("tenant");
        let now = "2026-11-16T12:00:00Z"
            .parse::<Timestamp>()
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
