//! The single MemCan writer: drains the feedback outbox (`FeedbackRepo::pending_memcan`) into MemCan. A live
//! entry is stored verbatim under its feedback id (`_import_records`, MemCan's synchronous embed-and-upsert, so a
//! retry or a newer revision replaces the same memory); a removed entry's memory is deleted. An operation counts
//! only when MemCan's answer is exactly the expected one.
//!
//! A call without a usable answer (timeout, broken connection, unexpected answer) has an unknown outcome: MemCan
//! may still carry it out. For [`SETTLE_PERIOD`] after it, a confirmed write or delete of that entry is not final;
//! the entry stays pending and the operation is repeated once the period is over, so a late write can neither
//! revive a removed memory nor overwrite a newer revision for good.
//!
//! Retries are scheduled per entry (back-off from [`FIRST_RETRY`] up to [`MAX_RETRY`]); a rejected entry never
//! blocks others. When MemCan is unreachable the sweep stops and the worker backs off; feedback stays stored
//! locally either way.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use db::error::DbError;
use db::repo::Db;
use db::repo::feedback::{MemcanOutcome, PendingFeedback};
use rmcp::model::ErrorCode;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Notify;

use super::client::{CALL_TIMEOUT, MemcanClient, MemcanError};
use super::memory::import_record;
use super::stored_flows;
use crate::config::{Config, MemcanConfig};

/// Entries written per sweep.
const BATCH: u32 = 20;
/// Sweep interval while MemCan is reachable; a nudge starts a sweep earlier.
pub(crate) const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
/// First worker wait after MemCan was unreachable; doubled per such sweep up to [`MAX_BACKOFF`].
pub(crate) const FIRST_BACKOFF: Duration = Duration::from_secs(30);
/// Longest worker wait.
pub(crate) const MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);
/// First retry of a failed entry; doubled per failed attempt up to [`MAX_RETRY`].
pub(crate) const FIRST_RETRY: TimeDelta = TimeDelta::seconds(30);
/// Longest wait between attempts of one entry.
pub(crate) const MAX_RETRY: TimeDelta = TimeDelta::minutes(30);
/// How long a MemCan operation without a usable answer may still take effect: ten call timeouts (200 s).
pub(crate) const SETTLE_PERIOD: TimeDelta = TimeDelta::seconds(10 * CALL_TIMEOUT.as_secs() as i64);

/// Wakes the writer; a nudge never waits for a sweep.
static NUDGE: Notify = Notify::const_new();

/// Asks the writer to sweep now (after feedback was recorded or removed); ignored while it backs off.
pub fn nudge() {
    NUDGE.notify_waiters();
}

/// Outcome of one sweep.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sweep {
    /// Entries MemCan confirmed.
    pub(crate) written: u32,
    /// Entries that failed or whose outcome is unknown.
    pub(crate) failed: u32,
    /// MemCan was unreachable or timed out; the sweep stopped.
    pub(crate) unavailable: bool,
}

/// Works through up to [`BATCH`] entries due at `now`; stops when MemCan is unreachable.
pub(crate) async fn sweep(
    db: &Db,
    config: &MemcanConfig,
    now: DateTime<Utc>,
) -> Result<Sweep, DbError> {
    let pending = db.feedback().pending_memcan(now, BATCH).await?;
    if pending.is_empty() {
        return Ok(Sweep::default());
    }
    let client = match MemcanClient::connect(config).await {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(
                ?error,
                queued = pending.len(),
                "MemCan is not reachable; feedback stays stored locally and queued for the next attempt"
            );
            return Ok(Sweep {
                unavailable: true,
                ..Sweep::default()
            });
        }
    };
    let mut outcome = Sweep::default();
    let mut last_error = None;
    for entry in &pending {
        let result = write(db, &client, entry, now).await?;
        let recorded = match &result {
            Ok(()) => MemcanOutcome::Confirmed,
            Err(MemcanError::Rejected { .. }) => MemcanOutcome::Failed {
                retry_at: now + retry_delay(entry.attempts),
            },
            Err(_) => MemcanOutcome::Unknown {
                retry_at: now + retry_delay(entry.attempts),
                settles_at: now + SETTLE_PERIOD,
            },
        };
        db.feedback()
            .record_memcan_outcome(entry.feedback.id, entry.revision, recorded, now)
            .await?;
        match result {
            Ok(()) => outcome.written += 1,
            Err(error) => {
                outcome.failed += 1;
                outcome.unavailable = error.is_unavailable();
                last_error = Some(error);
                if outcome.unavailable {
                    break;
                }
            }
        }
    }
    if let Some(error) = last_error {
        tracing::warn!(
            ?error,
            written = outcome.written,
            failed = outcome.failed,
            "MemCan writes of feedback failed; each entry is retried on its own schedule"
        );
    } else {
        tracing::trace!(written = outcome.written, "feedback written to MemCan");
    }
    Ok(outcome)
}

