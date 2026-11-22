//! Deterministic scoring (plan §C, ADR-017): a pure port of the daily-priorities skill's components, CI
//! classification, eligibility, judgement merge, tiers and sort. No IO and no clock: callers pass `now`.

mod ci;
mod facts;
mod parts;
mod rules;
mod text;

#[cfg(test)]
mod golden;
#[cfg(test)]
mod props;
#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use contracts::api::dashboard::{
    ExclusionReason, ItemKey, ItemKeyError, ItemKind, ItemScore, ItemTarget, PartSource,
    PartStatus, ScoreComponent, ScorePart, ScoreState, Tier,
};
use contracts::time::Timestamp;

pub use ci::{CheckContext, CheckRollup, CiSummary, classify_ci};
pub use facts::{
    CiEffective, EligibilityJudgement, Freshness, IssueFacts, JudgedPart, Judgement, MergeState,
    Mergeable, Milestone, MilestoneContext, OwnedChange, PrFacts, Relation, ReviewDecision,
    TodoFacts, TodoStatus,
};
pub use parts::{
    ESTIMATE, SIZE_MARKS, change_size_points, recency_points, size_mark, unblocks_points,
    waiting_points,
};
pub use rules::{BUILTIN_BRANCH_PATTERNS, ScoreRules, todo_display_ids};

pub(crate) use parts::pull_eligibility;
use parts::{Eligibility, Part};

/// P1 from this many tenths of a point.
pub const P1_FROM_TENTHS: i32 = 650;
/// P2 from this many tenths of a point (below P1).
pub const P2_FROM_TENTHS: i32 = 350;

/// A judgement that does not fit the item; the caller keeps the item provisional and logs it (plan §B.2).
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ScoreError {
    /// Recency and change size are always computed from scan facts.
    #[error("the {0:?} part is computed from scan facts and cannot be judged")]
    NotJudgeable(ScoreComponent),
    /// A value outside the component's closed set.
    #[error("{points} points is not an allowed value of the {component:?} part")]
    OutOfRange {
        /// Component.
        component: ScoreComponent,
        /// Value given.
        points: i32,
    },
    /// Defect likelihood and impact are integers 0–10.
    #[error("defect likelihood {likelihood} and impact {impact} must each be 0–10")]
    DefectOutOfRange {
        /// Likelihood given.
        likelihood: u8,
        /// Impact given.
        impact: u8,
    },
    /// Replacing a computed value needs its evidence.
    #[error("replacing the computed {0:?} part needs a basis")]
    OverrideWithoutBasis(ScoreComponent),
    /// Changing whether an item is ranked needs a reason.
    #[error("changing whether the item is ranked needs a reason")]
    EligibilityWithoutReason,
    /// The item's repository or number cannot form an item key.
    #[error(transparent)]
    Key(#[from] ItemKeyError),
}

impl JudgedPart {
    /// Defect severity from the LLM's likelihood and impact (0–10 each): points = likelihood + impact (J1).
    pub fn defect(likelihood: u8, impact: u8, basis: String) -> Result<Self, ScoreError> {
        if likelihood > 10 || impact > 10 {
            return Err(ScoreError::DefectOutOfRange { likelihood, impact });
        }
        Ok(Self {
            component: ScoreComponent::DefectSeverity,
            points: i32::from(likelihood + impact),
            basis,
        })
    }
}

/// Closed point values a judgement may give a component; `None` for the computed-only ones.
pub fn allowed_points(component: ScoreComponent) -> Option<&'static [i32]> {
    use ScoreComponent as C;
    const DEFECT: [i32; 21] = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
    ];
    Some(match component {
        C::Advancement => &[0, 10, 15, 20, 30],
        C::DefectSeverity => &DEFECT,
        C::Unblocks | C::HumanWaiting => &[0, 5, 10, 15],
        C::ChangeImpact | C::Completion => &[0, 5, 10],
        C::UrgencyBonus => &[0, 30],
        C::BlockedPenalty => &[-20, 0],
        C::DefaultBranchBonus => &[0, 5],
        C::MilestoneBonus => &[-10, 0, 10, 20],
        C::Recency | C::ChangeSize => return None,
    })
}

