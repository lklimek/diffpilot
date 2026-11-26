//! Targeted dashboard reads from a coalesced GitHub webhook window.

use std::collections::{BTreeMap, BTreeSet};

use contracts::api::dashboard::{ItemKey, ItemTarget};
use contracts::time::Timestamp;
use db::error::DbError;
use tokio::time::Instant;

use super::{Scope, facts, owned::Ownership, reread};
use crate::dashboard::plugin::PrKey;
use crate::dashboard::score::{OwnedChange, ScoreRules, pull_eligibility};
use crate::dashboard::snapshot::Snapshot;
use crate::dashboard::{judge, settings, snooze, zone};
use crate::github::work::{
    EventBudget, IssueCandidate, IssueDetail, ItemRef, PR_BATCH, PrCandidate, PrRelation,
    PullDetail, UserTokens, WorkClient, WorkError,
};
use crate::state::AppState;

const DAILY_ADDITIONS: usize = 20;

#[derive(Default)]
pub(crate) struct EventItems {
    pub(crate) pulls: BTreeMap<PrKey, ItemRef>,
    pub(crate) issues: BTreeMap<PrKey, ItemRef>,
    pub(crate) candidates: BTreeMap<PrKey, ItemRef>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum EventReadError {
    #[error("the dashboard could not store its event updates")]
    Db(#[from] DbError),
    #[error("an event could not re-read a dashboard item from GitHub")]
    Github(#[from] WorkError),
}

pub(crate) enum EventRead {
    Complete,
    Scanning,
}

/// Reads the affected snapshot items and ranks newly admitted PRs.
pub(crate) async fn reread_items(
    state: &AppState,
    scope: &Scope,
    items: &EventItems,
    budget: EventBudget,
    instant: Instant,
) -> Result<EventRead, EventReadError> {
    let readers = state.dashboard.readers();
    let writer = readers.writer(scope).await;
    let now = state.dashboard.now();
    let current = reread::read_current(state, scope).await?;
    if matches!(current, Err(reread::SnapshotUnavailable::Scanning)) {
        return Ok(EventRead::Scanning);
    }
    let current = if readers.limited_until(scope, now).is_some() {
        Err(reread::SnapshotUnavailable::Skipped("GitHub rate limit"))
    } else {
        current
    };
    let (record, mut stored) = match current {
        Ok(current) => current,
        Err(reason) => {
            log_skipped(
                scope,
                reason.description(),
                items
                    .pulls
                    .values()
                    .chain(items.issues.values())
                    .chain(items.candidates.values()),
            );
            return Ok(EventRead::Complete);
        }
    };
    let tokens = UserTokens {
        state: state.clone(),
        user_id: scope.user_id.clone(),
    };
    let client = WorkClient::new(state.config.github.api_url.clone(), tokens)?
        .with_event_budget(budget, instant);
    let rules = ScoreRules::new(&stored.own_login, &stored.settings);
    let mut pulls = Vec::new();
    let mut existing = BTreeSet::new();
    for pr in &stored.pull_requests {
        let key = PrKey::new(&pr.facts.repo, pr.facts.number);
        if items.pulls.contains_key(&key) || items.candidates.contains_key(&key) {
            pulls.push(ItemRef {
                repo: pr.facts.repo.clone(),
                number: pr.facts.number,
            });
        }
        existing.insert(key);
    }
    let candidates: Vec<_> = items
        .candidates
        .iter()
        .filter(|(key, _)| !existing.contains(*key))
        .collect();
    let issues: Vec<_> = stored
        .issues
        .iter()
        .filter_map(|issue| {
            let facts = &issue.facts;
            items
                .issues
                .contains_key(&PrKey::new(&facts.repo, facts.number))
                .then(|| ItemRef {
                    repo: facts.repo.clone(),
                    number: facts.number,
                })
        })
        .collect();

    let candidate_rules = if candidates.is_empty() {
        None
    } else {
        let login = state.db.users().get(&scope.user_id).await?.profile.login;
        let settings = settings::effective(&state.db, &scope.tenant_id, &scope.user_id).await?;
        let zone = settings
            .time_zone
            .as_deref()
            .and_then(zone::parse)
            .unwrap_or(jiff::tz::TimeZone::UTC);
        stored
            .event_additions
            .retain(|added| *added >= judge::day_start(now, &zone));
        Some((
            ScoreRules::new(&login, &settings),
            Ownership::new(&settings),
        ))
    };
    let candidate_limit = if stored.event_additions.len() < DAILY_ADDITIONS {
        reread::REREAD_LIMIT
    } else {
        0
    };
    let pull_limit = pulls.len().min(reread::REREAD_LIMIT);
    let issue_limit = issues.len().min(reread::REREAD_LIMIT - pull_limit);
    let remaining = reread::REREAD_LIMIT - pull_limit - issue_limit;
    let new_limit = candidates.len().min(candidate_limit).min(remaining);
    let admitting: BTreeSet<_> = candidates[..new_limit]
        .iter()
        .map(|(key, _)| *key)
        .collect();
    let unranked: Vec<_> = items
        .pulls
        .iter()
        .filter(|(key, _)| !existing.contains(*key) && !admitting.contains(*key))
        .map(|(_, item)| item)
        .collect();
    let history_limit = unranked.len().min(remaining - new_limit);
    log_skipped(
        scope,
        "event item or re-read limit",
        pulls[pull_limit..]
            .iter()
            .chain(issues[issue_limit..].iter())
            .chain(candidates[new_limit..].iter().map(|(_, item)| *item))
            .chain(unranked[history_limit..].iter().copied()),
    );

    let wanted: Vec<_> = pulls
        .into_iter()
        .take(pull_limit)
        .chain(
            candidates[..new_limit]
                .iter()
                .map(|(_, item)| (*item).clone()),
        )
        .chain(unranked[..history_limit].iter().map(|item| (*item).clone()))
        .collect();
    let mut dirty = false;
    let mut retry = None;
    for (index, batch) in wanted.chunks(PR_BATCH).enumerate() {
        let fetched = match client.pull_requests(batch).await {
            Ok(fetched) => fetched,
            Err(error) => {
                retry = read_failure(state, scope, error, &wanted[index * PR_BATCH..], now);
                break;
            }
        };
        let (updated, closed, history_changed) =
            reread::apply(&rules, &mut stored, &fetched.found, now);
        dirty |= history_changed;
        if updated + closed > 0 {
            stored.prs_reread_at = Some(now);
            dirty = true;
        }
        if let Some((rules, policies)) = &candidate_rules {
            for (item, detail) in &fetched.found {
                if admitting.contains(&PrKey::new(&item.repo, item.number))
                    && stored.event_additions.len() < DAILY_ADDITIONS
                    && add_candidate(&mut stored, item, detail, rules, policies, now)
                {
                    dirty = true;
                }
            }
        }
        if batch.iter().any(|item| {
            let key = PrKey::new(&item.repo, item.number);
            (!admitting.contains(&key) || items.pulls.contains_key(&key))
                && !fetched.found.contains_key(item)
        }) {
            retry = Some(EventReadError::Github(WorkError::ExecutionFailed));
        }
    }
    for (index, batch) in issues[..issue_limit].chunks(PR_BATCH).enumerate() {
        let fetched = match client.issues(batch).await {
            Ok(fetched) => fetched,
            Err(error) => {
                retry = read_failure(
                    state,
                    scope,
                    error,
                    &issues[index * PR_BATCH..issue_limit],
                    now,
                )
                .or(retry);
                break;
            }
        };
        dirty |= !fetched.found.is_empty();
        apply_issues(&mut stored, fetched.found);
        if !fetched.notes.is_empty() {
            retry = Some(EventReadError::Github(WorkError::ExecutionFailed));
        }
    }
    if !dirty {
        if !stored.pending_event_items.is_empty() {
            super::rank_pending_locked(state, scope, super::RankMode::Pending).await?;
        }
        return retry.map_or(Ok(EventRead::Complete), Err);
    }

    stored.last_event_at = Some(now);
    let updated = state
        .db
        .dashboard()
        .replace_current_snapshot(&scope.tenant_id, &scope.user_id, record.id, &stored)
        .await;
    match updated {
        Ok(()) => {
            if !stored.pending_event_items.is_empty() {
                super::rank_pending_locked(state, scope, super::RankMode::Pending).await?;
            }
            drop(writer);
            snooze::end_ended(state, scope, &stored).await;
        }
        Err(DbError::Conflict) => {}
        Err(error) => return Err(error.into()),
    }
    retry.map_or(Ok(EventRead::Complete), Err)
}

fn read_failure(
    state: &AppState,
    scope: &Scope,
    error: WorkError,
    skipped: &[ItemRef],
    now: Timestamp,
) -> Option<EventReadError> {
    match error {
        WorkError::EventBudget
        | WorkError::PrimaryRateLimit { .. }
        | WorkError::SecondaryRateLimit { .. }
        | WorkError::Token(_)
        | WorkError::Unauthorized => {
            state.dashboard.readers().remember_limit(scope, &error, now);
            log_skipped(scope, &error.to_string(), skipped.iter());
            None
        }
        error => Some(error.into()),
    }
}

fn log_skipped<'a>(scope: &Scope, reason: &str, items: impl Iterator<Item = &'a ItemRef>) {
    let items: Vec<_> = items.map(ToString::to_string).collect();
    if !items.is_empty() {
        tracing::info!(user_id = %scope.user_id, %reason, ?items, "GitHub webhook dashboard reads skipped");
    }
}

fn add_candidate(
    stored: &mut Snapshot,
    item: &ItemRef,
    detail: &PullDetail,
    rules: &ScoreRules,
    policies: &Ownership,
    now: Timestamp,
) -> bool {
    if detail.state != "OPEN" {
        return false;
    }
    let mut relations = BTreeMap::<String, BTreeSet<PrRelation>>::new();
    let author = detail
        .author
        .as_ref()
        .and_then(|author| author.login.as_deref());
    let mut record = |login: &str, relation| {
        if rules.is_scanned(login) {
            relations
                .entry(login.to_owned())
                .or_default()
                .insert(relation);
        }
    };
    if let Some(author) = author {
        record(author, PrRelation::Authored);
    }
    for assignee in &detail.assignees.nodes {
        record(&assignee.login, PrRelation::Assigned);
    }
    for reviewer in detail
        .review_requests
        .nodes
        .iter()
        .filter_map(|request| request.requested_reviewer.as_ref()?.login.as_deref())
    {
        record(reviewer, PrRelation::ReviewRequested);
    }
    let candidate = PrCandidate {
        title: detail.title.clone(),
        url: detail.url.clone(),
        updated_at: Some(detail.updated_at),
        relations,
    };
    // Directed work passes the ownership filter even when there is no ownership verdict yet.
    let owned = if policies.has_policy(&item.repo) {
        OwnedChange::Unknown
    } else {
        OwnedChange::NoPolicy
    };
    let has = |relation| {
        candidate
            .relations
            .values()
            .any(|relations| relations.contains(&relation))
    };
    if !candidate.ranked()
        || pull_eligibility(
            rules,
            author,
            detail.is_draft,
            has(PrRelation::Assigned),
            has(PrRelation::ReviewRequested),
            owned,
        )
        .excluded_by
        .is_some()
    {
        return false;
    }
    // The read's repository metadata grounds the item even when the inbound hint used a legacy name.
    if let Some(repo) = &detail.base_repository
        && !crate::github::work::repo_name(&item.repo).eq_ignore_ascii_case(&repo.name_with_owner)
    {
        return false;
    }
    let pr = facts::pr(
        rules,
        item,
        detail,
        facts::PrContext {
            candidate: &candidate,
            blockers: None,
            owned,
            milestones: Default::default(),
        },
    );
    let Ok(key) = ItemKey::new(&ItemTarget::PullRequest {
        repo: pr.facts.repo.clone(),
        number: pr.facts.number,
    }) else {
        return false;
    };
    stored.pull_requests.push(pr);
    stored.checked.pr = stored.checked.pr.saturating_add(1);
    stored.event_additions.push(now);
    stored.pending_event_items.push(key);
    true
}

fn apply_issues(stored: &mut Snapshot, mut detail: BTreeMap<ItemRef, Option<IssueDetail>>) {
    let before = stored.issues.len();
    stored.issues.retain_mut(|issue| {
        let item = ItemRef { repo: issue.facts.repo.clone(), number: issue.facts.number };
        let Some(answer) = detail.remove(&item) else { return true; };
        let Some(answer) = answer.filter(|issue| issue.state == "OPEN") else { return false; };
        let same_milestone = answer.milestone.as_ref().is_some_and(|new| {
            matches!(&issue.facts.milestone, crate::dashboard::score::Milestone::Assigned(old) if old == &new.title)
        });
        let milestone = if same_milestone { issue.facts.milestone_context.clone() } else { Default::default() };
        let candidate = IssueCandidate {
            title: answer.title,
            url: String::new(),
            author: answer.author.and_then(|author| author.login),
            assignees: Vec::new(),
            labels: answer.labels.nodes.into_iter().map(|label| label.name).collect(),
            milestone: answer.milestone,
            body: answer.body,
            created_at: answer.created_at,
            updated_at: answer.updated_at,
            comments: answer.comments.total_count,
            relations: BTreeSet::new(),
            mention_candidate: false,
            fallback_mention_logins: BTreeSet::new(),
            author_association: None,
        };
        let mut reread = facts::issue(&item, &candidate, milestone, answer.private.unwrap_or(issue.private));
        // Snapshot membership and relations come from the scan for both PRs and issues.
        reread.facts.relations = std::mem::take(&mut issue.facts.relations);
        *issue = reread;
        true
    });
    stored.checked.issue = stored
        .checked
        .issue
        .saturating_sub((before - stored.issues.len()) as u32);
}
