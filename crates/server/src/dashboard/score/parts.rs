//! Score components of pull requests, issues and TODOs from scan facts (skill `scoring.py`, `todos.py`
//! `score_row`, `score.py` `heuristic_part`).

use contracts::api::dashboard::{
    ChangeSizeMark, ExclusionReason, PartSource, PartStatus, ScoreComponent as C, SizeMark,
};
use contracts::time::Timestamp;

use super::facts::{
    CiEffective, IssueFacts, Mergeable, Milestone, MilestoneContext, OwnedChange, PrFacts,
    Relation, ReviewDecision, TodoFacts, TodoStatus,
};
use super::rules::ScoreRules;
use super::text;

/// Basis prefix of every estimate (plan §C states).
pub const ESTIMATE: &str = "estimate until judged: ";

/// One component with full-precision points; `None` while pending.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Part {
    pub component: C,
    pub points: Option<f64>,
    pub source: PartSource,
    pub status: PartStatus,
    pub basis: String,
}

impl Part {
    fn new(
        component: C,
        points: Option<f64>,
        source: PartSource,
        status: PartStatus,
        basis: String,
    ) -> Self {
        Self {
            component,
            points,
            source,
            status,
            basis,
        }
    }

    fn computed(component: C, points: f64, basis: impl Into<String>) -> Self {
        Self::new(
            component,
            Some(points),
            PartSource::Computed,
            PartStatus::Computed,
            basis.into(),
        )
    }

    fn not_applicable(component: C, basis: &str) -> Self {
        Self::new(
            component,
            Some(0.0),
            PartSource::NotApplicable,
            PartStatus::Computed,
            basis.into(),
        )
    }

    /// A computed default the LLM confirms or replaces.
    fn to_confirm(component: C, points: f64, rule: &str) -> Self {
        let basis = format!("{ESTIMATE}{rule} → {}", show(points));
        Self::new(
            component,
            Some(points),
            PartSource::Computed,
            PartStatus::Estimated,
            basis,
        )
    }

    /// A constant estimate of an LLM part (K13).
    fn estimate(component: C, points: f64, rule: &str) -> Self {
        let basis = format!("{ESTIMATE}{rule} → {}", show(points));
        Self::new(
            component,
            Some(points),
            PartSource::Llm,
            PartStatus::Estimated,
            basis,
        )
    }

    fn pending(component: C, source: PartSource, basis: &str) -> Self {
        Self::new(component, None, source, PartStatus::Pending, basis.into())
    }
}

fn show(points: f64) -> String {
    format!("{}", (points * 10.0).round() / 10.0).replace('-', "−")
}

/// Days from `then` to `now` the way the skill computes them (seconds, then days).
pub(super) fn days_between(then: Timestamp, now: Timestamp) -> f64 {
    // Whole microseconds, rounded down.
    let micros = (now - then).as_nanos().div_euclid(1_000);
    let seconds = micros as f64 / 1e6;
    seconds / 86_400.0
}

/// `17·(2/17)^(d/15) − 2`, d clamped at 0: 15 today, 0 after 15 days, towards −2.
pub fn recency_points(days: f64) -> f64 {
    17.0 * (2.0_f64 / 17.0).powf(days.max(0.0) / 15.0) - 2.0
}

/// `12.5·0.8^(lines/1000)` of the full diff.
pub fn change_size_points(lines: u64) -> f64 {
    12.5 * 0.8_f64.powf(lines as f64 / 1000.0)
}

/// Display size marks by changed lines (additions + deletions), the lower bound of each mark (plan §C.2 K16): a
/// fixed table set by the owner, 2026-11-18. Display only: not a score part, not the judge's size class.
pub const SIZE_MARKS: [(u64, SizeMark); 5] = [
    (0, SizeMark::Tiny),
    (150, SizeMark::Small),
    (500, SizeMark::Medium),
    (3000, SizeMark::Large),
    (10_000, SizeMark::Huge),
];

/// The size mark of a pull request with its evidence; `None` when the diff totals are unknown.
pub fn size_mark(pr: &PrFacts) -> Option<ChangeSizeMark> {
    let (additions, deletions) = (pr.additions?, pr.deletions?);
    let lines = u64::from(additions) + u64::from(deletions);
    let size = SIZE_MARKS
        .iter()
        .rev()
        .find(|(from, _)| lines >= *from)
        .map_or(SizeMark::Tiny, |(_, mark)| *mark);
    Some(ChangeSizeMark {
        size,
        additions,
        deletions,
    })
}