/// One scored item: wire score, full-precision total, eligibility and what the judge should answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Scored {
    /// Stable key.
    pub key: ItemKey,
    /// Display id (K8; `TODO-<prefix>` for TODOs).
    pub id: String,
    /// Kind.
    pub kind: ItemKind,
    /// Score as sent.
    pub score: ItemScore,
    /// Sum of the known parts at full precision (tier and sort use it).
    pub points: f64,
    /// Why the item is left out of the ranking; `None` when ranked.
    pub excluded: Option<String>,
    /// The rule that leaves it out; `None` when ranked.
    pub exclusion: Option<ExclusionReason>,
    /// Items the LLM merged into this one.
    pub also: Vec<ItemKey>,
    /// Components a judgement of this item answers (always-LLM parts, open conditional parts, defaults).
    pub asks: Vec<ScoreComponent>,
    /// The judge should confirm eligibility (ownership policy unavailable).
    pub asks_eligibility: bool,
    /// Computed inputs of a judgement outside the plan §B.4 judged inputs.
    pub signals: FingerprintSignals,
}

/// Computed values a judgement rests on beyond the plan §B.4 judged inputs, bucketed so they change only when the
/// scoring changes: WP11-JUDGE adds them (canonical JSON) to the judged-input fingerprint, so a part that opens or
/// closes with them is re-judged instead of freezing as an override.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize)]
pub struct FingerprintSignals {
    /// Human-waiting bucket in tenths (0, 50, 100, 150); null when pending.
    pub human_waiting: Option<i32>,
    /// Human-waiting is a default to confirm (unsure request, team request or unanswered comment).
    pub human_waiting_unsure: bool,
    /// Computed milestone bonus in tenths; null while the mapping is unknown.
    pub milestone: Option<i32>,
    /// A `blocked` label or blocked-by/depends-on wording.
    pub blocker_wording: bool,
}

fn key_of(target: ItemTarget) -> Result<ItemKey, ScoreError> {
    Ok(ItemKey::new(&target)?)
}

/// Scores a pull request at `now`, with its stored judgement if any.
pub fn score_pr(
    rules: &ScoreRules,
    pr: &PrFacts,
    judgement: Option<&Judgement>,
    now: Timestamp,
) -> Result<Scored, ScoreError> {
    let key = key_of(ItemTarget::PullRequest {
        repo: pr.repo.clone(),
        number: pr.number,
    })?;
    let id = rules.display_id(&pr.repo, pr.number);
    let parts = parts::pr_parts(rules, pr, now);
    finish(
        key,
        id,
        ItemKind::Pr,
        parts,
        parts::pr_eligibility(rules, pr),
        judgement,
        parts::blocker_wording(&pr.labels, &pr.body),
    )
}

/// Scores an issue at `now`, with its stored judgement if any.
pub fn score_issue(
    rules: &ScoreRules,
    issue: &IssueFacts,
    judgement: Option<&Judgement>,
    now: Timestamp,
) -> Result<Scored, ScoreError> {
    let key = key_of(ItemTarget::Issue {
        repo: issue.repo.clone(),
        number: issue.number,
    })?;
    let id = rules.display_id(&issue.repo, issue.number);
    let parts = parts::issue_parts(issue, now);
    finish(
        key,
        id,
        ItemKind::Issue,
        parts,
        parts::issue_eligibility(rules, issue),
        judgement,
        parts::blocker_wording(&issue.labels, &issue.body),
    )
}

/// Scores a MemCan TODO at `now` under its display id (see [`todo_display_ids`]).
pub fn score_todo(
    todo: &TodoFacts,
    id: String,
    judgement: Option<&Judgement>,
    now: Timestamp,
) -> Result<Scored, ScoreError> {
    let key = key_of(ItemTarget::Todo(todo.id))?;
    let eligibility = Eligibility {
        excluded_by: None,
        reason: "open TODO".into(),
        confirmed: true,
    };
    finish(
        key,
        id,
        ItemKind::Todo,
        parts::todo_parts(todo, now),
        eligibility,
        judgement,
        false,
    )
}

