//! Repository for the dashboard (`dashboard_settings`, `dashboard_refreshes`, `dashboard_judgements`; plan
//! `docs/v1.1-dashboard-plan.md` §D). Signatures frozen by WP11-A. Every function is scoped by
//! `(tenant_id, user_id)`, except the server-internal ones that say so; there is no list-all API for serving, and
//! another user's or tenant's id answers `NotFound`. Refresh transitions follow `docs/contracts/http-api.md`
//! "Refresh states"; each runs in one write transaction. One live refresh per user across tenants (the unique index
//! is on `user_id`): a live refresh in another tenant makes a start or Retry ranking answer `Conflict`.
//! `RankingStatus.message`, `total` and `resumes_at` are not stored here; the server derives them from the judge job.
//! Ranking results carry the ranking attempt (`ranking_attempt`, server-internal): a late result of a superseded
//! or restarted ranking is [`RankingUpdate::Stale`] and only records its LLM calls (`judged`, for the daily cap).

use contracts::api::dashboard::{
    DashboardSettings, ItemKey, RankingState, RefreshFailure, RefreshState, RefreshTrigger,
    ScanStep, ScheduledOutcome,
};
use contracts::ids::{JobId, RefreshId};
use contracts::time::Timestamp;
use jiff::SignedDuration;
use jiff::civil::Date;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, Sqlite, SqliteExecutor, SqlitePool, Transaction};

use super::jobs::{parse_ts, parse_wire, ts, wire_name};
use super::{Db, write_tx};
use crate::error::DbError;

/// Refreshes kept per user: the newest, plus the one holding the latest snapshot.
pub const REFRESHES_KEPT: u32 = 10;
/// Refreshes started this recently that judged or sent something are always kept: their `sent` counts feed the daily
/// ranking cap (a local day spans up to 25 hours, and a ranking may end after the day it started).
const RECENT_REFRESHES_KEPT: SignedDuration = SignedDuration::from_hours(48);
/// Log lines kept per refresh (the newest).
pub const LOG_LINES_KEPT: usize = 200;
/// Days a judgement is kept after its item was last seen.
pub const JUDGEMENT_RETENTION_DAYS: i64 = 90;

/// Failure message of a scan stopped by a server restart.
const INTERRUPTED_MESSAGE: &str = "A server restart interrupted the scan.";
/// Format of a stored local date (`last_scheduled_on`).
const LOCAL_DATE: &str = "%Y-%m-%d";

/// Stored judgements of a user's items by `(tenant, user, JSON array of item keys)`: one primary-key lookup per
/// item, so "nothing changed, no job" stays cheap.
const JUDGEMENTS_SQL: &str = "SELECT * FROM dashboard_judgements
    WHERE tenant_id = ?1 AND user_id = ?2 AND item_key IN (SELECT value FROM json_each(?3)) ORDER BY item_key";
/// LLM judgements a user's refreshes started since a time made (index `dashboard_refreshes_by_user`).
const JUDGED_SINCE_SQL: &str = "SELECT COALESCE(SUM(judged), 0) FROM dashboard_refreshes
    WHERE tenant_id = ?1 AND user_id = ?2 AND started_at >= ?3";
/// Items a user's refreshes sent to the judge that ranked on or after a time: started then, still live, or ended
/// then (a ranking that spans midnight counts wholly on both days, erring towards the cap).
const SENT_SINCE_SQL: &str = "SELECT COALESCE(SUM(sent), 0) FROM dashboard_refreshes
    WHERE tenant_id = ?1 AND user_id = ?2 AND sent > 0
      AND (started_at >= ?3 OR finished_at IS NULL OR finished_at >= ?3)";
/// Judgements not seen since a time (index `dashboard_judgements_by_last_seen`).
const DELETE_UNSEEN_SQL: &str = "DELETE FROM dashboard_judgements WHERE last_seen_at < ?1";

/// Columns of a [`RefreshRecord`]: everything but the snapshot document.
macro_rules! record_columns {
    () => {
        "id, user_id, trigger, state, progress, log, failure_code, failure_message, retry_at, ranking_state, judged,
         reused, left_provisional, ranking_attempt, snapshot IS NOT NULL AS has_snapshot, started_at, scanned_at, finished_at"
    };
}

/// Id of the refresh holding the current snapshot of user `?2` in tenant `?1`.
macro_rules! current_snapshot_id {
    () => {
        "SELECT id FROM dashboard_refreshes WHERE tenant_id = ?1 AND user_id = ?2 AND state IN ('ranking', 'finished')
         ORDER BY started_at DESC, rowid DESC LIMIT 1"
    };
}

/// Access to the dashboard tables.
#[derive(Debug)]
pub struct Dashboard<'a> {
    pool: &'a SqlitePool,
}

impl Db {
    /// Dashboard settings, refreshes with their snapshots, and stored judgements.
    pub fn dashboard(&self) -> Dashboard<'_> {
        Dashboard { pool: &self.pool }
    }
}

/// A user's stored settings row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredDashboardSettings {
    /// The settings.
    pub settings: DashboardSettings,
    /// The most recent scheduled run; none before the first.
    pub last_scheduled: Option<ScheduledRunRecord>,
    /// Last change of the row.
    pub updated_at: Timestamp,
}

/// A scheduled run as recorded per user (one per local date).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledRunRecord {
    /// The user's local date of the run.
    pub on: Date,
    /// What happened.
    pub outcome: ScheduledOutcome,
    /// Plain reason, if any.
    pub reason: Option<String>,
}

/// A user whose schedule is on and whose time zone is known (scheduler input).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleCandidate {
    /// Tenant.
    pub tenant_id: String,
    /// User.
    pub user_id: String,
    /// Their settings row.
    pub settings: StoredDashboardSettings,
}

/// A settings row of the scheduler's input that could not be decoded; the other rows are still returned.
#[derive(Debug)]
pub struct UnreadableCandidate {
    /// Tenant.
    pub tenant_id: String,
    /// User.
    pub user_id: String,
    /// Why the row could not be decoded.
    pub error: DbError,
}

/// Scan progress as stored in `dashboard_refreshes.progress`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshProgress {
    /// Scan steps in order.
    pub steps: Vec<ScanStep>,
    /// Progress, 0–100.
    pub percent: u32,
}

/// Judgement counts of a refresh (plan §B.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JudgeCounts {
    /// Items judged by the LLM.
    pub judged: u32,
    /// Items whose stored judgement was reused.
    pub reused: u32,
    /// Items left provisional by the caps.
    pub left_provisional: u32,
}

/// A stored refresh without its snapshot document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshRecord {
    /// Id.
    pub id: RefreshId,
    /// Owner.
    pub user_id: String,
    /// What started it.
    pub trigger: RefreshTrigger,
    /// State.
    pub state: RefreshState,
    /// Scan progress.
    pub progress: RefreshProgress,
    /// Log lines, oldest first (at most [`LOG_LINES_KEPT`]).
    pub log: Vec<String>,
    /// Why it failed; set exactly when `state` is `failed`.
    pub failure: Option<RefreshFailure>,
    /// State of the ranking; none before the snapshot is stored.
    pub ranking_state: Option<RankingState>,
    /// Attempt of the ranking: 0, incremented by each Retry ranking (server-internal, not on the wire).
    pub ranking_attempt: u32,
    /// Judgement counts.
    pub counts: JudgeCounts,
    /// A snapshot is stored.
    pub has_snapshot: bool,
    /// Start time.
    pub started_at: Timestamp,
    /// When the snapshot was stored.
    pub scanned_at: Option<Timestamp>,
    /// When it ended (finished, failed or cancelled).
    pub finished_at: Option<Timestamp>,
}

/// Result of [`Dashboard::set_ranking`] and [`Dashboard::finish_ranking`].
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RankingUpdate {
    /// The refresh was ranking in this attempt and is updated.
    Applied,
    /// The refresh is no longer ranking in this attempt (superseded, ended or restarted): the late result is
    /// ignored, except that `judged` is raised to its count.
    Stale,
}

/// How a scan ends before its snapshot is complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanEnd {
    /// The scan failed: state `failed`; the previous snapshot stays.
    Failed(RefreshFailure),
    /// The user cancelled it: state `cancelled`; the previous snapshot stays.
    Cancelled,
}
/// Result of [`Dashboard::start_refresh`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartOutcome {
    /// Started in `scanning`. `superseded`: the user's refresh that was `ranking`, now `finished` with ranking
    /// `cancelled` (its snapshot stays current until the new one is stored).
    Started {
        /// The new refresh.
        refresh: Box<RefreshRecord>,
        /// The superseded ranking refresh, if one was live.
        superseded: Option<RefreshId>,
    },
    /// Another refresh of the user is `scanning`: R2 answers 409 `refresh_running`; the schedule counts it as done.
    ScanRunning,
}

/// Result of [`Dashboard::restart_ranking`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestartOutcome {
    /// Back in `ranking` (ranking state `running`, counts kept).
    Restarted(Box<RefreshRecord>),
    /// Another refresh of the user is live (R4: 409 `refresh_running`).
    RefreshRunning,
    /// Not `finished` with ranking `failed`, `interrupted` or `cancelled`, or not the current snapshot (R4: 409
    /// `conflict`).
    NotRetryable,
}

/// A refresh that was live when the server stopped (startup recovery).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterruptedRefresh {
    /// Tenant.
    pub tenant_id: String,
    /// User.
    pub user_id: String,
    /// The refresh.
    pub id: RefreshId,
    /// Its state before recovery: `scanning` (now `failed`, code `interrupted`) or `ranking` (now `finished`,
    /// ranking `interrupted`).
    pub was: RefreshState,
}

/// The ranking caps a send is checked against (plan §B.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SendCaps {
    /// Items sent per refresh, across its ranking attempts.
    pub per_refresh: u32,
    /// Items sent per user and local day.
    pub per_day: u32,
}

/// Result of [`Dashboard::reserve_sends`].
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendReservation {
    /// Counted: the items may be sent.
    Reserved,
    /// The refresh's cap would be exceeded; nothing counted.
    OverRefreshCap,
    /// The user's daily cap would be exceeded; nothing counted.
    OverDailyCap,
    /// The refresh is no longer ranking in this attempt; nothing counted.
    Stale,
}

/// Items already sent to the judge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SentTotals {
    /// By this refresh, across its attempts.
    pub refresh: u32,
    /// By the user's refreshes that ranked since the day's start.
    pub day: u32,
}

/// One write of a live ranking (see [`Dashboard::write_ranking`]).
#[derive(Clone, Copy, Debug)]
pub struct RankingWrite<'a> {
    /// Live ranking state (`waiting_for_analysis`, `running`, `paused`).
    pub state: RankingState,
    /// Judgement counts.
    pub counts: JudgeCounts,
    /// Judgements to store (replacing the user's previous judgement of each item).
    pub judgements: &'a [StoredJudgement],
    /// New progress and log of the refresh (the log keeps its newest [`LOG_LINES_KEPT`] lines).
    pub progress: Option<(&'a RefreshProgress, &'a [String])>,
}

/// A stored LLM judgement of one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredJudgement {
    /// The item.
    pub item_key: ItemKey,
    /// SHA-256 hex of the judge-input version and the canonical judged inputs.
    pub input_fingerprint: String,
    /// `JUDGE_INPUT_VERSION` of the fingerprint.
    pub input_version: u32,
    /// `JUDGE_PROMPT_VERSION` of the prompt and output schema.
    pub prompt_version: u32,
    /// The validated judgement of the item (judge-work output, plan §B.2).
    pub body: Value,
    /// Refresh that judged it; none once that refresh is deleted.
    pub refresh_id: Option<RefreshId>,
    /// Job that judged it.
    pub job_id: Option<JobId>,
    /// Model that judged it.
    pub model: String,
    /// Judgement time (counts toward the daily cap).
    pub judged_at: Timestamp,
    /// Last refresh that saw the item.
    pub last_seen_at: Timestamp,
}

