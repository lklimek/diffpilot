//! The ranked items and the snapshot facts of R1/R1b, computed from a stored snapshot and the judgements present
//! when read. Tags and facts come from scan facts only; LLM text only from a judgement.

use std::collections::HashMap;

use contracts::api::dashboard::{
    AwaitingItem, Fact, FactTag, ItemCounts, ItemKey, ItemKind, ItemTarget, ItemText, RankingState,
    ScoreState, SnapshotMeta, Tier, WorkItem,
};
use contracts::common::RepoRef;
use contracts::time::Timestamp;
use db::repo::dashboard::RefreshRecord;

use super::{Snapshot, StoredIssue, StoredPr, StoredTodo};
use crate::dashboard::refresh::rank::ItemJudgement;
use crate::dashboard::score::{
    self, CiEffective, Judgement, Mergeable, Milestone, Relation, ScoreError, ScoreRules, Scored,
    TodoStatus, awaiting_others, todo_display_ids,
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
    /// Items left out of the ranking.
    pub excluded: u32,
    /// The user's pull requests waiting on others.
    pub awaiting: Vec<AwaitingItem>,
}

impl View {
    /// `SnapshotMeta` of the snapshot held by `record`.
    pub fn meta(&self, snapshot: &Snapshot, record: &RefreshRecord) -> SnapshotMeta {
        SnapshotMeta {
            refresh_id: record.id,
            scanned_at: snapshot.scanned_at,
            ranked_at: (record.ranking_state == Some(RankingState::Finished))
                .then_some(record.finished_at)
                .flatten(),
            item_counts: self.item_counts,
            excluded: self.excluded,
            awaiting: self.awaiting.clone(),
            history: snapshot.history.clone(),
            todos: snapshot.todos_state.clone(),
            notes: snapshot.notes.clone(),
            unreachable: snapshot.unreachable.clone(),
            checked: snapshot.checked,
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
    let todo_ids = todo_display_ids(
        &snapshot
            .todos
            .iter()
            .map(|t| t.facts.id)
            .collect::<Vec<_>>(),
    );
    let mut scored = Vec::new();
    let mut sources: HashMap<ItemKey, Source<'_>> = HashMap::new();
    for pr in &snapshot.pull_requests {
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
        items.push(work_item(&rules, item, source, text, memcan_ui));
    }
    let awaiting = awaiting(&rules, snapshot, &items);
    for left_out in &ranking.excluded {
        tracing::debug!(item = %left_out.id, reason = %left_out.reason, "dashboard item left out");
    }
    View {
        items,
        item_counts: counts,
        excluded: u32::try_from(ranking.excluded.len()).unwrap_or(u32::MAX),
        awaiting,
    }
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

fn work_item(
    rules: &ScoreRules,
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
                    issue_tags(&f.relations),
                    issue_facts(issue),
                    None,
                )
            }
            Source::Todo(todo) => (
                None,
                None,
                todo.facts.title.clone(),
                memcan_ui
                    .map(|base| format!("{}/tasks/{}", base.trim_end_matches('/'), todo.facts.id)),
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

fn issue_tags(relations: &[Relation]) -> Vec<FactTag> {
    let mut tags = Vec::new();
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
fn awaiting(rules: &ScoreRules, snapshot: &Snapshot, items: &[ViewItem]) -> Vec<AwaitingItem> {
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
            let key = item_key(ItemTarget::PullRequest {
                repo: f.repo.clone(),
                number: f.number,
            });
            let total_tenths = items
                .iter()
                .find(|item| Some(&item.item.key) == key.as_ref())
                .map(|item| item.item.score.total_tenths);
            Some(AwaitingItem {
                id: rules.display_id(&f.repo, f.number),
                repo: f.repo.clone(),
                number: f.number,
                title: f.title.clone(),
                url: github_url(&f.repo, "pull", f.number),
                waiting_on: f.review_requested_from_others.clone(),
                waiting_since: waiting_since(f.others_review_requested_at, stored.created_at),
                tags: ci_tag(stored).into_iter().collect(),
                total_tenths,
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