fn is_open(part: &Part) -> bool {
    matches!(part.status, PartStatus::Estimated | PartStatus::Pending)
}

/// The judge answers estimates (LLM parts and computed defaults) and pending LLM parts.
fn asked(part: &Part) -> bool {
    part.status == PartStatus::Estimated
        || (part.status == PartStatus::Pending && part.source == PartSource::Llm)
}

fn finish(
    key: ItemKey,
    id: String,
    kind: ItemKind,
    mut parts: Vec<Part>,
    mut eligibility: Eligibility,
    judgement: Option<&Judgement>,
    blocker_wording: bool,
) -> Result<Scored, ScoreError> {
    let computed = |component: ScoreComponent| parts.iter().find(|p| p.component == component);
    let human = computed(ScoreComponent::HumanWaiting);
    let signals = FingerprintSignals {
        human_waiting: human.and_then(|p| p.points).map(tenths),
        human_waiting_unsure: human.is_some_and(|p| p.status == PartStatus::Estimated),
        milestone: computed(ScoreComponent::MilestoneBonus)
            .and_then(|p| p.points)
            .map(tenths),
        blocker_wording,
    };
    let asks = parts
        .iter()
        .filter(|p| asked(p))
        .map(|p| p.component)
        .collect();
    let asks_eligibility = !eligibility.confirmed;
    let mut alarm = false;
    let mut also = Vec::new();
    if let Some(judgement) = judgement {
        apply(&mut parts, &mut eligibility, judgement)?;
        alarm = judgement.alarm;
        also.clone_from(&judgement.also);
    }
    let points: f64 = parts.iter().filter_map(|p| p.points).sum();
    let state = match judgement.map(|j| j.freshness) {
        None => ScoreState::Unjudged,
        Some(Freshness::Inherited) => ScoreState::Provisional,
        Some(Freshness::Current) if parts.iter().any(is_open) || !eligibility.confirmed => {
            ScoreState::Provisional
        }
        Some(Freshness::Current) => ScoreState::Final,
    };
    let wire: Vec<ScorePart> = parts
        .into_iter()
        .map(|p| ScorePart {
            component: p.component,
            points_tenths: p.points.map(tenths),
            source: p.source,
            status: p.status,
            basis: p.basis,
        })
        .collect();
    let score = ItemScore {
        total_tenths: wire.iter().filter_map(|p| p.points_tenths).sum(),
        state,
        tier: if alarm { Tier::P0 } else { tier(points) },
        parts: wire,
    };
    let exclusion = eligibility.excluded_by;
    let excluded = exclusion.map(|_| eligibility.reason);
    Ok(Scored {
        key,
        id,
        kind,
        score,
        points,
        excluded,
        exclusion,
        also,
        asks,
        asks_eligibility,
        signals,
    })
}

fn tenths(points: f64) -> i32 {
    (points * 10.0).round() as i32
}

/// P1 ≥ 65, P2 35–<65, P3 < 35 points, compared at full precision (P0 is the LLM's alarm).
pub fn tier(points: f64) -> Tier {
    if points >= f64::from(P1_FROM_TENTHS) / 10.0 {
        Tier::P1
    } else if points >= f64::from(P2_FROM_TENTHS) / 10.0 {
        Tier::P2
    } else {
        Tier::P3
    }
}

