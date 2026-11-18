//! Refresh orchestration (plan §A.4, `docs/contracts/http-api.md` "Refresh states"): start (superseding a
//! ranking), the refresh task (scan → snapshot → ranking), cancel, Retry ranking and startup recovery. Every state
//! change is written first, then published as a `refresh` event. A failed or cancelled scan never replaces the
//! previous snapshot (the database keeps it current).

mod facts;
mod history;
mod owned;
mod progress;
pub mod rank;
mod scan;

use std::future::Future;
use std::panic::AssertUnwindSafe;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use contracts::api::dashboard::{
    RankingState, RankingStatus, RefreshFailure, RefreshFailureCode, RefreshState, RefreshStatus,
    RefreshTrigger, ScanStepKey, StepState,
};
use contracts::api::settings::MemcanState;
use contracts::ids::RefreshId;
use db::error::DbError;
use db::repo::dashboard::{RefreshRecord, RestartOutcome, ScanEnd, StartOutcome};
use futures::FutureExt;

use self::progress::Progress;
use self::rank::RankRun;
use self::scan::ScanInput;
use super::score::ScoreRules;
use super::{settings, snapshot};
use crate::auth::UserTokenError;
use crate::github::work::{UserTokens, WorkClient, WorkError};
use crate::state::AppState;

/// Message of an `internal` failure.
const INTERNAL_MESSAGE: &str = "The refresh failed on the server. Try again later.";
/// Writes of a failed refresh's end before giving up to startup recovery.
const END_ATTEMPTS: u32 = 3;
/// Pause between them.
const END_RETRY: std::time::Duration = std::time::Duration::from_secs(2);

/// The user a refresh belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    /// Tenant.
    pub tenant_id: String,
    /// User.
    pub user_id: String,
}

/// A refresh could not be started.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// Another refresh of the user is scanning (R2: 409 `refresh_running`; the schedule counts it as done).
    #[error("a refresh of the user is already scanning")]
    Running,
    /// The latest refresh hit GitHub's rate limit, which resets at `retry_at` (R2: 429 `github_rate_limited`).
    #[error("the user's GitHub rate limit is exhausted")]
    RateLimited {
        /// When the limit resets.
        retry_at: DateTime<Utc>,
    },
    /// The database failed.
    #[error("the refresh could not be stored")]
    Db(#[from] DbError),
}

/// Cancel or Retry ranking was refused.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    /// The refresh ended (cancel), or its ranking cannot be retried (R3/R4: 409 `conflict`).
    #[error("the refresh is not in a state that allows this")]
    NotAllowed,
    /// Another refresh of the user is live (R4: 409 `refresh_running`).
    #[error("another refresh of the user is live")]
    Running,
    /// Unknown or another user's refresh (`NotFound`), or the database failed.
    #[error("the refresh could not be read or changed")]
    Db(#[from] DbError),
}

