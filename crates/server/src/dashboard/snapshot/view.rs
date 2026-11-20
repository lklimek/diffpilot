//! The ranked items and the snapshot facts of R1/R1b, computed from a stored snapshot and the judgements present
//! when read. Tags and facts come from scan facts only; LLM text only from a judgement.

use std::collections::HashMap;

use contracts::api::dashboard::{
    AwaitingItem, EXCLUDED_ITEMS_LIMIT, ExcludedItem, ExclusionCount, Exclusions, Fact, FactTag,
    ItemCounts, ItemKey, ItemKind, ItemTarget, ItemText, RankingState, ReadyToMergeItem,
    ScoreState, SnapshotMeta, Tier, WorkItem,
};
use contracts::common::RepoRef;
use contracts::time::Timestamp;
use db::repo::dashboard::RefreshRecord;
use jiff::SignedDuration;
use jiff::tz::TimeZone;

use super::{Snapshot, StoredIssue, StoredPr, StoredTodo};
use crate::dashboard::refresh::rank::ItemJudgement;
use crate::dashboard::score::{
    self, CiEffective, Judgement, MergeState, Mergeable, Milestone, Relation, ScoreError,
    ScoreRules, Scored, TodoStatus, awaiting_others, merge_ready, todo_display_ids,
};

/// A ranked item with what the request-time review link needs.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewItem {
    /// The item (without `review`).
    pub item: WorkItem,
    /// Head commit at scan time, for pull requests.
    pub head_sha: Option<String>,
}

/// Ranked items in rank order and the facts derived from them.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    /// Ranked items.
    pub items: Vec<ViewItem>,
    /// Counts by kind and tier.
    pub item_counts: ItemCounts,
    /// Items left out of the ranking: counts per rule and the capped list.
    pub exclusions: Exclusions,
    /// The user's pull requests waiting on others.
    pub awaiting: Vec<AwaitingItem>,
    /// The user's pull requests GitHub lets the user merge now.
    pub ready_to_merge: Vec<ReadyToMergeItem>,
    /// `SnapshotMeta.merge_state_pending`.
    pub merge_state_pending: Option<u32>,
}

impl View {
    /// `SnapshotMeta` of the snapshot held by `record`; the excluded items are listed only when `list_excluded`.
    pub fn meta(
        &self,
        snapshot: &Snapshot,
        record: &RefreshRecord,
        list_excluded: bool,
    ) -> SnapshotMeta {
        let exclusions = Exclusions {
            items: self.exclusions.items.clone().filter(|_| list_excluded),
            ..self.exclusions.clone()
        };
        SnapshotMeta {
            refresh_id: record.id,
            scanned_at: snapshot.scanned_at,
            ranked_at: (record.ranking_state == Some(RankingState::Finished))
                .then_some(record.finished_at)
                .flatten(),
            item_counts: self.item_counts,
            excluded: self.exclusions.total,
            awaiting: self.awaiting.clone(),
            ready_to_merge: self.ready_to_merge.clone(),
            merge_state_pending: self.merge_state_pending,
            history: snapshot.history.clone(),
            todos: snapshot.todos_state.clone(),
            notes: snapshot.notes.clone(),
            unreachable: snapshot.unreachable.clone(),
            checked: snapshot.checked,
            exclusions: Some(exclusions),
        }
    }
}