/// Merges a judgement (skill `score.py` `_merge_part`, `inherit`): a current one fills every open part it names
/// and may override computed ones with a basis; an inherited one fills only open LLM parts, marked `inherited`.
fn apply(
    parts: &mut [Part],
    eligibility: &mut Eligibility,
    judgement: &Judgement,
) -> Result<(), ScoreError> {
    let inherited = judgement.freshness == Freshness::Inherited;
    let label = format!(
        "[judged {}, item changed since] ",
        judgement.judged_at.get().strftime("%-d %b")
    );
    for judged in &judgement.parts {
        let allowed =
            allowed_points(judged.component).ok_or(ScoreError::NotJudgeable(judged.component))?;
        if !allowed.contains(&judged.points) {
            return Err(ScoreError::OutOfRange {
                component: judged.component,
                points: judged.points,
            });
        }
        let Some(part) = parts.iter_mut().find(|p| p.component == judged.component) else {
            continue;
        };
        let points = Some(f64::from(judged.points));
        let given = judged.basis.trim();
        let basis = if given.is_empty() {
            "judged by the LLM"
        } else {
            given
        };
        if inherited {
            if part.source == PartSource::Llm && is_open(part) {
                part.points = points;
                part.status = PartStatus::Inherited;
                part.basis = format!("{label}{basis}");
            }
            continue;
        }
        let status = if is_open(part) {
            PartStatus::Judged
        } else if part.points == points {
            continue;
        } else if given.is_empty() {
            return Err(ScoreError::OverrideWithoutBasis(judged.component));
        } else {
            PartStatus::Overridden
        };
        *part = Part {
            points,
            source: PartSource::Llm,
            status,
            basis: basis.to_owned(),
            ..part.clone()
        };
    }
    // An eligibility answer counts only while the scorer asks (ownership policy unreadable, the PR not directed at
    // the user); a stored one may predate the facts or rules that settle eligibility now.
    if let Some(flip) = judgement
        .eligibility
        .as_ref()
        .filter(|_| !eligibility.confirmed)
    {
        let given = flip.reason.trim();
        if flip.eligible != eligibility.excluded_by.is_none() && given.is_empty() {
            return Err(ScoreError::EligibilityWithoutReason);
        }
        let reason = if given.is_empty() {
            eligibility.reason.clone()
        } else {
            given.to_owned()
        };
        let reason = if inherited {
            format!("{label}{reason}")
        } else {
            reason
        };
        *eligibility = Eligibility {
            excluded_by: (!flip.eligible).then_some(ExclusionReason::JudgedNotEligible),
            reason,
            confirmed: !inherited,
        };
    }
    Ok(())
}

/// An item left out of the ranking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Excluded {
    /// Key.
    pub key: ItemKey,
    /// Display id.
    pub id: String,
    /// Plain reason.
    pub reason: String,
    /// The rule.
    pub rule: ExclusionReason,
    /// Display id of the item it was merged into.
    pub merged_into: Option<String>,
}

/// Ranked items in rank order, and the ones left out.
#[derive(Clone, Debug, PartialEq)]
pub struct Ranking {
    /// P0 alarms first, then score, advancement, defect severity and unblocks descending, then display id.
    pub ranked: Vec<Scored>,
    /// Ineligible and merged items, by display id.
    pub excluded: Vec<Excluded>,
}

fn part_points(item: &Scored, component: ScoreComponent) -> i32 {
    item.score
        .parts
        .iter()
        .find(|p| p.component == component)
        .and_then(|p| p.points_tenths)
        .unwrap_or(0)
}

fn rank_order(a: &Scored, b: &Scored) -> Ordering {
    use ScoreComponent as C;
    (b.score.tier == Tier::P0)
        .cmp(&(a.score.tier == Tier::P0))
        .then_with(|| b.points.total_cmp(&a.points))
        .then_with(|| part_points(b, C::Advancement).cmp(&part_points(a, C::Advancement)))
        .then_with(|| part_points(b, C::DefectSeverity).cmp(&part_points(a, C::DefectSeverity)))
        .then_with(|| part_points(b, C::Unblocks).cmp(&part_points(a, C::Unblocks)))
        .then_with(|| text::compare_ids(&a.id, &b.id))
        .then_with(|| a.key.cmp(&b.key))
}

