//! Targeted re-read (ADR-022 §5): after a plug-in's change signal, the changed pull requests of a user's current
//! snapshot are read again from GitHub with the scan's details query alone (no search, no enumeration, no context
//! reads, no ranking job) and written back while that snapshot is still current.

use std::collections::{BTreeMap, HashSet};

use contracts::api::dashboard::{RefreshFailureCode, RefreshState};
use contracts::time::Timestamp;
use db::error::DbError;
use serde_json::Value;

use super::facts::{self, PrContext};
use super::{Scope, history};
use crate::dashboard::plugin::PrKey;
use crate::dashboard::score::ScoreRules;
use crate::dashboard::snapshot::{self, Snapshot, StoredPr};
use crate::dashboard::snooze;
use crate::github::work::{
    ItemRef, PR_BATCH, PrCandidate, PullDetail, UserTokens, WorkClient, WorkError,
};
use crate::state::AppState;

/// Pull requests re-read per user and change signal: four detail batches, as one details step of the scan.
pub(crate) const REREAD_LIMIT: usize = PR_BATCH * 4;

/// A re-read could not run.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RereadError {
    /// GitHub or the user's token failed.
    #[error("GitHub could not be read")]
    Github(#[from] WorkError),
    /// The database failed.
    #[error("the dashboard snapshot could not be read or written")]
    Db(#[from] DbError),
    /// Some requested pull requests were not read.
    #[error("the GitHub details read was incomplete ({missing} pull requests missing)")]
    Incomplete { missing: usize },
}

/// What a re-read did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reread {
    /// Nothing to do or not now (no snapshot, none of its pull requests changed, a scan runs, the rate limit).
    Skipped(&'static str),
    /// The snapshot was written back with `updated` pull requests read again and `closed` ones dropped.
    Written {
        /// Re-read and still open.
        updated: usize,
        /// Closed or merged meanwhile.
        closed: usize,
    },
}

/// Re-reads the user's pull requests in `changed`; failures are logged at debug level (the next full refresh reads
/// everything again).
pub(crate) async fn changed_prs(state: &AppState, scope: &Scope, changed: &HashSet<PrKey>) {
    match reread(state, scope, changed).await {
        Ok(Reread::Written { updated, closed }) => {
            tracing::info!(user_id = %scope.user_id, updated, closed,
                "dashboard pull requests re-read after a plug-in change signal");
        }
        Ok(Reread::Skipped(why)) => {
            tracing::trace!(user_id = %scope.user_id, why, "dashboard re-read skipped");
        }
        Err(error) => {
            tracing::debug!(user_id = %scope.user_id, %error, error_chain = ?error,
                "dashboard re-read after a plug-in change signal failed; the next refresh reads them");
        }
    }
}

/// Re-reads at most [`REREAD_LIMIT`] of the user's pull requests in `changed`.
pub(crate) async fn reread(
    state: &AppState,
    scope: &Scope,
    changed: &HashSet<PrKey>,
) -> Result<Reread, RereadError> {
    let readers = state.dashboard.readers();
    let _writer = readers.writer(scope).await;
    if readers
        .limited_until(scope, state.dashboard.now())
        .is_some()
    {
        return Ok(Reread::Skipped("GitHub rate limit"));
    }
    let result = read_locked(state, scope, changed).await;
    if let Err(RereadError::Github(error)) = &result {
        readers.remember_limit(scope, error, state.dashboard.now());
    }
    result
}

async fn read_locked(
    state: &AppState,
    scope: &Scope,
    changed: &HashSet<PrKey>,
) -> Result<Reread, RereadError> {
    let dashboard = state.db.dashboard();
    let now = state.dashboard.now();
    let (record, mut stored) = match read_current(state, scope).await? {
        Ok(snapshot) => snapshot,
        Err(reason) => return Ok(Reread::Skipped(reason.description())),
    };
    let item = |pr: &StoredPr| ItemRef {
        repo: pr.facts.repo.clone(),
        number: pr.facts.number,
    };
    let pulls: Vec<_> = stored
        .pull_requests
        .iter()
        .filter(|pr| changed.contains(&PrKey::new(&pr.facts.repo, pr.facts.number)))
        .take(REREAD_LIMIT)
        .map(item)
        .collect();
    if pulls.is_empty() {
        return Ok(Reread::Skipped("none of the user's pull requests changed"));
    }
    let tokens = UserTokens {
        state: state.clone(),
        user_id: scope.user_id.clone(),
    };
    let client = WorkClient::new(state.config.github.api_url.clone(), tokens)?;
    let fetched = client.pull_requests(&pulls).await?;
    if fetched.found.len() != pulls.len() || !fetched.notes.is_empty() {
        return Err(RereadError::Incomplete {
            missing: pulls.len() - fetched.found.len(),
        });
    }
    let rules = ScoreRules::new(&stored.own_login, &stored.settings);
    let (updated, closed, _) = apply(&rules, &mut stored, &fetched.found, now);
    stored.prs_reread_at = Some(now);
    match dashboard
        .replace_current_snapshot(&scope.tenant_id, &scope.user_id, record.id, &stored)
        .await
    {
        Ok(()) => {
            snooze::end_ended(state, scope, &stored).await;
            Ok(Reread::Written { updated, closed })
        }
        Err(DbError::Conflict) => Ok(Reread::Skipped("a newer snapshot")),
        Err(error) => Err(error.into()),
    }
}

pub(super) enum SnapshotUnavailable {
    Scanning,
    Skipped(&'static str),
}

impl SnapshotUnavailable {
    pub(super) fn description(&self) -> &'static str {
        match self {
            Self::Scanning => "a scan runs",
            Self::Skipped(reason) => reason,
        }
    }
}

/// Reads the current snapshot only when a scan or GitHub backoff does not take precedence.
pub(super) async fn read_current(
    state: &AppState,
    scope: &Scope,
) -> Result<
    Result<(db::repo::dashboard::RefreshRecord, snapshot::Snapshot), SnapshotUnavailable>,
    DbError,
> {
    let dashboard = state.db.dashboard();
    let now = state.dashboard.now();
    if let Some(latest) = dashboard
        .latest_refresh(&scope.tenant_id, &scope.user_id)
        .await?
    {
        if latest.state == RefreshState::Scanning {
            return Ok(Err(SnapshotUnavailable::Scanning));
        }
        let limited = latest.failure.as_ref().is_some_and(|failure| {
            failure.code == RefreshFailureCode::GithubRateLimited
                && failure.retry_at.is_some_and(|at| at > now)
        });
        if limited {
            return Ok(Err(SnapshotUnavailable::Skipped("GitHub rate limit")));
        }
    }
    let Some((record, document)) = dashboard
        .latest_snapshot::<Value>(&scope.tenant_id, &scope.user_id)
        .await?
    else {
        return Ok(Err(SnapshotUnavailable::Skipped("no snapshot")));
    };
    let Some(stored) = snapshot::readable(document) else {
        return Ok(Err(SnapshotUnavailable::Skipped("unreadable snapshot")));
    };
    Ok(Ok((record, stored)))
}

/// Replaces the pull requests read again with their new facts, keeping what only the scan's other reads know
/// (relations, stack relations, ownership verdict, milestone catalogue). From those same reads, drop closed
/// entries from the ranking and reconcile the chart's lifecycle events, including unranked pull requests.
/// Returns (updated ranked items, closed ranked items, history changed).
pub(super) fn apply(
    rules: &ScoreRules,
    stored: &mut Snapshot,
    found: &BTreeMap<ItemRef, PullDetail>,
    now: Timestamp,
) -> (usize, usize, bool) {
    let history_changed = history::apply(rules, stored, found, now);
    let mut updated = 0;
    let mut closed = 0;
    let candidate = PrCandidate {
        title: String::new(),
        url: String::new(),
        updated_at: None,
        relations: BTreeMap::new(),
    };
    stored.pull_requests.retain_mut(|pr| {
        let item = ItemRef {
            repo: pr.facts.repo.clone(),
            number: pr.facts.number,
        };
        let Some(detail) = found.get(&item) else {
            return true;
        };
        if detail.state != "OPEN" {
            closed += 1;
            return false;
        }
        let old = &pr.facts;
        let mut new = facts::pr(
            rules,
            &item,
            detail,
            PrContext {
                candidate: &candidate,
                blockers: None,
                owned: old.owned_change,
                milestones: old.milestone_context.clone(),
            },
        );
        new.facts.relations.clone_from(&old.relations);
        new.facts.dependents.clone_from(&old.dependents);
        new.facts.stacked_on.clone_from(&old.stacked_on);
        *pr = new;
        updated += 1;
        true
    });
    (updated, closed, history_changed)
}
