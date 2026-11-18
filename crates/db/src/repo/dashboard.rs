//! Repository for the dashboard (`dashboard_settings`, `dashboard_refreshes`, `dashboard_judgements`; plan
//! `docs/v1.1-dashboard-plan.md` §D). Signatures frozen by WP11-A; bodies are filled in by WP11-DB. Every
//! function is scoped by `(tenant_id, user_id)`, except the server-internal ones that say so; there is no
//! list-all API for serving. Refresh transitions follow `docs/contracts/http-api.md` "Refresh states".

use chrono::{DateTime, NaiveDate, Utc};
use contracts::api::dashboard::{
    DashboardSettings, ItemKey, RankingState, RefreshFailure, RefreshState, RefreshTrigger,
    ScanStep, ScheduledOutcome,
};
use contracts::ids::{JobId, RefreshId};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;

use super::Db;
use crate::error::DbError;

/// Refreshes kept per user: the newest, plus the one holding the latest snapshot.
pub const REFRESHES_KEPT: u32 = 10;
/// Log lines kept per refresh (the newest).
pub const LOG_LINES_KEPT: usize = 200;
/// Days a judgement is kept after its item was last seen.
pub const JUDGEMENT_RETENTION_DAYS: i64 = 90;

/// Access to the dashboard tables.
#[derive(Debug)]
pub struct Dashboard<'a> {
    #[expect(dead_code, reason = "used once WP11-DB implements the operations")]
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
    pub updated_at: DateTime<Utc>,
}

/// A scheduled run as recorded per user (one per local date).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledRunRecord {
    /// The user's local date of the run.
    pub on: NaiveDate,
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
    /// Judgement counts.
    pub counts: JudgeCounts,
    /// A snapshot is stored.
    pub has_snapshot: bool,
    /// Start time.
    pub started_at: DateTime<Utc>,
    /// When the snapshot was stored.
    pub scanned_at: Option<DateTime<Utc>>,
    /// When it ended (finished, failed or cancelled).
    pub finished_at: Option<DateTime<Utc>>,
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
    pub judged_at: DateTime<Utc>,
    /// Last refresh that saw the item.
    pub last_seen_at: DateTime<Utc>,
}