/// Age of an outstanding request: ≤2 d 15, ≤7 d 10, older 5.
pub fn waiting_points(age_days: f64) -> f64 {
    if age_days <= 2.0 {
        15.0
    } else if age_days <= 7.0 {
        10.0
    } else {
        5.0
    }
}

/// 5 per distinct open dependent, at most 15.
pub fn unblocks_points(dependents: usize) -> f64 {
    dependents.min(3) as f64 * 5.0
}

fn recency(updated_at: Timestamp, now: Timestamp, what: &str) -> Part {
    let days = days_between(updated_at, now).max(0.0);
    Part::computed(
        C::Recency,
        recency_points(days),
        format!("{what} {days:.1} d ago"),
    )
}

fn blocked_label(labels: &[String]) -> bool {
    labels.iter().any(|label| text::has_word(label, "blocked"))
}

/// A `blocked` label or blocked-by/depends-on wording in the body.
pub(super) fn blocker_wording(labels: &[String], body: &str) -> bool {
    blocked_label(labels) || text::has_blocker_wording(body)
}

const BLOCKER_TEXT: &str =
    "`blocked` label or blocked-by/depends-on text: −20 only if the blocker is verified open";

fn defect_estimate(title: &str, bug: bool) -> Part {
    if bug || text::has_fix_wording(title) {
        Part::estimate(C::DefectSeverity, 8.0, "fix or bug wording")
    } else {
        Part::estimate(C::DefectSeverity, 0.0, "no fix or bug wording")
    }
}

fn impact_estimate(kind: Option<&str>) -> Part {
    let points = match kind {
        Some("docs" | "chore" | "ci" | "test" | "style" | "build") => 0.0,
        Some("refactor" | "perf") => 5.0,
        _ => 10.0,
    };
    let rule = kind.map_or("no conventional type".to_owned(), |k| {
        format!("conventional type `{k}`")
    });
    Part::estimate(C::ChangeImpact, points, &rule)
}

fn urgency_estimate() -> Part {
    Part::estimate(C::UrgencyBonus, 0.0, "no exposure evidence checked")
}

fn milestone_part(milestone: &Milestone, context: &MilestoneContext) -> Part {
    const UNKNOWN: &str = "milestone mapping unknown: +20 current or past release, +10 next existing milestone, \
                           −10 later, 0 none";
    let unknown = Part::pending(C::MilestoneBonus, PartSource::Llm, UNKNOWN);
    let title = match milestone {
        Milestone::Unknown => return unknown,
        Milestone::Unassigned => {
            return Part::computed(C::MilestoneBonus, 0.0, "no assigned milestone");
        }
        Milestone::Assigned(title) => title,
    };
    let Some(target) = text::version(title, true) else {
        return unknown;
    };
    let tags = context.release_tags.as_deref().unwrap_or_default();
    let released: Option<Vec<_>> = tags.iter().map(|tag| text::version(tag, false)).collect();
    let Some(current) = released.and_then(|r| r.into_iter().max()) else {
        return unknown;
    };
    let dotted = |(a, b, c): (u64, u64, u64)| format!("{a}.{b}.{c}");
    let basis = format!(
        "milestone `{title}`, highest stable release {}",
        dotted(current)
    );
    if target <= current {
        return Part::computed(
            C::MilestoneBonus,
            20.0,
            format!("{basis}: current or past release"),
        );
    }
    let titles = context.milestone_titles.as_deref().unwrap_or_default();
    let versions: Option<Vec<_>> = titles.iter().map(|t| text::version(t, true)).collect();
    let Some(versions) = versions.filter(|v| v.contains(&target)) else {
        return unknown;
    };
    let next = versions
        .into_iter()
        .filter(|v| *v > current)
        .min()
        .unwrap_or(target);
    let points = if target == next { 10.0 } else { -10.0 };
    Part::computed(
        C::MilestoneBonus,
        points,
        format!("{basis}; next existing milestone {}", dotted(next)),
    )
}

/// Whether the item is ranked, with the reason; `confirmed` is false when the LLM should check it (plan Q3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Eligibility {
    /// The rule that leaves the item out; `None` when eligible.
    pub excluded_by: Option<ExclusionReason>,
    pub reason: String,
    pub confirmed: bool,
}

impl Eligibility {
    fn yes(reason: &str) -> Self {
        Self {
            excluded_by: None,
            reason: reason.into(),
            confirmed: true,
        }
    }

    fn no(rule: ExclusionReason, reason: String) -> Self {
        Self {
            excluded_by: Some(rule),
            reason,
            confirmed: true,
        }
    }
}

