//! Watched pull requests (plan §2.8) and per-user events (plan §5.5).

use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use contracts::api::review_loop::{AutoPass, UserEvent, UserEventKind, WatchState};
use contracts::common::{PrRef, RepoRef};
use contracts::ids::UserEventId;
use contracts::time::Timestamp;

use super::super::jobs::ts;
use super::super::write_tx;
use super::{ReviewLoop, json, opt_json_col, opt_ts_col, ts_col, u32_col, wire, wire_col};
use crate::error::DbError;

/// Result of [`ReviewLoop::watch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchOutcome {
    /// Watching (newly, again after it ended, or already).
    Watching,
    /// The user watches `limit` pull requests already.
    LimitReached,
}

/// A stored watch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchRecord {
    /// Tenant.
    pub tenant_id: String,
    /// Watcher (`users.id`).
    pub user_id: String,
    /// `pull_requests.id`.
    pub pull_request_id: i64,
    /// Pull request.
    pub pr: PrRef,
    /// Title.
    pub title: String,
    /// Watching since.
    pub since: Timestamp,
    /// The last probe's values.
    pub probe: Option<serde_json::Value>,
    /// Last probe.
    pub last_probe_at: Option<Timestamp>,
    /// Latest automatic assessment.
    pub last_auto_pass: Option<AutoPass>,
    /// State.
    pub state: WatchState,
    /// When it ended.
    pub ended_at: Option<Timestamp>,
    /// Since when an automatic assessment pass is due; `None` when none is.
    pub pass_due_at: Option<Timestamp>,
    /// The round the last "questions ready" event was for (`round:<n>` or `head:<sha>`).
    pub notified_for: Option<String>,
}

/// A per-user event to store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewUserEvent {
    /// Recipient (`users.id`).
    pub user_id: String,
    /// Kind.
    pub kind: UserEventKind,
    /// `pull_requests.id`; none for `test`.
    pub pull_request_id: Option<i64>,
    /// Questions or decisions.
    pub count: u32,
    /// Plain text.
    pub text: String,
    /// Comments page link.
    pub url: Option<String>,
}

const WATCH_SELECT: &str = "SELECT w.tenant_id, w.user_id, w.pull_request_id, r.owner, r.name, p.number, p.title, w.since,
        w.probe, w.last_probe_at, w.last_auto_pass_at, w.last_auto_pass_questions, w.state, w.ended_at,
        w.pass_due_at, w.notified_for
    FROM pr_watches w JOIN pull_requests p ON p.id = w.pull_request_id JOIN repos r ON r.id = p.repo_id";

fn watch_from_row(row: &SqliteRow) -> Result<WatchRecord, DbError> {
    let pass_at = opt_ts_col(row, "last_auto_pass_at")?;
    let questions: Option<i64> = row.try_get("last_auto_pass_questions")?;
    Ok(WatchRecord {
        tenant_id: row.try_get("tenant_id")?,
        user_id: row.try_get("user_id")?,
        pull_request_id: row.try_get("pull_request_id")?,
        pr: PrRef {
            repo: RepoRef {
                owner: row.try_get("owner")?,
                name: row.try_get("name")?,
            },
            number: u32_col(row, "number")?,
        },
        title: row.try_get("title")?,
        since: ts_col(row, "since")?,
        probe: opt_json_col(row, "probe")?,
        last_probe_at: opt_ts_col(row, "last_probe_at")?,
        last_auto_pass: match (pass_at, questions) {
            (Some(at), Some(questions)) => Some(AutoPass {
                at,
                questions: u32::try_from(questions).map_err(|_| DbError::Corrupt {
                    column: "last_auto_pass_questions",
                })?,
            }),
            _ => None,
        },
        state: wire_col(row, "state")?,
        ended_at: opt_ts_col(row, "ended_at")?,
        pass_due_at: opt_ts_col(row, "pass_due_at")?,
        notified_for: row.try_get("notified_for")?,
    })
}

