//! Durable GitHub App deliveries and installation coverage (ADR-026).

use contracts::api::settings::RepositoryInstallation;
use contracts::common::RepoRef;
use contracts::time::Timestamp;
use jiff::SignedDuration;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

use super::{
    Db,
    jobs::{parse_ts, ts},
    read_tx, write_tx,
};
use crate::error::DbError;

mod dispatch;
mod installations;
mod queue;

pub use dispatch::DashboardTarget;
pub use queue::{DashboardItem, DashboardRead, Destination, DispatchOutcome, DispatchTarget};
mod tracking;

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
    /// GitHub's PR update time for a signed synchronize event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synchronize_at: Option<Timestamp>,
    /// The previous head reported by a synchronize event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synchronize_before: Option<String>,
    /// GitHub action selected by the receiver.
    pub action: Option<String>,
    /// GitHub App installation.
    pub installation_id: Option<i64>,
    /// The base repository of an explicit PR in a check payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_repository: Option<Repository>,
    /// Login that can route a re-read for ranking admission or authored pull request history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_login: Option<String>,
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
    /// Audit an unused action only if its repository is tracked.
    IgnoreRepository(&'a Repository),
    /// Record a verified delivery whose JSON or shape was invalid.
    Reject(Option<&'a Repository>, DeliveryError),
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
    /// An installation update reached a repository limit.
    InstallationCapped,
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
    /// Resolved destinations eligible for dispatch.
    pub targets: Vec<DispatchTarget>,
    received_at_raw: String,
}

impl PendingDelivery {
    fn from_row(
        (id, payload, attempts, received_at_raw): (String, String, i64, String),
    ) -> Result<Self, DbError> {
        let (changes, targets) = match serde_json::from_str(&payload)? {
            queue::Payload::Hints(changes) => (changes, Vec::new()),
            queue::Payload::Targets(targets) => (Vec::new(), targets),
        };
        Ok(Self {
            id: Uuid::parse_str(&id).map_err(|_| DbError::Corrupt {
                column: "delivery_id",
            })?,
            changes,
            attempts,
            targets,
            received_at_raw,
        })
    }
}

/// Closed set of stored processing errors.
#[derive(Clone, Copy, Debug, thiserror::Error)]
pub enum DeliveryError {
    /// Dispatch could not process the change.
    #[error("dispatch")]
    Dispatch,
    /// The verified bytes are not JSON.
    #[error("invalid_json")]
    InvalidJson,
    /// The verified JSON lacks required webhook fields.
    #[error("invalid_payload")]
    InvalidPayload,
    /// The installation update reached a repository limit.
    #[error("installation_cap")]
    InstallationCap,
}

enum Admitted<'a> {
    Signal(tracking::Tracked<'a>, &'a [WebhookChange]),
    Installation(i64, installations::Prepared<'a>),
}

impl Github<'_> {
    /// Checks tracking in a read transaction before deduplicating and writing tracked changes.
    pub async fn record(
        &self,
        delivery: &NewDelivery<'_>,
        now: Timestamp,
    ) -> Result<Outcome, DbError> {
        let admitted = match &delivery.effect {
            Effect::Ignore => {
                if let Some(id) = delivery
                    .installation_id
                    .filter(|_| delivery.event != "ping")
                {
                    let mut reader = read_tx(self.pool).await?;
                    let known = installations::known(&mut reader, id).await?;
                    reader.commit().await?;
                    if !known {
                        return Ok(Outcome::NotAllowed);
                    }
                }
                return self.audit(delivery, None, now).await;
            }
            Effect::IgnoreRepository(repo) => {
                return self.scoped_audit(delivery, Some(repo), None, now).await;
            }
            Effect::Reject(repo, error) => {
                return self.scoped_audit(delivery, *repo, Some(*error), now).await;
            }
            Effect::Signal(repo, changes) => {
                let mut reader = read_tx(self.pool).await?;
                let matched =
                    tracking::resolve(&mut reader, std::slice::from_ref(*repo), 1).await?;
                reader.commit().await?;
                matched
                    .into_iter()
                    .next()
                    .map(|tracked| Admitted::Signal(tracked, changes))
            }
            Effect::Installation(id, update) => {
                let mut reader = read_tx(self.pool).await?;
                let matched = installations::prepare(&mut reader, *id, *update).await?;
                reader.commit().await?;
                matched.map(|update| Admitted::Installation(*id, update))
            }
        };
        let Some(admitted) = admitted else {
            return Ok(Outcome::NotAllowed);
        };

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

        let (changes, error, comments_changed) = match admitted {
            Admitted::Signal(tracked, changes) => {
                if !tracked.bind(&mut tx).await? {
                    return Ok(Outcome::NotAllowed);
                }
                let hints = serde_json::to_string(changes)?;
                let comments_changed = changes.iter().any(|change| {
                    matches!(
                        change.kind,
                        ChangeKind::PullRequest | ChangeKind::PullRequestReview
                    )
                });
                if comments_changed {
                    dispatch::invalidate_comments(&mut tx, &hints, now, None).await?;
                }
                (hints, None, comments_changed)
            }
            Admitted::Installation(installation, update) => {
                let Some(applied) =
                    installations::apply(&mut tx, installation, update, now).await?
                else {
                    return Ok(Outcome::NotAllowed);
                };
                (
                    applied.removed,
                    applied.capped.then_some(DeliveryError::InstallationCap),
                    false,
                )
            }
        };
        let state = if error.is_some() { "done" } else { "pending" };
        sqlx::query(
            "INSERT INTO github_deliveries (delivery_id, body_sha256, event, action, installation_id,
                                            changes, state, received_at, processed_at, error, comment_hints)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, CASE WHEN ?11 THEN ?6 END)
             ON CONFLICT(delivery_id) DO UPDATE SET
                 changes = CASE WHEN json_type(github_deliveries.changes, '$[0].destination') = 'object'
                     THEN (SELECT json_group_array(json_set(value, '$.attempts', 0, '$.ready_at', NULL,
                                                          '$.hold_until', NULL, '$.exhausted', json('false')))
                           FROM json_each(github_deliveries.changes))
                     ELSE excluded.changes END,
                 state = excluded.state, processed_at = excluded.processed_at,
                 received_at = excluded.received_at, attempts = 0, error = excluded.error,
                 comment_hints = excluded.comment_hints",
        )
        .bind(id).bind(delivery.hash).bind(delivery.event).bind(delivery.action).bind(delivery.installation_id)
        .bind(changes).bind(state).bind(ts(now)).bind(error.map(|_| ts(now)))
        .bind(error.map(|code| code.to_string()))
        .bind(comments_changed)
        .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(if error.is_some() {
            Outcome::InstallationCapped
        } else {
            Outcome::Queued
        })
    }

    async fn scoped_audit(
        &self,
        delivery: &NewDelivery<'_>,
        repo: Option<&Repository>,
        error: Option<DeliveryError>,
        now: Timestamp,
    ) -> Result<Outcome, DbError> {
        if let Some(repo) = repo {
            let mut reader = read_tx(self.pool).await?;
            let matched = tracking::resolve(&mut reader, std::slice::from_ref(repo), 1).await?;
            reader.commit().await?;
            if matched.is_empty() {
                return Ok(Outcome::NotAllowed);
            }
        }
        self.audit(delivery, error, now).await
    }

    async fn audit(
        &self,
        delivery: &NewDelivery<'_>,
        error: Option<DeliveryError>,
        now: Timestamp,
    ) -> Result<Outcome, DbError> {
        let written = sqlx::query(
            "INSERT INTO github_deliveries (delivery_id, body_sha256, event, action, installation_id,
                                            changes, state, received_at, processed_at, error)
             SELECT ?1, ?2, ?3, ?4, ?5, '[]', 'ignored', ?6, ?6, ?7 WHERE NOT EXISTS (
                SELECT 1 FROM github_deliveries WHERE body_sha256 = ?2 AND received_at >= ?8
             )
             ON CONFLICT(delivery_id) DO NOTHING",
        ).bind(delivery.id.to_string()).bind(delivery.hash).bind(delivery.event)
            .bind(delivery.action).bind(delivery.installation_id).bind(ts(now))
            .bind(error.map(|code| code.to_string())).bind(ts(now - RETENTION))
            .execute(self.pool).await?;
        Ok(if written.rows_affected() == 0 {
            Outcome::Duplicate
        } else {
            Outcome::Ignored
        })
    }

    /// Restarts recent interrupted deliveries. Older deliveries wait for manual redelivery.
    pub async fn resume(&self, now: Timestamp) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE github_deliveries SET state = 'pending', attempts = 0, processed_at = NULL, error = NULL,
                 changes = CASE WHEN json_type(changes, '$[0].destination') = 'object'
                     THEN (SELECT json_group_array(json_set(value, '$.attempts', 0, '$.ready_at', NULL,
                                                          '$.hold_until', NULL, '$.exhausted', json('false')))
                           FROM json_each(changes))
                     ELSE changes END
             WHERE state = 'failed' AND received_at >= ?",
        ).bind(ts(now - RESUME_WINDOW)).execute(self.pool).await?;
        Ok(())
    }

    /// Oldest eligible delivery. For a pending retry, processed_at stores its next eligible time.
    pub async fn next_pending(&self, now: Timestamp) -> Result<Option<PendingDelivery>, DbError> {
        self.next_pending_excluding(now, &[]).await
    }

    /// Delivery waiting the longest since it became eligible, excluding open coalescing windows.
    pub async fn next_pending_excluding(
        &self,
        now: Timestamp,
        held: &[Uuid],
    ) -> Result<Option<PendingDelivery>, DbError> {
        // Coverage is applied at receipt. Removed repositories only fence in-flight REST lookups.
        // Scheduled destinations use their eligibility time once the receipt is too old for intake.
        let eligible_at = ts(now);
        let row: Option<(String, String, i64, String)> = sqlx::query_as(
            "SELECT delivery_id,
                    CASE WHEN event = 'installation_repositories' AND action = 'removed' THEN '[]' ELSE changes END,
                    attempts, received_at FROM github_deliveries
             WHERE state = 'pending' AND (processed_at IS NULL OR processed_at <= ?2)
               AND (received_at >= ?1 OR (processed_at >= ?1 AND json_type(changes, '$[0].destination') = 'object'))
               AND delivery_id NOT IN (SELECT value FROM json_each(?3))
             ORDER BY COALESCE(processed_at, received_at), rowid LIMIT 1",
        ).bind(ts(now - RESUME_WINDOW)).bind(&eligible_at).bind(serde_json::to_string(held)?)
         .fetch_optional(self.pool).await?;
        row.map(|row| {
            let mut delivery = PendingDelivery::from_row(row)?;
            delivery.targets.retain(|target| target.ready(&eligible_at));
            Ok(delivery)
        })
        .transpose()
    }

    /// When the next delayed retry becomes eligible.
    pub async fn next_retry(&self, now: Timestamp) -> Result<Option<Timestamp>, DbError> {
        let next: Option<String> = sqlx::query_scalar(
            "SELECT MIN(processed_at) FROM github_deliveries
             WHERE state = 'pending' AND processed_at > ?2
               AND (received_at >= ?1 OR json_type(changes, '$[0].destination') = 'object')",
        )
        .bind(ts(now - RESUME_WINDOW))
        .bind(ts(now))
        .fetch_one(self.pool)
        .await?;
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
        .bind(delivery.id.to_string()).bind(&delivery.received_at_raw).bind(delivery.attempts)
        .execute(self.pool).await?;
        Ok(result.rows_affected() != 0)
    }

    /// Known tracked repositories, with installation state independent of webhook configuration.
    pub async fn repository_installations(
        &self,
        tenant: Option<&str>,
    ) -> Result<Vec<RepositoryInstallation>, DbError> {
        tracking::list(self.pool, tenant).await
    }

    /// Removes installations whose repositories are no longer tracked by any tenant.
    pub async fn prune_installations(&self) -> Result<(), DbError> {
        tracking::prune(self.pool).await
    }

    /// Applies the result of the repository installation lookup, unless a newer lifecycle event already won.
    pub async fn record_installation_lookup(
        &self,
        repo: &RepoRef,
        found: Option<(i64, bool)>,
        started: Timestamp,
    ) -> Result<(), DbError> {
        let mut tx = write_tx(self.pool).await?;
        if let Some((installation, suspended)) = found {
            let offered = [Repository {
                name: repo.clone(),
                id: None,
            }];
            if !tracking::resolve(&mut tx, &offered, 1).await?.is_empty() {
                sqlx::query(
                    "INSERT INTO app_repositories (owner, name, installation_id, suspended, checked_at)
                     SELECT ?1, ?2, ?3, ?4, ?5
                     WHERE NOT EXISTS (
                       SELECT 1 FROM github_deliveries d
                       WHERE d.installation_id = ?3 AND d.received_at >= ?5 AND d.state IN ('pending', 'done', 'failed')
                         AND ((d.event = 'installation' AND d.action IN ('deleted', 'suspend', 'unsuspend'))
                           OR (d.event = 'installation_repositories' AND d.action = 'removed' AND EXISTS (
                               SELECT 1 FROM json_each(d.changes) removed
                               WHERE (removed.value ->> '$.name.owner') = ?1 COLLATE NOCASE
                                 AND (removed.value ->> '$.name.name') = ?2 COLLATE NOCASE
                           )))
                     )
                     AND (EXISTS (SELECT 1 FROM app_repositories WHERE owner = ?1 AND name = ?2)
                          OR (SELECT COUNT(*) FROM app_repositories) < ?6)
                     ON CONFLICT(owner, name) DO UPDATE SET
                       repository_id = CASE WHEN installation_id = excluded.installation_id THEN repository_id END,
                       installation_id = excluded.installation_id,
                       suspended = excluded.suspended, checked_at = excluded.checked_at
                     WHERE app_repositories.checked_at < excluded.checked_at",
                )
                .bind(&repo.owner).bind(&repo.name).bind(installation).bind(suspended)
                .bind(ts(started)).bind(INSTALLATION_CAP).execute(&mut *tx).await?;
            }
        } else {
            sqlx::query(
                "DELETE FROM app_repositories WHERE owner = ?1 AND name = ?2 AND checked_at < ?3",
            )
            .bind(&repo.owner)
            .bind(&repo.name)
            .bind(ts(started))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// When the last stored verified webhook delivery was received.
    pub async fn last_delivery_at(&self) -> Result<Option<Timestamp>, DbError> {
        let last: Option<String> =
            sqlx::query_scalar("SELECT MAX(received_at) FROM github_deliveries")
                .fetch_one(self.pool)
                .await?;
        last.as_deref()
            .map(|value| parse_ts(value, "received_at"))
            .transpose()
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

#[cfg(test)]
mod tests;