/// Ranks scored items (skill `score.py` `merge`, `absorb`). Merges (`also`) are claimed in key order: a target
/// must exist, be unclaimed and claim nothing itself; a claimed item claims nothing. Merged and ineligible items
/// are left out.
pub fn rank(mut items: Vec<Scored>) -> Ranking {
    items.sort_by(|a, b| a.key.cmp(&b.key));
    let present: HashSet<ItemKey> = items.iter().map(|i| i.key.clone()).collect();
    let mut owner: HashMap<ItemKey, String> = HashMap::new();
    let mut claimers: HashSet<ItemKey> = HashSet::new();
    for item in &mut items {
        let wanted = std::mem::take(&mut item.also);
        if owner.contains_key(&item.key) {
            continue;
        }
        for target in wanted {
            if target != item.key
                && present.contains(&target)
                && !owner.contains_key(&target)
                && !claimers.contains(&target)
            {
                owner.insert(target.clone(), item.id.clone());
                item.also.push(target);
            }
        }
        if !item.also.is_empty() {
            claimers.insert(item.key.clone());
        }
    }
    let (mut ranked, mut excluded) = (Vec::new(), Vec::new());
    for item in items {
        let left_out = match (owner.get(&item.key), item.exclusion, &item.excluded) {
            (Some(by), ..) => Some((
                ExclusionReason::MergedIntoAnother,
                format!("merged into {by}"),
                Some(by.clone()),
            )),
            (None, Some(rule), Some(reason)) => Some((rule, reason.clone(), None)),
            _ => None,
        };
        match left_out {
            Some((rule, reason, merged_into)) => excluded.push(Excluded {
                key: item.key,
                id: item.id,
                reason,
                rule,
                merged_into,
            }),
            None => ranked.push(item),
        }
    }
    ranked.sort_by(rank_order);
    excluded.sort_by(|a, b| text::compare_ids(&a.id, &b.id).then_with(|| a.key.cmp(&b.key)));
    Ranking { ranked, excluded }
}

/// The user's open PR (K15 W1, K17 R1): by a scanned login that is not an excluded author, and not a draft.
fn users_pr(rules: &ScoreRules, pr: &PrFacts) -> bool {
    pr.author
        .as_deref()
        .is_some_and(|a| rules.is_scanned(a) && !rules.is_skipped_author(a))
        && !pr.is_draft
}

/// The ready label (K6) as the PR carries it: the PR needs a human (K17 R6a, K15 W3a).
pub fn ready_label<'a>(rules: &ScoreRules, pr: &'a PrFacts) -> Option<&'a str> {
    pr.labels
        .iter()
        .find(|l| rules.is_ready_label(l))
        .map(String::as_str)
}

/// Nothing on the user's PR is the user's move (K15 W1, W2, W4): [`users_pr`], no conflicts, CI green with the
/// ignored checks left out, no change request (people or bots), no unresolved thread, no comment awaiting the user's
/// reply, no review requested from a scanned login, and not [`ready_to_merge`] (merging would be the user's move).
pub fn nothing_is_users_move(rules: &ScoreRules, pr: &PrFacts) -> bool {
    users_pr(rules, pr)
        && pr.mergeable != Mergeable::Conflicting
        && pr.ci == CiEffective::Green
        && pr.changes_requested_by.is_empty()
        && pr.review_decision != Some(ReviewDecision::ChangesRequested)
        && pr.unresolved_threads == 0
        && !pr.awaiting_our_reply
        && !pr.review_pending_for_us
        && !ready_to_merge(rules, pr)
}

/// A requested review keeps the PR waiting (K15 W3c): a review is requested from people or teams outside the
/// scanned logins, the review decision is not approved and GitHub does not let the user merge it.
pub fn waits_on_requested_review(rules: &ScoreRules, pr: &PrFacts) -> bool {
    !pr.review_requested_from_others.is_empty()
        && pr.review_decision != Some(ReviewDecision::Approved)
        && !github_lets_user_merge(rules, pr)
}

/// The user's PR waits on other people by GitHub's signals alone (K15): [`nothing_is_users_move`], and it carries
/// the ready label (W3a) or [`waits_on_requested_review`] (W3c). The read path adds a plug-in's statement (W3b).
/// Such a PR is listed under Waiting on others only: not ranked, not excluded, not judged.
pub fn waits_on_others(rules: &ScoreRules, pr: &PrFacts) -> bool {
    nothing_is_users_move(rules, pr)
        && (ready_label(rules, pr).is_some() || waits_on_requested_review(rules, pr))
}

