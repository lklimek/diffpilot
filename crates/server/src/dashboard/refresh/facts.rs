//! Scan answers → scoring facts (the skill's `analyze_pr` and issue mapping, plan §A.2). Pure; untrusted text is
//! capped here, before it is stored.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use crate::dashboard::score::{
    CheckContext, CheckRollup, IssueFacts, Mergeable, Milestone, MilestoneContext, OwnedChange,
    PrFacts, Relation, ReviewDecision, ScoreRules, classify_ci,
};
use crate::dashboard::snapshot::{StoredIssue, StoredLinkedIssue, StoredPr};
use crate::github::work::{
    Actor, Blockers, IssueCandidate, IssueRelation, ItemRef, PrCandidate, PrRelation, PullDetail,
    StatusRollup,
};

/// Characters kept of a title.
const TITLE_CHARS: usize = 300;
/// Characters kept of a body (searched for blocked-by wording; the judge sees 300 of them).
const BODY_CHARS: usize = 4000;
/// Characters kept of a label, a check name or a login.
const NAME_CHARS: usize = 100;

/// `text` cut to `max` characters.
pub(super) fn capped(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn name(text: &str) -> String {
    capped(text, NAME_CHARS)
}

fn is_bot_actor(rules: &ScoreRules, actor: &Actor) -> bool {
    actor
        .login
        .as_deref()
        .is_some_and(|login| rules.is_bot(login, actor.typename == "Bot"))
}

/// What the scan knows of a pull request besides its details.
pub(super) struct PrContext<'a> {
    /// How it was found.
    pub candidate: &'a PrCandidate,
    /// Stack relations; `None` when the listing of its repository failed.
    pub blockers: Option<Blockers>,
    /// Ownership-policy verdict.
    pub owned: OwnedChange,
    /// Release catalogue of its repository (used when it has a milestone).
    pub milestones: MilestoneContext,
}

fn rollup(found: Option<&StatusRollup>) -> Option<CheckRollup> {
    found.map(|rollup| CheckRollup {
        truncated: rollup.contexts.page_info.has_next_page,
        contexts: rollup
            .contexts
            .nodes
            .iter()
            .filter_map(|node| match node.typename.as_str() {
                "CheckRun" => Some(CheckContext::CheckRun {
                    name: name(node.name.as_deref().unwrap_or_default()),
                    workflow: node.workflow.as_deref().map(name),
                    status: node.status.clone(),
                    conclusion: node.conclusion.clone(),
                    started_at: node.started_at,
                }),
                "StatusContext" => Some(CheckContext::StatusContext {
                    context: name(node.context.as_deref().unwrap_or_default()),
                    state: node.state.clone().unwrap_or_default(),
                    created_at: node.created_at,
                }),
                _ => None,
            })
            .collect(),
    })
}

/// The newest time of `times`.
fn newest(times: impl Iterator<Item = DateTime<Utc>>) -> Option<DateTime<Utc>> {
    times.max()
}

