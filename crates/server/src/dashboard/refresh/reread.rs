//! Targeted re-read (ADR-022 §5): after a plug-in's change signal, the changed pull requests of a user's current
//! snapshot are read again from GitHub with the scan's details query alone (no search, no enumeration, no context
//! reads, no ranking job) and written back while that snapshot is still current.

use std::collections::{BTreeMap, HashSet};

use contracts::api::dashboard::{RefreshFailureCode, RefreshState};
use db::error::DbError;
use serde_json::Value;

use super::Scope;
use super::facts::{self, PrContext};
use crate::dashboard::plugin::PrKey;
use crate::dashboard::score::ScoreRules;
use crate::dashboard::snapshot::{self, StoredPr};
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
    let dashboard = state.db.dashboard();
    let now = state.dashboard.now();
    if let Some(latest) = dashboard
        .latest_refresh(&scope.tenant_id, &scope.user_id)
        .await?
    {
        if latest.state == RefreshState::Scanning {
            return Ok(Reread::Skipped("a scan runs"));
        }
        let limited = latest.failure.as_ref().is_some_and(|failure| {
            failure.code == RefreshFailureCode::GithubRateLimited
                && failure.retry_at.is_some_and(|at| at > now)
        });
        if limited {
            return Ok(Reread::Skipped("GitHub rate limit"));
        }
    }
    let Some((record, document)) = dashboard
        .latest_snapshot::<Value>(&scope.tenant_id, &scope.user_id)
        .await?
    else {
        return Ok(Reread::Skipped("no snapshot"));
    };
    let Some(mut stored) = snapshot::readable(document) else {
        return Ok(Reread::Skipped("unreadable snapshot"));
    };
    let pulls: Vec<ItemRef> = stored
        .pull_requests
        .iter()
        .filter(|pr| changed.contains(&PrKey::new(&pr.facts.repo, pr.facts.number)))
        .take(REREAD_LIMIT)
        .map(|pr| ItemRef {
            repo: pr.facts.repo.clone(),
            number: pr.facts.number,
        })
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
    let rules = ScoreRules::new(&stored.own_login, &stored.settings);
    let (updated, closed) = apply(&rules, &mut stored.pull_requests, &fetched.found);
    stored.prs_reread_at = Some(now);
    match dashboard
        .replace_current_snapshot(&scope.tenant_id, &scope.user_id, record.id, &stored)
        .await
    {
        Ok(()) => Ok(Reread::Written { updated, closed }),
        Err(DbError::Conflict) => Ok(Reread::Skipped("a newer snapshot")),
        Err(error) => Err(error.into()),
    }
}

/// Replaces the pull requests read again with their new facts, keeping what only the scan's other reads know
/// (relations, stack relations, ownership verdict, milestone catalogue); drops those no longer open. A pull request
/// GitHub did not answer for keeps its facts. Returns (updated, closed).
fn apply(
    rules: &ScoreRules,
    pulls: &mut Vec<StoredPr>,
    found: &BTreeMap<ItemRef, PullDetail>,
) -> (usize, usize) {
    let mut updated = 0;
    let mut closed = 0;
    let candidate = PrCandidate {
        title: String::new(),
        url: String::new(),
        updated_at: None,
        relations: BTreeMap::new(),
    };
    pulls.retain_mut(|pr| {
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
    (updated, closed)
}