impl Dashboard<'_> {
    /// The user's settings row; none means the defaults (`DashboardSettings::default`).
    pub async fn settings(
        &self,
        tenant_id: &str,
        user_id: &str,
    ) -> Result<Option<StoredDashboardSettings>, DbError> {
        sqlx::query("SELECT * FROM dashboard_settings WHERE tenant_id = ? AND user_id = ?")
            .bind(tenant_id)
            .bind(user_id)
            .fetch_optional(self.pool)
            .await?
            .as_ref()
            .map(settings_of)
            .transpose()
    }

    /// Saves validated settings (upsert); keeps the recorded scheduled run.
    pub async fn save_settings(
        &self,
        tenant_id: &str,
        user_id: &str,
        settings: &DashboardSettings,
        now: Timestamp,
    ) -> Result<(), DbError> {
        let saved = sqlx::query(
            "INSERT INTO dashboard_settings (user_id, tenant_id, linked_logins, enumerate_owners, repositories,
                 lookback_days, ready_label, skip_authors, known_bots, include_todos, memcan_extra_projects,
                 schedule_enabled, schedule_time, time_zone, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (user_id) DO UPDATE SET linked_logins = excluded.linked_logins,
                 enumerate_owners = excluded.enumerate_owners, repositories = excluded.repositories,
                 lookback_days = excluded.lookback_days, ready_label = excluded.ready_label,
                 skip_authors = excluded.skip_authors, known_bots = excluded.known_bots,
                 include_todos = excluded.include_todos,
                 memcan_extra_projects = excluded.memcan_extra_projects,
                 schedule_enabled = excluded.schedule_enabled, schedule_time = excluded.schedule_time,
                 time_zone = excluded.time_zone, updated_at = excluded.updated_at
             WHERE dashboard_settings.tenant_id = excluded.tenant_id",
        )
        .bind(user_id)
        .bind(tenant_id)
        .bind(serde_json::to_string(&settings.linked_logins)?)
        .bind(serde_json::to_string(&settings.enumerate_owners)?)
        .bind(serde_json::to_string(&settings.repositories)?)
        .bind(i64::from(settings.lookback_days))
        .bind(&settings.ready_label)
        .bind(serde_json::to_string(&settings.skip_authors)?)
        .bind(serde_json::to_string(&settings.known_bots)?)
        .bind(settings.include_todos)
        .bind(serde_json::to_string(&settings.memcan_extra_projects)?)
        .bind(settings.schedule_enabled)
        .bind(&settings.schedule_time)
        .bind(&settings.time_zone)
        .bind(ts(now))
        .execute(self.pool)
        .await
        .map_err(missing_owner)?;
        found(saved.rows_affected())
    }

    /// Stores the browser's time zone unless the user's row already holds one (R6b, first dashboard visit),
    /// creating a defaults row when there is none. `true` when stored, `false` when a stored zone was kept.
    pub async fn set_time_zone_if_unset(
        &self,
        tenant_id: &str,
        user_id: &str,
        time_zone: &str,
        now: Timestamp,
    ) -> Result<bool, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let saved = sqlx::query(
            "INSERT INTO dashboard_settings (user_id, tenant_id, time_zone, updated_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (user_id) DO UPDATE SET time_zone = excluded.time_zone, updated_at = excluded.updated_at
             WHERE dashboard_settings.tenant_id = excluded.tenant_id AND dashboard_settings.time_zone IS NULL",
        )
        .bind(user_id)
        .bind(tenant_id)
        .bind(time_zone)
        .bind(ts(now))
        .execute(&mut *tx)
        .await
        .map_err(missing_owner)?;
        if saved.rows_affected() > 0 {
            tx.commit().await?;
            return Ok(true);
        }
        let kept =
            sqlx::query("SELECT 1 FROM dashboard_settings WHERE tenant_id = ? AND user_id = ?")
                .bind(tenant_id)
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?;
        tx.commit().await?;
        found(u64::from(kept.is_some()))?;
        Ok(false)
    }

    /// Records the scheduled run of a local date, creating a defaults row when there is none.
    pub async fn record_scheduled_run(
        &self,
        tenant_id: &str,
        user_id: &str,
        run: &ScheduledRunRecord,
    ) -> Result<(), DbError> {
        let saved = sqlx::query(
            "INSERT INTO dashboard_settings (user_id, tenant_id, last_scheduled_on, last_scheduled_outcome,
                 last_scheduled_reason, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (user_id) DO UPDATE SET last_scheduled_on = excluded.last_scheduled_on,
                 last_scheduled_outcome = excluded.last_scheduled_outcome,
                 last_scheduled_reason = excluded.last_scheduled_reason, updated_at = excluded.updated_at
             WHERE dashboard_settings.tenant_id = excluded.tenant_id",
        )
        .bind(user_id)
        .bind(tenant_id)
        .bind(run.on.strftime(LOCAL_DATE).to_string())
        .bind(wire_name(run.outcome)?)
        .bind(&run.reason)
        .bind(ts(Timestamp::now()))
        .execute(self.pool)
        .await
        .map_err(missing_owner)?;
        found(saved.rows_affected())
    }

    /// Server-internal (scheduler): every user, in every tenant, whose schedule is on and time zone known; a row
    /// that cannot be decoded is reported on its own, so it never hides the others.
    pub async fn schedule_candidates(
        &self,
    ) -> Result<Vec<Result<ScheduleCandidate, UnreadableCandidate>>, DbError> {
        let rows = sqlx::query(
            "SELECT * FROM dashboard_settings WHERE schedule_enabled = 1 AND time_zone IS NOT NULL
             ORDER BY tenant_id, user_id",
        )
        .fetch_all(self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let tenant_id: String = row.try_get("tenant_id")?;
                let user_id: String = row.try_get("user_id")?;
                Ok(match settings_of(row) {
                    Ok(settings) => Ok(ScheduleCandidate {
                        tenant_id,
                        user_id,
                        settings,
                    }),
                    Err(error) => Err(UnreadableCandidate {
                        tenant_id,
                        user_id,
                        error,
                    }),
                })
            })
            .collect()
    }

    /// R2 and the schedule, in one write transaction: a refresh of the user in `ranking` is superseded
    /// ([`Dashboard::finish_ranking`] with `cancelled`), then a new one starts in `scanning`; a `scanning` one
    /// blocks ([`StartOutcome::ScanRunning`]). Deletes the user's older refreshes beyond [`REFRESHES_KEPT`], never
    /// the one holding the current snapshot nor one started in the last 24 hours that judged something (the daily
    /// cap counts it).
    pub async fn start_refresh(
        &self,
        tenant_id: &str,
        user_id: &str,
        trigger: RefreshTrigger,
        now: Timestamp,
    ) -> Result<StartOutcome, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let live: Option<(String, String)> = sqlx::query_as(
            "SELECT id, state FROM dashboard_refreshes
             WHERE tenant_id = ? AND user_id = ? AND state IN ('scanning', 'ranking')",
        )
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        let mut superseded = None;
        if let Some((id, state)) = live {
            if parse_wire::<RefreshState>(&state, "dashboard_refreshes.state")?
                == RefreshState::Scanning
            {
                return Ok(StartOutcome::ScanRunning);
            }
            sqlx::query(
                "UPDATE dashboard_refreshes SET state = 'finished', ranking_state = 'cancelled', finished_at = ?
                 WHERE id = ?",
            )
            .bind(ts(now))
            .bind(&id)
            .execute(&mut *tx)
            .await?;
            superseded = Some(parse_wire(&id, "dashboard_refreshes.id")?);
        }
        let id = RefreshId::new_random();
        sqlx::query(
            "INSERT INTO dashboard_refreshes (id, tenant_id, user_id, trigger, state, started_at)
             VALUES (?, ?, ?, ?, 'scanning', ?)",
        )
        .bind(id.to_string())
        .bind(tenant_id)
        .bind(user_id)
        .bind(wire_name(trigger)?)
        .bind(ts(now))
        .execute(&mut *tx)
        .await
        .map_err(missing_owner)?;
        sqlx::query(concat!(
            "DELETE FROM dashboard_refreshes WHERE tenant_id = ?1 AND user_id = ?2
               AND id NOT IN (SELECT id FROM dashboard_refreshes WHERE tenant_id = ?1 AND user_id = ?2
                              ORDER BY started_at DESC, rowid DESC LIMIT ?3)
               AND (started_at < ?4 OR (judged = 0 AND sent = 0)) AND id IS NOT (",
            current_snapshot_id!(),
            ")"
        ))
        .bind(tenant_id)
        .bind(user_id)
        .bind(i64::from(REFRESHES_KEPT))
        .bind(ts(now - RECENT_REFRESHES_KEPT))
        .execute(&mut *tx)
        .await?;
        let refresh = read_record(&mut *tx, tenant_id, user_id, id).await?;
        tx.commit().await?;
        Ok(StartOutcome::Started {
            refresh: Box::new(refresh),
            superseded,
        })
    }

    /// One of the user's refreshes; `NotFound` for an unknown id or another user's refresh.
    pub async fn refresh(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
    ) -> Result<RefreshRecord, DbError> {
        read_record(self.pool, tenant_id, user_id, id).await
    }

    /// The user's newest refresh, live or ended.
    pub async fn latest_refresh(
        &self,
        tenant_id: &str,
        user_id: &str,
    ) -> Result<Option<RefreshRecord>, DbError> {
        sqlx::query(concat!(
            "SELECT ",
            record_columns!(),
            " FROM dashboard_refreshes WHERE tenant_id = ? AND user_id = ?
              ORDER BY started_at DESC, rowid DESC LIMIT 1"
        ))
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(self.pool)
        .await?
        .as_ref()
        .map(record_of)
        .transpose()
    }

    /// Replaces the progress and log of a live refresh (the log keeps its newest [`LOG_LINES_KEPT`] lines);
    /// `Conflict` once it ended.
    pub async fn record_progress(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
        progress: &RefreshProgress,
        log: &[String],
    ) -> Result<(), DbError> {
        let newest = &log[log.len().saturating_sub(LOG_LINES_KEPT)..];
        let mut tx = write_tx(self.pool).await?;
        let updated = sqlx::query(
            "UPDATE dashboard_refreshes SET progress = ?, log = ?
             WHERE id = ? AND tenant_id = ? AND user_id = ? AND state IN ('scanning', 'ranking')",
        )
        .bind(serde_json::to_string(progress)?)
        .bind(serde_json::to_string(newest)?)
        .bind(id.to_string())
        .bind(tenant_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        changed(&mut tx, tenant_id, user_id, id, updated.rows_affected()).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Stores the snapshot of a `scanning` refresh and moves it to `ranking` (ranking state `running` until
    /// [`Dashboard::set_ranking`]); from now on it is the current snapshot. `Conflict` unless it is scanning.
    pub async fn store_snapshot<T: Serialize + Sync>(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
        snapshot: &T,
        scanned_at: Timestamp,
    ) -> Result<(), DbError> {
        let document = serde_json::to_string(snapshot)?;
        let mut tx = write_tx(self.pool).await?;
        let updated = sqlx::query(
            "UPDATE dashboard_refreshes SET state = 'ranking', ranking_state = 'running', snapshot = ?,
                 scanned_at = ?
             WHERE id = ? AND tenant_id = ? AND user_id = ? AND state = 'scanning'",
        )
        .bind(document)
        .bind(ts(scanned_at))
        .bind(id.to_string())
        .bind(tenant_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        changed(&mut tx, tenant_id, user_id, id, updated.rows_affected()).await?;
        tx.commit().await?;
        Ok(())
    }

    /// The current snapshot: that of the user's newest refresh in `ranking` or `finished` (a `failed` or
    /// `cancelled` refresh never holds one), with that refresh.
    pub async fn latest_snapshot<T: DeserializeOwned>(
        &self,
        tenant_id: &str,
        user_id: &str,
    ) -> Result<Option<(RefreshRecord, T)>, DbError> {
        let row = sqlx::query(concat!(
            "SELECT ",
            record_columns!(),
            ", snapshot FROM dashboard_refreshes WHERE id IS (",
            current_snapshot_id!(),
            ")"
        ))
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let snapshot: String = row.try_get("snapshot")?;
        Ok(Some((record_of(&row)?, serde_json::from_str(&snapshot)?)))
    }

    /// Records the live ranking state (`waiting_for_analysis`, `running`, `paused`) and the judgement counts of a
    /// refresh `ranking` in `attempt`. [`RankingUpdate::Stale`] for an earlier attempt or an ended ranking (only
    /// `judged` is raised to the reported count); `Conflict` for a newer attempt, a refresh that never ranked, or
    /// an ending ranking state (use [`Dashboard::finish_ranking`]).
    pub async fn set_ranking(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
        attempt: u32,
        state: RankingState,
        counts: JudgeCounts,
    ) -> Result<RankingUpdate, DbError> {
        let mut tx = write_tx(self.pool).await?;
        if ends(state) {
            return Err(refused(&mut tx, tenant_id, user_id, id).await?);
        }
        let rows = sqlx::query(
            "UPDATE dashboard_refreshes SET ranking_state = ?, judged = ?, reused = ?, left_provisional = ?
             WHERE id = ? AND tenant_id = ? AND user_id = ? AND state = 'ranking' AND ranking_attempt = ?",
        )
        .bind(wire_name(state)?)
        .bind(i64::from(counts.judged))
        .bind(i64::from(counts.reused))
        .bind(i64::from(counts.left_provisional))
        .bind(id.to_string())
        .bind(tenant_id)
        .bind(user_id)
        .bind(i64::from(attempt))
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let update = ranking_update(&mut tx, tenant_id, user_id, id, attempt, counts, rows).await?;
        tx.commit().await?;
        Ok(update)
    }

    /// Ends a `scanning` refresh before its snapshot is complete; `Conflict` for any other state.
    pub async fn end_scan(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
        end: &ScanEnd,
        now: Timestamp,
    ) -> Result<(), DbError> {
        let (state, failure) = match end {
            ScanEnd::Failed(failure) => (RefreshState::Failed, Some(failure)),
            ScanEnd::Cancelled => (RefreshState::Cancelled, None),
        };
        let mut tx = write_tx(self.pool).await?;
        let updated = sqlx::query(
            "UPDATE dashboard_refreshes SET state = ?, failure_code = ?, failure_message = ?, retry_at = ?,
                 finished_at = ?
             WHERE id = ? AND tenant_id = ? AND user_id = ? AND state = 'scanning'",
        )
        .bind(wire_name(state)?)
        .bind(failure.map(|f| wire_name(f.code)).transpose()?)
        .bind(failure.map(|f| f.message.as_str()))
        .bind(failure.and_then(|f| f.retry_at).map(ts))
        .bind(ts(now))
        .bind(id.to_string())
        .bind(tenant_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        changed(&mut tx, tenant_id, user_id, id, updated.rows_affected()).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Ends the ranking of a refresh `ranking` in `attempt` with `outcome` (`finished`, `failed`, `cancelled` or
    /// `interrupted`) and the final counts; the refresh becomes `finished` and its snapshot stays current.
    /// [`RankingUpdate::Stale`] for an earlier attempt or an ended ranking, so a late result of a superseded or
    /// restarted ranking is ignored except for its LLM calls (`judged` raised to its count); `Conflict` for a newer
    /// attempt, a refresh that never ranked, or a live ranking state.
    #[expect(
        clippy::too_many_arguments,
        reason = "frozen scope, refresh and attempt plus the outcome; a parameter struct would hide the guard"
    )]
    pub async fn finish_ranking(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
        attempt: u32,
        outcome: RankingState,
        counts: JudgeCounts,
        now: Timestamp,
    ) -> Result<RankingUpdate, DbError> {
        let mut tx = write_tx(self.pool).await?;
        if !ends(outcome) {
            return Err(refused(&mut tx, tenant_id, user_id, id).await?);
        }
        let rows = sqlx::query(
            "UPDATE dashboard_refreshes SET state = 'finished', ranking_state = ?, judged = ?, reused = ?,
                 left_provisional = ?, finished_at = ?
             WHERE id = ? AND tenant_id = ? AND user_id = ? AND state = 'ranking' AND ranking_attempt = ?",
        )
        .bind(wire_name(outcome)?)
        .bind(i64::from(counts.judged))
        .bind(i64::from(counts.reused))
        .bind(i64::from(counts.left_provisional))
        .bind(ts(now))
        .bind(id.to_string())
        .bind(tenant_id)
        .bind(user_id)
        .bind(i64::from(attempt))
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let update = ranking_update(&mut tx, tenant_id, user_id, id, attempt, counts, rows).await?;
        tx.commit().await?;
        Ok(update)
    }

    /// Cancels the ranking of a refresh `ranking` in `attempt` (R3, superseding): the refresh becomes `finished`
    /// with ranking `cancelled` and its stored judgement counts untouched (a report racing the cancel is kept).
    /// [`RankingUpdate::Stale`] for an earlier attempt or an ended ranking; `Conflict` for a newer attempt or a
    /// refresh that never ranked.
    pub async fn cancel_ranking(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
        attempt: u32,
        now: Timestamp,
    ) -> Result<RankingUpdate, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let rows = sqlx::query(
            "UPDATE dashboard_refreshes SET state = 'finished', ranking_state = 'cancelled', finished_at = ?
             WHERE id = ? AND tenant_id = ? AND user_id = ? AND state = 'ranking' AND ranking_attempt = ?",
        )
        .bind(ts(now))
        .bind(id.to_string())
        .bind(tenant_id)
        .bind(user_id)
        .bind(i64::from(attempt))
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let update = ranking_update(
            &mut tx,
            tenant_id,
            user_id,
            id,
            attempt,
            JudgeCounts::default(),
            rows,
        )
        .await?;
        tx.commit().await?;
        Ok(update)
    }

    /// Retry ranking (R4), in one write transaction: a `finished` refresh holding the current snapshot whose
    /// ranking ended `failed`, `interrupted` or `cancelled` goes back to `ranking` (ranking state `running`, counts
    /// kept) in its next ranking attempt. `NotFound` for an unknown id or another user's refresh.
    pub async fn restart_ranking(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
    ) -> Result<RestartOutcome, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let refresh = read_record(&mut *tx, tenant_id, user_id, id).await?;
        let retryable = refresh.state == RefreshState::Finished
            && matches!(
                refresh.ranking_state,
                Some(RankingState::Failed | RankingState::Interrupted | RankingState::Cancelled)
            );
        if !retryable {
            return Ok(RestartOutcome::NotRetryable);
        }
        let live: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM dashboard_refreshes
                 WHERE tenant_id = ? AND user_id = ? AND state IN ('scanning', 'ranking'))",
        )
        .bind(tenant_id)
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
        if live {
            return Ok(RestartOutcome::RefreshRunning);
        }
        let current: Option<String> = sqlx::query_scalar(current_snapshot_id!())
            .bind(tenant_id)
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
        if current != Some(id.to_string()) {
            return Ok(RestartOutcome::NotRetryable);
        }
        sqlx::query(
            "UPDATE dashboard_refreshes SET state = 'ranking', ranking_state = 'running', finished_at = NULL,
                 ranking_attempt = ranking_attempt + 1
             WHERE id = ?",
        )
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        let refresh = read_record(&mut *tx, tenant_id, user_id, id).await?;
        tx.commit().await?;
        Ok(RestartOutcome::Restarted(Box::new(refresh)))
    }

    /// Server-internal (startup), every tenant: `scanning` refreshes become `failed` (code `interrupted`, no
    /// snapshot); `ranking` ones become `finished` with ranking `interrupted`. Afterwards no refresh is live.
    pub async fn interrupt_live(&self, now: Timestamp) -> Result<Vec<InterruptedRefresh>, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let rows: Vec<(String, String, String, String)> = sqlx::query_as(
            "SELECT tenant_id, user_id, id, state FROM dashboard_refreshes WHERE state IN ('scanning', 'ranking')
             ORDER BY tenant_id, user_id",
        )
        .fetch_all(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE dashboard_refreshes SET state = 'failed', failure_code = 'interrupted', failure_message = ?,
                 finished_at = ?
             WHERE state = 'scanning'",
        )
        .bind(INTERRUPTED_MESSAGE)
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE dashboard_refreshes SET state = 'finished', ranking_state = 'interrupted', finished_at = ?
             WHERE state = 'ranking'",
        )
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        rows.into_iter()
            .map(|(tenant_id, user_id, id, was)| {
                Ok(InterruptedRefresh {
                    tenant_id,
                    user_id,
                    id: parse_wire(&id, "dashboard_refreshes.id")?,
                    was: parse_wire(&was, "dashboard_refreshes.state")?,
                })
            })
            .collect()
    }

    /// Counts `items` as sent to the judge by refresh `id` ranking in `attempt`, in one write transaction, when both
    /// caps allow it: the refresh's sends across its attempts and the user's sends since `day_start` (see
    /// [`Dashboard::sent_totals`]). Nothing is counted otherwise.
    #[expect(
        clippy::too_many_arguments,
        reason = "scope, refresh and attempt plus the items, caps and day; a parameter struct would hide the fence"
    )]
    pub async fn reserve_sends(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
        attempt: u32,
        items: u32,
        caps: SendCaps,
        day_start: Timestamp,
    ) -> Result<SendReservation, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT sent FROM dashboard_refreshes
             WHERE id = ? AND tenant_id = ? AND user_id = ? AND state = 'ranking' AND ranking_attempt = ?",
        )
        .bind(id.to_string())
        .bind(tenant_id)
        .bind(user_id)
        .bind(i64::from(attempt))
        .fetch_optional(&mut *tx)
        .await?;
        let Some(sent) = current else {
            return Ok(SendReservation::Stale);
        };
        let day: i64 = sqlx::query_scalar(SENT_SINCE_SQL)
            .bind(tenant_id)
            .bind(user_id)
            .bind(ts(day_start))
            .fetch_one(&mut *tx)
            .await?;
        let items = i64::from(items);
        if sent + items > i64::from(caps.per_refresh) {
            return Ok(SendReservation::OverRefreshCap);
        }
        if day + items > i64::from(caps.per_day) {
            return Ok(SendReservation::OverDailyCap);
        }
        sqlx::query("UPDATE dashboard_refreshes SET sent = sent + ? WHERE id = ?")
            .bind(items)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(SendReservation::Reserved)
    }

    /// Items sent to the judge by refresh `id` and by the user's refreshes that ranked since `day_start` (started
    /// then, still live, or ended then). `NotFound` for an unknown id or another user's refresh.
    pub async fn sent_totals(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
        day_start: Timestamp,
    ) -> Result<SentTotals, DbError> {
        let mut tx = super::read_tx(self.pool).await?;
        let refresh: i64 = sqlx::query_scalar(
            "SELECT sent FROM dashboard_refreshes WHERE id = ? AND tenant_id = ? AND user_id = ?",
        )
        .bind(id.to_string())
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        let day: i64 = sqlx::query_scalar(SENT_SINCE_SQL)
            .bind(tenant_id)
            .bind(user_id)
            .bind(ts(day_start))
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        let total = |n: i64| {
            u32::try_from(n).map_err(|_| DbError::Corrupt {
                column: "dashboard_refreshes.sent",
            })
        };
        Ok(SentTotals {
            refresh: total(refresh)?,
            day: total(day)?,
        })
    }

    /// Records a live ranking state and counts, stores judgements and optionally replaces progress and log, in one
    /// write transaction, only while the refresh is ranking in `attempt`. [`RankingUpdate::Stale`] for an earlier
    /// attempt or an ended ranking (nothing stored; `judged` raised as in [`Dashboard::set_ranking`]); `Conflict` for
    /// a newer attempt, a refresh that never ranked, or an ending ranking state.
    pub async fn write_ranking(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
        attempt: u32,
        write: &RankingWrite<'_>,
    ) -> Result<RankingUpdate, DbError> {
        let mut tx = write_tx(self.pool).await?;
        if ends(write.state) {
            return Err(refused(&mut tx, tenant_id, user_id, id).await?);
        }
        let (progress, log) = match write.progress {
            Some((progress, log)) => (
                Some(serde_json::to_string(progress)?),
                Some(serde_json::to_string(
                    &log[log.len().saturating_sub(LOG_LINES_KEPT)..],
                )?),
            ),
            None => (None, None),
        };
        let rows = sqlx::query(
            "UPDATE dashboard_refreshes SET ranking_state = ?, judged = ?, reused = ?, left_provisional = ?,
                 progress = COALESCE(?, progress), log = COALESCE(?, log)
             WHERE id = ? AND tenant_id = ? AND user_id = ? AND state = 'ranking' AND ranking_attempt = ?",
        )
        .bind(wire_name(write.state)?)
        .bind(i64::from(write.counts.judged))
        .bind(i64::from(write.counts.reused))
        .bind(i64::from(write.counts.left_provisional))
        .bind(progress)
        .bind(log)
        .bind(id.to_string())
        .bind(tenant_id)
        .bind(user_id)
        .bind(i64::from(attempt))
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let update =
            ranking_update(&mut tx, tenant_id, user_id, id, attempt, write.counts, rows).await?;
        if update == RankingUpdate::Applied {
            for judgement in write.judgements {
                upsert_judgement(&mut tx, tenant_id, user_id, judgement).await?;
            }
        }
        tx.commit().await?;
        Ok(update)
    }

    /// The user's stored judgements of these items (missing ones are absent).
    pub async fn judgements(
        &self,
        tenant_id: &str,
        user_id: &str,
        item_keys: &[ItemKey],
    ) -> Result<Vec<StoredJudgement>, DbError> {
        if item_keys.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(JUDGEMENTS_SQL)
            .bind(tenant_id)
            .bind(user_id)
            .bind(serde_json::to_string(item_keys)?)
            .fetch_all(self.pool)
            .await?;
        rows.iter().map(judgement_of).collect()
    }

    /// Stores judgements, replacing the user's previous judgement of each item.
    pub async fn save_judgements(
        &self,
        tenant_id: &str,
        user_id: &str,
        judgements: &[StoredJudgement],
    ) -> Result<(), DbError> {
        let mut tx = write_tx(self.pool).await?;
        for judgement in judgements {
            upsert_judgement(&mut tx, tenant_id, user_id, judgement).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Marks the user's judgements of these items as seen at `now` (retention).
    pub async fn mark_seen(
        &self,
        tenant_id: &str,
        user_id: &str,
        item_keys: &[ItemKey],
        now: Timestamp,
    ) -> Result<(), DbError> {
        if item_keys.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "UPDATE dashboard_judgements SET last_seen_at = ?4
             WHERE tenant_id = ?1 AND user_id = ?2 AND item_key IN (SELECT value FROM json_each(?3))",
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(serde_json::to_string(item_keys)?)
        .bind(ts(now))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// LLM judgements made by the user's refreshes started at or after `since` (their `judged` counts, so a
    /// re-judged item counts again); the daily cap counts from the local day's start. Accepted approximation: no
    /// column records when the judging happened, so a ranking that runs past `since` counts wholly by its start.
    pub async fn judged_since(
        &self,
        tenant_id: &str,
        user_id: &str,
        since: Timestamp,
    ) -> Result<u32, DbError> {
        let count: i64 = sqlx::query_scalar(JUDGED_SINCE_SQL)
            .bind(tenant_id)
            .bind(user_id)
            .bind(ts(since))
            .fetch_one(self.pool)
            .await?;
        u32::try_from(count).map_err(|_| DbError::Corrupt {
            column: "dashboard_refreshes.judged",
        })
    }

    /// Server-internal (retention): deletes judgements of every tenant not seen since `before`.
    pub async fn delete_unseen_judgements(&self, before: Timestamp) -> Result<u64, DbError> {
        Ok(sqlx::query(DELETE_UNSEEN_SQL)
            .bind(ts(before))
            .execute(self.pool)
            .await?
            .rows_affected())
    }
}

/// Stores a judgement, replacing the user's previous judgement of the item.
async fn upsert_judgement(
    tx: &mut Transaction<'static, Sqlite>,
    tenant_id: &str,
    user_id: &str,
    judgement: &StoredJudgement,
) -> Result<(), DbError> {
    let saved = sqlx::query(
            "INSERT INTO dashboard_judgements (tenant_id, user_id, item_key, input_fingerprint, input_version,
                 prompt_version, body, refresh_id, job_id, model, judged_at, last_seen_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (user_id, item_key) DO UPDATE SET input_fingerprint = excluded.input_fingerprint,
                 input_version = excluded.input_version, prompt_version = excluded.prompt_version,
                 body = excluded.body, refresh_id = excluded.refresh_id, job_id = excluded.job_id,
                 model = excluded.model, judged_at = excluded.judged_at, last_seen_at = excluded.last_seen_at
             WHERE dashboard_judgements.tenant_id = excluded.tenant_id",
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(judgement.item_key.as_str())
        .bind(&judgement.input_fingerprint)
        .bind(i64::from(judgement.input_version))
        .bind(i64::from(judgement.prompt_version))
        .bind(serde_json::to_string(&judgement.body)?)
        .bind(judgement.refresh_id.map(|id| id.to_string()))
        .bind(judgement.job_id.map(|id| id.to_string()))
        .bind(&judgement.model)
        .bind(ts(judgement.judged_at))
        .bind(ts(judgement.last_seen_at))
        .execute(&mut **tx)
        .await
        .map_err(missing_owner)?;
    found(saved.rows_affected())?;
    Ok(())
}

/// Whether a ranking state ends the ranking (the refresh is then `finished`).
fn ends(state: RankingState) -> bool {
    match state {
        RankingState::WaitingForAnalysis | RankingState::Running | RankingState::Paused => false,
        RankingState::Failed
        | RankingState::Interrupted
        | RankingState::Cancelled
        | RankingState::Finished => true,
    }
}

/// An upsert that changed no row met a row of the user in another tenant.
fn found(rows: u64) -> Result<(), DbError> {
    if rows == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// A write naming an unknown user, tenant or refresh (foreign key) reads as not found.
fn missing_owner(error: sqlx::Error) -> DbError {
    match &error {
        sqlx::Error::Database(database) if database.is_foreign_key_violation() => DbError::NotFound,
        _ => error.into(),
    }
}

/// After a guarded update of `rows` rows: none changed means the refresh is the user's but in another state
/// (`Conflict`) or not theirs (`NotFound`).
async fn changed(
    tx: &mut Transaction<'static, Sqlite>,
    tenant_id: &str,
    user_id: &str,
    id: RefreshId,
    rows: u64,
) -> Result<(), DbError> {
    if rows > 0 {
        return Ok(());
    }
    Err(refused(tx, tenant_id, user_id, id).await?)
}

/// After a guarded ranking update of `rows` rows: none changed means `NotFound` for another user's refresh,
/// `Conflict` for a newer attempt or a refresh that never ranked (a caller bug, never a late result), else a stale
/// result whose LLM calls are still recorded (`judged` raised to its count, for the daily cap).
async fn ranking_update(
    tx: &mut Transaction<'static, Sqlite>,
    tenant_id: &str,
    user_id: &str,
    id: RefreshId,
    attempt: u32,
    counts: JudgeCounts,
    rows: u64,
) -> Result<RankingUpdate, DbError> {
    if rows > 0 {
        return Ok(RankingUpdate::Applied);
    }
    let stored: Option<(i64, Option<String>)> = sqlx::query_as(
        "SELECT ranking_attempt, ranking_state FROM dashboard_refreshes
         WHERE id = ? AND tenant_id = ? AND user_id = ?",
    )
    .bind(id.to_string())
    .bind(tenant_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((stored_attempt, ranking_state)) = stored else {
        return Err(DbError::NotFound);
    };
    if i64::from(attempt) > stored_attempt || ranking_state.is_none() {
        return Err(DbError::Conflict);
    }
    sqlx::query("UPDATE dashboard_refreshes SET judged = MAX(judged, ?) WHERE id = ?")
        .bind(i64::from(counts.judged))
        .bind(id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(RankingUpdate::Stale)
}

/// The error for a refused change: `Conflict` for the user's refresh, `NotFound` for any other id.
async fn refused(
    tx: &mut Transaction<'static, Sqlite>,
    tenant_id: &str,
    user_id: &str,
    id: RefreshId,
) -> Result<DbError, DbError> {
    Ok(if owned(tx, tenant_id, user_id, id).await? {
        DbError::Conflict
    } else {
        DbError::NotFound
    })
}

async fn owned(
    tx: &mut Transaction<'static, Sqlite>,
    tenant_id: &str,
    user_id: &str,
    id: RefreshId,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM dashboard_refreshes WHERE id = ? AND tenant_id = ? AND user_id = ?)",
    )
    .bind(id.to_string())
    .bind(tenant_id)
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?)
}

async fn read_record<'c>(
    executor: impl SqliteExecutor<'c>,
    tenant_id: &str,
    user_id: &str,
    id: RefreshId,
) -> Result<RefreshRecord, DbError> {
    let row = sqlx::query(concat!(
        "SELECT ",
        record_columns!(),
        " FROM dashboard_refreshes WHERE id = ? AND tenant_id = ? AND user_id = ?"
    ))
    .bind(id.to_string())
    .bind(tenant_id)
    .bind(user_id)
    .fetch_optional(executor)
    .await?
    .ok_or(DbError::NotFound)?;
    record_of(&row)
}

fn json_column<T: DeserializeOwned>(row: &SqliteRow, column: &'static str) -> Result<T, DbError> {
    Ok(serde_json::from_str(&row.try_get::<String, _>(column)?)?)
}

fn optional_ts(row: &SqliteRow, column: &'static str) -> Result<Option<Timestamp>, DbError> {
    row.try_get::<Option<String>, _>(column)?
        .map(|at| parse_ts(&at, column))
        .transpose()
}

fn count(row: &SqliteRow, column: &'static str) -> Result<u32, DbError> {
    u32::try_from(row.try_get::<i64, _>(column)?).map_err(|_| DbError::Corrupt { column })
}

fn record_of(row: &SqliteRow) -> Result<RefreshRecord, DbError> {
    let failure = match row.try_get::<Option<String>, _>("failure_code")? {
        None => None,
        Some(code) => Some(RefreshFailure {
            code: parse_wire(&code, "failure_code")?,
            message: row.try_get::<Option<String>, _>("failure_message")?.ok_or(
                DbError::Corrupt {
                    column: "failure_message",
                },
            )?,
            retry_at: optional_ts(row, "retry_at")?,
        }),
    };
    Ok(RefreshRecord {
        id: parse_wire(&row.try_get::<String, _>("id")?, "id")?,
        user_id: row.try_get("user_id")?,
        trigger: parse_wire(&row.try_get::<String, _>("trigger")?, "trigger")?,
        state: parse_wire(&row.try_get::<String, _>("state")?, "state")?,
        progress: json_column(row, "progress")?,
        log: json_column(row, "log")?,
        failure,
        ranking_state: row
            .try_get::<Option<String>, _>("ranking_state")?
            .map(|state| parse_wire(&state, "ranking_state"))
            .transpose()?,
        ranking_attempt: count(row, "ranking_attempt")?,
        counts: JudgeCounts {
            judged: count(row, "judged")?,
            reused: count(row, "reused")?,
            left_provisional: count(row, "left_provisional")?,
        },
        has_snapshot: row.try_get("has_snapshot")?,
        started_at: parse_ts(&row.try_get::<String, _>("started_at")?, "started_at")?,
        scanned_at: optional_ts(row, "scanned_at")?,
        finished_at: optional_ts(row, "finished_at")?,
    })
}

fn settings_of(row: &SqliteRow) -> Result<StoredDashboardSettings, DbError> {
    let on: Option<String> = row.try_get("last_scheduled_on")?;
    let outcome: Option<String> = row.try_get("last_scheduled_outcome")?;
    let last_scheduled = match (on, outcome) {
        (Some(on), Some(outcome)) => Some(ScheduledRunRecord {
            on: Date::strptime(LOCAL_DATE, &on).map_err(|_| DbError::Corrupt {
                column: "last_scheduled_on",
            })?,
            outcome: parse_wire(&outcome, "last_scheduled_outcome")?,
            reason: row.try_get("last_scheduled_reason")?,
        }),
        (None, None) => None,
        _ => {
            return Err(DbError::Corrupt {
                column: "last_scheduled_outcome",
            });
        }
    };
    Ok(StoredDashboardSettings {
        settings: DashboardSettings {
            linked_logins: json_column(row, "linked_logins")?,
            repositories: json_column(row, "repositories")?,
            enumerate_owners: json_column(row, "enumerate_owners")?,
            lookback_days: count(row, "lookback_days")?,
            ready_label: row.try_get("ready_label")?,
            skip_authors: json_column(row, "skip_authors")?,
            known_bots: json_column(row, "known_bots")?,
            include_todos: row.try_get("include_todos")?,
            memcan_extra_projects: json_column(row, "memcan_extra_projects")?,
            schedule_enabled: row.try_get("schedule_enabled")?,
            schedule_time: row.try_get("schedule_time")?,
            time_zone: row.try_get("time_zone")?,
        },
        last_scheduled,
        updated_at: parse_ts(&row.try_get::<String, _>("updated_at")?, "updated_at")?,
    })
}

fn judgement_of(row: &SqliteRow) -> Result<StoredJudgement, DbError> {
    let version = |column: &'static str| -> Result<u32, DbError> { count(row, column) };
    Ok(StoredJudgement {
        item_key: ItemKey::parse(&row.try_get::<String, _>("item_key")?)
            .map_err(|_| DbError::Corrupt { column: "item_key" })?,
        input_fingerprint: row.try_get("input_fingerprint")?,
        input_version: version("input_version")?,
        prompt_version: version("prompt_version")?,
        body: json_column(row, "body")?,
        refresh_id: row
            .try_get::<Option<String>, _>("refresh_id")?
            .map(|id| parse_wire(&id, "refresh_id"))
            .transpose()?,
        job_id: row
            .try_get::<Option<String>, _>("job_id")?
            .map(|id| parse_wire(&id, "job_id"))
            .transpose()?,
        model: row.try_get("model")?,
        judged_at: parse_ts(&row.try_get::<String, _>("judged_at")?, "judged_at")?,
        last_seen_at: parse_ts(&row.try_get::<String, _>("last_seen_at")?, "last_seen_at")?,
    })
}

#[cfg(test)]
mod tests {
    use contracts::api::dashboard::{
        ItemTarget, PolicyRef, RefreshFailureCode, RepositoryRule, ScanStepKey, StepState,
    };
    use contracts::common::RepoRef;
    use contracts::job::spec::ReasoningEffort;
    use serde_json::json;

    use super::*;
    use crate::repo::DEFAULT_TENANT as T;
    use crate::repo::analyses::testing::seed_analysis;
    use crate::repo::testing::file_backed;

    fn at(minute: i64) -> Timestamp {
        "2026-11-18T06:00:00Z"
            .parse::<Timestamp>()
            .unwrap_or_default()
            + SignedDuration::from_mins(minute)
    }

    async fn seed_users(db: &Db) {
        sqlx::raw_sql(
            "INSERT INTO users (id, github_id, login, created_at, updated_at)
               VALUES ('u1', 1, 'alice', 't', 't'), ('u2', 2, 'bob', 't', 't'), ('u3', 3, 'carol', 't', 't');
             INSERT INTO tenants (id, name, created_at) VALUES ('other', 'other', 't');",
        )
        .execute(&db.pool)
        .await
        .expect("users");
    }

    async fn db() -> Db {
        let db = Db::in_memory().await.expect("db");
        seed_users(&db).await;
        db
    }

    fn started(outcome: StartOutcome) -> (RefreshRecord, Option<RefreshId>) {
        match outcome {
            StartOutcome::Started {
                refresh,
                superseded,
            } => (*refresh, superseded),
            StartOutcome::ScanRunning => panic!("a scan is running"),
        }
    }

    async fn start_in(db: &Db, tenant: &str, user: &str, minute: i64) -> RefreshId {
        let outcome = db
            .dashboard()
            .start_refresh(tenant, user, RefreshTrigger::Manual, at(minute))
            .await
            .expect("start");
        started(outcome).0.id
    }

    async fn start(db: &Db, user: &str, minute: i64) -> RefreshId {
        start_in(db, T, user, minute).await
    }

    /// A refresh of `user` in `ranking` holding the snapshot `{"n": minute}`.
    async fn ranking_in(db: &Db, tenant: &str, user: &str, minute: i64) -> RefreshId {
        let id = start_in(db, tenant, user, minute).await;
        db.dashboard()
            .store_snapshot(tenant, user, id, &json!({ "n": minute }), at(minute))
            .await
            .expect("snapshot");
        id
    }

    async fn ranking(db: &Db, user: &str, minute: i64) -> RefreshId {
        ranking_in(db, T, user, minute).await
    }

    const COUNTS: JudgeCounts = JudgeCounts {
        judged: 2,
        reused: 3,
        left_provisional: 1,
    };

    async fn finished(db: &Db, user: &str, minute: i64, outcome: RankingState) -> RefreshId {
        let id = ranking(db, user, minute).await;
        db.dashboard()
            .finish_ranking(T, user, id, 0, outcome, COUNTS, at(minute))
            .await
            .map(|update| assert_eq!(update, RankingUpdate::Applied))
            .expect("finish");
        id
    }

    async fn record(db: &Db, user: &str, id: RefreshId) -> RefreshRecord {
        db.dashboard().refresh(T, user, id).await.expect("refresh")
    }

    #[tokio::test]
    async fn cancel_ranking_keeps_the_stored_counts() {
        let db = db().await;
        let id = ranking(&db, "u1", 1).await;
        let dash = db.dashboard();
        assert_eq!(
            dash.set_ranking(T, "u1", id, 0, RankingState::Running, COUNTS)
                .await
                .expect("report"),
            RankingUpdate::Applied
        );
        assert_eq!(
            dash.cancel_ranking(T, "u1", id, 0, at(2))
                .await
                .expect("cancel"),
            RankingUpdate::Applied
        );
        let stored = record(&db, "u1", id).await;
        assert_eq!(
            (
                stored.state,
                stored.ranking_state,
                stored.counts,
                stored.finished_at
            ),
            (
                RefreshState::Finished,
                Some(RankingState::Cancelled),
                COUNTS,
                Some(at(2))
            )
        );
        assert_eq!(
            dash.cancel_ranking(T, "u1", id, 0, at(3))
                .await
                .expect("again"),
            RankingUpdate::Stale
        );
        assert!(matches!(
            dash.cancel_ranking(T, "u1", id, 1, at(3)).await,
            Err(DbError::Conflict)
        ));
        assert!(matches!(
            dash.cancel_ranking(T, "u2", id, 0, at(3)).await,
            Err(DbError::NotFound)
        ));
    }

    /// The current snapshot's refresh and its `n`.
    async fn current(db: &Db, user: &str) -> Option<(RefreshId, i64)> {
        db.dashboard()
            .latest_snapshot::<Value>(T, user)
            .await
            .expect("snapshot")
            .map(|(refresh, snapshot)| (refresh.id, snapshot["n"].as_i64().unwrap_or(-1)))
    }

    /// The user's live refreshes, in any tenant.
    async fn live(db: &Db, user: &str) -> Vec<(RefreshId, RefreshState)> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, state FROM dashboard_refreshes WHERE user_id = ? AND state IN ('scanning', 'ranking')",
        )
        .bind(user)
        .fetch_all(&db.pool)
        .await
        .expect("live");
        rows.into_iter()
            .map(|(id, state)| {
                (
                    super::super::jobs::parse_wire(&id, "id").expect("id"),
                    super::super::jobs::parse_wire(&state, "state").expect("state"),
                )
            })
            .collect()
    }

    async fn refresh_ids(db: &Db, user: &str) -> Vec<RefreshId> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM dashboard_refreshes WHERE user_id = ? ORDER BY started_at, rowid",
        )
        .bind(user)
        .fetch_all(&db.pool)
        .await
        .expect("ids");
        rows.iter()
            .map(|id| super::super::jobs::parse_wire(id, "id").expect("id"))
            .collect()
    }

    fn failure() -> RefreshFailure {
        RefreshFailure {
            code: RefreshFailureCode::GithubRateLimited,
            message: "GitHub's rate limit was reached. Retry at 06:41.".to_owned(),
            retry_at: Some(at(41)),
        }
    }

    fn date(day: i8) -> Date {
        Date::new(2026, 11, day).expect("date")
    }

    fn assert_err<T: std::fmt::Debug>(result: Result<T, DbError>, conflict: bool) {
        if conflict {
            assert!(matches!(result, Err(DbError::Conflict)), "{result:?}");
        } else {
            assert!(matches!(result, Err(DbError::NotFound)), "{result:?}");
        }
    }

    #[tokio::test]
    async fn absent_settings_are_none_and_saved_settings_round_trip() {
        let db = db().await;
        let dash = db.dashboard();
        assert_eq!(dash.settings(T, "u1").await.expect("read"), None);
        let mut settings = DashboardSettings {
            linked_logins: vec!["alice-work".to_owned()],
            repositories: vec![RepositoryRule {
                repo: RepoRef {
                    owner: "dashpay".to_owned(),
                    name: "platform".to_owned(),
                },
                alias: Some("PLAT".to_owned()),
                watch_issues: true,
                ownership_policy: Some(PolicyRef {
                    repo: RepoRef {
                        owner: "dashpay".to_owned(),
                        name: "stale_prs_are_bad".to_owned(),
                    },
                    path: "policies/platform.json".to_owned(),
                }),
                branch_patterns: vec!["vX.Y-dev".to_owned()],
            }],
            enumerate_owners: vec!["dashpay".to_owned()],
            lookback_days: 14,
            ready_label: "ready".to_owned(),
            skip_authors: Vec::new(),
            known_bots: vec!["Carol".to_owned()],
            include_todos: false,
            memcan_extra_projects: vec!["notes".to_owned()],
            schedule_enabled: false,
            schedule_time: "07:15".to_owned(),
            time_zone: Some("Europe/Warsaw".to_owned()),
        };
        dash.save_settings(T, "u1", &settings, at(0))
            .await
            .expect("save");
        assert_eq!(
            dash.settings(T, "u1").await.expect("read"),
            Some(StoredDashboardSettings {
                settings: settings.clone(),
                last_scheduled: None,
                updated_at: at(0),
            })
        );
        settings.lookback_days = 3;
        settings.time_zone = None;
        dash.save_settings(T, "u1", &settings, at(5))
            .await
            .expect("update");
        let stored = dash.settings(T, "u1").await.expect("read").expect("row");
        assert_eq!((stored.settings, stored.updated_at), (settings, at(5)));
        assert_eq!(dash.settings(T, "u2").await.expect("other user"), None);
    }

    #[tokio::test]
    async fn time_zone_and_scheduled_run_create_a_defaults_row_and_survive_saves() {
        let db = db().await;
        let dash = db.dashboard();
        dash.set_time_zone_if_unset(T, "u1", "Europe/Warsaw", at(0))
            .await
            .expect("zone");
        let stored = dash.settings(T, "u1").await.expect("read").expect("row");
        assert_eq!(
            stored.settings,
            DashboardSettings {
                time_zone: Some("Europe/Warsaw".to_owned()),
                ..DashboardSettings::default()
            }
        );
        assert_eq!((stored.last_scheduled, stored.updated_at), (None, at(0)));
        let run = ScheduledRunRecord {
            on: date(18),
            outcome: ScheduledOutcome::NeedsSignIn,
            reason: Some("Sign in again".to_owned()),
        };
        dash.record_scheduled_run(T, "u1", &run).await.expect("run");
        dash.record_scheduled_run(T, "u2", &run)
            .await
            .expect("a run creates a defaults row");
        let other = dash.settings(T, "u2").await.expect("read").expect("row");
        assert_eq!(
            (other.settings, other.last_scheduled),
            (DashboardSettings::default(), Some(run.clone()))
        );
        let saved = DashboardSettings {
            schedule_time: "06:30".to_owned(),
            ..DashboardSettings::default()
        };
        dash.save_settings(T, "u1", &saved, at(1))
            .await
            .expect("save");
        let stored = dash.settings(T, "u1").await.expect("read").expect("row");
        assert_eq!(
            (stored.settings, stored.last_scheduled),
            (saved, Some(run)),
            "saving keeps the recorded run"
        );
        let next = ScheduledRunRecord {
            on: date(19),
            outcome: ScheduledOutcome::Started,
            reason: None,
        };
        dash.record_scheduled_run(T, "u1", &next)
            .await
            .expect("next");
        let stored = dash.settings(T, "u1").await.expect("read").expect("row");
        assert_eq!(stored.last_scheduled, Some(next));
        assert_eq!(stored.settings.schedule_time, "06:30");
    }

    #[tokio::test]
    async fn a_stored_time_zone_is_kept_by_the_first_visit_save() {
        let db = db().await;
        let dash = db.dashboard();
        dash.set_time_zone_if_unset(T, "u1", "Europe/Warsaw", at(0))
            .await
            .expect("first zone");
        dash.set_time_zone_if_unset(T, "u1", "UTC", at(1))
            .await
            .expect("a stored zone is no error");
        let stored = dash.settings(T, "u1").await.expect("read").expect("row");
        assert_eq!(
            (stored.settings.time_zone.as_deref(), stored.updated_at),
            (Some("Europe/Warsaw"), at(0)),
            "the zone chosen first stays"
        );
        dash.save_settings(T, "u1", &DashboardSettings::default(), at(2))
            .await
            .expect("Settings clears the zone");
        dash.set_time_zone_if_unset(T, "u1", "UTC", at(3))
            .await
            .expect("zone again");
        let stored = dash.settings(T, "u1").await.expect("read").expect("row");
        assert_eq!(stored.settings.time_zone.as_deref(), Some("UTC"));
    }

    #[tokio::test]
    async fn settings_are_scoped_by_tenant_and_user() {
        let db = db().await;
        let dash = db.dashboard();
        dash.save_settings(T, "u1", &DashboardSettings::default(), at(0))
            .await
            .expect("save");
        assert_eq!(dash.settings("other", "u1").await.expect("read"), None);
        let run = ScheduledRunRecord {
            on: date(18),
            outcome: ScheduledOutcome::Skipped,
            reason: None,
        };
        let zone = DashboardSettings {
            time_zone: Some("UTC".to_owned()),
            ..DashboardSettings::default()
        };
        assert_err(dash.save_settings("other", "u1", &zone, at(1)).await, false);
        assert_err(
            dash.set_time_zone_if_unset("other", "u1", "UTC", at(1))
                .await,
            false,
        );
        assert_err(dash.record_scheduled_run("other", "u1", &run).await, false);
        let stored = dash.settings(T, "u1").await.expect("read").expect("row");
        assert_eq!(
            (stored.settings, stored.last_scheduled, stored.updated_at),
            (DashboardSettings::default(), None, at(0)),
            "another tenant's write changes nothing"
        );
        assert_err(dash.save_settings(T, "nobody", &zone, at(1)).await, false);
        assert_err(
            dash.set_time_zone_if_unset("missing", "u2", "UTC", at(1))
                .await,
            false,
        );
    }

    #[tokio::test]
    async fn schedule_candidates_have_the_schedule_on_and_a_time_zone() {
        let db = db().await;
        let dash = db.dashboard();
        sqlx::query("INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES ('u4', 4, 'dan', 't', 't')")
            .execute(&db.pool)
            .await
            .expect("user");
        dash.set_time_zone_if_unset(T, "u1", "Europe/Warsaw", at(0))
            .await
            .expect("on, zone");
        let off = DashboardSettings {
            schedule_enabled: false,
            time_zone: Some("UTC".to_owned()),
            ..DashboardSettings::default()
        };
        dash.save_settings(T, "u2", &off, at(0)).await.expect("off");
        dash.set_time_zone_if_unset("other", "u3", "America/New_York", at(0))
            .await
            .expect("other tenant");
        dash.save_settings(T, "u4", &DashboardSettings::default(), at(0))
            .await
            .expect("no zone");
        sqlx::query("UPDATE dashboard_settings SET repositories = '[1]' WHERE user_id = 'u3'")
            .execute(&db.pool)
            .await
            .expect("an undecodable row");
        let candidates = dash.schedule_candidates().await.expect("candidates");
        let unreadable: Vec<(&str, &str)> = candidates
            .iter()
            .filter_map(|c| c.as_ref().err())
            .map(|u| (u.tenant_id.as_str(), u.user_id.as_str()))
            .collect();
        assert_eq!(
            unreadable,
            [("other", "u3")],
            "reported, not failing the list"
        );
        let found: Vec<(&str, &str, Option<&str>)> = candidates
            .iter()
            .filter_map(|c| c.as_ref().ok())
            .map(|c| {
                (
                    c.tenant_id.as_str(),
                    c.user_id.as_str(),
                    c.settings.settings.time_zone.as_deref(),
                )
            })
            .collect();
        assert_eq!(found, [(T, "u1", Some("Europe/Warsaw"))]);
    }

    #[tokio::test]
    async fn start_refresh_scans_and_is_refused_while_one_scans() {
        let db = db().await;
        let dash = db.dashboard();
        assert_eq!(dash.latest_refresh(T, "u1").await.expect("none"), None);
        let (refresh, superseded) = started(
            dash.start_refresh(T, "u1", RefreshTrigger::Scheduled, at(0))
                .await
                .expect("start"),
        );
        assert_eq!(superseded, None);
        assert_eq!(
            refresh,
            RefreshRecord {
                id: refresh.id,
                user_id: "u1".to_owned(),
                trigger: RefreshTrigger::Scheduled,
                state: RefreshState::Scanning,
                progress: RefreshProgress::default(),
                log: Vec::new(),
                failure: None,
                ranking_state: None,
                ranking_attempt: 0,
                counts: JudgeCounts::default(),
                has_snapshot: false,
                started_at: at(0),
                scanned_at: None,
                finished_at: None,
            }
        );
        assert_eq!(record(&db, "u1", refresh.id).await, refresh);
        for trigger in [RefreshTrigger::Manual, RefreshTrigger::Scheduled] {
            assert_eq!(
                dash.start_refresh(T, "u1", trigger, at(1))
                    .await
                    .expect("refused"),
                StartOutcome::ScanRunning
            );
        }
        start(&db, "u2", 1).await;
        assert_eq!(
            dash.latest_refresh(T, "u1").await.expect("latest"),
            Some(refresh.clone()),
            "nothing was stored for the refused starts"
        );
        assert_eq!(
            live(&db, "u1").await,
            [(refresh.id, RefreshState::Scanning)]
        );
    }

    #[tokio::test]
    async fn start_supersedes_a_ranking_refresh_in_every_live_ranking_state() {
        for state in [
            RankingState::WaitingForAnalysis,
            RankingState::Running,
            RankingState::Paused,
        ] {
            let db = db().await;
            let dash = db.dashboard();
            let old = ranking(&db, "u1", 0).await;
            dash.set_ranking(T, "u1", old, 0, state, COUNTS)
                .await
                .map(|update| assert_eq!(update, RankingUpdate::Applied))
                .expect("ranking state");
            let (new, superseded) = started(
                dash.start_refresh(T, "u1", RefreshTrigger::Scheduled, at(10))
                    .await
                    .expect("start"),
            );
            assert_eq!(superseded, Some(old), "{state:?}");
            assert_eq!(new.state, RefreshState::Scanning);
            let old_record = record(&db, "u1", old).await;
            assert_eq!(
                (
                    old_record.state,
                    old_record.ranking_state,
                    old_record.counts,
                    old_record.has_snapshot,
                    old_record.finished_at
                ),
                (
                    RefreshState::Finished,
                    Some(RankingState::Cancelled),
                    COUNTS,
                    true,
                    Some(at(10))
                ),
                "{state:?}"
            );
            assert_eq!(
                current(&db, "u1").await,
                Some((old, 0)),
                "the superseded snapshot stays current until the new scan stores one"
            );
            assert_eq!(live(&db, "u1").await, [(new.id, RefreshState::Scanning)]);
        }
    }

    #[tokio::test]
    async fn supersede_and_start_are_one_transaction() {
        let db = db().await;
        let old = ranking(&db, "u1", 0).await;
        sqlx::raw_sql(
            "CREATE TEMP TRIGGER refuse_new_refresh BEFORE INSERT ON dashboard_refreshes
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )
        .execute(&db.pool)
        .await
        .expect("trigger");
        let refused = db
            .dashboard()
            .start_refresh(T, "u1", RefreshTrigger::Manual, at(1))
            .await;
        assert!(matches!(refused, Err(DbError::Storage(_))), "{refused:?}");
        let old_record = record(&db, "u1", old).await;
        assert_eq!(
            (old_record.state, old_record.ranking_state),
            (RefreshState::Ranking, Some(RankingState::Running)),
            "the supersede is rolled back with the failed start"
        );
    }

    #[tokio::test]
    async fn stored_snapshot_moves_the_scan_to_ranking_and_becomes_current() {
        let db = db().await;
        let dash = db.dashboard();
        let id = start(&db, "u1", 0).await;
        assert_eq!(current(&db, "u1").await, None, "a scan holds no snapshot");
        dash.store_snapshot(T, "u1", id, &json!({ "n": 7 }), at(2))
            .await
            .expect("store");
        let stored = record(&db, "u1", id).await;
        assert_eq!(
            (
                stored.state,
                stored.ranking_state,
                stored.has_snapshot,
                stored.scanned_at,
                stored.finished_at
            ),
            (
                RefreshState::Ranking,
                Some(RankingState::Running),
                true,
                Some(at(2)),
                None
            )
        );
        assert_eq!(current(&db, "u1").await, Some((id, 7)));
        assert_err(
            dash.store_snapshot(T, "u1", id, &json!({ "n": 8 }), at(3))
                .await,
            true,
        );
        assert_eq!(current(&db, "u1").await, Some((id, 7)));
    }

    #[tokio::test]
    async fn scan_ends_failed_or_cancelled_and_the_previous_snapshot_stays() {
        let db = db().await;
        let dash = db.dashboard();
        let first = finished(&db, "u1", 0, RankingState::Finished).await;
        let failing = start(&db, "u1", 10).await;
        dash.end_scan(T, "u1", failing, &ScanEnd::Failed(failure()), at(12))
            .await
            .expect("fail");
        let failed = record(&db, "u1", failing).await;
        assert_eq!(
            (
                failed.state,
                failed.failure,
                failed.finished_at,
                failed.has_snapshot,
                failed.ranking_state
            ),
            (
                RefreshState::Failed,
                Some(failure()),
                Some(at(12)),
                false,
                None
            )
        );
        assert_eq!(current(&db, "u1").await, Some((first, 0)));
        let cancelling = start(&db, "u1", 20).await;
        dash.end_scan(T, "u1", cancelling, &ScanEnd::Cancelled, at(21))
            .await
            .expect("cancel");
        let cancelled = record(&db, "u1", cancelling).await;
        assert_eq!(
            (
                cancelled.state,
                cancelled.failure.clone(),
                cancelled.finished_at
            ),
            (RefreshState::Cancelled, None, Some(at(21)))
        );
        assert_eq!(current(&db, "u1").await, Some((first, 0)));
        assert_eq!(
            dash.latest_refresh(T, "u1").await.expect("latest"),
            Some(cancelled)
        );
        let ranked = ranking(&db, "u1", 30).await;
        for id in [first, failing, cancelling, ranked] {
            for end in [ScanEnd::Cancelled, ScanEnd::Failed(failure())] {
                assert_err(dash.end_scan(T, "u1", id, &end, at(40)).await, true);
            }
            assert_err(
                dash.store_snapshot(T, "u1", id, &json!({ "n": 9 }), at(40))
                    .await,
                true,
            );
        }
    }

    #[tokio::test]
    async fn ranking_states_and_every_outcome() {
        for outcome in [
            RankingState::Finished,
            RankingState::Failed,
            RankingState::Cancelled,
            RankingState::Interrupted,
        ] {
            let db = db().await;
            let dash = db.dashboard();
            let id = ranking(&db, "u1", 0).await;
            for (n, state) in [
                RankingState::WaitingForAnalysis,
                RankingState::Paused,
                RankingState::Running,
            ]
            .into_iter()
            .enumerate()
            {
                let counts = JudgeCounts {
                    judged: u32::try_from(n).expect("small"),
                    reused: 5,
                    left_provisional: 0,
                };
                dash.set_ranking(T, "u1", id, 0, state, counts)
                    .await
                    .map(|update| assert_eq!(update, RankingUpdate::Applied))
                    .expect("live ranking state");
                let stored = record(&db, "u1", id).await;
                assert_eq!(
                    (stored.state, stored.ranking_state, stored.counts),
                    (RefreshState::Ranking, Some(state), counts)
                );
            }
            for ending in [
                RankingState::Failed,
                RankingState::Interrupted,
                RankingState::Cancelled,
                RankingState::Finished,
            ] {
                assert_err(dash.set_ranking(T, "u1", id, 0, ending, COUNTS).await, true);
            }
            for live_state in [
                RankingState::WaitingForAnalysis,
                RankingState::Running,
                RankingState::Paused,
            ] {
                assert_err(
                    dash.finish_ranking(T, "u1", id, 0, live_state, COUNTS, at(5))
                        .await,
                    true,
                );
            }
            dash.finish_ranking(T, "u1", id, 0, outcome, COUNTS, at(5))
                .await
                .map(|update| assert_eq!(update, RankingUpdate::Applied))
                .expect("finish");
            let stored = record(&db, "u1", id).await;
            assert_eq!(
                (
                    stored.state,
                    stored.ranking_state,
                    stored.counts,
                    stored.finished_at,
                    stored.has_snapshot
                ),
                (
                    RefreshState::Finished,
                    Some(outcome),
                    COUNTS,
                    Some(at(5)),
                    true
                ),
                "{outcome:?}"
            );
            assert_eq!(current(&db, "u1").await, Some((id, 0)), "{outcome:?}");
            assert_eq!(
                dash.set_ranking(T, "u1", id, 0, RankingState::Running, COUNTS)
                    .await
                    .expect("late"),
                RankingUpdate::Stale
            );
            assert_eq!(
                dash.finish_ranking(T, "u1", id, 0, RankingState::Finished, COUNTS, at(6))
                    .await
                    .expect("late"),
                RankingUpdate::Stale
            );
            assert_eq!(
                record(&db, "u1", id).await,
                stored,
                "a late result changes nothing"
            );
            assert!(live(&db, "u1").await.is_empty());
        }
    }

    /// A late result of attempt 0 after Retry ranking is ignored; attempt 1 keeps running and ends normally.
    #[tokio::test]
    async fn late_result_after_restart_ranking_is_stale() {
        let db = db().await;
        let dash = db.dashboard();
        let id = finished(&db, "u1", 0, RankingState::Failed).await;
        assert!(matches!(
            dash.restart_ranking(T, "u1", id).await.expect("restart"),
            RestartOutcome::Restarted(_)
        ));
        let restarted = record(&db, "u1", id).await;
        let late = JudgeCounts {
            judged: 9,
            ..COUNTS
        };
        assert_eq!(
            dash.set_ranking(T, "u1", id, 0, RankingState::Paused, late)
                .await
                .expect("late"),
            RankingUpdate::Stale
        );
        assert_eq!(
            dash.finish_ranking(T, "u1", id, 0, RankingState::Finished, late, at(5))
                .await
                .expect("late"),
            RankingUpdate::Stale
        );
        assert_eq!(
            record(&db, "u1", id).await,
            RefreshRecord {
                counts: JudgeCounts {
                    judged: 9,
                    ..restarted.counts
                },
                ..restarted.clone()
            },
            "a late result only records the calls it made"
        );
        assert_eq!(
            (
                restarted.state,
                restarted.ranking_state,
                restarted.ranking_attempt
            ),
            (RefreshState::Ranking, Some(RankingState::Running), 1)
        );
        assert_err(
            dash.set_ranking(T, "u1", id, 2, RankingState::Running, late)
                .await,
            true,
        );
        assert_eq!(
            dash.finish_ranking(T, "u1", id, 1, RankingState::Finished, late, at(6))
                .await
                .expect("current"),
            RankingUpdate::Applied
        );
        let done = record(&db, "u1", id).await;
        assert_eq!(
            (
                done.state,
                done.ranking_state,
                done.counts,
                done.ranking_attempt
            ),
            (
                RefreshState::Finished,
                Some(RankingState::Finished),
                late,
                1
            )
        );
    }

    /// A late result of a superseded ranking is ignored and leaves the new refresh untouched.
    #[tokio::test]
    async fn late_result_after_a_supersede_is_stale() {
        let db = db().await;
        let dash = db.dashboard();
        let old = ranking(&db, "u1", 0).await;
        let new = start(&db, "u1", 1).await;
        let (old_before, new_before) = (record(&db, "u1", old).await, record(&db, "u1", new).await);
        assert_eq!(old_before.ranking_state, Some(RankingState::Cancelled));
        assert_eq!(
            dash.set_ranking(T, "u1", old, 0, RankingState::Running, COUNTS)
                .await
                .expect("late"),
            RankingUpdate::Stale
        );
        assert_eq!(
            dash.finish_ranking(T, "u1", old, 0, RankingState::Finished, COUNTS, at(2))
                .await
                .expect("late"),
            RankingUpdate::Stale
        );
        let lower = JudgeCounts {
            judged: 1,
            ..COUNTS
        };
        assert_eq!(
            dash.finish_ranking(T, "u1", old, 0, RankingState::Failed, lower, at(3))
                .await
                .expect("late, lower"),
            RankingUpdate::Stale
        );
        assert_eq!(
            (record(&db, "u1", old).await, record(&db, "u1", new).await),
            (
                RefreshRecord {
                    counts: JudgeCounts {
                        judged: COUNTS.judged,
                        ..old_before.counts
                    },
                    ..old_before
                },
                new_before
            ),
            "only the calls made are recorded, never lowered; the new refresh is untouched"
        );
        assert_eq!(
            dash.judged_since(T, "u1", at(0)).await.expect("count"),
            COUNTS.judged
        );
    }

    /// An attempt newer than the stored one, or a refresh that never ranked, cannot come from a late result.
    #[tokio::test]
    async fn future_attempt_or_unranked_refresh_is_a_conflict() {
        let db = db().await;
        let dash = db.dashboard();
        let ranked = ranking(&db, "u1", 0).await;
        assert_err(
            dash.set_ranking(T, "u1", ranked, 1, RankingState::Running, COUNTS)
                .await,
            true,
        );
        assert_err(
            dash.finish_ranking(T, "u1", ranked, 1, RankingState::Finished, COUNTS, at(1))
                .await,
            true,
        );
        let scanning = start(&db, "u1", 2).await;
        assert_err(
            dash.set_ranking(T, "u1", scanning, 0, RankingState::Running, COUNTS)
                .await,
            true,
        );
        dash.end_scan(T, "u1", scanning, &ScanEnd::Failed(failure()), at(3))
            .await
            .expect("fail");
        assert_err(
            dash.finish_ranking(T, "u1", scanning, 0, RankingState::Finished, COUNTS, at(4))
                .await,
            true,
        );
        assert_eq!(
            record(&db, "u1", scanning).await.counts,
            JudgeCounts::default()
        );
    }

    /// R3 in each phase: a scan ends `cancelled`, a ranking ends `finished` with ranking `cancelled`, an ended
    /// refresh cannot be cancelled.
    #[tokio::test]
    async fn cancel_in_each_phase() {
        let db = db().await;
        let dash = db.dashboard();
        let scanning = start(&db, "u1", 0).await;
        dash.end_scan(T, "u1", scanning, &ScanEnd::Cancelled, at(1))
            .await
            .expect("cancel the scan");
        assert_eq!(
            record(&db, "u1", scanning).await.state,
            RefreshState::Cancelled
        );
        let ranked = ranking(&db, "u1", 2).await;
        dash.finish_ranking(T, "u1", ranked, 0, RankingState::Cancelled, COUNTS, at(3))
            .await
            .map(|update| assert_eq!(update, RankingUpdate::Applied))
            .expect("cancel the ranking");
        let stored = record(&db, "u1", ranked).await;
        assert_eq!(
            (stored.state, stored.ranking_state),
            (RefreshState::Finished, Some(RankingState::Cancelled))
        );
        assert_eq!(current(&db, "u1").await, Some((ranked, 2)));
        for id in [scanning, ranked] {
            assert_err(
                dash.end_scan(T, "u1", id, &ScanEnd::Cancelled, at(4)).await,
                true,
            );
        }
        assert_err(
            dash.finish_ranking(T, "u1", scanning, 0, RankingState::Cancelled, COUNTS, at(4))
                .await,
            true,
        );
        assert_eq!(
            dash.finish_ranking(T, "u1", ranked, 0, RankingState::Cancelled, COUNTS, at(4))
                .await
                .expect("ended"),
            RankingUpdate::Stale
        );
        assert!(live(&db, "u1").await.is_empty());
    }

    #[tokio::test]
    async fn progress_of_a_live_refresh_keeps_the_newest_log_lines() {
        let db = db().await;
        let dash = db.dashboard();
        let id = start(&db, "u1", 0).await;
        let progress = RefreshProgress {
            steps: vec![ScanStep {
                key: ScanStepKey::PrDetails,
                state: StepState::Running,
                done: Some(3),
                total: Some(10),
                detail: Some("acme/node-client#482".to_owned()),
            }],
            percent: 40,
        };
        let log: Vec<String> = (0..250).map(|i| format!("line {i}")).collect();
        dash.record_progress(T, "u1", id, &progress, &log)
            .await
            .expect("progress");
        let stored = record(&db, "u1", id).await;
        assert_eq!(stored.progress, progress);
        assert_eq!(stored.log.len(), LOG_LINES_KEPT);
        assert_eq!(
            (stored.log.first(), stored.log.last()),
            (Some(&"line 50".to_owned()), Some(&"line 249".to_owned()))
        );
        dash.store_snapshot(T, "u1", id, &json!({ "n": 0 }), at(1))
            .await
            .expect("snapshot");
        let short = vec!["ranking".to_owned()];
        dash.record_progress(T, "u1", id, &RefreshProgress::default(), &short)
            .await
            .expect("progress while ranking");
        assert_eq!(record(&db, "u1", id).await.log, short);
        dash.finish_ranking(T, "u1", id, 0, RankingState::Finished, COUNTS, at(2))
            .await
            .map(|update| assert_eq!(update, RankingUpdate::Applied))
            .expect("finish");
        assert_err(
            dash.record_progress(T, "u1", id, &progress, &log).await,
            true,
        );
        assert_eq!(record(&db, "u1", id).await.log, short);
    }

    #[tokio::test]
    async fn another_users_or_tenants_refresh_is_not_found() {
        let db = db().await;
        let dash = db.dashboard();
        let old = finished(&db, "u1", 0, RankingState::Failed).await;
        let scanning = start(&db, "u1", 1).await;
        let unknown = RefreshId::new_random();
        let before = (
            record(&db, "u1", old).await,
            record(&db, "u1", scanning).await,
        );
        for (tenant, user) in [(T, "u2"), ("other", "u1"), (T, "nobody")] {
            for id in [old, scanning, unknown] {
                assert_err(dash.refresh(tenant, user, id).await, false);
                assert_err(
                    dash.record_progress(tenant, user, id, &RefreshProgress::default(), &[])
                        .await,
                    false,
                );
                assert_err(
                    dash.store_snapshot(tenant, user, id, &json!({}), at(2))
                        .await,
                    false,
                );
                assert_err(
                    dash.end_scan(tenant, user, id, &ScanEnd::Cancelled, at(2))
                        .await,
                    false,
                );
                assert_err(
                    dash.set_ranking(tenant, user, id, 0, RankingState::Running, COUNTS)
                        .await,
                    false,
                );
                assert_err(
                    dash.finish_ranking(
                        tenant,
                        user,
                        id,
                        0,
                        RankingState::Cancelled,
                        COUNTS,
                        at(2),
                    )
                    .await,
                    false,
                );
                assert_err(dash.restart_ranking(tenant, user, id).await, false);
            }
            assert_eq!(
                dash.latest_refresh(tenant, user).await.expect("latest"),
                None
            );
            assert_eq!(
                dash.latest_snapshot::<Value>(tenant, user)
                    .await
                    .expect("snapshot"),
                None
            );
        }
        assert_eq!(
            (
                record(&db, "u1", old).await,
                record(&db, "u1", scanning).await
            ),
            before,
            "foreign calls change nothing"
        );
    }

    #[tokio::test]
    async fn restart_ranking_of_a_retryable_current_snapshot() {
        for outcome in [
            RankingState::Failed,
            RankingState::Interrupted,
            RankingState::Cancelled,
        ] {
            let db = db().await;
            let dash = db.dashboard();
            let id = finished(&db, "u1", 0, outcome).await;
            let restarted = match dash.restart_ranking(T, "u1", id).await.expect("restart") {
                RestartOutcome::Restarted(refresh) => *refresh,
                other => panic!("{outcome:?}: {other:?}"),
            };
            assert_eq!(
                (
                    restarted.state,
                    restarted.ranking_state,
                    restarted.counts,
                    restarted.finished_at,
                    restarted.scanned_at
                ),
                (
                    RefreshState::Ranking,
                    Some(RankingState::Running),
                    COUNTS,
                    None,
                    Some(at(0))
                ),
                "{outcome:?}"
            );
            assert_eq!(restarted.ranking_attempt, 1);
            assert_eq!(record(&db, "u1", id).await, restarted);
            assert_eq!(current(&db, "u1").await, Some((id, 0)));
            assert_eq!(
                dash.restart_ranking(T, "u1", id).await.expect("live"),
                RestartOutcome::NotRetryable,
                "a live ranking is not retried"
            );
        }
    }

    #[tokio::test]
    async fn restart_ranking_refuses_other_refreshes_and_while_one_is_live() {
        let db = db().await;
        let dash = db.dashboard();
        let restart = |id| dash.restart_ranking(T, "u1", id);
        let older = finished(&db, "u1", 0, RankingState::Failed).await;
        let done = finished(&db, "u1", 10, RankingState::Finished).await;
        assert_eq!(
            restart(older).await.expect("older"),
            RestartOutcome::NotRetryable,
            "not the current snapshot"
        );
        assert_eq!(
            restart(done).await.expect("done"),
            RestartOutcome::NotRetryable,
            "the ranking finished"
        );
        let failing = start(&db, "u1", 20).await;
        dash.end_scan(T, "u1", failing, &ScanEnd::Failed(failure()), at(21))
            .await
            .expect("fail");
        assert_eq!(
            restart(failing).await.expect("failed"),
            RestartOutcome::NotRetryable
        );
        let cancelled = start(&db, "u1", 22).await;
        dash.end_scan(T, "u1", cancelled, &ScanEnd::Cancelled, at(23))
            .await
            .expect("cancel");
        assert_eq!(
            restart(cancelled).await.expect("cancelled"),
            RestartOutcome::NotRetryable
        );

        // A superseded ranking is retryable once the new scan ended without a snapshot.
        let superseded = ranking(&db, "u1", 30).await;
        let scanning = start(&db, "u1", 31).await;
        assert_eq!(
            restart(superseded).await.expect("while scanning"),
            RestartOutcome::RefreshRunning
        );
        assert_eq!(
            restart(scanning).await.expect("scanning"),
            RestartOutcome::NotRetryable
        );
        dash.end_scan(T, "u1", scanning, &ScanEnd::Cancelled, at(32))
            .await
            .expect("cancel");
        assert!(matches!(
            restart(superseded).await.expect("retry"),
            RestartOutcome::Restarted(_)
        ));
        assert_eq!(live(&db, "u1").await, [(superseded, RefreshState::Ranking)]);
        assert_eq!(
            restart(done).await.expect("other live"),
            RestartOutcome::NotRetryable
        );
    }

    #[tokio::test]
    async fn interrupt_live_ends_every_live_refresh_after_a_restart() {
        let db = db().await;
        let dash = db.dashboard();
        let kept = finished(&db, "u1", 0, RankingState::Finished).await;
        let scanning = start(&db, "u1", 1).await;
        let ranked = ranking(&db, "u2", 1).await;
        let foreign = ranking_in(&db, "other", "u3", 1).await;
        let mut interrupted = dash.interrupt_live(at(30)).await.expect("interrupt");
        interrupted.sort_by(|a, b| a.user_id.cmp(&b.user_id));
        assert_eq!(
            interrupted,
            [
                InterruptedRefresh {
                    tenant_id: T.to_owned(),
                    user_id: "u1".to_owned(),
                    id: scanning,
                    was: RefreshState::Scanning,
                },
                InterruptedRefresh {
                    tenant_id: T.to_owned(),
                    user_id: "u2".to_owned(),
                    id: ranked,
                    was: RefreshState::Ranking,
                },
                InterruptedRefresh {
                    tenant_id: "other".to_owned(),
                    user_id: "u3".to_owned(),
                    id: foreign,
                    was: RefreshState::Ranking,
                },
            ]
        );
        let failed = record(&db, "u1", scanning).await;
        let failure = failed.failure.clone().expect("failure");
        assert_eq!(
            (
                failed.state,
                failure.code,
                failure.retry_at,
                failed.finished_at
            ),
            (
                RefreshState::Failed,
                RefreshFailureCode::Interrupted,
                None,
                Some(at(30))
            )
        );
        assert!(!failure.message.is_empty());
        assert_eq!(current(&db, "u1").await, Some((kept, 0)));
        let stopped = record(&db, "u2", ranked).await;
        assert_eq!(
            (stopped.state, stopped.ranking_state, stopped.finished_at),
            (
                RefreshState::Finished,
                Some(RankingState::Interrupted),
                Some(at(30))
            )
        );
        assert_eq!(current(&db, "u2").await, Some((ranked, 1)));
        for user in ["u1", "u2", "u3"] {
            assert!(live(&db, user).await.is_empty(), "{user}");
        }
        assert_eq!(dash.interrupt_live(at(31)).await.expect("again"), []);
        start(&db, "u1", 32).await;
        assert!(matches!(
            dash.restart_ranking(T, "u2", ranked).await.expect("retry"),
            RestartOutcome::Restarted(_)
        ));
    }

    #[tokio::test]
    async fn refresh_retention_keeps_ten() {
        let db = db().await;
        let dash = db.dashboard();
        let holder = finished(&db, "u1", 0, RankingState::Finished).await;
        let judged = StoredJudgement {
            refresh_id: Some(holder),
            ..judgement(1, "aa", at(0))
        };
        dash.save_judgements(T, "u1", std::slice::from_ref(&judged))
            .await
            .expect("judgement");
        let other = finished(&db, "u2", 0, RankingState::Finished).await;
        let mut ended = Vec::new();
        for minute in 1..=12 {
            let id = start(&db, "u1", minute).await;
            let end = if minute % 2 == 0 {
                ScanEnd::Cancelled
            } else {
                ScanEnd::Failed(failure())
            };
            dash.end_scan(T, "u1", id, &end, at(minute))
                .await
                .expect("end");
            ended.push(id);
        }
        let mut expected = vec![holder];
        expected.extend_from_slice(&ended[2..]);
        assert_eq!(
            refresh_ids(&db, "u1").await,
            expected,
            "recent refreshes without judgements follow the newest-ten rule"
        );
        assert_eq!(refresh_ids(&db, "u2").await, [other]);

        let later = 2 * 24 * 60;
        let newer = start(&db, "u1", later).await;
        let mut expected = vec![holder];
        expected.extend_from_slice(&ended[3..]);
        expected.push(newer);
        assert_eq!(
            refresh_ids(&db, "u1").await,
            expected,
            "older ones: the newest ten plus the one holding the current snapshot"
        );
        dash.store_snapshot(T, "u1", newer, &json!({ "n": 20 }), at(later + 1))
            .await
            .expect("snapshot");
        dash.finish_ranking(
            T,
            "u1",
            newer,
            0,
            RankingState::Finished,
            COUNTS,
            at(later + 2),
        )
        .await
        .map(|update| assert_eq!(update, RankingUpdate::Applied))
        .expect("finish");
        let last = start(&db, "u1", later + 30).await;
        let ids = refresh_ids(&db, "u1").await;
        assert_eq!(ids.len(), usize::try_from(REFRESHES_KEPT).expect("small"));
        assert!(!ids.contains(&holder), "an old snapshot is not kept");
        assert_eq!(ids.last(), Some(&last));
        assert_eq!(current(&db, "u1").await, Some((newer, 20)));
        assert_eq!(
            dash.judgements(T, "u1", &[key(1)]).await.expect("kept"),
            [StoredJudgement {
                refresh_id: None,
                ..judged
            }],
            "a judgement outlives its refresh"
        );
    }

    /// Two simultaneous starts for one user, with no refresh live and with one ranking: exactly one starts.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_live_refresh_per_user() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db = file_backed(dir.path()).await.expect("file database");
        seed_users(&db).await;
        for round in 0..8i64 {
            let ranking_before = live(&db, "u1").await.first().map(|(id, _)| *id);
            let tasks: Vec<_> = (0..2)
                .map(|i| {
                    let db = db.clone();
                    tokio::spawn(async move {
                        db.dashboard()
                            .start_refresh(T, "u1", RefreshTrigger::Manual, at(round * 10 + i))
                            .await
                    })
                })
                .collect();
            let mut outcomes = Vec::new();
            for task in tasks {
                outcomes.push(task.await.expect("task").expect("start"));
            }
            let mut new = None;
            let mut refused = 0;
            for outcome in outcomes {
                match outcome {
                    StartOutcome::Started {
                        refresh,
                        superseded,
                    } => {
                        assert!(new.is_none(), "round {round}: two refreshes started");
                        assert_eq!(superseded, ranking_before, "round {round}");
                        new = Some(refresh.id);
                    }
                    StartOutcome::ScanRunning => refused += 1,
                }
            }
            let new = new.expect("one started");
            assert_eq!(refused, 1, "round {round}");
            assert_eq!(live(&db, "u1").await, [(new, RefreshState::Scanning)]);
            db.dashboard()
                .store_snapshot(T, "u1", new, &json!({ "n": round }), at(round * 10 + 5))
                .await
                .expect("snapshot");
        }
    }

    /// Right after a start: besides the newest ten and the current snapshot, only refreshes started in the last 48
    /// hours that judged something are kept.
    async fn assert_retention(db: &Db, user: &str, now: Timestamp) {
        let holder = current(db, user).await.map(|(id, _)| id);
        let ids = refresh_ids(db, user).await;
        let older = ids
            .len()
            .saturating_sub(usize::try_from(REFRESHES_KEPT).expect("small"));
        for id in &ids[..older] {
            let kept = record(db, user, *id).await;
            assert!(
                Some(*id) == holder
                    || (kept.started_at >= now - RECENT_REFRESHES_KEPT && kept.counts.judged > 0),
                "{id} started {} kept at {now}",
                kept.started_at
            );
        }
    }

    /// One live refresh per user across tenants: a live refresh in another tenant makes a start or a Retry
    /// ranking answer `Conflict`, never a storage error.
    #[tokio::test]
    async fn live_refresh_in_another_tenant_is_a_conflict() {
        let db = db().await;
        let dash = db.dashboard();
        let retry = finished(&db, "u1", 0, RankingState::Failed).await;
        let elsewhere = start_in(&db, "other", "u1", 1).await;
        assert_err(
            dash.start_refresh(T, "u1", RefreshTrigger::Manual, at(2))
                .await,
            true,
        );
        assert_err(dash.restart_ranking(T, "u1", retry).await, true);
        assert_eq!(live(&db, "u1").await, [(elsewhere, RefreshState::Scanning)]);
        assert_eq!(
            record(&db, "u1", retry).await.ranking_state,
            Some(RankingState::Failed)
        );
    }

    /// A seeded random walk over every transition (both users): after each step at most one refresh per user is
    /// live, each operation's outcome follows the state table, and every live refresh can still end.
    #[tokio::test]
    async fn no_sequence_strands_the_one_live_rule() {
        let db = db().await;
        let dash = db.dashboard();
        let mut seed: u64 = 0x5eed_cafe;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let live_states = [
            RankingState::WaitingForAnalysis,
            RankingState::Running,
            RankingState::Paused,
        ];
        let endings = [
            RankingState::Failed,
            RankingState::Interrupted,
            RankingState::Cancelled,
            RankingState::Finished,
        ];
        let mut snapshots = 0i64;
        let mut seen = std::collections::BTreeMap::<&str, u32>::new();
        let mut saw = |event: &'static str| *seen.entry(event).or_default() += 1;
        for step in 0..1000i64 {
            let now = at(step * 20);
            let user = if next() % 3 == 0 { "u2" } else { "u1" };
            let before = live(&db, user).await;
            let ids = refresh_ids(&db, user).await;
            let pick = usize::try_from(next()).unwrap_or_default();
            // Mostly the newest refresh (where the live one is), sometimes any or an unknown one.
            let target = match (ids.last(), pick % 8) {
                (Some(newest), 0..=4) => *newest,
                (Some(_), 5 | 6) => ids[pick % ids.len()],
                _ => RefreshId::new_random(),
            };
            let state = match dash.refresh(T, user, target).await {
                Ok(found) => Some(found),
                Err(DbError::NotFound) => None,
                Err(other) => panic!("{other:?}"),
            };
            let target_state = state.as_ref().map(|r| r.state);
            // True when the transition happened.
            let expect = |result: Result<(), DbError>, allowed: bool| match (result, allowed) {
                (Ok(()), true) => true,
                (Err(DbError::Conflict), false) if state.is_some() => false,
                (Err(DbError::NotFound), false) if state.is_none() => false,
                (other, _) => panic!("step {step}: {other:?} for {state:?}"),
            };
            let op = next() % 9;
            match op {
                0 | 1 => {
                    let outcome = dash
                        .start_refresh(T, user, RefreshTrigger::Scheduled, now)
                        .await
                        .expect("start");
                    match before.first() {
                        Some((_, RefreshState::Scanning)) => {
                            assert_eq!(outcome, StartOutcome::ScanRunning, "step {step}");
                            saw("scan_running");
                        }
                        live_ranking => {
                            let (_, superseded) = started(outcome);
                            assert_eq!(superseded, live_ranking.map(|(id, _)| *id), "step {step}");
                            assert_retention(&db, user, now).await;
                            saw(if superseded.is_some() {
                                "superseded"
                            } else {
                                "started"
                            });
                        }
                    }
                }
                2 => {
                    snapshots += 1;
                    let result = dash
                        .store_snapshot(T, user, target, &json!({ "n": snapshots }), now)
                        .await;
                    if expect(result, target_state == Some(RefreshState::Scanning)) {
                        saw("snapshot");
                    }
                }
                3 => {
                    let end = if next() % 2 == 0 {
                        ScanEnd::Cancelled
                    } else {
                        ScanEnd::Failed(failure())
                    };
                    let result = dash.end_scan(T, user, target, &end, now).await;
                    if expect(result, target_state == Some(RefreshState::Scanning)) {
                        saw("end_scan");
                    }
                }
                4 | 5 => {
                    let current_attempt = state.as_ref().map_or(0, |r| r.ranking_attempt);
                    // Mostly the current attempt, sometimes an earlier one (a late result) or a later one (a bug).
                    let attempt = match next() % 6 {
                        0 => current_attempt.saturating_sub(1),
                        1 => current_attempt + 1,
                        _ => current_attempt,
                    };
                    let result = if op == 4 {
                        let ranking_state =
                            live_states[usize::try_from(next() % 3).unwrap_or_default()];
                        dash.set_ranking(T, user, target, attempt, ranking_state, COUNTS)
                            .await
                    } else {
                        let outcome = endings[usize::try_from(next() % 4).unwrap_or_default()];
                        dash.finish_ranking(T, user, target, attempt, outcome, COUNTS, now)
                            .await
                    };
                    let applies =
                        target_state == Some(RefreshState::Ranking) && attempt == current_attempt;
                    let refused = attempt > current_attempt
                        || state.as_ref().is_some_and(|r| r.ranking_state.is_none());
                    match result {
                        Ok(RankingUpdate::Applied) => {
                            assert!(applies, "step {step}");
                            saw(if op == 4 {
                                "set_ranking"
                            } else {
                                "finish_ranking"
                            });
                        }
                        Ok(RankingUpdate::Stale) => {
                            assert!(state.is_some() && !applies && !refused, "step {step}");
                            let expected = state.clone().map(|r| RefreshRecord {
                                counts: JudgeCounts {
                                    judged: r.counts.judged.max(COUNTS.judged),
                                    ..r.counts
                                },
                                ..r
                            });
                            assert_eq!(
                                dash.refresh(T, user, target).await.ok(),
                                expected,
                                "step {step}: a stale result only records the calls it made"
                            );
                            saw("stale");
                        }
                        Err(DbError::Conflict) => {
                            assert!(state.is_some() && refused, "step {step}");
                            assert_eq!(dash.refresh(T, user, target).await.ok(), state);
                            saw("future_or_unranked");
                        }
                        Err(DbError::NotFound) => assert!(state.is_none(), "step {step}"),
                        Err(other) => panic!("step {step}: {other:?}"),
                    }
                }
                6 => {
                    let holder = current(&db, user).await.map(|(id, _)| id);
                    let result = dash.restart_ranking(T, user, target).await;
                    let retryable = state.as_ref().is_some_and(|r| {
                        r.state == RefreshState::Finished
                            && matches!(
                                r.ranking_state,
                                Some(
                                    RankingState::Failed
                                        | RankingState::Interrupted
                                        | RankingState::Cancelled
                                )
                            )
                    });
                    match result {
                        Ok(RestartOutcome::Restarted(_)) => {
                            assert!(
                                retryable && before.is_empty() && holder == Some(target),
                                "step {step}"
                            );
                            saw("restarted");
                        }
                        Ok(RestartOutcome::RefreshRunning) => {
                            assert!(retryable && !before.is_empty(), "step {step}");
                            saw("refresh_running");
                        }
                        Ok(RestartOutcome::NotRetryable) => {
                            assert!(
                                state.is_some()
                                    && (!retryable
                                        || (before.is_empty() && holder != Some(target))),
                                "step {step}"
                            );
                            saw("not_retryable");
                        }
                        Err(DbError::NotFound) => assert!(state.is_none(), "step {step}"),
                        Err(other) => panic!("step {step}: {other:?}"),
                    }
                }
                7 => {
                    let result = dash
                        .record_progress(T, user, target, &RefreshProgress::default(), &[])
                        .await;
                    let allowed = matches!(
                        target_state,
                        Some(RefreshState::Scanning | RefreshState::Ranking)
                    );
                    if expect(result, allowed) {
                        saw("progress");
                    }
                }
                _ => {
                    if next() % 4 == 0 {
                        if !dash
                            .interrupt_live(now)
                            .await
                            .expect("interrupt")
                            .is_empty()
                        {
                            saw("interrupted");
                        }
                        assert!(
                            live(&db, "u1").await.is_empty() && live(&db, "u2").await.is_empty()
                        );
                    }
                }
            }
            for user in ["u1", "u2"] {
                assert!(live(&db, user).await.len() <= 1, "step {step}");
            }
        }
        let events: Vec<&str> = seen.keys().copied().collect();
        assert_eq!(
            events,
            [
                "end_scan",
                "finish_ranking",
                "future_or_unranked",
                "interrupted",
                "not_retryable",
                "progress",
                "refresh_running",
                "restarted",
                "scan_running",
                "set_ranking",
                "snapshot",
                "stale",
                "started",
                "superseded"
            ],
            "every transition happens in the walk: {seen:?}"
        );
        // Every live refresh can still end, after which a start is never blocked.
        for user in ["u1", "u2"] {
            for (id, state) in live(&db, user).await {
                if state == RefreshState::Scanning {
                    dash.end_scan(T, user, id, &ScanEnd::Cancelled, at(20_000))
                        .await
                        .expect("end scan");
                } else {
                    dash.finish_ranking(
                        T,
                        user,
                        id,
                        0,
                        RankingState::Cancelled,
                        COUNTS,
                        at(20_000),
                    )
                    .await
                    .map(|update| assert_eq!(update, RankingUpdate::Applied))
                    .expect("end ranking");
                }
            }
            start(&db, user, 20_001).await;
        }
    }

    fn key(number: u32) -> ItemKey {
        ItemKey::new(&ItemTarget::PullRequest {
            repo: RepoRef {
                owner: "dashpay".to_owned(),
                name: "platform".to_owned(),
            },
            number,
        })
        .expect("key")
    }

    fn judgement(number: u32, fingerprint: &str, judged_at: Timestamp) -> StoredJudgement {
        StoredJudgement {
            item_key: key(number),
            input_fingerprint: fingerprint.to_owned(),
            input_version: 1,
            prompt_version: 1,
            body: json!({ "id": number, "why": "Blocks the release." }),
            refresh_id: None,
            job_id: Some(JobId::new_random()),
            model: "gpt-6.1-sol".to_owned(),
            judged_at,
            last_seen_at: judged_at,
        }
    }

    #[tokio::test]
    async fn judgements_are_stored_per_user_and_replaced() {
        let db = db().await;
        let dash = db.dashboard();
        let refresh = ranking(&db, "u1", 0).await;
        let first = StoredJudgement {
            refresh_id: Some(refresh),
            ..judgement(1, "aa", at(1))
        };
        let second = StoredJudgement {
            job_id: None,
            ..judgement(2, "bb", at(1))
        };
        dash.save_judgements(T, "u1", &[first.clone(), second.clone()])
            .await
            .expect("save");
        let keys = [key(1), key(2), key(3)];
        assert_eq!(
            dash.judgements(T, "u1", &keys).await.expect("read"),
            [first.clone(), second.clone()]
        );
        assert_eq!(dash.judgements(T, "u1", &[]).await.expect("none"), []);
        assert_eq!(dash.judgements(T, "u2", &keys).await.expect("u2"), []);
        assert_eq!(
            dash.judgements("other", "u1", &keys).await.expect("other"),
            []
        );
        let replaced = StoredJudgement {
            input_fingerprint: "cc".to_owned(),
            prompt_version: 2,
            judged_at: at(5),
            last_seen_at: at(5),
            ..first.clone()
        };
        dash.save_judgements(T, "u1", std::slice::from_ref(&replaced))
            .await
            .expect("replace");
        assert_eq!(
            dash.judgements(T, "u1", &[key(1)]).await.expect("read"),
            std::slice::from_ref(&replaced)
        );
        let theirs = judgement(1, "dd", at(2));
        dash.save_judgements(T, "u2", std::slice::from_ref(&theirs))
            .await
            .expect("u2's own");
        assert_eq!(dash.judgements(T, "u2", &keys).await.expect("u2"), [theirs]);
        assert_err(
            dash.save_judgements("other", "u1", &[judgement(1, "ee", at(3))])
                .await,
            false,
        );
        assert_err(
            dash.save_judgements(T, "nobody", &[judgement(1, "ee", at(3))])
                .await,
            false,
        );
        assert_eq!(
            dash.judgements(T, "u1", &keys).await.expect("unchanged"),
            [replaced, second]
        );
    }

    /// The daily cap counts judgements made (each refresh's `judged`), so re-judging an item counts again.
    #[tokio::test]
    async fn judged_since_counts_judgements_made() {
        let db = db().await;
        let dash = db.dashboard();
        let judged = |judged| JudgeCounts {
            judged,
            reused: 7,
            left_provisional: 1,
        };
        let first = ranking(&db, "u1", 0).await;
        dash.finish_ranking(T, "u1", first, 0, RankingState::Finished, judged(3), at(1))
            .await
            .map(|update| assert_eq!(update, RankingUpdate::Applied))
            .expect("first");
        // The same item is judged again: its stored judgement is replaced, the call still counts.
        dash.save_judgements(T, "u1", &[judgement(1, "a", at(1))])
            .await
            .expect("judgement");
        let second = ranking(&db, "u1", 10).await;
        dash.set_ranking(T, "u1", second, 0, RankingState::Running, judged(2))
            .await
            .map(|update| assert_eq!(update, RankingUpdate::Applied))
            .expect("second, still ranking");
        dash.save_judgements(T, "u1", &[judgement(1, "b", at(11))])
            .await
            .expect("judged again");
        let theirs = ranking(&db, "u2", 0).await;
        dash.finish_ranking(T, "u2", theirs, 0, RankingState::Finished, judged(5), at(1))
            .await
            .map(|update| assert_eq!(update, RankingUpdate::Applied))
            .expect("u2");
        assert_eq!(dash.judged_since(T, "u1", at(0)).await.expect("count"), 5);
        assert_eq!(dash.judged_since(T, "u1", at(5)).await.expect("count"), 2);
        assert_eq!(dash.judged_since(T, "u1", at(11)).await.expect("count"), 0);
        assert_eq!(dash.judged_since(T, "u2", at(0)).await.expect("count"), 5);
        assert_eq!(
            dash.judged_since("other", "u1", at(0))
                .await
                .expect("count"),
            0
        );
    }

    /// Retention never forgets calls of the last 24 hours: more than ten refreshes of one day all count.
    #[tokio::test]
    async fn judged_since_survives_retention_within_a_day() {
        let db = db().await;
        let dash = db.dashboard();
        for minute in 0..15 {
            let id = ranking(&db, "u1", minute * 60).await;
            let counts = JudgeCounts {
                judged: 8,
                ..JudgeCounts::default()
            };
            dash.finish_ranking(
                T,
                "u1",
                id,
                0,
                RankingState::Finished,
                counts,
                at(minute * 60),
            )
            .await
            .map(|update| assert_eq!(update, RankingUpdate::Applied))
            .expect("finish");
        }
        let judged = refresh_ids(&db, "u1").await;
        assert_eq!(judged.len(), 15);
        let mut scans = Vec::new();
        for minute in 0..12 {
            let id = start(&db, "u1", 900 + minute).await;
            dash.end_scan(T, "u1", id, &ScanEnd::Cancelled, at(900 + minute))
                .await
                .expect("cancel");
            scans.push(id);
        }
        let mut expected = judged;
        expected.extend_from_slice(&scans[2..]);
        assert_eq!(
            refresh_ids(&db, "u1").await,
            expected,
            "recent refreshes are kept beyond the newest ten only when they judged something"
        );
        assert_eq!(dash.judged_since(T, "u1", at(0)).await.expect("count"), 120);
    }

    #[tokio::test]
    async fn unseen_judgements_are_deleted_after_the_retention() {
        let db = db().await;
        let dash = db.dashboard();
        let day = |n: i64| at(0) + SignedDuration::from_hours(24 * n);
        let seen = judgement(1, "a", day(0));
        dash.save_judgements(T, "u1", &[seen.clone(), judgement(2, "b", day(0))])
            .await
            .expect("u1");
        dash.save_judgements(T, "u2", &[judgement(1, "a", day(0))])
            .await
            .expect("u2");
        dash.mark_seen(T, "u1", &[key(1), key(9)], day(89))
            .await
            .expect("seen");
        dash.mark_seen("other", "u1", &[key(2)], day(89))
            .await
            .expect("another tenant marks nothing");
        dash.mark_seen(T, "u2", &[key(2)], day(89))
            .await
            .expect("another user marks nothing");
        let deleted = dash
            .delete_unseen_judgements(
                day(91) - SignedDuration::from_hours(24 * JUDGEMENT_RETENTION_DAYS),
            )
            .await
            .expect("delete");
        assert_eq!(deleted, 2);
        assert_eq!(
            dash.judgements(T, "u1", &[key(1), key(2)])
                .await
                .expect("kept"),
            [StoredJudgement {
                last_seen_at: day(89),
                ..seen
            }]
        );
        assert_eq!(dash.judgements(T, "u2", &[key(1)]).await.expect("u2"), []);
    }

    async fn plan(db: &Db, sql: &str, binds: usize) -> Vec<String> {
        let explain = format!("EXPLAIN QUERY PLAN {sql}");
        let mut query = sqlx::query_as::<_, (i64, i64, i64, String)>(sqlx::AssertSqlSafe(explain));
        for value in ["t", "u", "[]"].into_iter().take(binds) {
            query = query.bind(value);
        }
        let rows = query.fetch_all(&db.pool).await.expect("plan");
        rows.into_iter().map(|row| row.3).collect()
    }

    /// "Nothing changed → no job" costs one primary-key lookup per item; the daily cap and retention use indexes.
    #[tokio::test]
    async fn judgement_lookups_use_indexes() {
        let db = db().await;
        let lookup = plan(&db, JUDGEMENTS_SQL, 3).await;
        assert!(
            lookup.iter().any(|step| step.contains(
                "SEARCH dashboard_judgements USING INDEX sqlite_autoindex_dashboard_judgements_1 (user_id=? AND item_key=?)"
            )),
            "{lookup:?}"
        );
        assert!(
            !lookup
                .iter()
                .any(|step| step.starts_with("SCAN dashboard_judgements")),
            "{lookup:?}"
        );
        let since = plan(&db, JUDGED_SINCE_SQL, 3).await;
        assert!(
            since
                .iter()
                .any(|step| step.contains("dashboard_refreshes_by_user")),
            "{since:?}"
        );
        let unseen = plan(&db, DELETE_UNSEEN_SQL, 1).await;
        assert!(
            unseen
                .iter()
                .any(|step| step.contains("dashboard_judgements_by_last_seen")),
            "{unseen:?}"
        );
    }

    /// Migrations 0003/0004 on an empty database: the dashboard tables and indexes exist and every read answers
    /// "nothing yet".
    #[tokio::test]
    async fn dashboard_on_an_empty_database() {
        let db = Db::in_memory().await.expect("db");
        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'dashboard_%' ORDER BY name",
        )
        .fetch_all(&db.pool)
        .await
        .expect("indexes");
        assert_eq!(
            indexes,
            [
                "dashboard_judgements_by_judged_at",
                "dashboard_judgements_by_last_seen",
                "dashboard_refreshes_by_user",
                "dashboard_refreshes_one_live_per_user",
            ]
        );
        let attempt: (String, i64) = sqlx::query_as(
            "SELECT dflt_value, \"notnull\" FROM pragma_table_info('dashboard_refreshes') WHERE name = 'ranking_attempt'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("ranking_attempt column");
        assert_eq!(attempt, ("0".to_owned(), 1));
        let dash = db.dashboard();
        assert_eq!(dash.settings(T, "u1").await.expect("settings"), None);
        assert_eq!(dash.latest_refresh(T, "u1").await.expect("latest"), None);
        assert_eq!(
            dash.latest_snapshot::<Value>(T, "u1")
                .await
                .expect("snapshot"),
            None
        );
        assert!(
            dash.schedule_candidates()
                .await
                .expect("candidates")
                .is_empty()
        );
        assert_eq!(dash.interrupt_live(at(0)).await.expect("interrupt"), []);
        assert_eq!(dash.judgements(T, "u1", &[key(1)]).await.expect("none"), []);
        assert_eq!(dash.judged_since(T, "u1", at(0)).await.expect("count"), 0);
        assert_eq!(
            dash.delete_unseen_judgements(at(0)).await.expect("delete"),
            0
        );
    }

    /// A v1 database (0001 + 0002) with data upgrades in place: every v1 row is kept, and the dashboard and the
    /// ranking effort work on it.
    #[tokio::test]
    async fn v1_database_with_data_upgrades_in_place() {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await
            .expect("pool");
        super::super::migrate(&pool, 2).await.expect("v1 schema");
        let db = Db { pool };
        let analysis = seed_analysis(&db).await.expect("analysis");
        sqlx::raw_sql(
            "INSERT INTO memberships (tenant_id, user_id, created_at) VALUES ('default', 'u1', 't');
             INSERT INTO llm_selection (singleton, model, reasoning_effort, changed_by, changed_at)
               VALUES (1, 'gpt-6-astra', 'medium', 'lklimek', '2026-11-16T00:00:00.000000Z');",
        )
        .execute(&db.pool)
        .await
        .expect("membership, selection");
        for sql in [
            "INSERT INTO jobs (id, tenant_id, analysis_id, kind, priority, state, spec, credential_id, created_at, stage)
             VALUES ('j1', 'default', ?, 'analyse_flow', 1, 'succeeded', '{}', 'c1', 't', 'flow:f1')",
            "INSERT INTO flow_work (tenant_id, analysis_id, flow_id, requested_at, error, auto_retries)
             VALUES ('default', ?, 'f1', NULL, 'The flow analysis failed.', 2)",
            "INSERT INTO questions (id, tenant_id, analysis_id, step_id, user_id, text, state, created_at)
             VALUES ('q1', 'default', ?, 'f1.1', 'u1', 'why?', 'pending', 't')",
        ] {
            sqlx::query(sql)
                .bind(analysis.to_string())
                .execute(&db.pool)
                .await
                .expect("v1 row");
        }
        let snapshot = "SELECT
            (SELECT group_concat(id || login) FROM users) || '|' ||
            (SELECT group_concat(tenant_id || user_id) FROM memberships) || '|' ||
            (SELECT group_concat(owner || name) FROM repos) || '|' ||
            (SELECT group_concat(number || head_sha) FROM pull_requests) || '|' ||
            (SELECT group_concat(id || state || head_sha) FROM analyses) || '|' ||
            (SELECT group_concat(id || state || stage) FROM jobs) || '|' ||
            (SELECT group_concat(flow_id || error || auto_retries) FROM flow_work) || '|' ||
            (SELECT group_concat(id || text) FROM questions) || '|' ||
            (SELECT group_concat(model || reasoning_effort || changed_by) FROM llm_selection)";
        let before: String = sqlx::query_scalar(snapshot)
            .fetch_one(&db.pool)
            .await
            .expect("before");

        super::super::migrate(&db.pool, super::super::MIGRATIONS.len())
            .await
            .expect("upgrade in place");

        let after: String = sqlx::query_scalar(snapshot)
            .fetch_one(&db.pool)
            .await
            .expect("after");
        assert_eq!(after, before, "every v1 row is kept unchanged");
        let selection = db
            .settings()
            .llm_selection()
            .await
            .expect("selection")
            .expect("stored");
        assert_eq!(
            (
                selection.choice.model.as_str(),
                selection.judge_effort,
                selection.changed_by.as_str()
            ),
            ("gpt-6-astra", ReasoningEffort::Low, "lklimek")
        );
        let dash = db.dashboard();
        dash.save_settings(T, "u1", &DashboardSettings::default(), at(0))
            .await
            .expect("settings");
        let id = ranking(&db, "u1", 1).await;
        dash.save_judgements(
            T,
            "u1",
            &[StoredJudgement {
                refresh_id: Some(id),
                ..judgement(1, "a", at(2))
            }],
        )
        .await
        .expect("judgement");
        assert_eq!(current(&db, "u1").await, Some((id, 1)));
        let versions: Vec<i64> = sqlx::query_scalar(
            "SELECT version FROM _sqlx_migrations WHERE success ORDER BY version",
        )
        .fetch_all(&db.pool)
        .await
        .expect("versions");
        let all = i64::try_from(super::super::MIGRATIONS.len()).expect("count");
        assert_eq!(versions, (1..=all).collect::<Vec<_>>());
    }

    /// Migration 0005 upgrades a database with dashboard rows in place: refreshes, settings and judgements are kept,
    /// every refresh starts at ranking attempt 0, and the attempt cannot go below zero.
    #[tokio::test]
    async fn ranking_attempt_migration_keeps_dashboard_rows() {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await
            .expect("pool");
        super::super::migrate(&pool, 4).await.expect("0001-0004");
        let db = Db { pool };
        seed_users(&db).await;
        sqlx::raw_sql(
            "INSERT INTO dashboard_settings (user_id, tenant_id, time_zone, updated_at)
               VALUES ('u1', 'default', 'Europe/Warsaw', '2026-11-18T06:00:00.000000Z');
             INSERT INTO dashboard_refreshes (id, tenant_id, user_id, trigger, state, started_at, snapshot,
                 ranking_state, judged)
               VALUES ('00000000-0000-4000-8000-000000000001', 'default', 'u1', 'manual', 'finished',
                       '2026-11-18T06:00:00.000000Z', '{\"n\":1}', 'failed', 4),
                      ('00000000-0000-4000-8000-000000000002', 'default', 'u2', 'scheduled', 'ranking',
                       '2026-11-18T06:00:00.000000Z', '{\"n\":2}', 'paused', 1);
             INSERT INTO dashboard_judgements (tenant_id, user_id, item_key, input_fingerprint, input_version,
                 prompt_version, body, refresh_id, model, judged_at, last_seen_at)
               VALUES ('default', 'u1', 'dashpay~platform~pr~1', 'aa', 1, 1, '{}',
                       '00000000-0000-4000-8000-000000000001', 'gpt-6.1-sol', '2026-11-18T06:00:00.000000Z',
                       '2026-11-18T06:00:00.000000Z');",
        )
        .execute(&db.pool)
        .await
        .expect("dashboard rows");
        let rows = "SELECT
            (SELECT group_concat(user_id || time_zone) FROM dashboard_settings) || '|' ||
            (SELECT group_concat(id || user_id || state || ranking_state || judged || snapshot) FROM dashboard_refreshes)
            || '|' || (SELECT group_concat(item_key || refresh_id) FROM dashboard_judgements)";
        let before: String = sqlx::query_scalar(rows)
            .fetch_one(&db.pool)
            .await
            .expect("before");

        super::super::migrate(&db.pool, super::super::MIGRATIONS.len())
            .await
            .expect("upgrade in place");

        let after: String = sqlx::query_scalar(rows)
            .fetch_one(&db.pool)
            .await
            .expect("after");
        assert_eq!(after, before);
        let attempts: Vec<i64> =
            sqlx::query_scalar("SELECT ranking_attempt FROM dashboard_refreshes ORDER BY id")
                .fetch_all(&db.pool)
                .await
                .expect("attempts");
        assert_eq!(attempts, [0, 0]);
        let negative = sqlx::query("UPDATE dashboard_refreshes SET ranking_attempt = -1")
            .execute(&db.pool)
            .await;
        assert!(negative.is_err(), "the attempt cannot go below zero");
        let paused: RefreshId =
            parse_wire("00000000-0000-4000-8000-000000000002", "id").expect("id");
        let dash = db.dashboard();
        assert_eq!(
            dash.finish_ranking(T, "u2", paused, 0, RankingState::Finished, COUNTS, at(1))
                .await
                .expect("attempt 0"),
            RankingUpdate::Applied
        );
        let failed: RefreshId =
            parse_wire("00000000-0000-4000-8000-000000000001", "id").expect("id");
        let RestartOutcome::Restarted(restarted) = dash
            .restart_ranking(T, "u1", failed)
            .await
            .expect("restart")
        else {
            panic!("the failed ranking of the current snapshot is retryable");
        };
        assert_eq!(restarted.ranking_attempt, 1);
    }

    const CAPS: SendCaps = SendCaps {
        per_refresh: 10,
        per_day: 15,
    };

    #[tokio::test]
    async fn sends_are_reserved_within_both_caps_and_counted_across_attempts() {
        let db = db().await;
        let dash = db.dashboard();
        let id = ranking(&db, "u1", 1).await;
        let reserve = |items, attempt| dash.reserve_sends(T, "u1", id, attempt, items, CAPS, at(0));
        assert_eq!(
            reserve(8, 0).await.expect("reserve"),
            SendReservation::Reserved
        );
        assert_eq!(
            reserve(3, 0).await.expect("reserve"),
            SendReservation::OverRefreshCap,
            "8 + 3 > 10 sent items for this refresh"
        );
        assert_eq!(
            reserve(2, 0).await.expect("reserve"),
            SendReservation::Reserved
        );
        assert_eq!(
            dash.sent_totals(T, "u1", id, at(0)).await.expect("totals"),
            SentTotals {
                refresh: 10,
                day: 10
            }
        );
        // A Retry ranking keeps the refresh's count: the room does not come back.
        let applied = dash
            .finish_ranking(T, "u1", id, 0, RankingState::Failed, COUNTS, at(2))
            .await
            .expect("finish");
        assert_eq!(applied, RankingUpdate::Applied);
        assert!(matches!(
            dash.restart_ranking(T, "u1", id).await.expect("restart"),
            RestartOutcome::Restarted(_)
        ));
        assert_eq!(
            reserve(1, 0).await.expect("reserve"),
            SendReservation::Stale
        );
        assert_eq!(
            reserve(1, 1).await.expect("reserve"),
            SendReservation::OverRefreshCap
        );
        // A new refresh of the same day: its own room, the day's sum caps it.
        let applied = dash
            .finish_ranking(T, "u1", id, 1, RankingState::Finished, COUNTS, at(3))
            .await
            .expect("finish");
        assert_eq!(applied, RankingUpdate::Applied);
        let next = ranking(&db, "u1", 4).await;
        let reserve = |items| dash.reserve_sends(T, "u1", next, 0, items, CAPS, at(0));
        assert_eq!(
            reserve(6).await.expect("reserve"),
            SendReservation::OverDailyCap
        );
        assert_eq!(
            reserve(5).await.expect("reserve"),
            SendReservation::Reserved
        );
        assert_eq!(
            dash.sent_totals(T, "u1", next, at(0))
                .await
                .expect("totals"),
            SentTotals {
                refresh: 5,
                day: 15
            }
        );
        // Another user's sends are theirs.
        let other = ranking(&db, "u2", 5).await;
        assert_eq!(
            dash.reserve_sends(T, "u2", other, 0, 8, CAPS, at(0))
                .await
                .expect("reserve"),
            SendReservation::Reserved
        );
    }

    #[tokio::test]
    async fn sends_of_a_refresh_started_before_the_day_count_while_it_ranks_today() {
        let db = db().await;
        let dash = db.dashboard();
        let id = ranking(&db, "u1", 1).await;
        let day = at(60);
        assert_eq!(
            dash.reserve_sends(T, "u1", id, 0, 4, CAPS, day)
                .await
                .expect("reserve"),
            SendReservation::Reserved
        );
        assert_eq!(
            dash.sent_totals(T, "u1", id, day)
                .await
                .expect("totals")
                .day,
            4,
            "started before the day, still ranking"
        );
        let applied = dash
            .finish_ranking(T, "u1", id, 0, RankingState::Finished, COUNTS, at(30))
            .await
            .expect("finish");
        assert_eq!(applied, RankingUpdate::Applied);
        assert_eq!(
            dash.sent_totals(T, "u1", id, day)
                .await
                .expect("totals")
                .day,
            0
        );
    }

    #[tokio::test]
    async fn refreshes_that_sent_judge_items_recently_are_kept() {
        let db = db().await;
        let dash = db.dashboard();
        let first = ranking(&db, "u1", 0).await;
        let applied = dash
            .reserve_sends(T, "u1", first, 0, 3, CAPS, at(0))
            .await
            .expect("reserve");
        assert_eq!(applied, SendReservation::Reserved);
        let applied = dash
            .finish_ranking(
                T,
                "u1",
                first,
                0,
                RankingState::Failed,
                JudgeCounts::default(),
                at(1),
            )
            .await
            .expect("finish");
        assert_eq!(applied, RankingUpdate::Applied);
        for minute in 2..(2 + i64::from(REFRESHES_KEPT) + 2) {
            finished(&db, "u1", minute, RankingState::Finished).await;
        }
        assert_eq!(
            record(&db, "u1", first).await.id,
            first,
            "sent items count for the daily cap"
        );
    }

    #[tokio::test]
    async fn ranking_write_stores_judgements_only_for_the_current_attempt() {
        let db = db().await;
        let dash = db.dashboard();
        let id = ranking(&db, "u1", 1).await;
        let progress = RefreshProgress {
            steps: Vec::new(),
            percent: 100,
        };
        let log = vec!["ranking: 1 item".to_owned()];
        let write = RankingWrite {
            state: RankingState::Running,
            counts: COUNTS,
            judgements: &[judgement(1, "f1", at(2))],
            progress: Some((&progress, &log)),
        };
        assert_eq!(
            dash.write_ranking(T, "u1", id, 0, &write)
                .await
                .expect("write"),
            RankingUpdate::Applied
        );
        assert_eq!(
            dash.judgements(T, "u1", &[key(1)])
                .await
                .expect("read")
                .len(),
            1
        );
        assert_eq!(record(&db, "u1", id).await.log, log);
        assert_eq!(record(&db, "u1", id).await.counts, COUNTS);
        let applied = dash
            .cancel_ranking(T, "u1", id, 0, at(3))
            .await
            .expect("cancel");
        assert_eq!(applied, RankingUpdate::Applied);
        let late = RankingWrite {
            judgements: &[judgement(2, "f2", at(4))],
            progress: Some((&progress, &[])),
            ..write
        };
        assert_eq!(
            dash.write_ranking(T, "u1", id, 0, &late)
                .await
                .expect("write"),
            RankingUpdate::Stale
        );
        assert!(
            dash.judgements(T, "u1", &[key(2)])
                .await
                .expect("read")
                .is_empty()
        );
        assert_eq!(
            record(&db, "u1", id).await.log,
            log,
            "a stale note is dropped"
        );
        let ending = RankingWrite {
            state: RankingState::Finished,
            ..write
        };
        assert!(matches!(
            dash.write_ranking(T, "u1", id, 0, &ending).await,
            Err(DbError::Conflict)
        ));
    }

    /// A settings row stored before migration 0009, or created with column defaults, reads the default known bots.
    #[tokio::test]
    async fn rows_without_known_bots_read_the_default_list() {
        let db = db().await;
        let dash = db.dashboard();
        assert!(
            dash.set_time_zone_if_unset(T, "u1", "Europe/Warsaw", at(0))
                .await
                .expect("zone")
        );
        let stored = dash.settings(T, "u1").await.expect("read").expect("row");
        assert_eq!(
            stored.settings.known_bots,
            ["thepastaclaw", "Claudius-Maginificent", "DCG-Claude"]
        );
    }
}