enum Source<'a> {
    Pr(&'a StoredPr),
    Issue(&'a StoredIssue),
    Todo(&'a StoredTodo),
}

/// Scores and ranks the snapshot's items with `judgements` at its scan time. `memcan_ui` is `SR_MEMCAN_UI_URL`.
pub fn build(
    snapshot: &Snapshot,
    judgements: &HashMap<ItemKey, ItemJudgement>,
    memcan_ui: Option<&str>,
) -> View {
    let rules = ScoreRules::new(&snapshot.own_login, &snapshot.settings);
    let at = snapshot.scanned_at;
    let look_back = LookBack {
        days: snapshot.settings.lookback_days,
        since: at
            .checked_add(-SignedDuration::from_hours(
                24 * i64::from(snapshot.settings.lookback_days),
            ))
            .unwrap_or(Timestamp::UNIX_EPOCH),
    };
    let todo_ids = todo_display_ids(
        &snapshot
            .todos
            .iter()
            .map(|t| t.facts.id)
            .collect::<Vec<_>>(),
    );
    let mut scored = Vec::new();
    let mut sources: HashMap<ItemKey, Source<'_>> = HashMap::new();
    // A PR waiting only on others is listed under Waiting on others alone (B10).
    for pr in snapshot
        .pull_requests
        .iter()
        .filter(|pr| !score::waits_on_others(&rules, &pr.facts))
    {
        let key = item_key(ItemTarget::PullRequest {
            repo: pr.facts.repo.clone(),
            number: pr.facts.number,
        });
        let item = scored_with(key.as_ref(), judgements, |j| {
            score::score_pr(&rules, &pr.facts, j, at)
        });
        push(item, Source::Pr(pr), &mut scored, &mut sources);
    }
    for issue in &snapshot.issues {
        let key = item_key(ItemTarget::Issue {
            repo: issue.facts.repo.clone(),
            number: issue.facts.number,
        });
        let item = scored_with(key.as_ref(), judgements, |j| {
            score::score_issue(&rules, &issue.facts, j, at)
        });
        push(item, Source::Issue(issue), &mut scored, &mut sources);
    }
    for todo in &snapshot.todos {
        let key = item_key(ItemTarget::Todo(todo.facts.id));
        let id = todo_ids
            .get(&todo.facts.id)
            .cloned()
            .unwrap_or_else(|| format!("TODO-{}", todo.facts.id));
        let item = scored_with(key.as_ref(), judgements, |j| {
            score::score_todo(&todo.facts, id.clone(), j, at)
        });
        push(item, Source::Todo(todo), &mut scored, &mut sources);
    }
    let ranking = score::rank(scored);
    let mut counts = ItemCounts {
        total: 0,
        pr: 0,
        issue: 0,
        todo: 0,
        p0: 0,
        p1: 0,
        p2: 0,
        p3: 0,
        not_judged: 0,
    };
    let mut items = Vec::with_capacity(ranking.ranked.len());
    for item in ranking.ranked {
        let Some(source) = sources.get(&item.key) else {
            continue;
        };
        count(&mut counts, &item);
        let text = judgements
            .get(&item.key)
            .and_then(|judged| judged.text.clone());
        items.push(work_item(&rules, &look_back, item, source, text, memcan_ui));
    }
    let awaiting = awaiting(&rules, snapshot);
    let ready_to_merge = ready_to_merge(&rules, snapshot);
    let merge_state_pending = snapshot.merge_states_recorded.then(|| {
        let pending = snapshot
            .pull_requests
            .iter()
            .filter(|pr| score::merge_state_pending(&rules, &pr.facts))
            .count();
        u32::try_from(pending).unwrap_or(u32::MAX)
    });
    for left_out in &ranking.excluded {
        tracing::debug!(item = %left_out.id, reason = %left_out.reason, "dashboard item left out");
    }
    View {
        items,
        item_counts: counts,
        exclusions: exclusions(ranking.excluded, &sources, memcan_ui),
        awaiting,
        ready_to_merge,
        merge_state_pending,
    }
}

/// Counts per rule and the first [`EXCLUDED_ITEMS_LIMIT`] items by rule, then display id (B4).
fn exclusions(
    mut left_out: Vec<score::Excluded>,
    sources: &HashMap<ItemKey, Source<'_>>,
    memcan_ui: Option<&str>,
) -> Exclusions {
    left_out.sort_by_key(|e| e.rule);
    let mut counts: Vec<ExclusionCount> = Vec::new();
    for item in &left_out {
        match counts.last_mut() {
            Some(last) if last.reason == item.rule => last.count = last.count.saturating_add(1),
            _ => counts.push(ExclusionCount {
                reason: item.rule,
                count: 1,
            }),
        }
    }
    let items = left_out
        .into_iter()
        .filter_map(|e| {
            let source = sources.get(&e.key)?;
            Some(excluded_item(e, source, memcan_ui))
        })
        .take(EXCLUDED_ITEMS_LIMIT)
        .collect();
    Exclusions {
        total: counts.iter().map(|c| c.count).sum(),
        counts,
        items: Some(items),
    }
}

fn excluded_item(e: score::Excluded, source: &Source<'_>, memcan_ui: Option<&str>) -> ExcludedItem {
    let (kind, repo, number, url, title, author) = match source {
        Source::Pr(pr) => {
            let f = &pr.facts;
            (
                ItemKind::Pr,
                Some(f.repo.clone()),
                Some(f.number),
                Some(github_url(&f.repo, "pull", f.number)),
                f.title.clone(),
                f.author.clone(),
            )
        }
        Source::Issue(issue) => {
            let f = &issue.facts;
            (
                ItemKind::Issue,
                Some(f.repo.clone()),
                Some(f.number),
                Some(github_url(&f.repo, "issues", f.number)),
                f.title.clone(),
                f.author.clone(),
            )
        }
        Source::Todo(todo) => (
            ItemKind::Todo,
            None,
            None,
            todo_url(memcan_ui, todo),
            todo.facts.title.clone(),
            None,
        ),
    };
    ExcludedItem {
        key: e.key,
        kind,
        id: e.id,
        repo,
        number,
        url,
        title,
        author,
        reason: e.rule,
        merged_into: e.merged_into,
    }
}

fn todo_url(memcan_ui: Option<&str>, todo: &StoredTodo) -> Option<String> {
    memcan_ui.map(|base| format!("{}/tasks/{}", base.trim_end_matches('/'), todo.facts.id))
}

fn item_key(target: ItemTarget) -> Option<ItemKey> {
    ItemKey::new(&target).ok()
}

fn push<'a>(
    item: Option<Scored>,
    source: Source<'a>,
    scored: &mut Vec<Scored>,
    sources: &mut HashMap<ItemKey, Source<'a>>,
) {
    if let Some(item) = item {
        sources.insert(item.key.clone(), source);
        scored.push(item);
    }
}

/// Scores with the item's judgement; a judgement that does not fit leaves the item unjudged (plan §B.2: it stays
/// provisional and is logged). An item that cannot be scored at all is left out.
fn scored_with(
    key: Option<&ItemKey>,
    judgements: &HashMap<ItemKey, ItemJudgement>,
    score: impl Fn(Option<&Judgement>) -> Result<Scored, ScoreError>,
) -> Option<Scored> {
    let judgement = key
        .and_then(|key| judgements.get(key))
        .map(|j| &j.judgement);
    match score(judgement) {
        Ok(scored) => Some(scored),
        Err(error) if judgement.is_some() => {
            tracing::warn!(%error, item = ?key, "stored dashboard judgement does not fit the item; scored without it");
            score(None).ok()
        }
        Err(error) => {
            tracing::warn!(%error, item = ?key, "dashboard item could not be scored; left out");
            None
        }
    }
}

fn count(counts: &mut ItemCounts, item: &Scored) {
    counts.total += 1;
    match item.kind {
        ItemKind::Pr => counts.pr += 1,
        ItemKind::Issue => counts.issue += 1,
        ItemKind::Todo => counts.todo += 1,
    }
    match item.score.tier {
        Tier::P0 => counts.p0 += 1,
        Tier::P1 => counts.p1 += 1,
        Tier::P2 => counts.p2 += 1,
        Tier::P3 => counts.p3 += 1,
    }
    if item.score.state == ScoreState::Unjudged {
        counts.not_judged += 1;
    }
}

fn tag(text: impl Into<String>, tip: impl Into<String>) -> FactTag {
    FactTag {
        text: text.into(),
        tip: tip.into(),
    }
}

fn fact(label: &str, value: impl Into<String>) -> Fact {
    Fact {
        label: label.to_owned(),
        value: value.into(),
    }
}

/// GitHub link of a pull request or issue, built from repository and number only.
pub(crate) fn github_url(repo: &RepoRef, kind: &str, number: u32) -> String {
    format!(
        "https://github.com/{}/{}/{kind}/{number}",
        repo.owner, repo.name
    )
}

fn milestone_fact(milestone: &Milestone) -> Option<Fact> {
    match milestone {
        Milestone::Assigned(title) => Some(fact("Milestone", title.clone())),
        Milestone::Unknown => Some(fact("Milestone", "could not be read")),
        Milestone::Unassigned => None,
    }
}

/// The look-back setting (G9) at the scan: an issue opened since `since` is new.
struct LookBack {
    days: u32,
    since: Timestamp,
}

fn work_item(
    rules: &ScoreRules,
    look_back: &LookBack,
    scored: Scored,
    source: &Source<'_>,
    llm: Option<ItemText>,
    memcan_ui: Option<&str>,
) -> ViewItem {
    let (repo, number, title, url, author, private, updated_at, tags, facts, head_sha) =
        match source {
            Source::Pr(pr) => {
                let f = &pr.facts;
                (
                    Some(f.repo.clone()),
                    Some(f.number),
                    f.title.clone(),
                    Some(github_url(&f.repo, "pull", f.number)),
                    f.author.clone(),
                    pr.private,
                    f.updated_at,
                    pr_tags(rules, pr),
                    pr_facts(pr),
                    Some(pr.head_sha.clone()),
                )
            }
            Source::Issue(issue) => {
                let f = &issue.facts;
                (
                    Some(f.repo.clone()),
                    Some(f.number),
                    f.title.clone(),
                    Some(github_url(&f.repo, "issues", f.number)),
                    f.author.clone(),
                    issue.private,
                    f.updated_at,
                    issue_tags(&f.relations, issue.opened_at, look_back),
                    issue_facts(issue),
                    None,
                )
            }
            Source::Todo(todo) => (
                None,
                None,
                todo.facts.title.clone(),
                todo_url(memcan_ui, todo),
                None,
                false,
                todo.facts.changed_at,
                todo_tags(todo),
                todo_facts(todo),
                None,
            ),
        };
    ViewItem {
        item: WorkItem {
            id: scored.id,
            key: scored.key,
            kind: scored.kind,
            repo,
            number,
            title,
            url,
            author,
            private,
            updated_at,
            tags,
            change_size: match source {
                Source::Pr(pr) => score::size_mark(&pr.facts),
                Source::Issue(_) | Source::Todo(_) => None,
            },
            facts,
            score: scored.score,
            llm,
            review: None,
        },
        head_sha,
    }
}

fn ci_tag(pr: &StoredPr) -> Option<FactTag> {
    match pr.facts.ci {
        CiEffective::Green => Some(tag(
            "CI passing",
            "Every build, test and lint check on the head commit passed",
        )),
        CiEffective::Red => Some(tag(
            "CI failing",
            format!("Failing: {}", pr.ci_red.join(", ")),
        )),
        CiEffective::Pending => Some(tag(
            "CI running",
            if pr.ci_pending.is_empty() {
                "Checks have not reported yet".to_owned()
            } else {
                format!("Running: {}", pr.ci_pending.join(", "))
            },
        )),
        CiEffective::Unknown => Some(tag(
            "CI partly read",
            "More checks exist than were read; none of those read failed",
        )),
        CiEffective::None => None,
    }
}

fn pr_tags(rules: &ScoreRules, pr: &StoredPr) -> Vec<FactTag> {
    let f = &pr.facts;
    let mut tags = Vec::new();
    if f.relations.contains(&Relation::ReviewRequested) {
        tags.push(tag(
            "review requested",
            "Your review is requested, directly or through a team",
        ));
    }
    if f.relations.contains(&Relation::Assigned) {
        tags.push(tag("assigned to you", "You are an assignee of this PR"));
    }
    if f.is_draft {
        tags.push(tag("draft", "The PR is a draft"));
    }
    tags.extend(ci_tag(pr));
    if f.mergeable == Mergeable::Conflicting {
        tags.push(tag("conflicts", "GitHub reports merge conflicts"));
    }
    if !f.changes_requested_by.is_empty() {
        tags.push(tag(
            "changes requested",
            format!("Requested by {}", f.changes_requested_by.join(", ")),
        ));
    }
    if f.unresolved_threads > 0 {
        tags.push(tag(
            format!("{} unresolved threads", f.unresolved_threads),
            "Review threads not resolved on GitHub",
        ));
    }
    if f.labels.iter().any(|l| rules.is_ready_label(l)) {
        tags.push(tag("ready label", "Marked ready for a human review"));
    }
    tags
}

fn pr_facts(pr: &StoredPr) -> Vec<Fact> {
    let f = &pr.facts;
    let mut facts = Vec::new();
    if let Some(base) = &f.base_ref {
        facts.push(fact("Base branch", base.clone()));
    }
    if let (Some(added), Some(removed)) = (f.additions, f.deletions) {
        facts.push(fact("Size", format!("+{added} −{removed} lines")));
    }
    if !f.labels.is_empty() {
        facts.push(fact("Labels", f.labels.join(", ")));
    }
    facts.extend(milestone_fact(&f.milestone));
    if !f.approved_by.is_empty() {
        facts.push(fact("Approved by", f.approved_by.join(", ")));
    }
    if !f.review_requested_from_others.is_empty() {
        facts.push(fact(
            "Waiting on",
            f.review_requested_from_others.join(", "),
        ));
    }
    facts
}

fn issue_tags(
    relations: &[Relation],
    opened_at: Option<Timestamp>,
    look_back: &LookBack,
) -> Vec<FactTag> {
    let mut tags = Vec::new();
    if let Some(opened) = opened_at.filter(|at| *at >= look_back.since) {
        tags.push(tag(
            "new",
            format!(
                "Opened {} (UTC), within your look-back of {} {}",
                opened.to_zoned(TimeZone::UTC).date(),
                look_back.days,
                if look_back.days == 1 { "day" } else { "days" }
            ),
        ));
    }
    if relations.contains(&Relation::Assigned) {
        tags.push(tag("assigned to you", "You are an assignee of this issue"));
    }
    if relations.contains(&Relation::Mentioned) {
        tags.push(tag("mentions you", "The issue or a comment mentions you"));
    }
    if relations.contains(&Relation::RepoOpen) {
        tags.push(tag(
            "watched repository",
            "Open issue of a repository whose issues you watch",
        ));
    }
    tags
}

fn issue_facts(issue: &StoredIssue) -> Vec<Fact> {
    let f = &issue.facts;
    let mut facts = vec![fact("Comments", issue.comments.to_string())];
    if !f.labels.is_empty() {
        facts.push(fact("Labels", f.labels.join(", ")));
    }
    facts.extend(milestone_fact(&f.milestone));
    facts
}

fn todo_tags(todo: &StoredTodo) -> Vec<FactTag> {
    let status = match todo.facts.status {
        TodoStatus::Pending => "pending",
        TodoStatus::InProgress => "in progress",
        TodoStatus::Blocked => "blocked",
        TodoStatus::Postponed => "postponed",
    };
    vec![
        tag(todo.project.clone(), "MemCan project"),
        tag(status, "MemCan status"),
    ]
}

fn todo_facts(todo: &StoredTodo) -> Vec<Fact> {
    let mut facts = Vec::new();
    if let Some(priority) = &todo.facts.priority {
        facts.push(fact("Priority", priority.clone()));
    }
    if let Some(owner) = &todo.owner {
        facts.push(fact("Owner", owner.clone()));
    }
    if todo.blocked_by > 0 {
        facts.push(fact("Blocked by", format!("{} TODOs", todo.blocked_by)));
    }
    if let Some(description) = &todo.description {
        facts.push(fact("Description", description.clone()));
    }
    facts
}

/// The user's ready PRs waiting on others, longest wait first. `waiting_since` is the oldest open request to them;
/// when the timeline did not show it (older than its last 50 events, or a team request), the PR's creation time.
fn awaiting(rules: &ScoreRules, snapshot: &Snapshot) -> Vec<AwaitingItem> {
    let facts: Vec<_> = snapshot
        .pull_requests
        .iter()
        .map(|pr| pr.facts.clone())
        .collect();
    awaiting_others(rules, &facts)
        .into_iter()
        .filter_map(|f| {
            let stored = snapshot
                .pull_requests
                .iter()
                .find(|pr| pr.facts.repo == f.repo && pr.facts.number == f.number)?;
            Some(AwaitingItem {
                id: rules.display_id(&f.repo, f.number),
                repo: f.repo.clone(),
                number: f.number,
                title: f.title.clone(),
                url: github_url(&f.repo, "pull", f.number),
                waiting_on: f.review_requested_from_others.clone(),
                waiting_since: waiting_since(f.others_review_requested_at, stored.created_at),
                tags: ci_tag(stored).into_iter().collect(),
                change_size: score::size_mark(f),
                // Never ranked (B10).
                total_tenths: None,
            })
        })
        .collect()
}

/// `AwaitingItem.waiting_since`: the oldest open request to another reviewer (each login's newest request
/// event). Unknown (only team requests, or the timeline missed the event): the PR's creation time, a documented
/// over-estimate of the wait.
fn waiting_since(requested: Option<Timestamp>, created: Timestamp) -> Timestamp {
    requested.unwrap_or(created)
}

/// The user's pull requests GitHub lets the user merge now ([`merge_ready`]); they stay ranked.
fn ready_to_merge(rules: &ScoreRules, snapshot: &Snapshot) -> Vec<ReadyToMergeItem> {
    let facts: Vec<_> = snapshot
        .pull_requests
        .iter()
        .map(|pr| pr.facts.clone())
        .collect();
    merge_ready(rules, &facts)
        .into_iter()
        .filter_map(|f| {
            let stored = snapshot
                .pull_requests
                .iter()
                .find(|pr| pr.facts.repo == f.repo && pr.facts.number == f.number)?;
            let mut tags: Vec<FactTag> = ci_tag(stored).into_iter().collect();
            if f.merge_state == MergeState::Unstable {
                tags.push(tag(
                    "non-required checks not passing",
                    "GitHub allows the merge while a check that is not required has not passed (merge state \
                     unstable)",
                ));
            }
            Some(ReadyToMergeItem {
                id: rules.display_id(&f.repo, f.number),
                repo: f.repo.clone(),
                number: f.number,
                title: f.title.clone(),
                url: github_url(&f.repo, "pull", f.number),
                tags,
                change_size: score::size_mark(f),
            })
        })
        .collect()
}
