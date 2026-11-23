//! Scan answers → scoring facts (the skill's `analyze_pr` and issue mapping, plan §A.2). Pure; untrusted text is
//! capped here, before it is stored.

use std::collections::{BTreeMap, BTreeSet};

use contracts::time::Timestamp;

use crate::dashboard::score::{
    CheckContext, CheckRollup, IssueFacts, MergeState, Mergeable, Milestone, MilestoneContext,
    OwnedChange, PrFacts, Relation, ReviewDecision, ScoreRules, classify_ci,
};
use crate::dashboard::snapshot::{StoredIssue, StoredLinkedIssue, StoredPr};
use crate::github::work::{
    Actor, Blockers, IssueCandidate, IssueRelation, ItemRef, PrCandidate, PrRelation, PullDetail,
    ReviewNode, StatusRollup,
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

/// Bots whose last change request is followed by a review of theirs on another commit, in any state: the request no
/// longer counts (K15 W2, K17 R5). A request on the current `head` (no newer head exists) or of an unknown commit
/// keeps counting.
fn rereviewed_bots(rules: &ScoreRules, reviews: &[ReviewNode], head: &str) -> BTreeSet<String> {
    let mut requested: BTreeMap<&str, Option<&str>> = BTreeMap::new();
    let mut moved_on = BTreeSet::new();
    for review in reviews {
        let Some(author) = review.author.as_ref().filter(|a| is_bot_actor(rules, a)) else {
            continue;
        };
        let Some(login) = author.login.as_deref() else {
            continue;
        };
        let commit = review.commit.as_ref().map(|c| c.oid.as_str());
        if review.state == "CHANGES_REQUESTED" {
            requested.insert(login, commit);
            moved_on.remove(&name(login));
        } else if let (Some(Some(at)), Some(commit)) = (requested.get(login), commit)
            && *at != commit
            && *at != head
        {
            moved_on.insert(name(login));
        }
    }
    moved_on
}

/// The newest time of `times`.
fn newest(times: impl Iterator<Item = Timestamp>) -> Option<Timestamp> {
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

    let mut latest: Vec<(String, String, Option<Timestamp>, bool)> = Vec::new();
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
        |check| rules.is_ignored_check(&item.repo, check),
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
        merge_state: merge_state(&detail.merge_state_status),
        viewer_can_write: detail
            .base_repository
            .as_ref()
            .and_then(|r| r.viewer_permission.as_deref())
            .is_some_and(|p| matches!(p, "WRITE" | "MAINTAIN" | "ADMIN")),
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
        changes_requested_by: {
            let mut logins = with_state("CHANGES_REQUESTED");
            // While GitHub's decision still requests changes, GitHub enforces the request: it stays.
            if detail.review_decision.as_deref() != Some("CHANGES_REQUESTED") {
                let rereviewed =
                    rereviewed_bots(rules, &detail.reviews.nodes, &detail.head_ref_oid);
                logins.retain(|login| !rereviewed.contains(login));
            }
            logins
        },
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
        approved_by_bots: {
            let mut bots: Vec<String> = latest
                .iter()
                .filter(|(_, state, _, bot)| state == "APPROVED" && *bot)
                .map(|(l, ..)| name(l))
                .collect();
            bots.sort();
            bots
        },
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
        base_sha: detail.base_ref_oid.as_deref().map(name),
        created_at: detail.created_at,
        ci_red: ci.red.iter().map(|c| name(c)).collect(),
        ci_pending: ci.pending.iter().map(|c| name(c)).collect(),
        ci_ignored: ci.ignored_not_passing.iter().map(|c| name(c)).collect(),
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

/// GitHub's `mergeStateStatus`; an unknown value is [`MergeState::Unknown`].
pub(super) fn merge_state(status: &str) -> MergeState {
    match status {
        "CLEAN" => MergeState::Clean,
        "HAS_HOOKS" => MergeState::HasHooks,
        "UNSTABLE" => MergeState::Unstable,
        "BEHIND" => MergeState::Behind,
        "BLOCKED" => MergeState::Blocked,
        "DIRTY" => MergeState::Dirty,
        "DRAFT" => MergeState::Draft,
        _ => MergeState::Unknown,
    }
}

fn pr_relations(candidate: &PrCandidate) -> Vec<Relation> {
    let found: BTreeSet<PrRelation> = candidate.relations.values().flatten().copied().collect();
    found
        .into_iter()
        .filter_map(|relation| match relation {
            PrRelation::Authored => Some(Relation::Authored),
            PrRelation::Assigned => Some(Relation::Assigned),
            PrRelation::ReviewRequested => Some(Relation::ReviewRequested),
            PrRelation::Mentioned => None,
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
        opened_at: Some(candidate.created_at),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use contracts::api::dashboard::DashboardSettings;
    use contracts::common::RepoRef;

    use super::*;
    use crate::dashboard::plugin::Overlay;

    fn node() -> serde_json::Value {
        serde_json::json!({
            "number": 1199, "title": "feat(api): paginate identity lookups",
            "url": "https://github.com/acme/platform/pull/1199", "state": "OPEN", "isDraft": false,
            "isCrossRepository": false, "additions": 10, "deletions": 2,
            "createdAt": "2026-11-08T10:00:00Z", "updatedAt": "2026-11-17T11:00:00Z",
            "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN", "reviewDecision": null,
            "baseRefName": "main", "baseRefOid": "3d1f0c9a7b2e4d6f8a0c1e3b5d7f9a1c3e5b7d9f",
            "headRefName": "feat/pages", "headRefOid": "8c8acb6a35253739c2cb54e178ffa230638e9dfc",
            "milestone": null, "baseRepository": null,
            "author": {"__typename": "User", "login": "alice"},
            "labels": {"nodes": []}, "participants": {"totalCount": 3},
            "closingIssuesReferences": {"nodes": []}, "assignees": {"nodes": []},
            "reviewRequests": {"nodes": []},
            "reviews": {"totalCount": 2, "nodes": [
                {"author": {"__typename": "Bot", "login": "review-helper"}, "state": "APPROVED",
                 "submittedAt": "2026-11-17T10:00:00Z"},
                {"author": {"__typename": "User", "login": "ana"}, "state": "APPROVED",
                 "submittedAt": "2026-11-17T11:00:00Z"}
            ]},
            "comments": {"nodes": []}, "reviewThreads": {"nodes": []},
            "timelineItems": {"nodes": []}, "commits": {"nodes": []}
        })
    }

    fn stored(detail: &PullDetail) -> StoredPr {
        let rules = ScoreRules::new("alice", &DashboardSettings::default());
        let candidate = PrCandidate {
            updated_at: None,
            title: detail.title.clone(),
            url: detail.url.clone(),
            relations: BTreeMap::new(),
        };
        let item = ItemRef {
            repo: RepoRef {
                owner: "acme".into(),
                name: "platform".into(),
            },
            number: 1199,
        };
        pr(
            &rules,
            &item,
            detail,
            PrContext {
                candidate: &candidate,
                blockers: None,
                owned: OwnedChange::NoPolicy,
                milestones: MilestoneContext::default(),
            },
        )
    }

    /// A GitHub App approver without the `[bot]` suffix keeps its Bot type in the facts, so its approval is not a
    /// human's (B10 "a merge the user can make").
    #[test]
    fn a_bot_typed_approver_is_recorded_as_a_bot() {
        let detail: PullDetail = serde_json::from_value(node()).expect("detail");
        let stored = stored(&detail);
        assert_eq!(
            (
                stored.facts.approved_by.as_slice(),
                stored.facts.approved_by_bots.as_slice()
            ),
            (
                ["ana".to_owned(), "review-helper".to_owned()].as_slice(),
                ["review-helper".to_owned()].as_slice()
            )
        );
    }

    /// The head and base commits the scan read are kept (the PR badge takes Ready to merge only at both); an answer
    /// without the base commit keeps none.
    #[test]
    fn the_scanned_head_and_base_commits_are_kept() {
        let detail: PullDetail = serde_json::from_value(node()).expect("detail");
        let pr = stored(&detail);
        assert_eq!(
            (pr.head_sha.as_str(), pr.base_sha.as_deref()),
            (
                "8c8acb6a35253739c2cb54e178ffa230638e9dfc",
                Some("3d1f0c9a7b2e4d6f8a0c1e3b5d7f9a1c3e5b7d9f")
            )
        );
        let mut without = node();
        without
            .as_object_mut()
            .expect("object")
            .remove("baseRefOid");
        let detail: PullDetail = serde_json::from_value(without).expect("detail");
        assert_eq!(stored(&detail).base_sha, None);
    }

    /// GitHub's merge state and the user's write access to the base repository are kept (Ready to merge); an
    /// unknown state or permission, or none, is not mergeable.
    #[test]
    fn merge_state_and_write_access_are_recorded() {
        let with = |state: &str, permission: Option<&str>| {
            let mut node = node();
            node["mergeStateStatus"] = state.into();
            node["baseRepository"] = serde_json::json!({
                "nameWithOwner": "acme/platform", "isPrivate": false, "defaultBranchRef": {"name": "main"},
                "viewerPermission": permission,
            });
            let detail: PullDetail = serde_json::from_value(node).expect("detail");
            let facts = stored(&detail).facts;
            (facts.merge_state, facts.viewer_can_write)
        };
        assert_eq!(with("CLEAN", Some("WRITE")), (MergeState::Clean, true));
        assert_eq!(
            with("HAS_HOOKS", Some("MAINTAIN")),
            (MergeState::HasHooks, true)
        );
        assert_eq!(
            with("UNSTABLE", Some("ADMIN")),
            (MergeState::Unstable, true)
        );
        assert_eq!(
            with("BLOCKED", Some("TRIAGE")),
            (MergeState::Blocked, false)
        );
        assert_eq!(with("BEHIND", Some("READ")), (MergeState::Behind, false));
        assert_eq!(with("DIRTY", None), (MergeState::Dirty, false));
        assert_eq!(with("DRAFT", Some("WRITE")), (MergeState::Draft, true));
        assert_eq!(
            with("SOMETHING_NEW", Some("NEW")),
            (MergeState::Unknown, false)
        );
        let detail: PullDetail = serde_json::from_value(node()).expect("no base repository");
        assert!(!stored(&detail).facts.viewer_can_write);
    }

    const OLD_HEAD: &str = "0a2e66fcf7525eb47f81eeedefd2591d4827d1e6";
    const HEAD: &str = "8c8acb6a35253739c2cb54e178ffa230638e9dfc";

    /// `(login, typename, state, commit)` as a review node; `None` leaves the commit out.
    fn review(login: &str, typename: &str, state: &str, commit: Option<&str>) -> serde_json::Value {
        let mut node = serde_json::json!({"author": {"__typename": typename, "login": login}, "state": state,
            "submittedAt": "2026-11-22T17:37:41Z"});
        if let Some(oid) = commit {
            node["commit"] = serde_json::json!({"oid": oid});
        }
        node
    }

    /// The user's approved, ready-labelled PR with green CI, mergeable, nothing else to answer (dashpay/platform#5951
    /// at the 2026-11-23 scan), with `reviews` in order: its change requests and whether it waits on others (K15).
    fn blocked_by(reviews: Vec<serde_json::Value>) -> (Vec<String>, bool) {
        blocked_by_with("APPROVED", reviews)
    }

    /// [`blocked_by`] with GitHub's review decision `decision`.
    fn blocked_by_with(decision: &str, reviews: Vec<serde_json::Value>) -> (Vec<String>, bool) {
        let mut node = node();
        node["labels"] = serde_json::json!({"nodes": [{"name": "ready-for-human"}]});
        node["reviewDecision"] = decision.into();
        node["mergeStateStatus"] = "UNSTABLE".into();
        node["baseRepository"] = serde_json::json!({"nameWithOwner": "acme/platform", "isPrivate": false,
            "defaultBranchRef": {"name": "main"}, "viewerPermission": "WRITE"});
        node["commits"] = serde_json::json!({"nodes": [{"commit": {"statusCheckRollup": {"state": "SUCCESS",
            "contexts": {"pageInfo": {"hasNextPage": false}, "nodes": [{"__typename": "CheckRun", "name": "build",
            "status": "COMPLETED", "conclusion": "SUCCESS", "startedAt": "2026-11-22T22:00:00Z"}]}}}}]});
        node["reviews"] = serde_json::json!({"totalCount": reviews.len(), "nodes": reviews});
        let detail: PullDetail = serde_json::from_value(node).expect("detail");
        let facts = stored(&detail).facts;
        let rules = ScoreRules::new("alice", &DashboardSettings::default());
        let waits =
            crate::dashboard::snapshot::waiting(&rules, &facts, &Overlay::default()).is_some();
        (facts.changes_requested_by, waits)
    }

    /// K15 W2, K17 R5 (owner, 2026-11-23): a bot's change request ends when the same bot reviews another commit, in
    /// any state; the PR then waits on others (dashpay/platform#5951: thepastaclaw's request on an old head, its
    /// COMMENTED round on the new one).
    #[test]
    fn a_bots_change_request_ends_when_it_reviews_a_newer_head() {
        let bot = |state, commit| review("thepastaclaw", "User", state, Some(commit));
        assert_eq!(
            blocked_by(vec![
                bot("CHANGES_REQUESTED", OLD_HEAD),
                bot("COMMENTED", HEAD),
                review("alice", "User", "APPROVED", Some(HEAD)),
            ]),
            (vec![], true)
        );
        assert_eq!(
            blocked_by(vec![
                review("review-helper", "Bot", "CHANGES_REQUESTED", Some(OLD_HEAD)),
                review("review-helper", "Bot", "APPROVED", Some(HEAD)),
            ]),
            (vec![], true),
            "a later approval as before"
        );
    }

    /// A bot's change request stands while the bot has not reviewed another commit, while the request is on the
    /// current head (a later review of an older commit is no newer head), or the request's commit is unknown; a new
    /// request on the newer head stands; another bot's review clears nothing; a person's request stands until GitHub
    /// clears it.
    #[test]
    fn a_change_request_stands_until_its_bot_reviews_a_newer_head() {
        let bot = |state, commit| review("thepastaclaw", "User", state, commit);
        let blocked = |who: &str| (vec![who.to_owned()], false);
        assert_eq!(
            blocked_by(vec![
                bot("CHANGES_REQUESTED", Some(OLD_HEAD)),
                bot("COMMENTED", Some(OLD_HEAD)),
            ]),
            blocked("thepastaclaw"),
            "not re-reviewed"
        );
        assert_eq!(
            blocked_by(vec![
                bot("COMMENTED", Some(HEAD)),
                bot("CHANGES_REQUESTED", Some(OLD_HEAD)),
            ]),
            blocked("thepastaclaw"),
            "a review before the request"
        );
        assert_eq!(
            blocked_by(vec![
                bot("CHANGES_REQUESTED", Some(OLD_HEAD)),
                bot("COMMENTED", Some(HEAD)),
                bot("CHANGES_REQUESTED", Some(HEAD)),
            ]),
            blocked("thepastaclaw"),
            "a new request on the newer head"
        );
        assert_eq!(
            blocked_by(vec![
                bot("CHANGES_REQUESTED", None),
                bot("COMMENTED", Some(HEAD)),
            ]),
            blocked("thepastaclaw"),
            "the request's commit unknown"
        );
        assert_eq!(
            blocked_by(vec![
                bot("CHANGES_REQUESTED", Some(HEAD)),
                bot("COMMENTED", Some(OLD_HEAD)),
            ]),
            blocked("thepastaclaw"),
            "the request on the current head, a later review of an older commit"
        );
        assert_eq!(
            blocked_by(vec![
                bot("CHANGES_REQUESTED", Some(OLD_HEAD)),
                review("DCG-Claude", "User", "COMMENTED", Some(HEAD)),
            ]),
            blocked("thepastaclaw"),
            "another bot's review"
        );
        assert_eq!(
            blocked_by(vec![
                review("ana", "User", "CHANGES_REQUESTED", Some(OLD_HEAD)),
                review("ana", "User", "COMMENTED", Some(HEAD)),
            ]),
            blocked("ana"),
            "a person's stale request"
        );
    }

    /// While GitHub's review decision still requests changes (a bot whose review counts on GitHub), the bot's request
    /// stays in the facts, so the row keeps its "changes requested" tag and stays the user's move.
    #[test]
    fn a_bots_change_request_stands_while_github_still_requests_changes() {
        let bot = |state, commit| review("thepastaclaw", "User", state, Some(commit));
        assert_eq!(
            blocked_by_with(
                "CHANGES_REQUESTED",
                vec![bot("CHANGES_REQUESTED", OLD_HEAD), bot("COMMENTED", HEAD)]
            ),
            (vec!["thepastaclaw".to_owned()], false)
        );
    }
}
