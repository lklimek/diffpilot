//! What the judge sees of a snapshot's items (plan §B.2) and the judged-input fingerprint (plan §B.4): the facts a
//! judgement depends on, the scorer's signals and both versions; ages and volatile counters stay out, so a judgement
//! survives a day without change.

use std::collections::HashMap;

use contracts::agent::judge_work::{
    Eligibility, JudgeFacts, JudgeItem, KnownPart, LinkedIssue, PreviousJudgement, TodoJudgeFacts,
};
use contracts::api::dashboard::{
    ItemKey, ItemKind, PartSource, PartStatus, ScoreComponent, ScorePart,
};
use contracts::common::{LlmLanguage, RepoRef};
use contracts::time::Timestamp;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::JUDGE_PROMPT_VERSION;
use super::store::Body;
use crate::dashboard::score::{
    self, Judgement, Milestone, PrFacts, ScoreError, ScoreRules, Scored, todo_display_ids,
};
use crate::dashboard::snapshot::{Snapshot, StoredIssue, StoredPr, StoredTodo};

/// Version of the judged inputs and their fingerprint; a bump re-judges every item within the caps.
pub const JUDGE_INPUT_VERSION: u32 = 3;
/// Body excerpt sent per pull request or issue, in characters.
const EXCERPT_CHARS: usize = 300;
/// Release tags or milestone titles listed in `release_context`, at most each.
const RELEASE_CONTEXT_ENTRIES: usize = 20;
/// Basis sent for a computed human-waiting part of a pull request (its own basis names the request's age), before
/// the age class.
const WAITING_BASIS: &str = "computed from the outstanding human request";
/// Unsure cases of a pull request's human-waiting default (the scorer's hints name ages).
const WAITING_UNSURE: &str =
    "unsure: a team review request, an approximate request date or an unanswered comment";
/// Prefix of a computed default the judge confirms or replaces (replacing the scorer's estimate prefix).
const DEFAULT_PREFIX: &str = "default to confirm: ";

/// A snapshot item as scored without a judgement.
#[derive(Clone, Copy, Debug)]
pub(super) enum Source<'a> {
    Pr(&'a StoredPr),
    Issue(&'a StoredIssue),
    Todo(&'a StoredTodo),
}

/// One snapshot item with what a judgement of it needs.
#[derive(Clone, Debug)]
pub(super) struct Candidate<'a> {
    pub source: Source<'a>,
    /// Scored without a judgement.
    pub scored: Scored,
    /// The judge input (without `previous`).
    pub item: JudgeItem,
    /// SHA-256 hex of both versions, the judged inputs and the scorer's signals.
    pub fingerprint: String,
}

impl Candidate<'_> {
    /// The scorer asks the judge something and the item can be ranked: only these are judged, and only their
    /// stored judgements apply.
    pub fn asked(&self) -> bool {
        (!self.item.asks.is_empty() || self.item.asks_eligibility)
            && (self.scored.excluded.is_none() || self.item.asks_eligibility)
    }

    /// Scores the item with `judgement` as of `at`.
    pub fn score(
        &self,
        rules: &ScoreRules,
        judgement: Option<&Judgement>,
        at: Timestamp,
    ) -> Result<Scored, ScoreError> {
        match self.source {
            Source::Pr(pr) => score::score_pr(rules, &pr.facts, judgement, at),
            Source::Issue(issue) => score::score_issue(rules, &issue.facts, judgement, at),
            Source::Todo(todo) => {
                score::score_todo(&todo.facts, self.scored.id.clone(), judgement, at)
            }
        }
    }
}

