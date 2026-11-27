//! The ranked items and the snapshot facts of R1/R1b, computed from a stored snapshot and the judgements present
//! when read. Tags and facts come from scan facts only; LLM text only from a judgement.

use std::collections::{HashMap, HashSet};

use contracts::api::dashboard::{
    AwaitingItem, AwaitingReason, EXCLUDED_ITEMS_LIMIT, ExcludedItem, ExclusionCount,
    ExclusionReason, Exclusions, Fact, FactFilter, FactTag, ItemCounts, ItemKey, ItemKind,
    ItemTarget, ItemText, Judgement as JudgementView, JudgementState, OutdatedBecause,
    RankingState, ReadyToMergeItem, RefreshState, ScoreState, SnapshotMeta, StatusCounts, Tier,
    WorkItem,
};
use contracts::common::RepoRef;
use contracts::time::Timestamp;
use db::repo::dashboard::{RefreshRecord, Snooze};
use jiff::SignedDuration;
use jiff::tz::TimeZone;

use super::{Snapshot, StoredIssue, StoredPr, StoredTodo};
use crate::dashboard::plugin::{Move, Overlay};
use crate::dashboard::refresh::rank::ItemJudgement;
use crate::dashboard::score::{
    self, CiEffective, Freshness, Judgement, Mergeable, Milestone, PrFacts, Relation, ScoreError,
    ScoreRules, Scored, TodoStatus, merge_ready, todo_display_ids,
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
    /// Repository counts for visible ranked items (live, not live).
    pub(crate) repository_counts: (u32, u32),
    /// Counts by kind and tier.
    pub item_counts: ItemCounts,
    /// Items left out of the ranking: counts per rule and the capped list.
    pub exclusions: Exclusions,
    /// The user's pull requests waiting on others.
    pub awaiting: Vec<AwaitingItem>,
    /// The user's pull requests ready to merge (K17), without those a plug-in holds back (ADR-022).
    pub ready_to_merge: Vec<ReadyToMergeItem>,
    /// `SnapshotMeta.merge_state_pending`.
    pub merge_state_pending: Option<u32>,
}

impl View {
    /// Applies webhook coverage to GitHub items and counts distinct repositories among the ranked items.
    pub(crate) fn apply_installations(&mut self, live: &HashSet<RepoRef>) {
        let repositories: HashSet<_> = self
            .items
            .iter_mut()
            .filter_map(|entry| {
                let repo = crate::dashboard::installations::key(entry.item.repo.as_ref()?);
                entry.item.live = Some(live.contains(&repo));
                Some(repo)
            })
            .collect();
        let live_count = repositories.intersection(live).count();
        self.repository_counts = (
            u32::try_from(live_count).unwrap_or(u32::MAX),
            u32::try_from(repositories.len() - live_count).unwrap_or(u32::MAX),
        );
    }

    /// Shows only the items covered by a single-item ranking as queued or judging.
    pub(crate) fn apply_rejudge_targets(
        &mut self,
        record: &RefreshRecord,
        targets: &HashSet<ItemKey>,
    ) {
        let capped: HashSet<_> = record.progress.left_provisional_keys.iter().collect();
        for entry in &mut self.items {
            if let Some(judgement) = &mut entry.item.judgement {
                match judgement.state {
                    JudgementState::Judging if !targets.contains(&entry.item.key) => {
                        judgement.state = JudgementState::Outdated;
                    }
                    JudgementState::Queued => {
                        judgement.state = if capped.contains(&entry.item.key) {
                            JudgementState::Capped
                        } else {
                            JudgementState::NotJudged
                        };
                    }
                    _ => {}
                }
            }
        }
    }