/// Delay before retrying an entry that had failed `attempts` times before this failure.
pub(crate) fn retry_delay(attempts: u32) -> TimeDelta {
    FIRST_RETRY
        .checked_mul(1 << attempts.min(16))
        .map_or(MAX_RETRY, |delay| delay.min(MAX_RETRY))
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ImportAnswer {
    imported: u64,
    skipped: u64,
    errors: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteAnswer {
    status: String,
    memory_id: String,
}

/// Performs the entry's operation: `Ok(Err(_))` is a MemCan failure, `Err(_)` a local database failure.
async fn write(
    db: &Db,
    client: &MemcanClient<'_>,
    entry: &PendingFeedback,
    now: DateTime<Utc>,
) -> Result<Result<(), MemcanError>, DbError> {
    let id = entry.feedback.id.to_string();
    if entry.removed {
        const TOOL: &str = "delete_memory";
        let not_found = format!("delete_memory failed: memory not found: {id}");
        return Ok(match client.call(TOOL, json!({ "memory_id": id })).await {
            Ok(answer) => match serde_json::from_str::<DeleteAnswer>(&answer) {
                Ok(answer) if answer.status == "deleted" && answer.memory_id == id => Ok(()),
                _ => Err(MemcanError::InvalidResponse { tool: TOOL }),
            },
            // MemCan's error for an id it does not hold: never written, or already deleted.
            Err(MemcanError::Rejected {
                code: Some(code),
                detail,
                ..
            }) if code == ErrorCode::INVALID_PARAMS.0 && detail == not_found => Ok(()),
            Err(error) => Err(error),
        });
    }
    const TOOL: &str = "_import_records";
    let pr = db.analyses().summary(entry.analysis_id).await?.pr;
    let flows = stored_flows(db, entry.analysis_id).await?;
    let record = import_record(entry, &pr, &flows, now).to_string();
    let expected = ImportAnswer {
        imported: 1,
        skipped: 0,
        errors: Vec::new(),
    };
    Ok(
        match client.call(TOOL, json!({ "records": record })).await {
            Ok(answer) if serde_json::from_str::<ImportAnswer>(&answer).ok() == Some(expected) => {
                Ok(())
            }
            Ok(_) => Err(MemcanError::InvalidResponse { tool: TOOL }),
            Err(error) => Err(error),
        },
    )
}

/// The wait before the next sweep and whether a nudge may cut it short.
pub(crate) fn next_wait(
    backoff: &mut Option<Duration>,
    outcome: &Result<Sweep, DbError>,
) -> (Duration, bool) {
    match outcome {
        Ok(sweep) if !sweep.unavailable => {
            *backoff = None;
            (SWEEP_INTERVAL, true)
        }
        _ => {
            let wait = backoff.map_or(FIRST_BACKOFF, |previous| (previous * 2).min(MAX_BACKOFF));
            *backoff = Some(wait);
            (wait, false)
        }
    }
}

/// Runs the writer until the process ends; started once at server start.
pub async fn run_writer(db: Db, config: Arc<Config>) {
    let mut backoff = None;
    loop {
        let nudged = NUDGE.notified();
        tokio::pin!(nudged);
        // Registered before the sweep, so a nudge sent during it is not lost.
        nudged.as_mut().enable();
        let outcome = sweep(&db, &config.memcan, Utc::now()).await;
        if let Err(error) = &outcome {
            tracing::warn!(%error, sqlite_code = ?error.sqlite_code(),
                "MemCan writer could not read or update the feedback queue; it retries after a back-off");
        }
        let (wait, interruptible) = next_wait(&mut backoff, &outcome);
        if interruptible {
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = &mut nudged => {}
            }
        } else {
            tokio::time::sleep(wait).await;
        }
    }
}