/// Facts of an open pull request (skill `analyze_pr`).
pub(super) fn pr(
    rules: &ScoreRules,
    item: &ItemRef,
    detail: &PullDetail,
    context: PrContext<'_>,
) -> StoredPr {
    let scanned = |login: &str| rules.is_scanned(login);
    let requests: Vec<_> = detail
        .review_requests
        .nodes
        .iter()
        .filter_map(|node| node.requested_reviewer.as_ref())
        .collect();
    let requested_logins: BTreeSet<String> = requests
        .iter()
        .filter_map(|r| r.login.as_deref())
        .map(str::to_lowercase)
        .collect();
    let mut others: Vec<String> = Vec::new();
    for reviewer in &requests {
        match (&reviewer.login, &reviewer.name) {
            (Some(login), _)
                if !scanned(login) && !rules.is_bot(login, reviewer.typename == "Bot") =>
            {
                others.push(name(login));
            }
            (None, Some(team)) if reviewer.typename == "Team" => {
                others.push(format!("team:{}", name(team)));
            }
            _ => {}
        }
    }
    others.sort_by_key(|o| o.to_lowercase());
    others.dedup();
    let request_time = |login: &str| {
        newest(
            detail
                .timeline_items
                .nodes
                .iter()
                .filter(|event| {
                    event
                        .requested_reviewer
                        .as_ref()
                        .and_then(|r| r.login.as_deref())
                        .is_some_and(|l| l.eq_ignore_ascii_case(login))
                })
                .filter_map(|event| event.created_at),
        )
    };
    let pending_for_us = requested_logins.iter().any(|login| scanned(login));
    let our_request = newest(
        requested_logins
            .iter()
            .filter(|login| scanned(login))
            .filter_map(|login| request_time(login)),
    );
    let (review_requested_at, approx) = match (pending_for_us, our_request) {
        (true, Some(at)) => (Some(at), false),
        (true, None) => (Some(detail.updated_at), true),
        (false, _) => (None, false),
    };
    // Each open request's time is its newest request event; the oldest of them is how long the PR waits.
    let others_review_requested_at = others
        .iter()
        .filter(|o| !o.starts_with("team:"))
        .filter_map(|login| request_time(login))
        .min();

    let mut latest: Vec<(String, String, Option<DateTime<Utc>>, bool)> = Vec::new();
    for review in &detail.reviews.nodes {
        let Some(author) = &review.author else {
            continue;
        };
        let Some(login) = &author.login else {
            continue;
        };
        if !matches!(
            review.state.as_str(),
            "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED"
        ) {
            continue;
        }
        let bot = is_bot_actor(rules, author);
        latest.retain(|(l, ..)| l != login);
        latest.push((
            login.clone(),
            review.state.clone(),
            review.submitted_at,
            bot,
        ));
    }
    let with_state = |state: &str| -> Vec<String> {
        let mut logins: Vec<String> = latest
            .iter()
            .filter(|(_, s, ..)| s == state)
            .map(|(l, ..)| name(l))
            .collect();
        logins.sort();
        logins
    };
    let human_cr: Vec<_> = latest
        .iter()
        .filter(|(login, state, _, bot)| state == "CHANGES_REQUESTED" && !bot && !scanned(login))
        .collect();
    let changes_requested_human_at = newest(
        human_cr
            .iter()
            .filter(|(login, ..)| !requested_logins.contains(&login.to_lowercase()))
            .filter_map(|(_, _, at, _)| *at),
    );
    let last_comment = detail.comments.nodes.iter().max_by_key(|c| c.created_at);
    let last_author = last_comment.and_then(|c| c.author.as_ref());
    let awaiting_our_reply = last_author.is_some_and(|a| {
        a.login
            .as_deref()
            .is_some_and(|login| !scanned(login) && !is_bot_actor(rules, a))
    });
    let ci = classify_ci(
        rollup(
            detail
                .commits
                .nodes
                .first()
                .and_then(|n| n.commit.status_check_rollup.as_ref()),
        )
        .as_ref(),
    );
    let default_branch = detail
        .base_repository
        .as_ref()
        .and_then(|r| r.default_branch_ref.as_ref())
        .map(|b| b.name.clone());
    let long_lived =
        |branch: &str| rules.is_long_lived(&item.repo, branch, default_branch.as_deref());
    let (dependents, stacked_on) = match context.blockers {
        Some(found) => (
            Some(
                if detail.is_cross_repository || long_lived(&detail.head_ref_name) {
                    Vec::new()
                } else {
                    found.dependents
                },
            ),
            Some(if long_lived(&detail.base_ref_name) {
                Vec::new()
            } else {
                found.stacked_on
            }),
        ),
        None => (None, None),
    };
    let milestone = match &detail.milestone {
        Some(m) => Milestone::Assigned(name(&m.title)),
        None => Milestone::Unassigned,
    };
    let author = detail.author.as_ref();
    let facts = PrFacts {
        repo: item.repo.clone(),
        number: item.number,
        title: capped(&detail.title, TITLE_CHARS),
        body: capped(&detail.body, BODY_CHARS),
        author: author.and_then(|a| a.login.as_deref()).map(name),
        author_is_bot: author.is_some_and(|a| a.typename == "Bot"),
        relations: pr_relations(context.candidate),
        is_draft: detail.is_draft,
        labels: detail.labels.nodes.iter().map(|l| name(&l.name)).collect(),
        updated_at: detail.updated_at,
        additions: u32::try_from(detail.additions).ok(),
        deletions: u32::try_from(detail.deletions).ok(),
        base_ref: Some(name(&detail.base_ref_name)),
        default_branch,
        dependents,
        stacked_on,
        milestone_context: if matches!(milestone, Milestone::Assigned(_)) {
            context.milestones
        } else {
            MilestoneContext::default()
        },
        milestone,
        ci: ci.effective,
        mergeable: match detail.mergeable.as_str() {
            "MERGEABLE" => Mergeable::Mergeable,
            "CONFLICTING" => Mergeable::Conflicting,
            _ => Mergeable::Unknown,
        },
        review_decision: match detail.review_decision.as_deref() {
            Some("CHANGES_REQUESTED") => Some(ReviewDecision::ChangesRequested),
            Some("APPROVED") => Some(ReviewDecision::Approved),
            Some("REVIEW_REQUIRED") => Some(ReviewDecision::ReviewRequired),
            _ => None,
        },
        unresolved_threads: u32::try_from(
            detail
                .review_threads
                .nodes
                .iter()
                .filter(|t| !t.is_resolved)
                .count(),
        )
        .unwrap_or(u32::MAX),
        review_pending_for_us: pending_for_us,
        review_requested_at,
        review_request_age_is_approx: approx,
        changes_requested_by: with_state("CHANGES_REQUESTED"),
        changes_requested_by_human: !human_cr.is_empty(),
        changes_requested_human_at,
        reviews_truncated: detail
            .reviews
            .total_count
            .is_some_and(|total| total as usize > detail.reviews.nodes.len()),
        awaiting_our_reply,
        last_comment_author: last_author.and_then(|a| a.login.as_deref()).map(name),
        last_comment_at: last_comment.map(|c| c.created_at),
        owned_change: context.owned,
        approved_by: with_state("APPROVED"),
        review_requested_from_others: others,
        others_review_requested_at,
    };
    StoredPr {
        facts,
        private: detail
            .base_repository
            .as_ref()
            .is_some_and(|r| r.is_private),
        head_sha: name(&detail.head_ref_oid),
        created_at: detail.created_at,
        ci_red: ci.red.iter().map(|c| name(c)).collect(),
        ci_pending: ci.pending.iter().map(|c| name(c)).collect(),
        linked_issues: detail
            .closing_issues_references
            .nodes
            .iter()
            .map(|issue| StoredLinkedIssue {
                number: issue.number,
                title: capped(&issue.title, TITLE_CHARS),
            })
            .collect(),
    }
}