    /// Summary counts across the ranked items, before item filters and pagination.
    pub(crate) fn status_counts(&self) -> StatusCounts {
        let mut counts = StatusCounts {
            outdated: 0,
            not_judged: 0,
            not_live: 0,
            excluded: self.exclusions.total,
        };
        for entry in &self.items {
            match entry.item.judgement.map(|j| j.state) {
                Some(JudgementState::Outdated | JudgementState::Judging) => counts.outdated += 1,
                Some(
                    JudgementState::NotJudged | JudgementState::Queued | JudgementState::Capped,
                ) => {
                    counts.not_judged += 1;
                }
                _ => {}
            }
            counts.not_live += u32::from(entry.item.live == Some(false));
        }
        counts
    }

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

/// Scores and ranks the snapshot's items with `judgements` at its scan time. `memcan_ui` is `SR_MEMCAN_UI_URL`;
/// `overlay` holds the plug-in statements shown now (ADR-022): notes on pull requests, Ready to merge holds and
/// whose move a pull request is (Waiting on others).
pub fn build(
    snapshot: &Snapshot,
    judgements: &HashMap<ItemKey, ItemJudgement>,
    memcan_ui: Option<&str>,
    overlay: &Overlay,
    record: Option<&RefreshRecord>,
) -> View {
    let ranking_live = record.is_some_and(|record| record.state == RefreshState::Ranking);
    let capped: HashSet<_> = record
        .into_iter()
        .flat_map(|record| &record.progress.left_provisional_keys)
        .collect();
    let bulk: HashSet<_> = record
        .into_iter()
        .flat_map(|record| &record.progress.bulk_rejudge_keys)
        .collect();
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
    // A PR waiting on others is listed under Waiting on others alone (K15).
    for pr in snapshot
        .pull_requests
        .iter()
        .filter(|pr| waiting(&rules, &pr.facts, overlay).is_none())
    {
        let key = item_key(ItemTarget::PullRequest {
            repo: pr.facts.repo.clone(),
            number: pr.facts.number,
        });
        let item = scored_with(key.as_ref(), judgements, |j| {
            score::score_pr(&rules, &pr.facts, j, at)
        });
        let item = item.map(|item| snoozed(item, &snapshot.snoozes));
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
        let item = item.map(|item| snoozed(item, &snapshot.snoozes));
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
        let item = item.map(|item| snoozed(item, &snapshot.snoozes));
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
        let judged = judgements.get(&item.key);
        let text = judged.and_then(|judged| judged.text.clone());
        let judgement = (item.asks_eligibility || !item.asks.is_empty()).then(|| {
            judgement_for(
                judged,
                ranking_live,
                capped.contains(&item.key),
                bulk.contains(&item.key),
            )
        });
        let mut view_item = work_item(&rules, &look_back, item, source, text, memcan_ui);
        view_item.item.judgement = judgement;
        if let Source::Pr(pr) = source {
            view_item.item.plugin_notes = overlay.notes(&pr.facts.repo, pr.facts.number);
        }
        items.push(view_item);
    }
    let awaiting = awaiting(&rules, snapshot, overlay);
    let ready_to_merge = ready_to_merge(&rules, snapshot, overlay);
    let merge_state_pending = snapshot.merge_states_recorded.then(|| {
        let pending = snapshot
            .pull_requests
            .iter()
            .filter(|pr| score::merge_state_pending_for_ready(&rules, &pr.facts))
            .count();
        u32::try_from(pending).unwrap_or(u32::MAX)
    });
    for left_out in &ranking.excluded {
        tracing::debug!(item = %left_out.id, reason = %left_out.reason, "dashboard item left out");
    }
    View {
        items,
        repository_counts: (0, 0),
        item_counts: counts,
        exclusions: exclusions(ranking.excluded, &sources, &snapshot.snoozes, memcan_ui),
        awaiting,
        ready_to_merge,
        merge_state_pending,
    }
}

fn judgement_for(
    stored: Option<&ItemJudgement>,
    ranking_live: bool,
    capped: bool,
    bulk_requested: bool,
) -> JudgementView {
    let Some(stored) = stored else {
        return JudgementView {
            state: if ranking_live {
                JudgementState::Queued
            } else if capped {
                JudgementState::Capped
            } else {
                JudgementState::NotJudged
            },
            judged_at: None,
            outdated_because: None,
        };
    };
    let stale = stored.judgement.freshness == Freshness::Inherited;
    JudgementView {
        state: if !stale {
            JudgementState::Current
        } else if ranking_live && (stored.rejudge_requested || bulk_requested) {
            JudgementState::Judging
        } else {
            JudgementState::Outdated
        },
        judged_at: Some(stored.judgement.judged_at),
        outdated_because: stale.then_some(if stored.older_prompt {
            OutdatedBecause::PromptChanged
        } else {
            OutdatedBecause::FactsChanged
        }),
    }
}

/// Leaves a snoozed item the rules would rank out of the ranking (owner SN1); another rule's exclusion wins, and a
/// P0 alarm stays listed (a snooze never hides an alarm).
fn snoozed(mut item: Scored, snoozes: &[Snooze]) -> Scored {
    if item.exclusion.is_none()
        && item.score.tier != Tier::P0
        && let Some(snooze) = snoozes.iter().find(|s| s.key == item.key)
    {
        item.exclusion = Some(ExclusionReason::Snoozed);
        item.excluded = Some(format!("snoozed until {}", snooze.until));
    }
    item
}

/// Counts per rule and the first [`EXCLUDED_ITEMS_LIMIT`] items by rule, then display id (B4).
fn exclusions(
    mut left_out: Vec<score::Excluded>,
    sources: &HashMap<ItemKey, Source<'_>>,
    snoozes: &[Snooze],
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
            let until = (e.rule == ExclusionReason::Snoozed)
                .then(|| snoozes.iter().find(|s| s.key == e.key).map(|s| s.until))
                .flatten();
            Some(excluded_item(e, source, until, memcan_ui))
        })
        .take(EXCLUDED_ITEMS_LIMIT)
        .collect();
    Exclusions {
        total: counts.iter().map(|c| c.count).sum(),
        counts,
        items: Some(items),
    }
}