/// Every scoreable item of the snapshot, scored at its scan time like the read path (`snapshot::build`).
pub(super) fn candidates(snapshot: &Snapshot, language: LlmLanguage) -> Vec<Candidate<'_>> {
    let rules = ScoreRules::new(&snapshot.own_login, &snapshot.settings);
    let at = snapshot.scanned_at;
    let todo_ids = todo_display_ids(
        &snapshot
            .todos
            .iter()
            .map(|t| t.facts.id)
            .collect::<Vec<_>>(),
    );
    let mut scored: Vec<(Source<'_>, Scored)> = Vec::new();
    let mut keep = |source, result: Result<Scored, ScoreError>| match result {
        Ok(item) => scored.push((source, item)),
        Err(error) => tracing::debug!(%error, "dashboard item not judged: it cannot be scored"),
    };
    for pr in &snapshot.pull_requests {
        keep(Source::Pr(pr), score::score_pr(&rules, &pr.facts, None, at));
    }
    for issue in &snapshot.issues {
        keep(
            Source::Issue(issue),
            score::score_issue(&rules, &issue.facts, None, at),
        );
    }
    for todo in &snapshot.todos {
        let id = todo_ids
            .get(&todo.facts.id)
            .cloned()
            .unwrap_or_else(|| format!("TODO-{}", todo.facts.id));
        keep(
            Source::Todo(todo),
            score::score_todo(&todo.facts, id, None, at),
        );
    }
    let merges = suggested_merges(&scored);
    scored
        .into_iter()
        .map(|(source, scored)| {
            let item = JudgeItem {
                key: scored.key.clone(),
                id: scored.id.clone(),
                kind: scored.kind,
                facts: facts(&rules, source, &scored),
                known: known(&scored),
                asks: scored.asks.clone(),
                asks_eligibility: scored.asks_eligibility,
                eligibility: eligibility(&scored),
                previous: None,
                suggested_merges: merges.get(&scored.key).cloned().unwrap_or_default(),
            };
            let fingerprint = fingerprint(&item, &scored, language);
            Candidate {
                source,
                scored,
                item,
                fingerprint,
            }
        })
        .collect()
}

/// Judged inputs as hashed: the item as sent without its display id and previous judgement, and the known parts
/// without their basis (it may name release tags or a failed lookup, which are no judged inputs), plus the signals.
#[derive(Serialize)]
struct Fingerprinted<'a> {
    key: &'a ItemKey,
    kind: ItemKind,
    facts: &'a JudgeFacts,
    known: Vec<KnownValue>,
    asks: &'a [ScoreComponent],
    asks_eligibility: bool,
    eligibility: &'a Eligibility,
    suggested_merges: &'a [ItemKey],
    signals: &'a score::FingerprintSignals,
    /// The user's LLM text language: the judge writes its texts in it.
    language: LlmLanguage,
}

/// A known part as fingerprinted: component and points only.
#[derive(Serialize)]
struct KnownValue {
    component: ScoreComponent,
    points_tenths: Option<i32>,
}

/// `SHA-256(JUDGE_INPUT_VERSION ‖ JUDGE_PROMPT_VERSION ‖ canonical JSON)` in hex (plan §B.4).
fn fingerprint(item: &JudgeItem, scored: &Scored, language: LlmLanguage) -> String {
    let inputs = Fingerprinted {
        key: &item.key,
        kind: item.kind,
        facts: &item.facts,
        known: item
            .known
            .iter()
            .map(|part| KnownValue {
                component: part.component,
                points_tenths: part.points_tenths,
            })
            .collect(),
        asks: &item.asks,
        asks_eligibility: item.asks_eligibility,
        eligibility: &item.eligibility,
        suggested_merges: &item.suggested_merges,
        signals: &scored.signals,
        language,
    };
    let mut hash = Sha256::new();
    hash.update(format!("{JUDGE_INPUT_VERSION}\n{JUDGE_PROMPT_VERSION}\n"));
    // Serialising plain structs, strings and enums cannot fail; an empty document would still hash stably.
    hash.update(serde_json::to_vec(&inputs).unwrap_or_default());
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The previous judgement of a changed item, as the judge sees it.
/// The previous judgement as sent to the judge: its texts only when written in `language`, else its points alone
/// (old-language texts must not be copied into the new judgement).
pub(super) fn previous(body: &Body, language: LlmLanguage) -> PreviousJudgement {
    let texts = body.language == Some(language);
    let text = |value: &String| if texts { value.clone() } else { String::new() };
    PreviousJudgement {
        parts: body
            .parts
            .iter()
            .map(|part| KnownPart {
                component: part.component,
                points_tenths: Some(part.points * 10),
                status: PartStatus::Judged,
                basis: text(&part.basis),
            })
            .collect(),
        why: text(&body.why),
        next: text(&body.next),
        alarm: body.alarm.clone().filter(|_| texts),
    }
}

/// Computed parts sent as known, and the computed defaults of asked components (status `estimated`) the judge
/// confirms or replaces (plan §B.1 J6, J8, J13); never recency or change size, and no basis naming an age.
fn known(scored: &Scored) -> Vec<KnownPart> {
    let pr_waiting = |part: &ScorePart| {
        part.component == ScoreComponent::HumanWaiting && scored.kind == ItemKind::Pr
    };
    scored
        .score
        .parts
        .iter()
        .filter(|part| {
            !matches!(
                part.component,
                ScoreComponent::Recency | ScoreComponent::ChangeSize
            )
        })
        .filter_map(|part| {
            let asked = scored.asks.contains(&part.component);
            let basis = if asked {
                if part.source != PartSource::Computed || part.status != PartStatus::Estimated {
                    return None;
                }
                if pr_waiting(part) {
                    format!(
                        "{DEFAULT_PREFIX}{} → {}; {WAITING_UNSURE}",
                        waiting_class(scored),
                        points(part.points_tenths)
                    )
                } else {
                    let rule = part
                        .basis
                        .strip_prefix(score::ESTIMATE)
                        .unwrap_or(&part.basis);
                    format!("{DEFAULT_PREFIX}{rule}")
                }
            } else if pr_waiting(part) {
                format!("{WAITING_BASIS}: {}", waiting_class(scored))
            } else {
                part.basis.clone()
            };
            Some(KnownPart {
                component: part.component,
                points_tenths: part.points_tenths,
                status: part.status,
                basis,
            })
        })
        .collect()
}

/// The scorer's human-waiting bucket as an age class (ages are no judged inputs, plan §B.4).
fn waiting_class(scored: &Scored) -> &'static str {
    match scored.signals.human_waiting {
        Some(150) => "a request within 2 days",
        Some(100) => "a request within a week",
        Some(50) => "a request older than a week",
        Some(_) => "no definite outstanding request",
        None => "request unknown",
    }
}