#[expect(
    unused_variables,
    reason = "signatures frozen by WP11-A; bodies are filled in by WP11-DB"
)]
impl Dashboard<'_> {
    /// The user's settings row; none means the defaults (`DashboardSettings::default`).
    pub async fn settings(
        &self,
        tenant_id: &str,
        user_id: &str,
    ) -> Result<Option<StoredDashboardSettings>, DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::settings",
        })
    }

    /// Saves validated settings (upsert); keeps the recorded scheduled run.
    pub async fn save_settings(
        &self,
        tenant_id: &str,
        user_id: &str,
        settings: &DashboardSettings,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::save_settings",
        })
    }

    /// Stores the time zone, creating a defaults row when there is none.
    pub async fn set_time_zone(
        &self,
        tenant_id: &str,
        user_id: &str,
        time_zone: &str,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::set_time_zone",
        })
    }

    /// Records the scheduled run of a local date, creating a defaults row when there is none.
    pub async fn record_scheduled_run(
        &self,
        tenant_id: &str,
        user_id: &str,
        run: &ScheduledRunRecord,
    ) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::record_scheduled_run",
        })
    }

    /// Server-internal (scheduler): every user, in every tenant, whose schedule is on and time zone known.
    pub async fn schedule_candidates(&self) -> Result<Vec<ScheduleCandidate>, DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::schedule_candidates",
        })
    }

    /// R2 and the schedule, in one write transaction: a refresh of the user in `ranking` is superseded
    /// ([`Dashboard::finish_ranking`] with `cancelled`), then a new one starts in `scanning`; a `scanning` one
    /// blocks ([`StartOutcome::ScanRunning`]). Deletes the user's older refreshes beyond [`REFRESHES_KEPT`], never
    /// the one holding the current snapshot.
    pub async fn start_refresh(
        &self,
        tenant_id: &str,
        user_id: &str,
        trigger: RefreshTrigger,
        now: DateTime<Utc>,
    ) -> Result<StartOutcome, DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::start_refresh",
        })
    }

    /// One of the user's refreshes; `NotFound` for an unknown id or another user's refresh.
    pub async fn refresh(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
    ) -> Result<RefreshRecord, DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::refresh",
        })
    }

    /// The user's newest refresh, live or ended.
    pub async fn latest_refresh(
        &self,
        tenant_id: &str,
        user_id: &str,
    ) -> Result<Option<RefreshRecord>, DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::latest_refresh",
        })
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
        Err(DbError::NotImplemented {
            operation: "dashboard::record_progress",
        })
    }

    /// Stores the snapshot of a `scanning` refresh and moves it to `ranking` (ranking state `running` until
    /// [`Dashboard::set_ranking`]); from now on it is the current snapshot. `Conflict` unless it is scanning.
    pub async fn store_snapshot<T: Serialize + Sync>(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
        snapshot: &T,
        scanned_at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::store_snapshot",
        })
    }

    /// The current snapshot: that of the user's newest refresh in `ranking` or `finished` (a `failed` or
    /// `cancelled` refresh never holds one), with that refresh.
    pub async fn latest_snapshot<T: DeserializeOwned>(
        &self,
        tenant_id: &str,
        user_id: &str,
    ) -> Result<Option<(RefreshRecord, T)>, DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::latest_snapshot",
        })
    }

    /// Records the live ranking state (`waiting_for_analysis`, `running`, `paused`) and the judgement counts of a
    /// `ranking` refresh; `Conflict` for any other refresh or an ending ranking state (use
    /// [`Dashboard::finish_ranking`]).
    pub async fn set_ranking(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
        state: RankingState,
        counts: JudgeCounts,
    ) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::set_ranking",
        })
    }

    /// Ends a `scanning` refresh before its snapshot is complete; `Conflict` for any other state.
    pub async fn end_scan(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
        end: &ScanEnd,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::end_scan",
        })
    }

    /// Ends the ranking of a `ranking` refresh with `outcome` (`finished`, `failed`, `cancelled` or
    /// `interrupted`) and the final counts; the refresh becomes `finished` and its snapshot stays current.
    /// `Conflict` for any other refresh or a live ranking state.
    pub async fn finish_ranking(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
        outcome: RankingState,
        counts: JudgeCounts,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::finish_ranking",
        })
    }

    /// Retry ranking (R4), in one write transaction: a `finished` refresh holding the current snapshot whose
    /// ranking ended `failed`, `interrupted` or `cancelled` goes back to `ranking` (ranking state `running`, counts
    /// kept). `NotFound` for an unknown id or another user's refresh.
    pub async fn restart_ranking(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: RefreshId,
    ) -> Result<RestartOutcome, DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::restart_ranking",
        })
    }

    /// Server-internal (startup), every tenant: `scanning` refreshes become `failed` (code `interrupted`, no
    /// snapshot); `ranking` ones become `finished` with ranking `interrupted`. Afterwards no refresh is live.
    pub async fn interrupt_live(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<InterruptedRefresh>, DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::interrupt_live",
        })
    }

    /// The user's stored judgements of these items (missing ones are absent).
    pub async fn judgements(
        &self,
        tenant_id: &str,
        user_id: &str,
        item_keys: &[ItemKey],
    ) -> Result<Vec<StoredJudgement>, DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::judgements",
        })
    }

    /// Stores judgements, replacing the user's previous judgement of each item.
    pub async fn save_judgements(
        &self,
        tenant_id: &str,
        user_id: &str,
        judgements: &[StoredJudgement],
    ) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::save_judgements",
        })
    }

    /// Marks the user's judgements of these items as seen at `now` (retention).
    pub async fn mark_seen(
        &self,
        tenant_id: &str,
        user_id: &str,
        item_keys: &[ItemKey],
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::mark_seen",
        })
    }

    /// Judgements of the user made at or after `since` (the daily cap counts from the local day's start).
    pub async fn judged_since(
        &self,
        tenant_id: &str,
        user_id: &str,
        since: DateTime<Utc>,
    ) -> Result<u32, DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::judged_since",
        })
    }

    /// Server-internal (retention): deletes judgements of every tenant not seen since `before`.
    pub async fn delete_unseen_judgements(&self, before: DateTime<Utc>) -> Result<u64, DbError> {
        Err(DbError::NotImplemented {
            operation: "dashboard::delete_unseen_judgements",
        })
    }
}