/// Why the refresh task stopped before storing its snapshot.
#[derive(Debug, thiserror::Error)]
enum RunError {
    #[error(transparent)]
    Github(#[from] WorkError),
    /// No candidate search was answered: GitHub search is down, the scan read nothing.
    #[error("GitHub answered none of the scan's searches")]
    SearchUnavailable,
    #[error("the user's dashboard data could not be read")]
    Db(#[from] DbError),
    #[error("the scan took longer than its limit")]
    TimedOut,
    /// The refresh ended (cancelled) meanwhile; not a failure.
    #[error(transparent)]
    Ended(#[from] progress::Ended),
}

/// Starts a refresh of the user (R2, the schedule): supersedes a ranking one, refuses while one scans or while
/// the last refresh's GitHub rate limit has not reset.
pub async fn start(
    state: &AppState,
    scope: &Scope,
    trigger: RefreshTrigger,
) -> Result<RefreshStatus, StartError> {
    let now = state.dashboard.now();
    let dashboard = state.db.dashboard();
    let limited = dashboard
        .latest_refresh(&scope.tenant_id, &scope.user_id)
        .await?
        .and_then(|latest| latest.failure)
        .filter(|failure| failure.code == RefreshFailureCode::GithubRateLimited)
        .and_then(|failure| failure.retry_at)
        .filter(|retry_at| *retry_at > now);
    if let Some(retry_at) = limited {
        return Err(StartError::RateLimited { retry_at });
    }
    let (refresh, superseded) = match dashboard
        .start_refresh(&scope.tenant_id, &scope.user_id, trigger, now)
        .await?
    {
        StartOutcome::ScanRunning => return Err(StartError::Running),
        StartOutcome::Started {
            refresh,
            superseded,
        } => (refresh, superseded),
    };
    if let Some(old) = superseded {
        state.dashboard.abort(old);
        tracing::info!(user_id = %scope.user_id, refresh = %old, "dashboard ranking superseded by a new refresh");
        publish(state, scope, old).await;
    }
    tracing::info!(user_id = %scope.user_id, refresh = %refresh.id, ?trigger, "dashboard refresh started");
    let status = status_of(&refresh);
    // Before the task runs, so this first status never follows the task's own events.
    state.dashboard.publish(status.clone());
    let id = refresh.id;
    let task = run(state.clone(), scope.clone(), id, trigger);
    state
        .dashboard
        .spawn(id, guarded(state.clone(), scope.clone(), id, task));
    Ok(status)
}

/// Cancels a refresh (R3): a scan ends `cancelled`, a ranking `cancelled` (the refresh `finished`, its snapshot
/// current); the task stops at its next await.
pub async fn cancel(
    state: &AppState,
    scope: &Scope,
    id: RefreshId,
) -> Result<RefreshStatus, ControlError> {
    let dashboard = state.db.dashboard();
    // A scan that stores its snapshot meanwhile is cancelled as a ranking on the second pass.
    for _ in 0..2 {
        let record = dashboard
            .refresh(&scope.tenant_id, &scope.user_id, id)
            .await?;
        let now = state.dashboard.now();
        let ended = match record.state {
            RefreshState::Scanning => dashboard
                .end_scan(
                    &scope.tenant_id,
                    &scope.user_id,
                    id,
                    &ScanEnd::Cancelled,
                    now,
                )
                .await
                .map(|()| true),
            RefreshState::Ranking => dashboard
                .cancel_ranking(
                    &scope.tenant_id,
                    &scope.user_id,
                    id,
                    record.ranking_attempt,
                    now,
                )
                .await
                .map(|update| update == db::repo::dashboard::RankingUpdate::Applied),
            RefreshState::Finished | RefreshState::Failed | RefreshState::Cancelled => {
                return Err(ControlError::NotAllowed);
            }
        };
        match ended {
            Ok(true) => {
                state.dashboard.abort(id);
                tracing::info!(user_id = %scope.user_id, refresh = %id, was = ?record.state, "dashboard refresh cancelled");
                let record = dashboard
                    .refresh(&scope.tenant_id, &scope.user_id, id)
                    .await?;
                let status = status_of(&record);
                state.dashboard.publish(status.clone());
                return Ok(status);
            }
            Ok(false) | Err(DbError::Conflict) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(ControlError::NotAllowed)
}

/// Retry ranking (R4) of a finished refresh holding the current snapshot whose ranking failed, was interrupted
/// or cancelled: a new ranking attempt in the refresh's task.
pub async fn rank_again(
    state: &AppState,
    scope: &Scope,
    id: RefreshId,
) -> Result<RefreshStatus, ControlError> {
    let record = match state
        .db
        .dashboard()
        .restart_ranking(&scope.tenant_id, &scope.user_id, id)
        .await?
    {
        RestartOutcome::Restarted(record) => record,
        RestartOutcome::RefreshRunning => return Err(ControlError::Running),
        RestartOutcome::NotRetryable => return Err(ControlError::NotAllowed),
    };
    tracing::info!(user_id = %scope.user_id, refresh = %id, attempt = record.ranking_attempt, "dashboard ranking restarted");
    let run = RankRun {
        scope: scope.clone(),
        refresh_id: id,
        attempt: record.ranking_attempt,
    };
    let status = status_of(&record);
    state.dashboard.publish(status.clone());
    let task = state.dashboard.ranker().rank(state.clone(), run);
    state
        .dashboard
        .spawn(id, guarded(state.clone(), scope.clone(), id, task));
    Ok(status)
}

/// Startup recovery (plan §A.4): no task survived the restart, so every live refresh ends (`scanning` → `failed`
/// interrupted, `ranking` → ranking `interrupted`, Retry ranking offered).
pub async fn recover(state: &AppState) -> Result<(), DbError> {
    let interrupted = state
        .db
        .dashboard()
        .interrupt_live(state.dashboard.now())
        .await?;
    for refresh in &interrupted {
        tracing::info!(user_id = %refresh.user_id, refresh = %refresh.id, was = ?refresh.was,
            "dashboard refresh interrupted by the server restart");
        super::judge::cancel_jobs(state, refresh.id, None).await;
    }
    Ok(())
}

/// Reads a refresh's stored status and publishes it; a failed read is logged (the stream's next event or a
/// reconnect carries it).
pub async fn publish(state: &AppState, scope: &Scope, id: RefreshId) {
    match state
        .db
        .dashboard()
        .refresh(&scope.tenant_id, &scope.user_id, id)
        .await
    {
        Ok(record) => state
            .dashboard
            .publish(super::judge::status(state, &record).await),
        Err(error) => {
            tracing::debug!(%error, refresh = %id, "dashboard refresh status not published")
        }
    }
}

/// The wire status of a stored refresh. The ranking step follows the ranking state; the ranking's total and
/// failure text are the ranking step's (`judge`); `resumes_at` needs the jobs (`judge::status`).
pub fn status_of(record: &RefreshRecord) -> RefreshStatus {
    let mut steps = record.progress.steps.clone();
    let ranking_step = steps.iter().find(|s| s.key == ScanStepKey::Ranking);
    let ranking_total = ranking_step.and_then(|s| s.total);
    let ranking_detail = ranking_step.and_then(|s| s.detail.clone());
    if let Some(ranking) = record.ranking_state
        && let Some(step) = steps.iter_mut().find(|s| s.key == ScanStepKey::Ranking)
    {
        step.state = match ranking {
            RankingState::WaitingForAnalysis | RankingState::Running | RankingState::Paused => {
                StepState::Running
            }
            RankingState::Finished => StepState::Done,
            RankingState::Failed | RankingState::Interrupted => StepState::Failed,
            RankingState::Cancelled => StepState::Skipped,
        };
        step.done = Some(record.counts.judged);
    }
    RefreshStatus {
        id: record.id,
        trigger: record.trigger,
        state: record.state,
        judged: record.counts.judged,
        reused: record.counts.reused,
        left_provisional: record.counts.left_provisional,
        steps,
        percent: if record.has_snapshot {
            100
        } else {
            record.progress.percent
        },
        started_at: record.started_at,
        scanned_at: record.scanned_at,
        ranking: record.ranking_state.map(|state| RankingStatus {
            state,
            judged: record.counts.judged,
            total: ranking_total.map_or(record.counts.judged, |total| {
                total.max(record.counts.judged)
            }),
            resumes_at: None,
            message: match state {
                RankingState::Interrupted => Some(
                    "A server restart interrupted the ranking. Retry ranking to finish it."
                        .to_owned(),
                ),
                RankingState::Failed => {
                    Some(ranking_detail.unwrap_or_else(|| "The LLM ranking failed.".to_owned()))
                }
                _ => None,
            },
        }),
        failure: record.failure.clone(),
        log: record.log.clone(),
    }
}

/// Runs a refresh's task; a panic is logged and ends the refresh `failed` (`internal`) or its ranking `failed`,
/// so the user is never left with a refresh that runs only in the database.
async fn guarded(
    state: AppState,
    scope: Scope,
    id: RefreshId,
    task: impl Future<Output = ()> + Send,
) {
    if AssertUnwindSafe(task).catch_unwind().await.is_ok() {
        return;
    }
    tracing::error!(user_id = %scope.user_id, refresh = %id,
        "dashboard refresh task panicked; the refresh is ended as failed");
    let dashboard = state.db.dashboard();
    let now = state.dashboard.now();
    let ended = match dashboard
        .refresh(&scope.tenant_id, &scope.user_id, id)
        .await
    {
        Ok(record) if record.state == RefreshState::Scanning => {
            end_failed_scan(&state, &scope, id, internal_failure()).await;
            return;
        }
        Ok(record) if record.state == RefreshState::Ranking => dashboard
            .finish_ranking(
                &scope.tenant_id,
                &scope.user_id,
                id,
                record.ranking_attempt,
                RankingState::Failed,
                record.counts,
                now,
            )
            .await
            .map(drop),
        Ok(_) => Ok(()),
        Err(error) => Err(error),
    };
    match ended {
        Ok(()) | Err(DbError::Conflict) => publish(&state, &scope, id).await,
        Err(error) => tracing::error!(%error, refresh = %id,
            "a panicked dashboard ranking could not be ended; startup recovery ends it"),
    }
}

fn internal_failure() -> RefreshFailure {
    RefreshFailure {
        code: RefreshFailureCode::Internal,
        message: INTERNAL_MESSAGE.to_owned(),
        retry_at: None,
    }
}

/// Ends a scanning refresh `failed`, retrying a failed write (a refresh left `scanning` blocks every new one);
/// a refresh that already ended is left as it is.
async fn end_failed_scan(state: &AppState, scope: &Scope, id: RefreshId, failure: RefreshFailure) {
    for attempt in 1..=END_ATTEMPTS {
        let ended = state
            .db
            .dashboard()
            .end_scan(
                &scope.tenant_id,
                &scope.user_id,
                id,
                &ScanEnd::Failed(failure.clone()),
                state.dashboard.now(),
            )
            .await;
        match ended {
            Ok(()) | Err(DbError::Conflict) => {
                publish(state, scope, id).await;
                return;
            }
            Err(error) if attempt < END_ATTEMPTS => {
                tracing::warn!(%error, refresh = %id, attempt, "a failed dashboard refresh could not be stored; retrying");
                tokio::time::sleep(END_RETRY).await;
            }
            Err(error) => tracing::error!(%error, refresh = %id,
                "a failed dashboard refresh could not be stored; startup recovery ends it"),
        }
    }
}

/// The refresh task: scan and store the snapshot, then rank it. A failed scan ends the refresh `failed`.
async fn run(state: AppState, scope: Scope, id: RefreshId, trigger: RefreshTrigger) {
    match scan_and_store(&state, &scope, id, trigger).await {
        Ok(true) => {
            let run = RankRun {
                scope: scope.clone(),
                refresh_id: id,
                attempt: 0,
            };
            state.dashboard.ranker().rank(state.clone(), run).await;
        }
        Ok(false) => {
            tracing::debug!(refresh = %id, "dashboard scan ended after its refresh was cancelled");
        }
        Err(failure) => {
            tracing::info!(user_id = %scope.user_id, refresh = %id, code = ?failure.code,
                "dashboard refresh failed; the previous snapshot stays");
            end_failed_scan(&state, &scope, id, failure).await;
        }
    }
}

/// Scans and stores the snapshot; `Ok(false)` when the refresh stopped scanning meanwhile (cancelled).
async fn scan_and_store(
    state: &AppState,
    scope: &Scope,
    id: RefreshId,
    trigger: RefreshTrigger,
) -> Result<bool, RefreshFailure> {
    let settings = settings::effective(&state.db, &scope.tenant_id, &scope.user_id)
        .await
        .map_err(|error| failure_of(&RunError::Db(error), state, None))?;
    let zone: Tz = settings
        .time_zone
        .as_deref()
        .and_then(|zone| zone.parse().ok())
        .unwrap_or(Tz::UTC);
    let db_failure = |error| failure_of(&RunError::Db(error), state, Some(zone));
    let own_login = state
        .db
        .users()
        .get(&scope.user_id)
        .await
        .map_err(db_failure)?
        .profile
        .login;
    let previous = snapshot::load(state, &scope.tenant_id, &scope.user_id)
        .await
        .map_err(db_failure)?
        .map(|(_, snapshot)| snapshot);
    let rules = ScoreRules::new(&own_login, &settings);
    let memcan = state.config.memcan.status().state != MemcanState::NotConfigured;
    let mut progress = Progress::new(state.clone(), scope.clone(), id, zone);
    let trigger_text = match trigger {
        RefreshTrigger::Manual => "manual",
        RefreshTrigger::Scheduled => "scheduled",
    };
    progress.log(format!("refresh started ({trigger_text})"));
    let tokens = UserTokens {
        state: state.clone(),
        user_id: scope.user_id.clone(),
    };
    let mut client = WorkClient::new(state.config.github.api_url.clone(), tokens)
        .map_err(|error| failure_of(&RunError::Github(error), state, Some(zone)))?;
    client.search_interval = state.dashboard.search_interval();
    let input = ScanInput {
        rules: &rules,
        own_login: &own_login,
        settings: &settings,
        zone,
        previous: previous.as_ref(),
        memcan,
    };
    let scanned = tokio::time::timeout(
        state.dashboard.scan_limit(),
        scan::scan(&client, &input, &mut progress),
    )
    .await;
    let snapshot = match scanned {
        Ok(Ok(snapshot)) => snapshot,
        Ok(Err(RunError::Ended(_))) => return Ok(false),
        Ok(Err(error)) => return Err(failed(&mut progress, error, state, zone).await),
        Err(_) => return Err(failed(&mut progress, RunError::TimedOut, state, zone).await),
    };
    progress.log("scan complete");
    if progress.record().await.is_err() {
        return Ok(false);
    }
    let stored = state
        .db
        .dashboard()
        .store_snapshot(
            &scope.tenant_id,
            &scope.user_id,
            id,
            &snapshot,
            snapshot.scanned_at,
        )
        .await;
    match stored {
        Ok(()) => {
            tracing::info!(user_id = %scope.user_id, refresh = %id, prs = snapshot.pull_requests.len(),
                issues = snapshot.issues.len(), todos = snapshot.todos.len(), "dashboard snapshot stored");
            publish(state, scope, id).await;
            Ok(true)
        }
        Err(DbError::Conflict) => Ok(false),
        Err(error) => Err(failure_of(&RunError::Db(error), state, Some(zone))),
    }
}

async fn failed(
    progress: &mut Progress,
    error: RunError,
    state: &AppState,
    zone: Tz,
) -> RefreshFailure {
    let failure = failure_of(&error, state, Some(zone));
    tracing::debug!(error = ?error, "dashboard scan failed");
    progress.log(format!("failed: {}", failure.message));
    progress.fail_running(&failure.message).await;
    failure
}

/// The documented failure of a refresh: code, plain message (no internal error text) and the retry time.
fn failure_of(error: &RunError, state: &AppState, zone: Option<Tz>) -> RefreshFailure {
    let now = state.dashboard.now();
    let (code, retry_at) = match error {
        RunError::Github(
            WorkError::Token(UserTokenError::SignInRequired) | WorkError::Unauthorized,
        ) => (RefreshFailureCode::SignInRequired, None),
        RunError::Github(WorkError::Token(UserTokenError::Github(_))) => {
            (RefreshFailureCode::GithubUnavailable, None)
        }
        RunError::Github(WorkError::Token(_) | WorkError::Interrupted | WorkError::Client(_))
        | RunError::Db(_)
        | RunError::Ended(_) => (RefreshFailureCode::Internal, None),
        RunError::Github(WorkError::PrimaryRateLimit { reset }) => {
            (RefreshFailureCode::GithubRateLimited, *reset)
        }
        RunError::Github(WorkError::SecondaryRateLimit { retry_after }) => (
            RefreshFailureCode::GithubRateLimited,
            retry_after
                .and_then(|wait| chrono::TimeDelta::from_std(wait).ok())
                .map(|wait| now + wait),
        ),
        RunError::Github(_) | RunError::SearchUnavailable => {
            (RefreshFailureCode::GithubUnavailable, None)
        }
        RunError::TimedOut => (RefreshFailureCode::TimedOut, None),
    };
    let message = match code {
        RefreshFailureCode::SignInRequired => {
            "Your GitHub sign-in has ended. Sign in again, then refresh.".to_owned()
        }
        RefreshFailureCode::GithubRateLimited => match retry_at {
            Some(at) => {
                let local = at.with_timezone(&zone.unwrap_or(Tz::UTC));
                format!(
                    "GitHub's rate limit is used up. Retry at {}.",
                    local.format("%H:%M")
                )
            }
            None => "GitHub's rate limit is used up. Try again later.".to_owned(),
        },
        RefreshFailureCode::GithubUnavailable => {
            "GitHub could not be read. Try again later.".to_owned()
        }
        RefreshFailureCode::TimedOut => {
            "The refresh took longer than 5 minutes and was stopped.".to_owned()
        }
        RefreshFailureCode::Interrupted => "A server restart interrupted the scan.".to_owned(),
        RefreshFailureCode::Internal => INTERNAL_MESSAGE.to_owned(),
    };
    RefreshFailure {
        code,
        message,
        retry_at,
    }
}

#[cfg(test)]
mod tests;