/// An excluded author's item is always left out (K7), whatever its relation to the user.
fn skipped(rules: &ScoreRules, author: Option<&str>) -> Option<Eligibility> {
    author.filter(|a| rules.is_skipped_author(a)).map(|a| {
        Eligibility::no(
            ExclusionReason::ExcludedAuthor,
            format!("author @{a} is excluded"),
        )
    })
}

/// Skill `pr_eligibility` with the excluded authors (K7) and per-repository ownership policies (K4).
pub(super) fn pr_eligibility(rules: &ScoreRules, pr: &PrFacts) -> Eligibility {
    let author = pr.author.as_deref().unwrap_or("?");
    if let Some(no) = skipped(rules, pr.author.as_deref()) {
        return no;
    }
    if pr.is_draft && !rules.is_scanned(author) {
        return Eligibility::no(
            ExclusionReason::Draft,
            format!("draft by @{author} (only our own drafts count)"),
        );
    }
    // The ownership policy filters only what is not directed at the user (D19): authored by, assigned to or a
    // review requested directly from a scanned login. A team-only request stays filtered: like human waiting, the
    // scorer treats it as a hint only.
    let directed = rules.is_scanned(author)
        || pr.relations.contains(&Relation::Assigned)
        || pr.review_pending_for_us;
    if directed
        && matches!(
            pr.owned_change,
            OwnedChange::None | OwnedChange::Minor | OwnedChange::Unknown
        )
    {
        return Eligibility::yes(
            "directed at you (the ownership policy applies only to other pull requests)",
        );
    }
    match pr.owned_change {
        OwnedChange::Significant => Eligibility::yes("significant change in owned paths"),
        OwnedChange::None => Eligibility::no(
            ExclusionReason::OutsideOwnedAreas,
            "no change in owned paths (ownership policy)".into(),
        ),
        OwnedChange::Minor => Eligibility::no(
            ExclusionReason::OutsideOwnedAreas,
            "only a minor change in owned paths (ownership policy)".into(),
        ),
        OwnedChange::Unknown => Eligibility {
            excluded_by: None,
            reason: "ownership policy unavailable: change in owned paths unverified".into(),
            confirmed: false,
        },
        OwnedChange::NoPolicy if pr.is_draft => Eligibility::yes("draft by us"),
        OwnedChange::NoPolicy => Eligibility::yes("open non-draft PR"),
    }
}

/// Issues are ranked unless their author is excluded (K7).
pub(super) fn issue_eligibility(rules: &ScoreRules, issue: &IssueFacts) -> Eligibility {
    skipped(rules, issue.author.as_deref()).unwrap_or_else(|| Eligibility::yes("open issue"))
}

fn advancement(rules: &ScoreRules, pr: &PrFacts) -> Part {
    if pr.is_draft {
        Part::computed(C::Advancement, 15.0, "draft PR")
    } else if pr.labels.iter().any(|l| rules.is_ready_label(l)) {
        Part::to_confirm(
            C::Advancement,
            30.0,
            "ready label (stale after a newer unaddressed bot review → 20)",
        )
    } else {
        Part::computed(C::Advancement, 20.0, "open non-draft PR")
    }
}

