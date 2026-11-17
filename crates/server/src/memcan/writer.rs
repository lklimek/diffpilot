//! The single MemCan writer: drains the feedback outbox (`FeedbackRepo::pending_memcan`) into MemCan. A live
//! entry is stored verbatim under its feedback id (`_import_records`, MemCan's synchronous embed-and-upsert,
//! so a retry or a newer revision replaces the same memory); a removed entry's memory is deleted. An entry counts
//! as written only after MemCan confirmed it. When MemCan is unavailable the sweep stops at the first failure
//! and the worker backs off; feedback stays stored locally either way.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use db::error::DbError;
use db::repo::Db;
use db::repo::feedback::{MemcanState, PendingFeedback};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Notify;

use super::client::{MemcanClient, MemcanError};
use super::memory::import_record;
use super::stored_flows;
use crate::config::{Config, MemcanConfig};

/// Entries written per sweep.
const BATCH: u32 = 20;
/// Sweep interval without failures; a nudge starts a sweep earlier.
pub(crate) const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
/// First wait after a failed sweep; doubled per failed sweep up to [`MAX_BACKOFF`].
pub(crate) const FIRST_BACKOFF: Duration = Duration::from_secs(30);
pub(crate) const MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);

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
    /// Entries or connection attempts that failed.
    pub(crate) failed: u32,
}

/// Writes up to [`BATCH`] pending entries; stops at the first failure that means MemCan is unavailable.
pub(crate) async fn sweep(
    db: &Db,
    config: &MemcanConfig,
    now: DateTime<Utc>,
) -> Result<Sweep, DbError> {
    let pending = db.feedback().pending_memcan(BATCH).await?;
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
                written: 0,
                failed: 1,
            });
        }
    };
    let mut outcome = Sweep::default();
    let mut last_error = None;
    for entry in &pending {
        let result = write(db, &client, entry, now).await?;
        let state = match &result {
            Ok(()) => MemcanState::Written,
            Err(_) => MemcanState::Failed,
        };
        db.feedback()
            .set_memcan_state(entry.feedback.id, entry.revision, state)
            .await?;
        match result {
            Ok(()) => outcome.written += 1,
            Err(error) => {
                outcome.failed += 1;
                let unavailable = error.is_unavailable();
                last_error = Some(error);
                if unavailable {
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
            "MemCan writes of feedback failed; the entries are retried after a back-off"
        );
    } else {
        tracing::trace!(written = outcome.written, "feedback written to MemCan");
    }
    Ok(outcome)
}

#[derive(Deserialize)]
struct ImportAnswer {
    imported: u64,
}

/// Writes one entry: `Ok(Err(_))` is a MemCan failure, `Err(_)` a local database failure.
async fn write(
    db: &Db,
    client: &MemcanClient<'_>,
    entry: &PendingFeedback,
    now: DateTime<Utc>,
) -> Result<Result<(), MemcanError>, DbError> {
    if entry.removed {
        const TOOL: &str = "delete_memory";
        let deleted = client
            .call(TOOL, json!({ "memory_id": entry.feedback.id.to_string() }))
            .await;
        return Ok(match deleted {
            // Never written (or already deleted): nothing left in MemCan.
            Err(MemcanError::Rejected { detail, .. }) if detail.contains("not found") => Ok(()),
            other => other.map(|_| ()),
        });
    }
    const TOOL: &str = "_import_records";
    let pr = db.analyses().summary(entry.analysis_id).await?.pr;
    let flows = stored_flows(db, entry.analysis_id).await?;
    let record = import_record(entry, &pr, &flows, now).to_string();
    let answer = match client.call(TOOL, json!({ "records": record })).await {
        Ok(answer) => answer,
        Err(error) => return Ok(Err(error)),
    };
    Ok(match serde_json::from_str::<ImportAnswer>(&answer) {
        Ok(ImportAnswer { imported: 1 }) => Ok(()),
        _ => Err(MemcanError::InvalidResponse { tool: TOOL }),
    })
}

/// The wait before the next sweep and whether a nudge may cut it short.
pub(crate) fn next_wait(
    backoff: &mut Option<Duration>,
    outcome: &Result<Sweep, DbError>,
) -> (Duration, bool) {
    match outcome {
        Ok(sweep) if sweep.failed == 0 => {
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
            tracing::warn!(%error, "MemCan writer could not read the feedback queue; it retries after a back-off");
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
