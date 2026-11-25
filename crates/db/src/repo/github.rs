//! Durable GitHub App deliveries and installation coverage (ADR-026).

use contracts::common::RepoRef;
use contracts::time::Timestamp;
use jiff::SignedDuration;
use serde::{Deserialize, Serialize};
use sqlx::{SqliteConnection, SqlitePool};
use uuid::Uuid;

use super::{
    Db,
    jobs::{parse_ts, ts},
    write_tx,
};
use crate::error::DbError;

const RETENTION: SignedDuration = SignedDuration::from_hours(14 * 24);
const RESUME_WINDOW: SignedDuration = SignedDuration::from_hours(3 * 24);
const INSTALLATION_CAP: i64 = 2_000;

/// Installation-wide GitHub webhook state.
pub struct Github<'a> {
    pool: &'a SqlitePool,
}

impl Db {
    /// GitHub App delivery and installation records.
    pub fn github(&self) -> Github<'_> {
        Github { pool: &self.pool }
    }
}

/// A repository identified by GitHub.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Repository {
    /// Stable GitHub identity, when supplied by the webhook.
    pub id: Option<i64>,
    /// Spelling from GitHub.
    pub name: RepoRef,
}

/// What the dispatcher should re-read.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// Pull request facts.
    PullRequest,
    /// Pull request facts and the review loop.
    PullRequestReview,
    /// Issue facts.
    Issue,
    /// Pull requests with a matching head commit.
    Check,
}

/// A typed dispatch hint, without any raw webhook JSON.
#[derive(Debug, Deserialize, Serialize)]
pub struct WebhookChange {
    /// What should be re-read.
    pub kind: ChangeKind,
    /// The repository receiving the event.
    pub repository: Repository,
    /// Where the pull request's head lives, when supplied.
    pub head_repository: Option<Repository>,
    /// Pull request or issue number, when supplied.
    pub number: Option<u32>,
    /// Head commit for mapping check events.
    pub head_sha: Option<String>,
    /// GitHub action selected by the receiver.
    pub action: Option<String>,
    /// GitHub App installation.
    pub installation_id: Option<i64>,
}

/// A live-map change from an installation lifecycle event.
#[derive(Clone, Copy)]
pub enum InstallationUpdate<'a> {
    /// Newly accessible repositories.
    Add(&'a [Repository]),
    /// Repositories removed from this installation.
    Remove(&'a [Repository]),
    /// Delete this installation's rows.
    Delete,
    /// Update this installation's suspension state.
    Suspend(bool),
}

/// The allowed effect of a verified delivery.
pub enum Effect<'a> {
    /// Persist the delivery for audit only.
    Ignore,
    /// Queue hints if the event repository is tracked.
    Signal(&'a Repository, &'a [WebhookChange]),
    /// Update live coverage for tracked repositories.
    Installation(i64, InstallationUpdate<'a>),
}

/// Fields that survive receipt of a verified webhook.
pub struct NewDelivery<'a> {
    /// Canonicalized delivery GUID.
    pub id: Uuid,
    /// SHA-256 of the signed bytes.
    pub hash: &'a str,
    /// Syntactically valid event name.
    pub event: &'a str,
    /// Syntactically valid action name.
    pub action: Option<&'a str>,
    /// Installation that sent the event.
    pub installation_id: Option<i64>,
    /// The effect selected at receipt.
    pub effect: Effect<'a>,
}

/// A verified receipt's result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Ready for dispatch.
    Queued,
    /// Already received or processed.
    Duplicate,
    /// No subscribed action applies.
    Ignored,
    /// No tenant tracks the repository.
    NotAllowed,
}

/// A pending delivery's dispatch inputs and retry state.
#[derive(Debug)]
pub struct PendingDelivery {
    /// GitHub delivery GUID.
    pub id: Uuid,
    /// Durable, typed change hints.
    pub changes: Vec<WebhookChange>,
    /// Dispatch failures in this attempt series.
    pub attempts: i64,
    received_at: String,
}

/// Closed set of stored processing errors.
#[derive(Clone, Copy, Debug, thiserror::Error)]
pub enum DeliveryError {
    /// Dispatch could not process the change.
    #[error("dispatch")]
    Dispatch,
}