fn unblocks(dependents: Option<&[u32]>) -> Part {
    let Some(deps) = dependents else {
        return Part::pending(
            C::Unblocks,
            PartSource::Computed,
            "dependents lookup failed",
        );
    };
    let found = if deps.is_empty() {
        "none".to_owned()
    } else {
        deps.iter()
            .map(|n| format!("#{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let basis = format!("same-repo PRs stacked on it: {found} (other dependents not checked)");
    Part::computed(C::Unblocks, unblocks_points(deps.len()), basis)
}

/// Skill `_human_waiting`: the freshest outstanding human request; unsure cases are defaults to confirm.
fn human_waiting(rules: &ScoreRules, pr: &PrFacts, now: Timestamp) -> Part {
    let author = pr.author.as_deref().unwrap_or_default();
    let author_human =
        !author.is_empty() && !rules.is_scanned(author) && !rules.is_bot(author, pr.author_is_bot);
    let age = |at: Option<Timestamp>| at.map(|t| days_between(t, now));
    let authored = pr.relations.contains(&Relation::Authored);
    let mut definite: Vec<(f64, String)> = Vec::new();
    let mut hints: Vec<String> = Vec::new();
    let mut approx = None;
    if pr.review_pending_for_us && author_human {
        match age(pr.review_requested_at) {
            Some(days) if !pr.review_request_age_is_approx => {
                definite.push((
                    days,
                    format!("review requested by @{author} {days:.1} d ago"),
                ));
            }
            days => approx = Some(days.unwrap_or(999.0)),
        }
    } else if pr.relations.contains(&Relation::ReviewRequested) && author_human {
        hints.push(format!("review requested through a team on @{author}'s PR: if they wait on us, score by request age"));
    }
    let cr_age = age(pr.changes_requested_human_at);
    let rerequested = authored && pr.changes_requested_by_human && cr_age.is_none();
    match cr_age {
        Some(days) if authored && pr.changes_requested_by_human => {
            definite.push((days, format!("human change request {days:.1} d ago")));
        }
        _ if authored
            && pr.review_decision == Some(ReviewDecision::ChangesRequested)
            && !pr.changes_requested_by_human
            && (pr.reviews_truncated || pr.changes_requested_by.is_empty()) =>
        {
            hints.push("changes requested but no change request among the last 20 reviews: verify; a human one scores by age".into());
        }
        _ => {}
    }
    let mut value = 0.0;
    let mut basis = if rerequested {
        "human change request re-requested: waiting on the reviewer".to_owned()
    } else {
        "no outstanding human request".to_owned()
    };
    if let Some((days, why)) = definite
        .into_iter()
        .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
    {
        value = waiting_points(days);
        basis = why;
    }
    if let Some(days) = approx.filter(|d| waiting_points(*d) > value) {
        value = waiting_points(days);
        basis = format!("review requested by @{author}, about {days:.1} d ago (from updatedAt)");
        hints.push("approximate request age: verify the request date in the timeline".into());
    }
    if let Some(days) =
        age(pr.last_comment_at).filter(|d| pr.awaiting_our_reply && waiting_points(*d) > value)
    {
        let who = pr.last_comment_author.as_deref().unwrap_or("?");
        hints.push(format!(
            "last comment by @{who} {days:.1} d ago unanswered: if it asks us something → {}",
            show(waiting_points(days))
        ));
    }
    if hints.is_empty() {
        Part::computed(C::HumanWaiting, value, basis)
    } else {
        Part::to_confirm(
            C::HumanWaiting,
            value,
            &format!("{basis}; {}", hints.join("; ")),
        )
    }
}

fn change_size(pr: &PrFacts) -> Part {
    match (pr.additions, pr.deletions) {
        (Some(add), Some(del)) => {
            let lines = u64::from(add) + u64::from(del);
            let basis = format!("{lines} changed lines (+{add}/−{del}, full diff)");
            Part::computed(C::ChangeSize, change_size_points(lines), basis)
        }
        _ => Part::pending(
            C::ChangeSize,
            PartSource::Computed,
            "diff totals unavailable",
        ),
    }
}

fn completion_estimate(pr: &PrFacts) -> Part {
    if pr.ci == CiEffective::Green
        && pr.mergeable == Mergeable::Mergeable
        && !pr.is_draft
        && pr.unresolved_threads == 0
    {
        Part::estimate(
            C::Completion,
            5.0,
            "CI green, mergeable, not a draft, no unresolved threads",
        )
    } else {
        Part::estimate(
            C::Completion,
            0.0,
            "CI not green, conflicts, draft or unresolved threads",
        )
    }
}

fn blocked(labels: &[String], body: &str, stacked_on: Option<&[u32]>, base: Option<&str>) -> Part {
    match stacked_on {
        Some(stack) if !stack.is_empty() => {
            let nums = stack
                .iter()
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join(", ");
            let basis = format!("stacked on open {nums} (base `{}`)", base.unwrap_or("?"));
            Part::computed(C::BlockedPenalty, -20.0, basis)
        }
        _ if blocker_wording(labels, body) => {
            Part::pending(C::BlockedPenalty, PartSource::Llm, BLOCKER_TEXT)
        }
        None => Part::pending(
            C::BlockedPenalty,
            PartSource::Llm,
            "base PR lookup failed: −20 if stacked on an open PR",
        ),
        Some(_) => Part::computed(
            C::BlockedPenalty,
            0.0,
            "no open base PR, no blocked label or dependency text",
        ),
    }
}

fn default_branch(pr: &PrFacts) -> Part {
    match (pr.base_ref.as_deref(), pr.default_branch.as_deref()) {
        (Some(base), Some(default)) => {
            let points = if base == default { 5.0 } else { 0.0 };
            Part::computed(
                C::DefaultBranchBonus,
                points,
                format!("base `{base}`, repository default `{default}`"),
            )
        }
        _ => Part::pending(
            C::DefaultBranchBonus,
            PartSource::Computed,
            "base or default branch unknown",
        ),
    }
}

/// The twelve parts of a pull request before judgement (skill `pr_score_parts` + K13 estimates).
pub(super) fn pr_parts(rules: &ScoreRules, pr: &PrFacts, now: Timestamp) -> Vec<Part> {
    let kind = text::conventional_type(&pr.title).map(|(kind, _)| kind);
    let bug =
        kind.as_deref() == Some("fix") || pr.labels.iter().any(|l| l.eq_ignore_ascii_case("bug"));
    vec![
        advancement(rules, pr),
        defect_estimate(&pr.title, bug),
        unblocks(pr.dependents.as_deref()),
        human_waiting(rules, pr, now),
        recency(pr.updated_at, now, "updated"),
        impact_estimate(kind.as_deref()),
        completion_estimate(pr),
        change_size(pr),
        urgency_estimate(),
        blocked(
            &pr.labels,
            &pr.body,
            pr.stacked_on.as_deref(),
            pr.base_ref.as_deref(),
        ),
        default_branch(pr),
        milestone_part(&pr.milestone, &pr.milestone_context),
    ]
}

/// The twelve parts of an issue before judgement (skill `issue_score_parts`; J6 defaults to confirm).
pub(super) fn issue_parts(issue: &IssueFacts, now: Timestamp) -> Vec<Part> {
    let bug = issue.labels.iter().any(|l| l.eq_ignore_ascii_case("bug"));
    let blocked = if blocker_wording(&issue.labels, &issue.body) {
        Part::pending(C::BlockedPenalty, PartSource::Llm, BLOCKER_TEXT)
    } else {
        Part::computed(
            C::BlockedPenalty,
            0.0,
            "no blocked label or blocked-by/depends-on text",
        )
    };
    vec![
        Part::computed(C::Advancement, 0.0, "standalone issue: unstarted work"),
        defect_estimate(&issue.title, bug),
        Part::to_confirm(C::Unblocks, 0.0, "no known dependents"),
        Part::to_confirm(C::HumanWaiting, 0.0, "no outstanding human request known"),
        recency(issue.updated_at, now, "updated"),
        impact_estimate(None),
        Part::estimate(C::Completion, 0.0, "issue without a pull request"),
        Part::not_applicable(C::ChangeSize, "issue: no diff"),
        urgency_estimate(),
        blocked,
        Part::not_applicable(C::DefaultBranchBonus, "issue: no target branch"),
        milestone_part(&issue.milestone, &issue.milestone_context),
    ]
}

/// The twelve parts of a MemCan TODO before judgement (skill `todos.py` `score_row`, J13).
pub(super) fn todo_parts(todo: &TodoFacts, now: Timestamp) -> Vec<Part> {
    let (advancement, status) = match todo.status {
        TodoStatus::InProgress => (10.0, "in_progress"),
        TodoStatus::Pending => (0.0, "pending"),
        TodoStatus::Blocked => (0.0, "blocked"),
        TodoStatus::Postponed => (0.0, "postponed"),
    };
    let bug = todo.title.contains("[HIGH]");
    let priority = todo.priority.as_deref().unwrap_or("unset").to_lowercase();
    let impact = match priority.as_str() {
        "high" => 10.0,
        "medium" => 5.0,
        _ => 0.0,
    };
    let blocked = if todo.status == TodoStatus::Blocked {
        Part::to_confirm(
            C::BlockedPenalty,
            -20.0,
            "MemCan status blocked, blocker unverified",
        )
    } else {
        Part::to_confirm(C::BlockedPenalty, 0.0, "no verified open blocker")
    };
    vec![
        Part::computed(C::Advancement, advancement, format!("TODO status {status}")),
        defect_estimate(&todo.title, bug),
        Part::to_confirm(C::Unblocks, 0.0, "no verified dependents"),
        Part::computed(
            C::HumanWaiting,
            0.0,
            "no outstanding human request on a TODO",
        ),
        recency(todo.changed_at, now, "changed"),
        Part::estimate(
            C::ChangeImpact,
            impact,
            &format!("MemCan priority {priority}"),
        ),
        Part::estimate(C::Completion, 0.0, "steps remain or unknown"),
        Part::not_applicable(C::ChangeSize, "TODO: no diff"),
        urgency_estimate(),
        blocked,
        Part::not_applicable(C::DefaultBranchBonus, "TODO: no target branch"),
        Part::not_applicable(C::MilestoneBonus, "standalone TODO: no milestone"),
    ]
}
