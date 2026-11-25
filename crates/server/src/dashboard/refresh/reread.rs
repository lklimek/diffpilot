//! Targeted re-read (ADR-022 §5): after a plug-in's change signal, the changed pull requests of a user's current
//! snapshot are read again from GitHub with the scan's details query alone (no search, no enumeration, no context
//! reads, no ranking job) and written back while that snapshot is still current. The fact check while My work is
//! open (plan §M F3) picks them by a change probe (Q18) of every pull request of the snapshot instead.

use std::collections::{BTreeMap, HashSet};

use contracts::api::dashboard::{RefreshFailureCode, RefreshState};
use db::error::DbError;
use serde_json::Value;

use super::Scope;
use super::facts::{self, PrContext};
use crate::dashboard::plugin::PrKey;
use crate::dashboard::score::ScoreRules;
use crate::dashboard::snapshot::{self, StoredPr};
use crate::dashboard::snooze;
use crate::github::work::{
    ItemRef, PR_BATCH, PROBE_BATCH, PrCandidate, PullDetail, UserTokens, WorkClient, WorkError,
};
use crate::state::AppState;

/// Pull requests re-read per user and change signal: four detail batches, as one details step of the scan.
pub(crate) const REREAD_LIMIT: usize = PR_BATCH * 4;
/// Pull requests a fact check probes: ten probe queries (plan §M).
pub(crate) const PROBE_LIMIT: usize = PROBE_BATCH * 10;

/// A re-read could not run.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RereadError {
    /// GitHub or the user's token failed.
    #[error("GitHub could not be read")]
    Github(#[from] WorkError),
    /// The database failed.
    #[error("the dashboard snapshot could not be read or written")]
    Db(#[from] DbError),
    /// Some requested pull requests were not read; the last good freshness time is retained.
    #[error("the GitHub {stage} read was incomplete ({missing} pull requests missing)")]
    Incomplete { stage: &'static str, missing: usize },
}

/// What a re-read did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reread {
    /// Nothing to do or not now (no snapshot, none of its pull requests changed, a scan runs, the rate limit).
    Skipped(&'static str),
    /// A fact check's probe found no pull request changed on GitHub; nothing was written.
    Unchanged,
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
        Ok(Reread::Unchanged) => {}
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
    read_again(state, scope, Some(changed)).await
}

/// The fact check (plan §M F3): probes at most [`PROBE_LIMIT`] of the snapshot's pull requests and re-reads at most
/// [`REREAD_LIMIT`] whose change mark moved; only a complete, successful check advances the last check time.
pub(crate) async fn check(state: &AppState, scope: &Scope) -> Result<Reread, RereadError> {
    read_again(state, scope, None).await
}

/// Re-reads the pull requests in `changed`, or (none) those a change probe finds changed.
async fn read_again(
    state: &AppState,
    scope: &Scope,
    changed: Option<&HashSet<PrKey>>,
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
    changed: Option<&HashSet<PrKey>>,
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
    let item = |pr: &StoredPr| ItemRef {
        repo: pr.facts.repo.clone(),
        number: pr.facts.number,
    };
    let tokens = UserTokens {
        state: state.clone(),
        user_id: scope.user_id.clone(),
    };
    let client = WorkClient::new(state.config.github.api_url.clone(), tokens)?;
    let mut unread = stored.pull_requests.len().saturating_sub(PROBE_LIMIT);
    let pulls: Vec<ItemRef> = match changed {
        Some(changed) => stored
            .pull_requests
            .iter()
            .filter(|pr| changed.contains(&PrKey::new(&pr.facts.repo, pr.facts.number)))
            .take(REREAD_LIMIT)
            .map(item)
            .collect(),
        None => {
            let probed: Vec<&StoredPr> = stored.pull_requests.iter().take(PROBE_LIMIT).collect();
            let probes = client
                .pr_probes(&probed.iter().map(|pr| item(pr)).collect::<Vec<_>>())
                .await?;
            if probes.len() != probed.len() {
                return Err(RereadError::Incomplete {
                    stage: "probe",
                    missing: probed.len() - probes.len(),
                });
            }
            let moved: Vec<_> = probed
                .into_iter()
                .filter(|pr| {
                    probes
                        .get(&item(pr))
                        .is_some_and(|probe| pr.mark.as_ref() != Some(&facts::probe_mark(probe)))
                })
                .map(item)
                .collect();
            unread += moved.len().saturating_sub(REREAD_LIMIT);
            moved.into_iter().take(REREAD_LIMIT).collect()
        }
    };
    if pulls.is_empty() {
        if changed.is_some() {
            return Ok(Reread::Skipped("none of the user's pull requests changed"));
        }
        if unread > 0 {
            return Err(RereadError::Incomplete {
                stage: "probe",
                missing: unread,
            });
        }
        state
            .dashboard
            .readers()
            .checked(scope, record.id, state.dashboard.now());
        return Ok(Reread::Unchanged);
    }
    let fetched = client.pull_requests(&pulls).await?;
    if fetched.found.len() != pulls.len() || !fetched.notes.is_empty() {
        return Err(RereadError::Incomplete {
            stage: "details",
            missing: pulls.len() - fetched.found.len(),
        });
    }
    let rules = ScoreRules::new(&stored.own_login, &stored.settings);
    let (updated, closed) = apply(&rules, &mut stored.pull_requests, &fetched.found);
    stored.prs_reread_at = Some(now);
    match dashboard
        .replace_current_snapshot(&scope.tenant_id, &scope.user_id, record.id, &stored)
        .await
    {
        Ok(()) => {
            snooze::end_ended(state, scope, &stored).await;
            if changed.is_none() {
                if unread > 0 {
                    return Err(RereadError::Incomplete {
                        stage: "bounded fact check",
                        missing: unread,
                    });
                }
                state
                    .dashboard
                    .readers()
                    .checked(scope, record.id, state.dashboard.now());
            }
            Ok(Reread::Written { updated, closed })
        }
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