fn excluded_item(
    e: score::Excluded,
    source: &Source<'_>,
    snoozed_until: Option<Timestamp>,
    memcan_ui: Option<&str>,
) -> ExcludedItem {
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
        snoozed_until,
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

fn tag(key: impl Into<String>, label: &str, tip: impl Into<String>) -> FactTag {
    FactTag {
        filter: FactFilter {
            key: key.into(),
            label: label.to_owned(),
        },
        text: label.to_owned(),
        tip: tip.into(),
    }
}

fn label_tag(label: &str) -> FactTag {
    tag(
        format!("label:{}", label.to_lowercase()),
        label,
        "GitHub label",
    )
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
                    issue_tags(issue, look_back),
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
    let (stack, release_base) = match source {
        Source::Pr(pr) => (
            pr.stack.clone(),
            pr.facts
                .base_ref
                .as_ref()
                .filter(|base| rules.matches_branch_pattern(&pr.facts.repo, base))
                .cloned(),
        ),
        Source::Issue(_) | Source::Todo(_) => (None, None),
    };
    ViewItem {
        item: WorkItem {
            judgement: None,
            live: None,
            stack,
            release_base,
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
            comments: None,
            imported_unseen: None,
            watched: false,
            plugin_notes: Vec::new(),
        },
        head_sha,
    }
}

fn ci_tag(pr: &StoredPr) -> Option<FactTag> {
    match pr.facts.ci {
        CiEffective::Green if pr.ci_ignored.is_empty() => Some(tag(
            "ci_passing",
            "CI passing",
            "Every build, test and lint check on the head commit passed",
        )),
        CiEffective::Green => Some(tag(
            "ci_passing",
            "CI passing",
            format!(
                "Every build, test and lint check on the head commit passed; ignored checks not passing: {}",
                pr.ci_ignored.join(", ")
            ),
        )),
        CiEffective::Red => Some(tag(
            "ci_failing",
            "CI failing",
            format!("Failing: {}", pr.ci_red.join(", ")),
        )),
        CiEffective::Pending => Some(tag(
            "ci_running",
            "CI running",
            if pr.ci_pending.is_empty() {
                "Checks have not reported yet".to_owned()
            } else {
                format!("Running: {}", pr.ci_pending.join(", "))
            },
        )),
        CiEffective::Unknown => Some(tag(
            "ci_partly_read",
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
            "review_requested",
            "review requested",
            "Your review is requested, directly or through a team",
        ));
    }
    if f.relations.contains(&Relation::Assigned) {
        tags.push(tag(
            "assigned_to_you",
            "assigned to you",
            "You are an assignee of this PR",
        ));
    }
    if f.is_draft {
        tags.push(tag("draft", "draft", "The PR is a draft"));
    }
    tags.extend(ci_tag(pr));
    if f.mergeable == Mergeable::Conflicting {
        tags.push(tag(
            "conflicts",
            "conflicts",
            "GitHub reports merge conflicts",
        ));
    }
    if !f.changes_requested_by.is_empty() {
        tags.push(tag(
            "changes_requested",
            "changes requested",
            format!("Requested by {}", f.changes_requested_by.join(", ")),
        ));
    }
    if f.unresolved_threads > 0 {
        tags.push(FactTag {
            text: format!("{} unresolved threads", f.unresolved_threads),
            ..tag(
                "unresolved_threads",
                "unresolved threads",
                "Review threads not resolved on GitHub",
            )
        });
    }
    tags.extend(f.labels.iter().map(|label| {
        let mut tag = label_tag(label);
        if rules.is_ready_label(label) {
            tag.text = "ready label".to_owned();
            tag.tip = "Marked ready for a human review".to_owned();
        }
        tag
    }));
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

fn issue_tags(issue: &StoredIssue, look_back: &LookBack) -> Vec<FactTag> {
    let relations = &issue.facts.relations;
    let mut tags = Vec::new();
    if let Some(opened) = issue.opened_at.filter(|at| *at >= look_back.since) {
        tags.push(tag(
            "new",
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
        tags.push(tag(
            "assigned_to_you",
            "assigned to you",
            "You are an assignee of this issue",
        ));
    }
    if relations.contains(&Relation::Mentioned) {
        tags.push(tag(
            "mentions_you",
            "mentions you",
            "The issue or a comment mentions you",
        ));
    }
    if relations.contains(&Relation::RepoOpen) {
        tags.push(tag(
            "watched_repository",
            "watched repository",
            "Open issue of a repository whose issues you watch",
        ));
    }
    tags.extend(issue.facts.labels.iter().map(|label| label_tag(label)));
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
    let (key, status) = match todo.facts.status {
        TodoStatus::Pending => ("pending", "pending"),
        TodoStatus::InProgress => ("in_progress", "in progress"),
        TodoStatus::Blocked => ("blocked", "blocked"),
        TodoStatus::Postponed => ("postponed", "postponed"),
    };
    vec![
        tag(
            format!("project:{}", todo.project),
            &todo.project,
            "MemCan project",
        ),
        tag(key, status, "MemCan status"),
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

/// Why the user's PR waits on others now (K15 W3), and whom to ping; `None` when it does not. The first that
/// applies: a plug-in's statement that reviewers or maintainers own the stage (those it says owe a review, else the
/// requested reviewers), the ready label, a requested review. A people's statement can cover human review blockers
/// older than its source data with GitHub clock evidence, but never active bot requests. The read path and the
/// judge's input both apply it.
pub(crate) fn waiting(
    rules: &ScoreRules,
    f: &PrFacts,
    overlay: &Overlay,
) -> Option<(AwaitingReason, Vec<String>)> {
    if !score::can_wait_for_review(rules, f)
        || f.changes_requested_by_bot
            .unwrap_or(!f.changes_requested_by.is_empty())
    {
        return None;
    }
    let requested = f.review_requested_from_others.clone();
    if let Some(statement) = overlay.whose_move(&f.repo, f.number)
        && let Move::People { owed, reason } = statement.whose
        && f.changes_requested_human_at.map_or_else(
            || {
                f.changes_requested_by.iter().all(|login| {
                    requested
                        .iter()
                        .any(|other| other.eq_ignore_ascii_case(login))
                })
            },
            |at| {
                at < statement.as_of
                    && statement
                        .github_updated_at
                        .is_some_and(|updated| at < updated && updated <= f.updated_at)
            },
        )
    {
        let reason = AwaitingReason::Plugin {
            plugin: statement.plugin,
            source: statement.source,
            reason,
            url: statement.url,
        };
        return Some((reason, if owed.is_empty() { requested } else { owed }));
    }
    if !score::nothing_is_users_move(rules, f) {
        return None;
    }
    if let Some(label) = score::ready_label(rules, f) {
        let reason = AwaitingReason::ReadyLabel {
            label: label.to_owned(),
        };
        return Some((reason, requested));
    }
    score::waits_on_requested_review(rules, f)
        .then_some((AwaitingReason::ReviewRequested, requested))
}

/// The user's PRs waiting on others, longest wait first, each with why and whom to ping. `waiting_since` is the
/// oldest open request to others; when the timeline did not show it (older than its last 50 events, a team request,
/// or none), the PR's creation time.
fn awaiting(rules: &ScoreRules, snapshot: &Snapshot, overlay: &Overlay) -> Vec<AwaitingItem> {
    let mut rows: Vec<(&PrFacts, Timestamp)> = snapshot
        .pull_requests
        .iter()
        .filter(|pr| waiting(rules, &pr.facts, overlay).is_some())
        .map(|pr| {
            (
                &pr.facts,
                waiting_since(pr.facts.others_review_requested_at, pr.created_at),
            )
        })
        .collect();
    score::by_longest_wait(rules, &mut rows);
    rows.into_iter()
        .filter_map(|(f, since)| {
            let stored = snapshot
                .pull_requests
                .iter()
                .find(|pr| pr.facts.repo == f.repo && pr.facts.number == f.number)?;
            let (reason, waiting_on) = waiting(rules, f, overlay)?;
            Some(AwaitingItem {
                id: rules.display_id(&f.repo, f.number),
                repo: f.repo.clone(),
                number: f.number,
                title: f.title.clone(),
                url: github_url(&f.repo, "pull", f.number),
                waiting_on,
                reason,
                waiting_since: since,
                tags: ci_tag(stored).into_iter().collect(),
                change_size: score::size_mark(f),
                // Never ranked (K15).
                total_tenths: None,
            })
        })
        .collect()
}

/// `AwaitingItem.waiting_since`: the oldest open request to another reviewer (each login's newest request
/// event). Unknown (only team requests, no request, or the timeline missed the event): the PR's creation time, a
/// documented over-estimate of the wait.
fn waiting_since(requested: Option<Timestamp>, created: Timestamp) -> Timestamp {
    requested.unwrap_or(created)
}

/// The user's pull requests ready to merge ([`merge_ready`]; they stay ranked), without those a current plug-in
/// statement holds back (ADR-022 §6): those stay in the ranking only, with the plug-in's note.
fn ready_to_merge(
    rules: &ScoreRules,
    snapshot: &Snapshot,
    overlay: &Overlay,
) -> Vec<ReadyToMergeItem> {
    let facts: Vec<_> = snapshot
        .pull_requests
        .iter()
        .map(|pr| pr.facts.clone())
        .collect();
    merge_ready(rules, &facts)
        .into_iter()
        .filter(|f| overlay.hold(&f.repo, f.number).is_none())
        .filter_map(|f| {
            let stored = snapshot
                .pull_requests
                .iter()
                .find(|pr| pr.facts.repo == f.repo && pr.facts.number == f.number)?;
            Some(ReadyToMergeItem {
                id: rules.display_id(&f.repo, f.number),
                repo: f.repo.clone(),
                number: f.number,
                title: f.title.clone(),
                url: github_url(&f.repo, "pull", f.number),
                tags: ci_tag(stored).into_iter().collect(),
                change_size: score::size_mark(f),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use contracts::api::dashboard::ItemTarget;
    use jiff::SignedDuration;
    use uuid::Uuid;

    use super::*;
    use crate::dashboard::score::{TodoFacts, TodoStatus};

    #[test]
    fn judgement_states_table() {
        use JudgementState::*;
        use OutdatedBecause::*;

        let answer = |freshness, older_prompt, rejudge_requested| ItemJudgement {
            judgement: Judgement {
                parts: Vec::new(),
                eligibility: None,
                alarm: false,
                also: Vec::new(),
                judged_at: at(),
                freshness,
            },
            text: None,
            older_prompt,
            rejudge_requested,
        };
        let fresh = answer(Freshness::Current, false, false);
        let changed = answer(Freshness::Inherited, false, false);
        let bumped = answer(Freshness::Inherited, true, true);
        for (stored, live, capped, state, reason) in [
            (None, false, false, NotJudged, None),
            (None, true, false, Queued, None),
            (None, true, true, Queued, None),
            (None, false, true, Capped, None),
            (Some(&fresh), false, false, Current, None),
            (Some(&changed), true, false, Outdated, Some(FactsChanged)),
            (Some(&bumped), false, false, Outdated, Some(PromptChanged)),
            (Some(&bumped), true, false, Judging, Some(PromptChanged)),
        ] {
            let rendered = judgement_for(stored, live, capped, false);
            assert_eq!(rendered.state, state);
            assert_eq!(rendered.outdated_because, reason);
            assert_eq!(rendered.judged_at, stored.map(|_| at()));
        }
    }

    fn at() -> Timestamp {
        "2026-11-18T09:00:00Z".parse().expect("time")
    }

    fn todo(n: u128, alarm: bool) -> Scored {
        let facts = TodoFacts {
            id: Uuid::from_u128(n),
            title: "t".into(),
            status: TodoStatus::Pending,
            priority: None,
            changed_at: at(),
        };
        let judgement = Judgement {
            parts: Vec::new(),
            eligibility: None,
            alarm,
            also: Vec::new(),
            judged_at: at(),
            freshness: Freshness::Current,
        };
        score::score_todo(&facts, format!("TODO-{n}"), Some(&judgement), at()).expect("scores")
    }

    fn snooze(item: &Scored) -> Snooze {
        Snooze {
            key: item.key.clone(),
            snoozed_at: at(),
            until: at() + SignedDuration::from_hours(24),
        }
    }

    /// SN1: a snooze leaves a ranked item out with its reason; a P0 alarm stays, and another rule's exclusion wins.
    #[test]
    fn a_snooze_never_hides_an_alarm_or_replaces_another_rule() {
        let plain = todo(1, false);
        let alarm = todo(2, true);
        assert_eq!(alarm.score.tier, Tier::P0);
        let mut merged = todo(3, false);
        merged.exclusion = Some(ExclusionReason::MergedIntoAnother);
        let other = ItemKey::new(&ItemTarget::Todo(Uuid::from_u128(9))).expect("key");
        let snoozes = [snooze(&plain), snooze(&alarm), snooze(&merged)];

        let left_out = snoozed(plain.clone(), &snoozes);
        assert_eq!(left_out.exclusion, Some(ExclusionReason::Snoozed));
        assert_eq!(
            left_out.excluded.as_deref(),
            Some("snoozed until 2026-11-19T09:00:00Z")
        );
        assert_eq!(snoozed(alarm, &snoozes).exclusion, None, "an alarm stays");
        assert_eq!(
            snoozed(merged, &snoozes).exclusion,
            Some(ExclusionReason::MergedIntoAnother)
        );
        let unrelated = Snooze {
            key: other,
            ..snooze(&plain)
        };
        assert_eq!(snoozed(plain, &[unrelated]).exclusion, None);
    }
}