impl ReviewLoop<'_> {
    /// Watches a PR unless the user already watches `limit` others (ended watches do not count); watching an
    /// ended watch again starts it anew.
    pub async fn watch(
        &self,
        tenant_id: &str,
        user_id: &str,
        pull_request_id: i64,
        limit: u32,
        now: Timestamp,
    ) -> Result<WatchOutcome, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let current: Option<String> = sqlx::query_scalar(
            "SELECT state FROM pr_watches WHERE user_id = ? AND pull_request_id = ?",
        )
        .bind(user_id)
        .bind(pull_request_id)
        .fetch_optional(&mut *tx)
        .await?;
        if current.as_deref().is_some_and(|state| state != "ended") {
            return Ok(WatchOutcome::Watching);
        }
        let live: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pr_watches WHERE user_id = ? AND state <> 'ended'",
        )
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
        if live >= i64::from(limit) {
            return Ok(WatchOutcome::LimitReached);
        }
        sqlx::query(
            "INSERT INTO pr_watches (user_id, pull_request_id, tenant_id, since, state) VALUES (?, ?, ?, ?, 'active')
             ON CONFLICT (user_id, pull_request_id) DO UPDATE SET since = excluded.since, state = 'active',
                 ended_at = NULL, probe = NULL, last_probe_at = NULL, last_auto_pass_at = NULL,
                 last_auto_pass_questions = NULL, pass_due_at = NULL, notified_for = NULL",
        )
        .bind(user_id)
        .bind(pull_request_id)
        .bind(tenant_id)
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(WatchOutcome::Watching)
    }

    /// Stops watching; false when the PR was not watched.
    pub async fn unwatch(&self, user_id: &str, pull_request_id: i64) -> Result<bool, DbError> {
        let deleted =
            sqlx::query("DELETE FROM pr_watches WHERE user_id = ? AND pull_request_id = ?")
                .bind(user_id)
                .bind(pull_request_id)
                .execute(self.pool)
                .await?;
        Ok(deleted.rows_affected() > 0)
    }

    /// The user's watches, newest first.
    pub async fn watches(&self, user_id: &str) -> Result<Vec<WatchRecord>, DbError> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "{WATCH_SELECT} WHERE w.user_id = ? ORDER BY w.since DESC, w.pull_request_id DESC"
        )))
        .bind(user_id)
        .fetch_all(self.pool)
        .await?
        .iter()
        .map(watch_from_row)
        .collect()
    }

    /// Every watch not ended, for the probe (by user, then PR).
    pub async fn live_watches(&self) -> Result<Vec<WatchRecord>, DbError> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "{WATCH_SELECT} WHERE w.state <> 'ended' ORDER BY w.user_id, w.pull_request_id"
        )))
        .fetch_all(self.pool)
        .await?
        .iter()
        .map(watch_from_row)
        .collect()
    }

    /// Stores a probe's values.
    pub async fn record_probe(
        &self,
        user_id: &str,
        pull_request_id: i64,
        probe: &serde_json::Value,
        now: Timestamp,
    ) -> Result<(), DbError> {
        let updated = sqlx::query(
            "UPDATE pr_watches SET probe = ?, last_probe_at = ? WHERE user_id = ? AND pull_request_id = ?",
        )
        .bind(json(probe)?)
        .bind(ts(now))
        .bind(user_id)
        .bind(pull_request_id)
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Sets a watch's state; `ended` records the end time.
    pub async fn set_watch_state(
        &self,
        user_id: &str,
        pull_request_id: i64,
        state: WatchState,
        now: Timestamp,
    ) -> Result<(), DbError> {
        let updated = sqlx::query(
            "UPDATE pr_watches SET state = ?1, ended_at = CASE WHEN ?1 = 'ended' THEN ?2 ELSE NULL END
             WHERE user_id = ?3 AND pull_request_id = ?4",
        )
        .bind(wire(state)?)
        .bind(ts(now))
        .bind(user_id)
        .bind(pull_request_id)
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Marks an automatic pass due since `at` unless one is due already; `None` clears it.
    pub async fn set_pass_due(
        &self,
        user_id: &str,
        pull_request_id: i64,
        at: Option<Timestamp>,
    ) -> Result<(), DbError> {
        let updated = sqlx::query(
            "UPDATE pr_watches SET pass_due_at = CASE WHEN ?1 IS NULL THEN NULL ELSE COALESCE(pass_due_at, ?1) END
             WHERE user_id = ?2 AND pull_request_id = ?3",
        )
        .bind(at.map(ts))
        .bind(user_id)
        .bind(pull_request_id)
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Records the round a "questions ready" event was sent for.
    pub async fn set_notified(
        &self,
        user_id: &str,
        pull_request_id: i64,
        round: &str,
    ) -> Result<(), DbError> {
        let updated = sqlx::query(
            "UPDATE pr_watches SET notified_for = ? WHERE user_id = ? AND pull_request_id = ?",
        )
        .bind(round)
        .bind(user_id)
        .bind(pull_request_id)
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Passes charged to the user since `since` (the daily cap).
    pub async fn charged_passes(&self, user_id: &str, since: Timestamp) -> Result<u32, DbError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM comment_passes WHERE user_id = ? AND created_at >= ?",
        )
        .bind(user_id)
        .bind(ts(since))
        .fetch_one(self.pool)
        .await?;
        u32::try_from(count).map_err(|_| DbError::Corrupt { column: "count" })
    }

    /// Records an automatic assessment of a watched PR.
    pub async fn record_auto_pass(
        &self,
        user_id: &str,
        pull_request_id: i64,
        questions: u32,
        now: Timestamp,
    ) -> Result<(), DbError> {
        let updated = sqlx::query(
            "UPDATE pr_watches SET last_auto_pass_at = ?, last_auto_pass_questions = ?
             WHERE user_id = ? AND pull_request_id = ?",
        )
        .bind(ts(now))
        .bind(questions)
        .bind(user_id)
        .bind(pull_request_id)
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Stores a per-user event with a delivery to each of the user's enabled channels subscribed to its kind.
    pub async fn add_user_event(
        &self,
        tenant_id: &str,
        event: &NewUserEvent,
        now: Timestamp,
    ) -> Result<UserEventId, DbError> {
        let mut tx = write_tx(self.pool).await?;
        // Without a round nothing but a (random) id can collide.
        let id = super::notify::insert_event(&mut tx, tenant_id, event, None, now)
            .await?
            .ok_or(DbError::Conflict)?;
        tx.commit().await?;
        Ok(id)
    }

    /// A user's events after `after` (exclusive), oldest first; at most `limit`.
    pub async fn user_events(
        &self,
        user_id: &str,
        after: Option<Timestamp>,
        limit: u32,
    ) -> Result<Vec<UserEvent>, DbError> {
        sqlx::query(
            "SELECT e.id, e.kind, r.owner, r.name, p.number, e.count, e.text, e.url, e.at
             FROM user_events e LEFT JOIN pull_requests p ON p.id = e.pull_request_id
             LEFT JOIN repos r ON r.id = p.repo_id
             WHERE e.user_id = ?1 AND (?2 IS NULL OR e.at > ?2) ORDER BY e.at, e.id LIMIT ?3",
        )
        .bind(user_id)
        .bind(after.map(ts))
        .bind(limit)
        .fetch_all(self.pool)
        .await?
        .iter()
        .map(|row| {
            let owner: Option<String> = row.try_get("owner")?;
            let name: Option<String> = row.try_get("name")?;
            let number: Option<i64> = row.try_get("number")?;
            let pr = match (owner, name, number) {
                (Some(owner), Some(name), Some(number)) => Some(PrRef {
                    repo: RepoRef { owner, name },
                    number: u32::try_from(number)
                        .map_err(|_| DbError::Corrupt { column: "number" })?,
                }),
                _ => None,
            };
            Ok(UserEvent {
                id: wire_col(row, "id")?,
                kind: wire_col(row, "kind")?,
                pr,
                count: u32_col(row, "count")?,
                text: row.try_get("text")?,
                url: row.try_get("url")?,
                at: ts_col(row, "at")?,
            })
        })
        .collect()
    }
}