impl Github<'_> {
    /// Deduplicates, applies the tracked-repository gate and records the result in one write transaction.
    pub async fn record(
        &self,
        delivery: &NewDelivery<'_>,
        now: Timestamp,
    ) -> Result<Outcome, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let id = delivery.id.to_string();
        let prior: Option<(String, String, String)> = sqlx::query_as(
            "SELECT body_sha256, state, event FROM github_deliveries WHERE delivery_id = ?",
        )
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await?;
        let duplicate = if let Some((hash, state, event)) = prior {
            hash != delivery.hash
                || event != delivery.event
                || !matches!(state.as_str(), "pending" | "failed")
        } else {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM github_deliveries
                               WHERE body_sha256 = ? AND received_at >= ?)",
            )
            .bind(delivery.hash)
            .bind(ts(now - RETENTION))
            .fetch_one(&mut *tx)
            .await?
        };
        if duplicate {
            return Ok(Outcome::Duplicate);
        }

        let (outcome, changes) = match &delivery.effect {
            Effect::Ignore => (Outcome::Ignored, &[][..]),
            Effect::Signal(repo, changes) if is_tracked(&mut tx, repo).await? => {
                (Outcome::Queued, *changes)
            }
            Effect::Installation(id, update)
                if apply_installation(&mut tx, *id, *update, now).await? =>
            {
                (Outcome::Queued, &[][..])
            }
            _ => (Outcome::NotAllowed, &[][..]),
        };
        let (state, processed_at) = match outcome {
            Outcome::Queued => ("pending", None),
            Outcome::Ignored => ("ignored", Some(ts(now))),
            Outcome::NotAllowed => ("not_allowed", Some(ts(now))),
            Outcome::Duplicate => unreachable!("duplicate deliveries return before recording"),
        };
        sqlx::query(
            "INSERT INTO github_deliveries (delivery_id, body_sha256, event, action, installation_id,
                                            changes, state, received_at, processed_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(delivery_id) DO UPDATE SET
                 changes = excluded.changes, state = excluded.state, processed_at = excluded.processed_at,
                 received_at = excluded.received_at, attempts = 0, error = NULL",
        )
        .bind(id).bind(delivery.hash).bind(delivery.event).bind(delivery.action).bind(delivery.installation_id)
        .bind(serde_json::to_string(changes)?).bind(state).bind(ts(now)).bind(processed_at)
        .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(outcome)
    }

    /// Restarts recent interrupted deliveries. Older deliveries wait for manual redelivery.
    pub async fn resume(&self, now: Timestamp) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE github_deliveries SET state = 'pending', attempts = 0, processed_at = NULL, error = NULL
             WHERE state = 'failed' AND received_at >= ?",
        ).bind(ts(now - RESUME_WINDOW)).execute(self.pool).await?;
        Ok(())
    }

    /// Oldest eligible delivery. For a pending retry, processed_at stores its next eligible time.
    pub async fn next_pending(&self, now: Timestamp) -> Result<Option<PendingDelivery>, DbError> {
        let row: Option<(String, String, i64, String)> = sqlx::query_as(
            "SELECT delivery_id, changes, attempts, received_at FROM github_deliveries
             WHERE state = 'pending' AND received_at >= ? AND (processed_at IS NULL OR processed_at <= ?)
             ORDER BY received_at, rowid LIMIT 1",
        ).bind(ts(now - RESUME_WINDOW)).bind(ts(now)).fetch_optional(self.pool).await?;
        row.map(|(id, changes, attempts, received_at)| {
            Ok(PendingDelivery {
                id: Uuid::parse_str(&id).map_err(|_| DbError::Corrupt {
                    column: "delivery_id",
                })?,
                changes: serde_json::from_str(&changes)?,
                attempts,
                received_at,
            })
        })
        .transpose()
    }

    /// When the next delayed retry becomes eligible.
    pub async fn next_retry(&self, now: Timestamp) -> Result<Option<Timestamp>, DbError> {
        let next: Option<String> = sqlx::query_scalar(
            "SELECT MIN(processed_at) FROM github_deliveries WHERE state = 'pending' AND received_at >= ?",
        ).bind(ts(now - RESUME_WINDOW)).fetch_one(self.pool).await?;
        next.as_deref()
            .map(|value| parse_ts(value, "processed_at"))
            .transpose()
    }

    /// Completes a delivery unless redelivery replaced the worker's attempt.
    pub async fn finish(&self, delivery: &PendingDelivery, now: Timestamp) -> Result<(), DbError> {
        self.update_pending(delivery, "done", None, now).await?;
        Ok(())
    }

    /// Records a dispatch failure, delaying the retry or marking the delivery failed.
    pub async fn fail(
        &self,
        delivery: &PendingDelivery,
        error: DeliveryError,
        retry_at: Option<Timestamp>,
        now: Timestamp,
    ) -> Result<bool, DbError> {
        let state = if retry_at.is_some() {
            "pending"
        } else {
            "failed"
        };
        self.update_pending(delivery, state, Some(error), retry_at.unwrap_or(now))
            .await
    }

    async fn update_pending(
        &self,
        delivery: &PendingDelivery,
        state: &str,
        error: Option<DeliveryError>,
        at: Timestamp,
    ) -> Result<bool, DbError> {
        let result = sqlx::query(
            "UPDATE github_deliveries SET state = ?, error = ?, processed_at = ?, attempts = attempts + ?
             WHERE delivery_id = ? AND received_at = ? AND attempts = ? AND state = 'pending'",
        )
        .bind(state).bind(error.map(|code| code.to_string())).bind(ts(at)).bind(i64::from(error.is_some()))
        .bind(delivery.id.to_string()).bind(&delivery.received_at).bind(delivery.attempts)
        .execute(self.pool).await?;
        Ok(result.rows_affected() != 0)
    }

    /// Last verified receipt and count of untracked deliveries in the last 14 days.
    pub async fn status(&self, now: Timestamp) -> Result<(Option<Timestamp>, u32), DbError> {
        let (last, dropped): (Option<String>, i64) = sqlx::query_as(
            "SELECT (SELECT MAX(received_at) FROM github_deliveries), COUNT(*)
             FROM github_deliveries WHERE state = 'not_allowed' AND received_at >= ?",
        )
        .bind(ts(now - RETENTION))
        .fetch_one(self.pool)
        .await?;
        Ok((
            last.as_deref()
                .map(|value| parse_ts(value, "received_at"))
                .transpose()?,
            u32::try_from(dropped).unwrap_or(u32::MAX),
        ))
    }

    /// Deletes receipts past their retention window.
    pub async fn prune_deliveries(&self, now: Timestamp) -> Result<(), DbError> {
        sqlx::query("DELETE FROM github_deliveries WHERE received_at < ?")
            .bind(ts(now - RETENTION))
            .execute(self.pool)
            .await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct TrackingMatch {
    tenant_id: Option<String>,
    owner: Option<String>,
    name: Option<String>,
    github_id: Option<i64>,
}

async fn is_tracked(conn: &mut SqliteConnection, repository: &Repository) -> Result<bool, DbError> {
    let matches: Vec<TrackingMatch> = sqlx::query_as(
        "WITH named_tracking AS (
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
         )
         SELECT DISTINCT t.tenant_id, t.owner, t.name, r.github_id
         FROM named_tracking t LEFT JOIN repos r
           ON r.tenant_id = t.tenant_id AND r.owner = t.owner AND r.name = t.name
         WHERE r.github_id = ?1
            OR (t.owner = ?2 COLLATE NOCASE AND t.name = ?3 COLLATE NOCASE
                AND (?1 IS NULL OR r.github_id IS NULL AND NOT EXISTS (
                    SELECT 1 FROM app_repositories ar
                    WHERE ar.owner = ?2 AND ar.name = ?3 AND ar.repository_id != ?1
                )))
         UNION ALL
         SELECT NULL, NULL, NULL, NULL WHERE EXISTS (
           SELECT 1 FROM dashboard_settings ds JOIN memberships m USING (tenant_id, user_id),
                         json_each(ds.enumerate_owners) owner
           WHERE owner.value = ?2 COLLATE NOCASE
           UNION ALL
           SELECT 1 FROM repos r JOIN pull_requests pr ON pr.repo_id = r.id
           WHERE (r.github_id = ?1 OR (?1 IS NULL AND r.owner = ?2 AND r.name = ?3))
             AND (EXISTS (SELECT 1 FROM pr_watches w JOIN memberships m USING (tenant_id, user_id)
                          WHERE w.pull_request_id = pr.id AND w.tenant_id = r.tenant_id AND w.state != 'ended')
               OR EXISTS (SELECT 1 FROM analyses a JOIN memberships m
                          ON m.tenant_id = a.tenant_id AND m.user_id = a.created_by
                          WHERE a.pull_request_id = pr.id AND a.tenant_id = r.tenant_id))
         )",
    )
    .bind(repository.id)
    .bind(&repository.name.owner)
    .bind(&repository.name.name)
    .fetch_all(&mut *conn)
    .await?;

    // Rules and snapshots initially have only names. Bind their first verified identity in
    // the tenant's repository cache so a rename cannot make the old rule track a newcomer.
    if let Some(id) = repository.id {
        for matched in &matches {
            if let (Some(tenant), Some(owner), Some(name), None) = (
                &matched.tenant_id,
                &matched.owner,
                &matched.name,
                matched.github_id,
            ) {
                sqlx::query(
                    "INSERT INTO repos (tenant_id, owner, name, github_id) VALUES (?, ?, ?, ?)
                     ON CONFLICT (tenant_id, owner, name) DO UPDATE SET github_id = excluded.github_id
                     WHERE repos.github_id IS NULL",
                )
                .bind(tenant)
                .bind(owner)
                .bind(name)
                .bind(id)
                .execute(&mut *conn)
                .await?;
            }
        }
    }
    Ok(!matches.is_empty())
}

