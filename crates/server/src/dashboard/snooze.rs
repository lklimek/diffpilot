//! Snoozed items (owner SN1): the user leaves a ranked item out of the ranking for some days. It wakes at its date,
//! or earlier on a review request to the user or a mention of the user newer than the snooze. Applied when the
//! snapshot is read; a snooze that ended is marked ended (and kept) when a new snapshot is stored, so a wake lasts
//! after the request or mention is gone.

use std::collections::BTreeMap;

use contracts::api::dashboard::{ItemKey, ItemTarget};
use contracts::time::Timestamp;
use db::repo::dashboard::{Snooze, SnoozeEnd};

use super::refresh::Scope;
use super::snapshot::Snapshot;
use crate::state::AppState;

/// The snapshot holds the item (a pull request, issue or TODO it scanned).
pub fn holds(snapshot: &Snapshot, key: &ItemKey) -> bool {
    let is = |target: ItemTarget| ItemKey::new(&target).is_ok_and(|k| &k == key);
    snapshot.pull_requests.iter().any(|pr| {
        is(ItemTarget::PullRequest {
            repo: pr.facts.repo.clone(),
            number: pr.facts.number,
        })
    }) || snapshot.issues.iter().any(|issue| {
        is(ItemTarget::Issue {
            repo: issue.facts.repo.clone(),
            number: issue.facts.number,
        })
    }) || snapshot
        .todos
        .iter()
        .any(|todo| is(ItemTarget::Todo(todo.facts.id)))
}

/// Why the snooze ended at `now`, if it did: its date passed, or `snapshot` shows a review request to the user (with
/// a known time) or a mention of the user on the item newer than the snooze; mentions whose id is in `hidden` do not
/// count.
pub fn ended(
    snooze: &Snooze,
    snapshot: &Snapshot,
    hidden: &[String],
    now: Timestamp,
) -> Option<SnoozeEnd> {
    if snooze.until <= now {
        return Some(SnoozeEnd::Date);
    }
    let requested = matches!(snooze.key.target(), ItemTarget::PullRequest { .. })
        && snapshot.pull_requests.iter().any(|pr| {
            let f = &pr.facts;
            ItemKey::new(&ItemTarget::PullRequest {
                repo: f.repo.clone(),
                number: f.number,
            })
            .is_ok_and(|k| k == snooze.key)
                && f.review_pending_for_us
                && !f.review_request_age_is_approx
                && f.review_requested_at
                    .is_some_and(|at| at > snooze.snoozed_at)
        });
    if requested {
        return Some(SnoozeEnd::ReviewRequest);
    }
    snapshot
        .mentions
        .iter()
        .any(|m| {
            m.item_key == snooze.key && m.created_at > snooze.snoozed_at && !hidden.contains(&m.id)
        })
        .then_some(SnoozeEnd::Mention)
}

/// Marks the user's snoozes that ended against `snapshot` ended with their reason (hidden mentions do not count);
/// the rows are kept. A failure is logged; the next stored snapshot tries again.
pub async fn end_ended(state: &AppState, scope: &Scope, snapshot: &Snapshot) {
    let dashboard = state.db.dashboard();
    let result = async {
        let snoozes = dashboard.snoozes(&scope.tenant_id, &scope.user_id).await?;
        if snoozes.is_empty() {
            return Ok(0);
        }
        let hidden = dashboard
            .hidden_mentions(&scope.tenant_id, &scope.user_id)
            .await?;
        let now = state.dashboard.now();
        let mut by_reason: BTreeMap<SnoozeEnd, Vec<ItemKey>> = BTreeMap::new();
        for snooze in &snoozes {
            if let Some(reason) = ended(snooze, snapshot, &hidden, now) {
                by_reason
                    .entry(reason)
                    .or_default()
                    .push(snooze.key.clone());
            }
        }
        let mut count = 0;
        for (reason, keys) in by_reason {
            count += dashboard
                .wake(&scope.tenant_id, &scope.user_id, &keys, reason, now)
                .await?;
        }
        Ok::<_, db::error::DbError>(count)
    }
    .await;
    match result {
        Ok(0) => {}
        Ok(count) => {
            tracing::info!(user_id = %scope.user_id, count, "dashboard snoozes ended against the snapshot");
        }
        Err(error) => {
            tracing::warn!(%error, user_id = %scope.user_id,
                "ended dashboard snoozes could not be marked; the next stored snapshot tries again");
        }
    }
}