/// Sorts `rows` longest wait first (K15 W5) by the oldest open request; an unknown request time sorts last and
/// the caller chooses the `waiting_since` it shows.
pub fn by_longest_wait(rules: &ScoreRules, rows: &mut [&PrFacts]) {
    let id = |pr: &PrFacts| rules.display_id(&pr.repo, pr.number);
    rows.sort_by(|a, b| {
        let wait = |pr: &PrFacts| {
            (
                pr.others_review_requested_at.is_none(),
                pr.others_review_requested_at,
            )
        };
        wait(a)
            .cmp(&wait(b))
            .then_with(|| text::compare_ids(&id(a), &id(b)))
    });
}

/// The user's PR the user could merge, merge state aside: [`users_pr`] and the user can write to the base
/// repository.
fn mergeable_by_user(rules: &ScoreRules, pr: &PrFacts) -> bool {
    users_pr(rules, pr) && pr.viewer_can_write
}

/// The user's PR whose GitHub merge button is active: [`mergeable_by_user`] and the merge state is clean, has hooks
/// or unstable (a check that is not required has not passed). A merge the user can make, so never waiting on a
/// requested review (K15 W3c).
pub fn github_lets_user_merge(rules: &ScoreRules, pr: &PrFacts) -> bool {
    mergeable_by_user(rules, pr)
        && matches!(
            pr.merge_state,
            MergeState::Clean | MergeState::HasHooks | MergeState::Unstable
        )
}

/// The PR targets its repository's default branch or one of its release branches (K17 R2, K9).
fn targets_release_line(rules: &ScoreRules, pr: &PrFacts) -> bool {
    pr.base_ref
        .as_deref()
        .is_some_and(|base| rules.is_long_lived(&pr.repo, base, pr.default_branch.as_deref()))
}

/// GitHub's merge state counts as clean (K17 R4): clean or has hooks, or unstable while only ignored, skipped or
/// neutral checks have not passed (CI green with the ignored checks left out).
fn merge_state_clean(pr: &PrFacts) -> bool {
    match pr.merge_state {
        MergeState::Clean | MergeState::HasHooks => true,
        MergeState::Unstable => pr.ci == CiEffective::Green,
        _ => false,
    }
}

/// The user's PR that is ready to merge now (Ready to merge, K17): [`mergeable_by_user`], into the default branch or
/// a release branch, no conflicts, the merge state counts as clean, CI is not red, nothing is left to answer (no
/// unresolved review thread, no change request from people or bots, no comment awaiting the user's reply, every
/// review read: more than were fetched could hide a change request) and no ready label (it needs a human). Such a
/// PR is ranked.
pub fn ready_to_merge(rules: &ScoreRules, pr: &PrFacts) -> bool {
    mergeable_by_user(rules, pr)
        && targets_release_line(rules, pr)
        && pr.mergeable != Mergeable::Conflicting
        && merge_state_clean(pr)
        && pr.ci != CiEffective::Red
        && pr.unresolved_threads == 0
        && pr.changes_requested_by.is_empty()
        && !pr.awaiting_our_reply
        && !pr.reviews_truncated
        && ready_label(rules, pr).is_none()
}

/// The user's PR whose merge state GitHub had not computed by the scan: it is read again (Q17), for Waiting on
/// others and Ready to merge.
pub fn merge_state_pending(rules: &ScoreRules, pr: &PrFacts) -> bool {
    mergeable_by_user(rules, pr) && pr.merge_state == MergeState::Unknown
}

/// [`merge_state_pending`] for a PR Ready to merge could list (into the default branch or a release branch, no
/// ready label): the panel's count.
pub fn merge_state_pending_for_ready(rules: &ScoreRules, pr: &PrFacts) -> bool {
    merge_state_pending(rules, pr)
        && targets_release_line(rules, pr)
        && ready_label(rules, pr).is_none()
}

/// The PRs [`ready_to_merge`], by display id.
pub fn merge_ready<'a>(rules: &ScoreRules, prs: &'a [PrFacts]) -> Vec<&'a PrFacts> {
    let mut rows: Vec<&PrFacts> = prs.iter().filter(|pr| ready_to_merge(rules, pr)).collect();
    rows.sort_by(|a, b| {
        text::compare_ids(
            &rules.display_id(&a.repo, a.number),
            &rules.display_id(&b.repo, b.number),
        )
    });
    rows
}