async fn apply_installation(
    conn: &mut SqliteConnection,
    installation: i64,
    update: InstallationUpdate<'_>,
    now: Timestamp,
) -> Result<bool, DbError> {
    let mut tracked = false;
    match update {
        InstallationUpdate::Add(repos) => {
            for repo in repos {
                let Some(id) = repo.id else { continue };
                if !is_tracked(conn, repo).await? {
                    continue;
                }
                tracked = true;
                sqlx::query("DELETE FROM app_repositories WHERE owner = ? AND name = ? AND repository_id != ?")
                    .bind(&repo.name.owner).bind(&repo.name.name).bind(id).execute(&mut *conn).await?;
                sqlx::query(
                    "INSERT INTO app_repositories (owner, name, repository_id, installation_id, suspended, checked_at)
                     SELECT ?1, ?2, ?3, ?4, 0, ?5 WHERE
                         EXISTS (SELECT 1 FROM app_repositories WHERE repository_id = ?3)
                         OR (SELECT COUNT(*) FROM app_repositories) < ?6
                     ON CONFLICT(repository_id) DO UPDATE SET owner = excluded.owner, name = excluded.name,
                     installation_id = excluded.installation_id, suspended = excluded.suspended, checked_at = excluded.checked_at",
                ).bind(&repo.name.owner).bind(&repo.name.name).bind(id).bind(installation).bind(ts(now))
                    .bind(INSTALLATION_CAP).execute(&mut *conn).await?;
            }
        }
        InstallationUpdate::Remove(repos) => {
            for repo in repos {
                tracked |= sqlx::query(
                    "DELETE FROM app_repositories WHERE installation_id = ? AND repository_id = ?",
                )
                .bind(installation)
                .bind(repo.id)
                .execute(&mut *conn)
                .await?
                .rows_affected()
                    != 0;
            }
        }
        InstallationUpdate::Delete => {
            tracked = sqlx::query("DELETE FROM app_repositories WHERE installation_id = ?")
                .bind(installation)
                .execute(conn)
                .await?
                .rows_affected()
                != 0;
        }
        InstallationUpdate::Suspend(suspended) => {
            tracked = sqlx::query("UPDATE app_repositories SET suspended = ?, checked_at = ? WHERE installation_id = ?")
                .bind(suspended).bind(ts(now)).bind(installation).execute(conn).await?.rows_affected() != 0;
        }
    }
    Ok(tracked)
}

#[cfg(test)]
mod tests;