/// Points in tenths as shown in a basis.
fn points(tenths: Option<i32>) -> String {
    tenths.map_or_else(
        || "pending".to_owned(),
        |t| format!("{}", f64::from(t) / 10.0),
    )
}

fn eligibility(scored: &Scored) -> Eligibility {
    let reason = match (&scored.excluded, scored.asks_eligibility) {
        (Some(reason), _) => reason.clone(),
        (None, true) => "ownership policy unavailable: change in owned paths unchecked".to_owned(),
        (None, false) => "ranked by the computed rules".to_owned(),
    };
    Eligibility {
        eligible: scored.excluded.is_none(),
        reason,
    }
}

/// The serde name of a unit enum value.
fn wire<T: Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

/// Whitespace-normalised, at most `max` characters.
fn excerpt(text: &str, max: usize) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

fn place(repo: &RepoRef) -> String {
    format!("{}/{}", repo.owner, repo.name)
}

fn milestone_title(milestone: &Milestone) -> Option<String> {
    match milestone {
        Milestone::Assigned(title) => Some(title.clone()),
        Milestone::Unassigned | Milestone::Unknown => None,
    }
}

/// Releases and milestones of the repository, only while the scorer asks the judge to map the milestone.
fn release_context(scored: &Scored, context: &score::MilestoneContext) -> Option<String> {
    if !scored.asks.contains(&ScoreComponent::MilestoneBonus) {
        return None;
    }
    let list = |values: &Option<Vec<String>>| match values {
        Some(values) => values
            .iter()
            .take(RELEASE_CONTEXT_ENTRIES)
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
        None => "could not be read".to_owned(),
    };
    Some(format!(
        "stable releases: {}; milestones: {}",
        list(&context.release_tags),
        list(&context.milestone_titles)
    ))
}

fn size_class(pr: &PrFacts) -> Option<String> {
    let lines = u64::from(pr.additions?) + u64::from(pr.deletions?);
    Some(
        match lines {
            0..50 => "<50",
            50..200 => "<200",
            200..1000 => "<1000",
            1000..5000 => "<5000",
            _ => ">=5000",
        }
        .to_owned(),
    )
}

fn thread_class(threads: u32) -> String {
    match threads {
        0 => "0",
        1..=2 => "1-2",
        _ => "3+",
    }
    .to_owned()
}