fn pr_relations(candidate: &PrCandidate) -> Vec<Relation> {
    let found: BTreeSet<PrRelation> = candidate.relations.values().flatten().copied().collect();
    found
        .into_iter()
        .map(|relation| match relation {
            PrRelation::Authored => Relation::Authored,
            PrRelation::Assigned => Relation::Assigned,
            PrRelation::ReviewRequested => Relation::ReviewRequested,
        })
        .collect()
}

/// Facts of an open issue.
pub(super) fn issue(
    item: &ItemRef,
    candidate: &IssueCandidate,
    milestones: MilestoneContext,
    private: bool,
) -> StoredIssue {
    let milestone = match &candidate.milestone {
        Some(m) => Milestone::Assigned(name(&m.title)),
        None => Milestone::Unassigned,
    };
    StoredIssue {
        facts: IssueFacts {
            repo: item.repo.clone(),
            number: item.number,
            title: capped(&candidate.title, TITLE_CHARS),
            body: capped(&candidate.body, BODY_CHARS),
            author: candidate.author.as_deref().map(name),
            relations: candidate
                .relations
                .iter()
                .map(|relation| match relation {
                    IssueRelation::RepoOpen => Relation::RepoOpen,
                    IssueRelation::Assigned => Relation::Assigned,
                    IssueRelation::Mentioned => Relation::Mentioned,
                })
                .collect(),
            labels: candidate.labels.iter().map(|l| name(l)).collect(),
            updated_at: candidate.updated_at,
            milestone_context: if matches!(milestone, Milestone::Assigned(_)) {
                milestones
            } else {
                MilestoneContext::default()
            },
            milestone,
        },
        private,
        comments: candidate.comments,
    }
}