fn facts(rules: &ScoreRules, source: Source<'_>, scored: &Scored) -> JudgeFacts {
    match source {
        Source::Pr(stored) => {
            let pr = &stored.facts;
            let ids = |numbers: &Option<Vec<u32>>| {
                sorted(
                    numbers
                        .iter()
                        .flatten()
                        .map(|n| rules.display_id(&pr.repo, *n))
                        .collect(),
                )
            };
            JudgeFacts {
                title: pr.title.clone(),
                text: excerpt(&pr.body, EXCERPT_CHARS),
                place: place(&pr.repo),
                relations: sorted(pr.relations.iter().map(|r| wire(*r)).collect()),
                author: pr.author.clone(),
                labels: sorted(pr.labels.clone()),
                draft: pr.is_draft,
                ci: Some(wire(pr.ci)),
                failing_checks: sorted(stored.ci_red.clone()),
                mergeable: Some(wire(pr.mergeable)),
                review_decision: pr.review_decision.map(wire),
                human_change_request: pr.changes_requested_by_human,
                unresolved_threads: Some(thread_class(pr.unresolved_threads)),
                awaiting_our_reply: pr.awaiting_our_reply,
                base_branch: pr.base_ref.clone(),
                targets_default_branch: pr
                    .base_ref
                    .as_deref()
                    .zip(pr.default_branch.as_deref())
                    .map(|(base, default)| base == default),
                targets_release_branch: pr.base_ref.as_deref().map(|base| {
                    rules.targets_release_branch(&pr.repo, base, pr.default_branch.as_deref())
                }),
                dependents: ids(&pr.dependents),
                stacked_on: ids(&pr.stacked_on),
                milestone: milestone_title(&pr.milestone),
                release_context: release_context(scored, &pr.milestone_context),
                linked_issues: stored
                    .linked_issues
                    .iter()
                    .map(|issue| LinkedIssue {
                        reference: format!("{}#{}", place(&pr.repo), issue.number),
                        title: issue.title.clone(),
                    })
                    .collect(),
                owned_change: Some(wire(pr.owned_change)),
                change_size: size_class(pr),
                todo: None,
            }
        }
        Source::Issue(stored) => {
            let issue = &stored.facts;
            JudgeFacts {
                title: issue.title.clone(),
                text: excerpt(&issue.body, EXCERPT_CHARS),
                place: place(&issue.repo),
                relations: sorted(issue.relations.iter().map(|r| wire(*r)).collect()),
                author: issue.author.clone(),
                labels: sorted(issue.labels.clone()),
                milestone: milestone_title(&issue.milestone),
                release_context: release_context(scored, &issue.milestone_context),
                ..JudgeFacts::default()
            }
        }
        Source::Todo(todo) => JudgeFacts {
            title: todo.facts.title.clone(),
            text: todo.description.clone().unwrap_or_default(),
            place: todo.project.clone(),
            todo: Some(TodoJudgeFacts {
                status: wire(todo.facts.status),
                priority: todo.facts.priority.clone(),
                owner: todo.owner.clone(),
                blocked_by: u32::try_from(todo.blocked_by).unwrap_or(u32::MAX),
            }),
            ..JudgeFacts::default()
        },
    }
}

/// Server-suggested merges (skill `todos.py`): a TODO whose title names an eligible pull request or issue of the
/// snapshot (`owner/repo#n` or its GitHub URL) is suggested for that item; the first such reference counts.
fn suggested_merges(scored: &[(Source<'_>, Scored)]) -> HashMap<ItemKey, Vec<ItemKey>> {
    let targets: Vec<(&RepoRef, u32, &ItemKey)> = scored
        .iter()
        .filter(|(_, item)| item.excluded.is_none())
        .filter_map(|(source, item)| match source {
            Source::Pr(pr) => Some((&pr.facts.repo, pr.facts.number, &item.key)),
            Source::Issue(issue) => Some((&issue.facts.repo, issue.facts.number, &item.key)),
            Source::Todo(_) => None,
        })
        .collect();
    let mut merges: HashMap<ItemKey, Vec<ItemKey>> = HashMap::new();
    for (source, todo) in scored {
        let Source::Todo(stored) = source else {
            continue;
        };
        let title = stored.facts.title.to_ascii_lowercase();
        let first = targets
            .iter()
            .filter_map(|(repo, number, key)| {
                references(&title, repo, *number).map(|position| (position, *key))
            })
            .min_by_key(|(position, _)| *position);
        if let Some((_, key)) = first {
            merges
                .entry(key.clone())
                .or_default()
                .push(todo.key.clone());
        }
    }
    merges
}

/// Where `text` (lowercase) first references `repo#number`, as `owner/repo#n` (not inside a longer path) or as
/// `github.com/owner/repo/pull|issues/n`.
fn references(text: &str, repo: &RepoRef, number: u32) -> Option<usize> {
    let name = format!("{}/{}", repo.owner, repo.name).to_ascii_lowercase();
    let forms = [
        (format!("{name}#{number}"), true),
        (format!("github.com/{name}/pull/{number}"), false),
        (format!("github.com/{name}/issues/{number}"), false),
    ];
    forms
        .iter()
        .flat_map(|(form, bare)| {
            text.match_indices(form.as_str())
                .filter(move |(at, _)| {
                    let before = text[..*at].chars().next_back();
                    let after = text[*at + form.len()..].chars().next();
                    let starts = !*bare
                        || before.is_none_or(|c| !(c.is_alphanumeric() || "_./-".contains(c)));
                    starts && after.is_none_or(|c| !c.is_ascii_digit())
                })
                .map(|(at, _)| at)
        })
        .min()
}
