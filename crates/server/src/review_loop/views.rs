//! Read views of the review loop (RC1, RC2, RC4, RC5, RC10, RC11, RC16, RC17): built from the stored read, items,
//! assessments and decisions; no GitHub call.

use std::collections::{HashMap, HashSet};

use contracts::api::analyses::AnalysisSummary;
use contracts::api::deps::{CrateChanges, DependencyView, RepositoryKind};
use contracts::api::files::{PartOwner, PrFile};
use contracts::api::review_loop::{
    AssessmentProgress, AssessmentState, AuthorKind, CarryOverPage, CodeContext, CodeLine,
    CommentCounts, CommentItem, CommentItemKind, CommentLog, CommentsOverview, CommentsSummary,
    ContextQuery, DecideRest, DecideRestRow, DecisionBatchRow, DecisionVia, EffectState,
    EffectsPreview, FeedSubscriber, FlowStepRef, GithubRead, ImportedGithubDecision, ItemState,
    ItemsQuery, KindCount, NotAssessedReason, NoteElsewhere, NoteSource, PrBadgeView, PrPage,
    QueueEntry, Round, WatchState,
};
use contracts::api::tour::{CiState, Tour};
use contracts::common::{AnnotationType, LineRange, LineSide, PrRef};
use contracts::ids::{AnalysisId, CarryEventId, CommentItemId, CommitSha, StepId};
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::review_loop::{
    ItemRecord, PassCause, PassRecord, PassState, StoredDecision, WatchRecord,
};

use super::badge::{self, BadgeInputs, MyWork};
use super::observe::Snapshot;
use super::queue;
use super::{Loaded, LoopError, PrHandle, READ_INTERVAL};
use crate::auth::Session;
use crate::dashboard::plugin;
use crate::github::mirror;
use crate::pipeline::context::StoredResults;
use crate::pipeline::tour::{self, TourLoadError};
use crate::pipeline::{deps, files};
use crate::state::AppState;

/// Lines served by default around the commented lines.
const DEFAULT_CONTEXT_LINES: u32 = 40;
/// Most lines one context request serves.
const MAX_CONTEXT_LINES: u32 = 400;
/// Carry events per page (RC11).
const CARRY_PAGE: u32 = 50;

fn live_pass(pass: Option<&PassRecord>) -> Option<&PassRecord> {
    pass.filter(|pass| matches!(pass.state, PassState::Queued | PassState::Running))
}

fn sent_in(record: &ItemRecord, pass: &PassRecord) -> bool {
    pass.sent.iter().any(|sent| sent.item == record.id)
}

async fn reader_watch(
    state: &AppState,
    session: &Session,
    pr_id: i64,
) -> Result<Option<WatchRecord>, DbError> {
    Ok(state
        .db
        .review_loop()
        .watches(&session.user_id)
        .await?
        .into_iter()
        .find(|watch| watch.pull_request_id == pr_id))
}

fn assessment_state(
    loaded: &Loaded,
    record: &ItemRecord,
    pass: Option<&PassRecord>,
    limited: bool,
) -> AssessmentState {
    if record.status_comment || record.kind == CommentItemKind::PrQuestion {
        return AssessmentState::NotAssessed {
            reason: NotAssessedReason::NotRun,
        };
    }
    let live = live_pass(pass);
    let assessment = loaded.assessments.get(&record.no);
    if live.is_some_and(|pass| sent_in(record, pass)) {
        AssessmentState::Running
    } else if let Some(pass) = live
        && matches!(
            pass.cause,
            PassCause::Opened | PassCause::Read | PassCause::Watch
        )
        && pass.may_continue
        && waits(record)
        && assessment.is_none_or(|stored| {
            let head = loaded
                .snapshot
                .as_ref()
                .map_or(pass.head.0.as_str(), |snapshot| snapshot.head.as_str());
            stored.assessment.head.0 != head
        })
    {
        AssessmentState::Queued
    } else if assessment.is_some() {
        AssessmentState::Assessed
    } else if limited && waits(record) {
        AssessmentState::NotAssessed {
            reason: NotAssessedReason::DailyLimit,
        }
    } else {
        AssessmentState::NotAssessed {
            reason: match pass
                .filter(|pass| sent_in(record, pass))
                .map(|pass| pass.state)
            {
                Some(PassState::Failed) => NotAssessedReason::Failed,
                Some(PassState::Finished) => NotAssessedReason::Missing,
                Some(PassState::Queued | PassState::Running) | None => NotAssessedReason::NotRun,
            },
        }
    }
}

fn item_status(
    loaded: &Loaded,
    record: &ItemRecord,
    standing: &HashMap<String, &StoredDecision>,
    pass: Option<&PassRecord>,
    limited: bool,
) -> queue::StatusItem {
    let imported_decision = standing.get(&record.id.to_string()).and_then(|stored| {
        let decision = &stored.decision;
        if decision.via != DecisionVia::ImportedGithub {
            return None;
        }
        let imported = decision.imported.as_ref()?;
        Some(ImportedGithubDecision {
            kind: decision.kind,
            by: imported.by.clone(),
            at: imported.at,
            evidence_url: imported.evidence_url.clone(),
        })
    });
    queue::StatusItem {
        no: record.no,
        kind: record.kind,
        state: record.state,
        assessment_state: assessment_state(loaded, record, pass, limited),
        status_comment: record.status_comment,
        imported_decision,
        not_a_review_point: record
            .active_mark
            .as_ref()
            .map(|active| active.mark.clone()),
        duplicate_of: record.duplicate_of.and_then(|id| loaded.number_of(id)),
        decision_follows_primary: loaded.follower_link(record).is_some_and(|primary| {
            standing
                .get(&record.id.to_string())
                .is_some_and(|follower| {
                    standing
                        .get(&primary.to_string())
                        .is_some_and(|source| follower.primary_decision == Some(source.decision.id))
                })
        }),
        gone_at: record.gone_at,
        gates: loaded.gates(record),
    }
}

/// The item as the API shows it, including the reader's Ask answer count.
fn item_view(
    loaded: &Loaded,
    record: &ItemRecord,
    status: queue::StatusItem,
    standing: &HashMap<String, &StoredDecision>,
    ask_answers: u32,
) -> CommentItem {
    let assessment = loaded
        .assessments
        .get(&record.no)
        .filter(|_| !record.status_comment);
    let repeat_of = record.repeat_of.and_then(|id| loaded.number_of(id));
    CommentItem {
        not_a_review_point: status.not_a_review_point,
        status_comment: status.status_comment,
        imported_decision: status.imported_decision,
        import_proposal: (!standing.contains_key(&record.id.to_string()))
            .then(|| loaded.import_proposal(record))
            .flatten(),
        llm_title: assessment
            .and_then(|stored| stored.description.as_deref())
            .map(llm_title),
        decision_follows_primary: status.decision_follows_primary,
        point: record
            .quote
            .as_ref()
            .zip(record.url.as_ref())
            .and_then(|(quote, url)| {
                let source_kind = match record.kind {
                    contracts::api::review_loop::CommentItemKind::ReviewBody => {
                        contracts::api::review_loop::CommentSourceKind::ReviewBody
                    }
                    contracts::api::review_loop::CommentItemKind::PrComment => {
                        contracts::api::review_loop::CommentSourceKind::PrComment
                    }
                    _ => return None,
                };
                Some(contracts::api::review_loop::CommentPoint {
                    source_url: url.clone(),
                    source_kind,
                    quote: quote.clone(),
                })
            }),
        duplicate_of: status.duplicate_of,
        id: record.id,
        no: status.no,
        kind: status.kind,
        stable_key: record.stable_key.clone(),
        author: record.author.clone(),
        author_kind: record.author_kind,
        url: record.url.clone(),
        location: record.location.clone(),
        title: record.title.clone(),
        body: record.body.clone(),
        bot_severity: record.bot_severity.clone(),
        gates: status.gates,
        state: status.state,
        revision: record.revision,
        first_seen_head: record.first_seen_head.clone(),
        first_round: record.first_round,
        written: loaded
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.written.get(&record.stable_key))
            .cloned(),
        github: record.github.clone(),
        outside: record.outside.clone(),
        gone_at: status.gone_at,
        repeat_of,
        assessment_state: status.assessment_state,
        assessment: assessment.map(|stored| stored.assessment.clone()),
        decisions: loaded
            .decisions
            .iter()
            .filter(|stored| stored.item == record.id)
            .map(|stored| stored.decision.clone())
            .collect(),
        ask_answers,
    }
}

fn llm_title(description: &str) -> String {
    const ABBREVIATIONS: &[&str] = &["e.g.", "i.e.", "vs.", "etc.", "cf."];
    let text = description.split_whitespace().collect::<Vec<_>>().join(" ");
    let first = text
        .match_indices(['.', '!', '?'])
        .find_map(|(index, mark)| {
            let end = index + mark.len();
            let sentence = &text[..end];
            let last = sentence
                .rsplit(' ')
                .next()
                .unwrap_or_default()
                .trim_start_matches(['(', '[', '"', '\'', '`']);
            let abbreviated = mark == "."
                && ABBREVIATIONS
                    .iter()
                    .any(|word| last.eq_ignore_ascii_case(word));
            (text[end..].starts_with(' ') && !abbreviated).then_some(sentence)
        })
        .unwrap_or(&text);
    if first.chars().count() <= 90 {
        return first.to_owned();
    }
    let prefix: String = first.chars().take(89).collect();
    let cut = if first[prefix.len()..].starts_with(' ') {
        prefix.as_str()
    } else {
        prefix
            .rsplit_once(' ')
            .map_or(prefix.as_str(), |(whole, _)| whole)
    };
    format!("{cut}…")
}

/// RC4: the items with the reader's Ask answer counts.
pub async fn items(
    state: &AppState,
    session: &Session,
    handle: PrHandle,
    query: &ItemsQuery,
) -> Result<Vec<CommentItem>, LoopError> {
    let loaded = Loaded::of(state, handle).await?;
    let pass = state.db.review_loop().latest_pass(loaded.handle.id).await?;
    let answers: HashMap<CommentItemId, u32> = state
        .db
        .questions()
        .answers_per_item(loaded.handle.id, &session.user_id)
        .await?
        .into_iter()
        .collect();
    let limited = reader_watch(state, session, loaded.handle.id)
        .await?
        .is_some_and(|watch| watch.state == WatchState::LimitReached);
    let standing = loaded.standing();
    let unseen = if query.imported_unseen == Some(true) {
        Some(
            state
                .db
                .review_loop()
                .unseen_comment_imports(
                    &session.tenant_id,
                    &session.user_id,
                    loaded.handle.id,
                    false,
                )
                .await?
                .into_iter()
                .collect::<std::collections::HashSet<_>>(),
        )
    } else {
        None
    };
    Ok(loaded
        .items
        .iter()
        .filter(|record| unseen.as_ref().is_none_or(|ids| ids.contains(&record.id)))
        .filter(|record| query.state.is_none_or(|wanted| record.state == wanted))
        .filter(|record| query.kind.is_none_or(|wanted| record.kind == wanted))
        .map(|record| {
            let asked = answers.get(&record.id).copied().unwrap_or(0);
            let status = item_status(&loaded, record, &standing, pass.as_ref(), limited);
            item_view(&loaded, record, status, &standing, asked)
        })
        .collect())
}

fn read_view(loaded: &Loaded) -> GithubRead {
    let (threads, reviews, comments) = loaded
        .snapshot
        .as_ref()
        .map_or((0, 0, 0), |snapshot| snapshot.counts);
    GithubRead {
        read_at: loaded.row.read_at,
        threads,
        complete: loaded.snapshot.as_ref().is_some_and(|s| s.complete),
        reviews,
        comments,
        failure: loaded.row.read_failure.map(|(failure, _)| failure),
        next_read_at: loaded.row.read_at.map(|at| at + READ_INTERVAL),
    }
}

fn effects_open(stored: &StoredDecision) -> bool {
    stored.decision.effects.iter().any(|effect| {
        matches!(
            effect.state,
            EffectState::Waiting | EffectState::Open | EffectState::Claimed
        )
    })
}

fn failed(stored: &StoredDecision) -> u32 {
    u32::try_from(
        stored
            .decision
            .effects
            .iter()
            .filter(|effect| effect.state == EffectState::Failed)
            .count(),
    )
    .unwrap_or(u32::MAX)
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn needs_you(
    loaded: &Loaded,
    entries: &[QueueEntry],
    pass: Option<&PassRecord>,
    limited: bool,
) -> (u32, u32, u32) {
    let standing = loaded.standing();
    let items: HashMap<_, _> = loaded
        .items
        .iter()
        .map(|record| {
            (
                record.no,
                item_status(loaded, record, &standing, pass, limited),
            )
        })
        .collect();
    let (needs_you, gates) = queue::needs_you(entries, &items);
    let marked = count(
        items
            .values()
            .filter(|item| queue::comment_status(item) == queue::CommentStatus::NotAReviewPoint)
            .count(),
    );
    (needs_you, gates, marked)
}

/// The Needs you count from one dashboard PR's batched data.
pub(crate) fn dashboard_needs_you(
    handle: PrHandle,
    view: db::repo::review_loop::PrTallyView,
) -> Result<u32, DbError> {
    let loaded = Loaded::from_view(handle, view.pr).ok_or(DbError::NotFound)?;
    let entries = queue::queue(&loaded.queue_items(&[]));
    Ok(needs_you(&loaded, &entries, view.latest_pass.as_ref(), false).0)
}

fn counts(
    loaded: &Loaded,
    waiting: usize,
    (needs_you, needs_you_gates, marked): (u32, u32, u32),
) -> CommentCounts {
    let standing = loaded.standing();
    CommentCounts {
        not_a_review_point: Some(marked),
        needs_you: Some(needs_you),
        needs_you_gates: Some(needs_you_gates),
        imported_unseen: None,
        unresolved_threads: loaded
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.unresolved_threads),
        undecided: count(waiting),
        asked_again: count(
            loaded
                .items
                .iter()
                .filter(|item| item.state == ItemState::Reopened && waits(item))
                .count(),
        ),
        waiting_for_agents: count(standing.values().filter(|s| effects_open(s)).count()),
        failed_effects: standing.values().map(|s| failed(s)).sum(),
        outside: count(
            loaded
                .items
                .iter()
                .filter(|item| item.state == ItemState::Outside)
                .count(),
        ),
        total: count(loaded.items.len()),
    }
}

/// Assessment progress; `limited`: the daily limit holds the reader's automatic passes back, so the items not
/// assessed at the head wait for it.
fn progress(loaded: &Loaded, pass: Option<&PassRecord>, limited: bool) -> AssessmentProgress {
    let needing: Vec<&ItemRecord> = loaded
        .items
        .iter()
        .filter(|item| {
            item.kind != contracts::api::review_loop::CommentItemKind::PrQuestion
                && item.state != ItemState::Outside
                && !item.status_comment
                && item.active_mark.is_none()
                && item.gone_at.is_none()
        })
        .collect();
    let head = loaded.snapshot.as_ref().map(|s| CommitSha(s.head.clone()));
    let assessed = needing
        .iter()
        .filter(|item| {
            loaded
                .assessments
                .get(&item.no)
                .is_some_and(|stored| Some(&stored.assessment.head) == head.as_ref())
        })
        .count();
    let latest = loaded
        .assessments
        .values()
        .max_by_key(|stored| stored.assessment.assessed_at);
    let live = live_pass(pass);
    let waiting = needing
        .iter()
        .filter(|item| assessment_state(loaded, item, pass, limited) == AssessmentState::Queued)
        .count();
    AssessmentProgress {
        in_pass: Some(live.map_or(0, |pass| count(pass.sent.len()))),
        waiting: Some(count(waiting)),
        since: live.map(|pass| pass.created_at),
        running: live.is_some(),
        assessed: count(assessed),
        total: count(needing.len()),
        limited: if limited && live.is_none() {
            count(needing.len().saturating_sub(assessed))
        } else {
            0
        },
        head: latest.map(|stored| stored.assessment.head.clone()),
        at: pass.and_then(|pass| pass.finished_at),
    }
}

/// Open and still produced by the read: an item marked `gone_at` (such as a superseded body finding) waits for nobody.
fn waits(item: &ItemRecord) -> bool {
    queue::is_open(item.state)
        && item.gone_at.is_none()
        && !item.status_comment
        && item.active_mark.is_none()
}

fn rounds(loaded: &Loaded, events: &[contracts::api::review_loop::CarryEvent]) -> Vec<Round> {
    let Some(snapshot) = &loaded.snapshot else {
        return Vec::new();
    };
    let standing = loaded.standing();
    snapshot
        .rounds
        .iter()
        .enumerate()
        .map(|(index, round)| {
            let next = snapshot.rounds.get(index + 1).map(|next| next.finished_at);
            let within =
                |at: Timestamp| at >= round.finished_at && next.is_none_or(|next| at < next);
            let of_round: Vec<&ItemRecord> = loaded
                .items
                .iter()
                .filter(|item| item.first_round == Some(round.number))
                .collect();
            let mut kinds: Vec<KindCount> = Vec::new();
            for item in &of_round {
                if let Some(stored) = standing.get(&item.id.to_string()) {
                    match kinds.iter_mut().find(|k| k.kind == stored.decision.kind) {
                        Some(kind) => kind.count += 1,
                        None => kinds.push(KindCount {
                            kind: stored.decision.kind,
                            count: 1,
                        }),
                    }
                }
            }
            let carried = |kind| {
                count(
                    events
                        .iter()
                        .filter(|event| event.kind == kind && within(event.at))
                        .count(),
                )
            };
            Round {
                number: round.number,
                url: round.url.clone(),
                head: CommitSha(round.head.clone()),
                finished_at: round.finished_at,
                verdict: round.verdict.clone(),
                new: count(of_round.len()),
                still_open: count(
                    loaded
                        .items
                        .iter()
                        .filter(|item| item.first_round.is_some_and(|first| first <= round.number))
                        .filter(|item| waits(item))
                        .count(),
                ),
                carried: carried(contracts::api::review_loop::CarryKind::Carried),
                asked_again: carried(contracts::api::review_loop::CarryKind::AskedAgain),
                undecided: count(of_round.iter().filter(|item| waits(item)).count()),
                decisions: kinds,
            }
        })
        .collect()
}

/// The badge of a loaded PR, with the reader's [`MyWork`] rules.
fn badge_of(
    state: &AppState,
    loaded: &Loaded,
    queue_items: &[queue::QueueItem],
    needs_you: u32,
    pass: Option<&PassRecord>,
    my_work: &MyWork,
) -> PrBadgeView {
    let standing = loaded.standing();
    let snapshot = loaded.snapshot.as_ref();
    let now = Timestamp::now();
    let overlay = state.plugins.overlay(now);
    let pr = &loaded.handle.pr;
    let (ci, ignored_not_passing) = snapshot
        .and_then(|snapshot| Some((snapshot.ci.as_ref()?, &snapshot.pending_checks)))
        .map(|(ci, pending)| {
            badge::without_ignored(ci, pending, |name| my_work.is_ignored_check(&pr.repo, name))
        })
        .unzip();
    let waiting: HashSet<u32> = queue::waiting(queue_items)
        .into_iter()
        .map(|item| item.no)
        .collect();
    let review_items = loaded.items.iter().filter(|item| {
        waiting.contains(&item.no)
            && item.kind != CommentItemKind::PrQuestion
            && item.author_kind != AuthorKind::Bot
    });
    let pending_human_review = count(
        review_items
            .clone()
            .filter(|item| !loaded.assessments.contains_key(&item.no))
            .count(),
    );
    let reassessing_human_review =
        pass.filter(|pass| pass.state == PassState::Running)
            .map_or(0, |pass| {
                count(
                    review_items
                        .filter(|item| {
                            loaded.assessments.contains_key(&item.no) && sent_in(item, pass)
                        })
                        .count(),
                )
            });
    badge::badge(&BadgeInputs {
        open: snapshot.is_none_or(|snapshot| snapshot.state == "OPEN"),
        read_failed: loaded.row.read_failure.is_some() || snapshot.is_none(),
        ci,
        ignored_not_passing: ignored_not_passing.unwrap_or_default(),
        ready_to_merge: snapshot.and_then(|snapshot| {
            my_work.ready_to_merge(
                &pr.repo,
                pr.number,
                &snapshot.head,
                &snapshot.base,
                snapshot.base_sha.as_deref()?,
            )
        }),
        draft: snapshot.is_some_and(|snapshot| snapshot.draft),
        unresolved_threads: snapshot.map_or(0, |snapshot| snapshot.unresolved_threads),
        failed_effects: standing.values().map(|s| failed(s)).sum(),
        needs_you,
        pending_human_review,
        reassessing_human_review,
        executing: count(standing.values().filter(|s| effects_open(s)).count()),
        bot_round: super::bot_round_running(state, pr, snapshot),
        changes_requested: snapshot.map_or_else(Vec::new, |s| s.changes_requested_by.clone()),
        held_by: overlay.hold(&pr.repo, pr.number).map(|hold| hold.source),
        holders_not_current: state
            .plugins
            .statuses(now)
            .into_iter()
            .filter(|status| {
                status.holds_ready_to_merge
                    && status.state != contracts::api::dashboard::PluginState::Current
            })
            .map(|status| status.source)
            .collect(),
    })
}

/// The PR's state badge from what is stored, with the reader's [`MyWork`] rules.
pub(crate) async fn badge(
    state: &AppState,
    handle: PrHandle,
    my_work: &MyWork,
) -> Result<PrBadgeView, LoopError> {
    let loaded = Loaded::of(state, handle).await?;
    let pass = state.db.review_loop().latest_pass(loaded.handle.id).await?;
    let queue_items = loaded.queue_items(&[]);
    let entries = queue::queue(&queue_items);
    let needs_you = needs_you(&loaded, &entries, pass.as_ref(), false).0;
    Ok(badge_of(
        state,
        &loaded,
        &queue_items,
        needs_you,
        pass.as_ref(),
        my_work,
    ))
}

async fn latest_analysis(
    state: &AppState,
    session: &Session,
    pr_id: i64,
) -> Result<Option<AnalysisSummary>, LoopError> {
    let user = state.db.users().get(&session.user_id).await?;
    let Some(record) = state
        .db
        .analyses()
        .latest_for_pr(pr_id, user.language)
        .await?
        .filter(|record| record.tenant_id == session.tenant_id)
    else {
        return Ok(None);
    };
    Ok(Some(
        state
            .db
            .analyses()
            .summary_for_user(record.id, &session.user_id)
            .await?,
    ))
}

fn pr_url(pr: &PrRef) -> String {
    format!(
        "https://github.com/{}/{}/pull/{}",
        pr.repo.owner, pr.repo.name, pr.number
    )
}

/// RC2: the stored Comments overview and the reader's watch.
pub async fn overview(
    state: &AppState,
    session: &Session,
    handle: PrHandle,
) -> Result<CommentsOverview, LoopError> {
    let loaded = Loaded::of(state, handle).await?;
    let snapshot = loaded.snapshot()?;
    let rl = state.db.review_loop();
    let skips = rl.skips(&session.user_id, loaded.handle.id).await?;
    let queue_items = loaded.queue_items(&skips);
    let entries = queue::queue(&queue_items);
    let waiting = entries.iter().map(|entry| entry.item_nos.len()).sum();
    let pass = rl.latest_pass(loaded.handle.id).await?;
    let watch = reader_watch(state, session, loaded.handle.id).await?;
    let limited = watch
        .as_ref()
        .is_some_and(|watch| watch.state == WatchState::LimitReached);
    let needs_you = needs_you(&loaded, &entries, pass.as_ref(), limited);
    let badge = badge_of(
        state,
        &loaded,
        &queue_items,
        needs_you.0,
        pass.as_ref(),
        &badge::my_work(state, &session.tenant_id, &session.user_id).await?,
    );
    let events = rl
        .carry_events(&session.tenant_id, Some(loaded.handle.id), None, u32::MAX)
        .await?;
    let analysis = latest_analysis(state, session, loaded.handle.id)
        .await?
        .map(|summary| summary.id);
    let mut counts = counts(&loaded, waiting, needs_you);
    if !session.is_access_token() {
        counts.imported_unseen = Some(count(
            rl.unseen_comment_imports(&session.tenant_id, &session.user_id, loaded.handle.id, true)
                .await?
                .len(),
        ));
    }
    Ok(CommentsOverview {
        watch: watch.map(|watch| super::watch::view(watch, badge.clone())),
        pr: loaded.handle.pr.clone(),
        title: snapshot.title.clone(),
        url: if snapshot.url.is_empty() {
            pr_url(&loaded.handle.pr)
        } else {
            snapshot.url.clone()
        },
        head: CommitSha(snapshot.head.clone()),
        purpose: loaded.row.purpose.clone(),
        read: read_view(&loaded),
        counts,
        rounds: rounds(&loaded, &events),
        assessment: progress(&loaded, pass.as_ref(), limited),
        queue: entries,
        badge,
        analysis,
    })
}

/// RC1. A pull request never read from GitHub still shows its analysis: the header comes from the stored pull
/// request and the comments card names the failed read.
pub async fn pr_page(
    state: &AppState,
    session: &Session,
    handle: PrHandle,
) -> Result<PrPage, LoopError> {
    let loaded = Loaded::of(state, handle).await?;
    let snapshot = loaded.snapshot.as_ref();
    let rl = state.db.review_loop();
    let queue_items = loaded.queue_items(&[]);
    let entries = queue::queue(&queue_items);
    let waiting = entries.iter().map(|entry| entry.item_nos.len()).sum();
    let pass = rl.latest_pass(loaded.handle.id).await?;
    let my_work = badge::my_work(state, &session.tenant_id, &session.user_id).await?;
    let pr = &loaded.handle.pr;
    let stored = state.db.pull_requests().get(loaded.handle.id).await?.pr;
    let watch = reader_watch(state, session, loaded.handle.id).await?;
    let limited = watch
        .as_ref()
        .is_some_and(|watch| watch.state == WatchState::LimitReached);
    let needs_you = needs_you(&loaded, &entries, pass.as_ref(), limited);
    let badge = badge_of(
        state,
        &loaded,
        &queue_items,
        needs_you.0,
        pass.as_ref(),
        &my_work,
    );
    let watch = watch.map(|watch| super::watch::view(watch, badge.clone()));
    let url = snapshot.map_or_else(|| stored.html_url.clone(), |snapshot| snapshot.url.clone());
    let url = if url.is_empty() { pr_url(pr) } else { url };
    Ok(PrPage {
        pr: pr.clone(),
        title: snapshot.map_or_else(|| stored.title.clone(), |s| s.title.clone()),
        author: stored.author,
        url,
        state: snapshot.map(|s| s.state.clone()).unwrap_or_default(),
        draft: snapshot.is_some_and(|s| s.draft),
        base: snapshot.map(|s| s.base.clone()).unwrap_or_default(),
        // TODO(WP13-WEB-S): name the PR whose head branch is the base (stacked PRs); needs a GitHub lookup.
        stack_parent: None,
        head: snapshot.map_or(stored.head_sha, |s| CommitSha(s.head.clone())),
        ci: snapshot
            .and_then(|s| s.ci.clone())
            .filter(|ci| ci.state != CiState::NotChecked),
        plugin_notes: state
            .plugins
            .overlay(Timestamp::now())
            .notes(&pr.repo, pr.number),
        watch,
        badge: badge.clone(),
        analysis: latest_analysis(state, session, loaded.handle.id).await?,
        comments: CommentsSummary {
            counts: counts(&loaded, waiting, needs_you),
            rounds: snapshot.map_or(0, |s| count(s.rounds.len())),
            assessment: progress(&loaded, pass.as_ref(), limited),
            read: read_view(&loaded),
        },
    })
}

/// RC10.
pub async fn log(
    state: &AppState,
    session: &Session,
    handle: PrHandle,
) -> Result<CommentLog, LoopError> {
    let rl = state.db.review_loop();
    let decisions = rl.pr_decisions(handle.id).await?;
    let standing = super::standing(&decisions);
    let mut carry_events = rl
        .carry_events(&session.tenant_id, Some(handle.id), None, u32::MAX)
        .await?;
    carry_events.reverse();
    let subscribers = rl
        .subscriptions(&session.tenant_id, Timestamp::now())
        .await?
        .into_iter()
        .map(|subscription| FeedSubscriber {
            connected: state.feed.is_connected(&subscription.token_id),
            // Another user's target only as its kind and delivery state: its URL or topic can be a credential.
            push: super::notify::push_view(
                state,
                &subscription,
                subscription.user_id == session.user_id,
            ),
            token_name: subscription.token_name,
            capabilities: subscription.capabilities,
            last_seen_at: subscription.last_seen_at,
        })
        .collect();
    Ok(CommentLog {
        marks: rl.mark_log(&session.tenant_id, handle.id).await?,
        subscribers,
        open_effects: count(
            standing
                .values()
                .flat_map(|s| s.decision.effects.iter())
                .filter(|effect| {
                    matches!(
                        effect.state,
                        EffectState::Waiting | EffectState::Open | EffectState::Claimed
                    )
                })
                .count(),
        ),
        failed_effects: standing.values().map(|s| failed(s)).sum(),
        decisions: decisions
            .into_iter()
            .map(|stored| stored.decision)
            .collect(),
        carry_events,
    })
}

fn cursor_of(event: &contracts::api::review_loop::CarryEvent) -> String {
    format!("{}~{}", event.at, event.id)
}

fn parse_cursor(cursor: &str) -> Option<(Timestamp, CarryEventId)> {
    let (at, id) = cursor.split_once('~')?;
    Some((
        at.parse().ok()?,
        CarryEventId(uuid::Uuid::parse_str(id).ok()?),
    ))
}

/// RC11: one PR (`pr` = `owner/name/number`) or all of the tenant.
pub async fn carry_overs(
    state: &AppState,
    session: &Session,
    pr: Option<PrHandle>,
    cursor: Option<&str>,
) -> Result<CarryOverPage, LoopError> {
    let rl = state.db.review_loop();
    let before = match cursor {
        Some(cursor) => Some(parse_cursor(cursor).ok_or(LoopError::InvalidCursor)?),
        None => None,
    };
    let pr_id = pr.as_ref().map(|handle| handle.id);
    let mut events = rl
        .carry_events(&session.tenant_id, pr_id, before, CARRY_PAGE + 1)
        .await?;
    let next_cursor = if events.len() > CARRY_PAGE as usize {
        events.truncate(CARRY_PAGE as usize);
        events.last().map(cursor_of)
    } else {
        None
    };
    let counts = rl.carry_counts(&session.tenant_id, pr_id).await?;
    let mut totals = contracts::api::review_loop::CarryCounts::default();
    let mut per_pr = Vec::new();
    for (id, counts) in counts {
        totals.carried += counts.carried;
        totals.asked_again_bot_edited += counts.asked_again_bot_edited;
        totals.asked_again_code_changed += counts.asked_again_code_changed;
        totals.effects_pending += counts.effects_pending;
        let pr_ref = match &pr {
            Some(handle) if handle.id == id => handle.pr.clone(),
            _ => pr_ref_of(state, &session.tenant_id, id).await?,
        };
        per_pr.push(contracts::api::review_loop::PrCarryCounts { pr: pr_ref, counts });
    }
    Ok(CarryOverPage {
        totals,
        per_pr,
        events,
        next_cursor,
    })
}

/// The owner/name/number of a stored PR, from its carry events (every PR with counts has one).
async fn pr_ref_of(state: &AppState, tenant: &str, id: i64) -> Result<PrRef, LoopError> {
    state
        .db
        .review_loop()
        .carry_events(tenant, Some(id), None, 1)
        .await?
        .into_iter()
        .next()
        .map(|event| event.pr)
        .ok_or(LoopError::NotFound)
}

/// RC16.
pub async fn preview(
    state: &AppState,
    handle: PrHandle,
    rows: &[DecisionBatchRow],
) -> Result<Vec<EffectsPreview>, LoopError> {
    let loaded = Loaded::of(state, handle).await?;
    let deciding: Vec<u32> = rows.iter().map(|row| row.item_no).collect();
    rows.iter()
        .zip(loaded.plan(rows)?)
        .map(|(row, (record, planned))| {
            Ok(EffectsPreview {
                item_no: record.no,
                plan_digest: loaded.plan_digest(row, &deciding, &planned.effects)?,
                effects: planned.effects,
            })
        })
        .collect()
}

/// RC17.
pub async fn decide_rest(state: &AppState, handle: PrHandle) -> Result<DecideRest, LoopError> {
    let loaded = Loaded::of(state, handle).await?;
    let queue_items = loaded.queue_items(&[]);
    let (proposed, left_out) = queue::decide_rest(&queue_items);
    let mut rows = Vec::new();
    let mut left_out = left_out;
    let batch: Vec<DecisionBatchRow> = proposed
        .iter()
        .map(|(item, request)| DecisionBatchRow {
            item_no: item.no,
            decision: request.clone(),
        })
        .collect();
    for row in batch {
        match loaded.plan(std::slice::from_ref(&row)) {
            Ok(mut planned) => {
                let Some((_, planned)) = planned.pop() else {
                    continue;
                };
                let Some(recommendation) = loaded
                    .assessments
                    .get(&row.item_no)
                    .map(|stored| stored.assessment.recommendation.clone())
                else {
                    continue;
                };
                rows.push(DecideRestRow {
                    row,
                    recommendation,
                    effects: planned.effects,
                });
            }
            Err(_) => left_out.push(contracts::api::review_loop::LeftOutItem {
                item_no: row.item_no,
                reason: contracts::api::review_loop::LeftOutReason::Incomplete,
            }),
        }
    }
    left_out.sort_by_key(|item| item.item_no);
    Ok(DecideRest { rows, left_out })
}

/// RC5: lines of the item's file at the PR head (fetched into the mirror on demand), the commented range, the
/// assessment's enclosing range, the flow steps containing the lines and plug-in notes.
pub async fn context(
    state: &AppState,
    session: &Session,
    handle: PrHandle,
    no: u32,
    query: &ContextQuery,
) -> Result<CodeContext, LoopError> {
    let loaded = Loaded::of(state, handle).await?;
    let snapshot: &Snapshot = loaded.snapshot()?;
    let record = loaded.item(no)?;
    let location = record.location.as_ref().ok_or(LoopError::NotFound)?;
    let head = CommitSha(snapshot.head.clone());
    let code = loaded
        .assessments
        .get(&no)
        .and_then(|stored| stored.assessment.code.clone())
        .filter(|code| code.path == location.path);
    let mirror = mirror::ensure_commits(&state.config, &loaded.handle.pr.repo, &[&head]).await?;
    let bytes = mirror::read_blob(&mirror, &head, &location.path)
        .await?
        .ok_or(LoopError::NotFound)?;
    let text = String::from_utf8_lossy(&bytes);
    let all: Vec<&str> = text.lines().collect();
    let total = count(all.len());
    let anchor = code
        .as_ref()
        .map(|code| code.enclosing)
        .or(location.lines)
        .unwrap_or(LineRange {
            start_line: 1,
            end_line: 1,
        });
    let (from, to) = match (query.from, query.to) {
        (Some(from), Some(to)) => (from, to),
        _ => {
            let span = anchor.end_line.saturating_sub(anchor.start_line) + 1;
            let pad = DEFAULT_CONTEXT_LINES.saturating_sub(span) / 2;
            let from = anchor.start_line.saturating_sub(pad).max(1);
            (from, from + DEFAULT_CONTEXT_LINES.max(span) - 1)
        }
    };
    if from == 0 || to < from {
        return Err(LoopError::BadRange);
    }
    if to - from + 1 > MAX_CONTEXT_LINES {
        return Err(LoopError::RangeTooLarge);
    }
    let to = to.min(total);
    let lines = (from..=to)
        .filter_map(|line| {
            let text = all.get(usize::try_from(line).ok()? - 1)?;
            Some(CodeLine {
                line,
                text: (*text).to_owned(),
            })
        })
        .collect();
    let from_analysis = analysis_context(
        state,
        session,
        loaded.handle.id,
        &location.path,
        location.lines,
    )
    .await;
    let mut notes = from_analysis.notes;
    notes.extend(
        state
            .plugins
            .overlay(Timestamp::now())
            .notes(&loaded.handle.pr.repo, loaded.handle.pr.number)
            .into_iter()
            .filter_map(|note| {
                let source = match note.plugin.as_str() {
                    plugin::pr_hygiene::ID => NoteSource::PrHygiene,
                    plugin::thepastaclaw::ID => NoteSource::Thepastaclaw,
                    _ => return None,
                };
                Some(NoteElsewhere {
                    source,
                    label: format!(
                        "{}: {}",
                        note.source,
                        note.summary.unwrap_or_else(|| "statement".to_owned())
                    ),
                    count: None,
                    url: note.url,
                })
            }),
    );
    let (analysis, analysis_head, in_flows) = (
        from_analysis.analysis,
        from_analysis.head,
        from_analysis.in_flows,
    );
    Ok(CodeContext {
        path: location.path.clone(),
        head,
        lines,
        total_lines: total,
        marked: location.lines,
        code,
        analysis,
        analysis_head,
        in_flows,
        notes,
    })
}

/// What the PR's latest review-ready analysis says about an item's lines.
#[derive(Default)]
struct AnalysisContext {
    analysis: Option<AnalysisId>,
    head: Option<CommitSha>,
    /// Flow steps whose head lines contain the lines.
    in_flows: Vec<FlowStepRef>,
    /// Flows flags on those lines, Files incidental notes on them, Dependencies notes of the file's crate.
    notes: Vec<NoteElsewhere>,
}

/// The flow steps containing `lines` of `path` and the notes about them elsewhere, from the stored documents of the
/// PR's latest analysis (its Tour, Files and Dependencies views); nothing without a review-ready analysis, and only
/// the steps when a view cannot be built.
async fn analysis_context(
    state: &AppState,
    session: &Session,
    pr_id: i64,
    path: &str,
    lines: Option<LineRange>,
) -> AnalysisContext {
    let summary = match latest_analysis(state, session, pr_id).await {
        Ok(Some(summary)) => summary,
        Ok(None) => return AnalysisContext::default(),
        Err(error) => {
            tracing::debug!(?error, "no analysis read for the comment context");
            return AnalysisContext::default();
        }
    };
    let found = AnalysisContext {
        analysis: Some(summary.id),
        head: Some(summary.pr.head_sha.clone()),
        ..AnalysisContext::default()
    };
    let Some(lines) = lines else {
        return found;
    };
    let (tour, files, deps) = match analysis_views(state, session, &summary).await {
        Ok(views) => views,
        Err(error) => {
            tracing::debug!(?error, "no Tour for the comment context");
            return found;
        }
    };
    let (in_flows, notes) = notes_on(summary.id, &tour, &files, deps.as_ref(), path, lines);
    AnalysisContext {
        in_flows,
        notes,
        ..found
    }
}

/// The flow steps whose head lines contain `lines` of `path`, and the notes elsewhere about them: the LLM's Flows
/// flags on those lines, Files incidental-change notes on them and the Dependencies notes of the file's crate.
fn notes_on(
    analysis: AnalysisId,
    tour: &Tour,
    files: &[PrFile],
    deps: Option<&DependencyView>,
    path: &str,
    lines: LineRange,
) -> (Vec<FlowStepRef>, Vec<NoteElsewhere>) {
    let base = format!("/analyses/{analysis}");
    let overlaps = |range: &LineRange| {
        range.start_line <= lines.end_line && lines.start_line <= range.end_line
    };
    let mut in_flows = Vec::new();
    let mut flags = 0_u32;
    let mut flagged: Option<StepId> = None;
    for flow in &tour.flows {
        for step in &flow.steps {
            if step.facts.side != LineSide::Head
                || step.facts.file != path
                || !overlaps(&step.facts.range)
            {
                continue;
            }
            in_flows.push(FlowStepRef {
                flow: flow.id.clone(),
                flow_position: flow.position,
                flow_title: flow.llm.title.clone(),
                step: step.id.clone(),
                step_number: step.number,
            });
            let here = step
                .llm
                .annotations
                .iter()
                .filter(|annotation| {
                    annotation.kind == AnnotationType::Flag && overlaps(&annotation.range)
                })
                .count();
            if here > 0 && flagged.is_none() {
                flagged = Some(step.id.clone());
            }
            flags += count(here);
        }
    }
    let mut notes = Vec::new();
    if let Some(step) = flagged {
        notes.push(NoteElsewhere {
            source: NoteSource::Flows,
            label: format!(
                "Flows: the LLM flagged these lines ({flags} {})",
                if flags == 1 { "flag" } else { "flags" }
            ),
            count: Some(flags),
            url: format!("{base}#{step}"),
        });
    }
    let incidental = files
        .iter()
        .filter(|file| file.path == path)
        .flat_map(|file| file.hunks.iter().flat_map(|hunk| hunk.parts.iter()))
        .filter(|part| {
            part.side == LineSide::Head
                && matches!(part.owner, PartOwner::Incidental { .. })
                && overlaps(&part.range)
        })
        .count();
    if incidental > 0 {
        let link: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("file", path)
            .append_pair("side", "head")
            .append_pair("line", &lines.start_line.to_string())
            .append_pair("end", &lines.end_line.to_string())
            .finish();
        notes.push(NoteElsewhere {
            source: NoteSource::Files,
            label: "Files: incidental-change note on these lines".to_owned(),
            count: Some(count(incidental)),
            url: format!("{base}/files#lines?{link}"),
        });
    }
    if let Some((name, changes)) = deps.and_then(|view| crate_of(view, path)) {
        let crate_notes = count(changes.notes.len()) + changes.notes_hidden;
        if crate_notes > 0 {
            notes.push(NoteElsewhere {
                source: NoteSource::Dependencies,
                label: format!(
                    "Dependencies: {crate_notes} {} on what changed in crate {name}",
                    if crate_notes == 1 { "note" } else { "notes" }
                ),
                count: Some(crate_notes),
                url: format!("{base}/deps"),
            });
        }
    }
    (in_flows, notes)
}

/// The Tour, Files and Dependencies views of a review-ready analysis from its stored documents (a dependency map
/// is not computed here: without a stored one there is no Dependencies view).
async fn analysis_views(
    state: &AppState,
    session: &Session,
    summary: &AnalysisSummary,
) -> Result<(Tour, Vec<PrFile>, Option<DependencyView>), TourLoadError> {
    tour::ready(summary)?;
    let results = StoredResults::load(&state.db, summary.id).await?;
    let plan = results.plan.clone();
    let map = deps::stored(state, summary.id).await?;
    let controlled = state
        .db
        .settings()
        .controlled_owners(&session.tenant_id)
        .await?;
    tour::with_projection(state, summary, results, |projection, tour| {
        let dependencies = map.as_ref().map(|map| {
            deps::project(&deps::ProjectionInput {
                map,
                pr_repo: &summary.pr.repo,
                files: projection.files,
                plan: plan.as_ref(),
                controlled: &controlled.owners,
            })
        });
        let files = files::from_tour(projection, &tour);
        (tour, files, dependencies)
    })
    .await
}

/// The changed crate of this repository holding `path` (the deepest manifest directory above it).
fn crate_of<'a>(view: &'a DependencyView, path: &str) -> Option<(&'a str, &'a CrateChanges)> {
    view.repositories
        .iter()
        .filter(|repository| repository.kind == RepositoryKind::This)
        .flat_map(|repository| repository.crates.iter())
        .filter_map(|krate| {
            let dir = krate.manifest.as_deref()?.strip_suffix("Cargo.toml")?;
            let changes = krate.changes.as_ref()?;
            path.starts_with(dir)
                .then_some((dir.len(), krate.name.as_str(), changes))
        })
        .max_by_key(|(depth, ..)| *depth)
        .map(|(_, name, changes)| (name, changes))
}

#[cfg(test)]
mod notes_tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn llm_title_stops_after_the_first_short_sentence() {
        assert_eq!(
            llm_title("`item.path` is unchecked!\nA second sentence explains why."),
            "`item.path` is unchecked!"
        );
    }

    #[test]
    fn llm_title_keeps_abbreviations_and_marks_truncation() {
        let sentence = "Use retries, e.g. for 503, i.e. transient errors, vs. validation failures etc. before returning.";
        let title = "Use retries, e.g. for 503, i.e. transient errors, vs. validation failures etc. before…";
        assert_eq!(llm_title(&format!("{sentence} Another sentence.")), title);
        let exact = "Ł".repeat(90);
        assert_eq!(llm_title(&exact), exact);
        let too_long = "Ł".repeat(91);
        assert_eq!(llm_title(&too_long), format!("{}…", "Ł".repeat(89)));
        assert_eq!(
            llm_title("Use exponential backoff, e.g. for 503. The logs explain why."),
            "Use exponential backoff, e.g. for 503."
        );
    }

    fn tour() -> Tour {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/pr-482/api/default/tour.json"
        );
        serde_json::from_str(&std::fs::read_to_string(path).expect("golden Tour")).expect("Tour")
    }

    fn files() -> Vec<PrFile> {
        serde_json::from_value(json!([{
            "path": "src/rpc/client.rs", "old_path": null, "status": "modified", "binary": false,
            "added": 20, "removed": 2, "group": {"kind": "code"}, "not_rendered": null, "viewed": null,
            "hunks": [{"id": "h1", "base": null, "head": {"start_line": 95, "end_line": 101}, "after_line": null,
                "parts": [
                    {"side": "head", "range": {"start_line": 95, "end_line": 99},
                     "owner": {"kind": "step", "flow_id": "f1", "step_id": "f1.5"}, "also_in": []},
                    {"side": "head", "range": {"start_line": 100, "end_line": 101},
                     "owner": {"kind": "incidental", "flow_id": "f1", "text": "Renames a local."}, "also_in": []}
                ]}]
        }]))
        .expect("files")
    }

    fn krate(name: &str, manifest: &str, notes: usize, hidden: u32) -> serde_json::Value {
        json!({"name": name, "version": "0.1.0", "manifest": manifest, "source_at": null, "revision": null,
            "used_by": [], "facts_about": 0,
            "changes": {"changed_lines": 10, "files": 1, "flows": [], "facts": [], "facts_hidden": 0,
                "other_facts": 0,
                "notes": (0..notes).map(|n| json!({"text": format!("note {n}"), "lines": [], "changed_lines": 3}))
                    .collect::<Vec<_>>(),
                "notes_hidden": hidden, "other_dependencies": 0}})
    }

    fn deps() -> DependencyView {
        serde_json::from_value(json!({
            "status": "available", "controlled_owners": ["acme"], "limits": [], "files_outside_crates": 0,
            "notes": "produced",
            "repositories": [{"kind": "this", "repo": {"owner": "acme", "name": "node-client"}, "location": null,
                "controlled": true, "crates": [
                    krate("node-client", "Cargo.toml", 1, 1),
                    krate("rpc-core", "src/rpc/Cargo.toml", 1, 0),
                    krate("macros", "macros/Cargo.toml", 3, 0)
                ]}]
        }))
        .expect("deps")
    }

    /// F1: the comment context names the LLM's Flows flags on the item's lines, the Files incidental-change note on
    /// them and the Dependencies notes of the deepest crate holding the file, each with a link to its place.
    #[test]
    fn notes_elsewhere_come_from_the_stored_views() {
        let analysis = AnalysisId(uuid::Uuid::from_u128(
            0x6d3a_2f1e_0482_4c5e_9a7b_0000_0000_0482,
        ));
        let lines = LineRange {
            start_line: 98,
            end_line: 101,
        };
        let (steps, notes) = notes_on(
            analysis,
            &tour(),
            &files(),
            Some(&deps()),
            "src/rpc/client.rs",
            lines,
        );
        assert_eq!(
            steps
                .iter()
                .map(|step| step.step.0.as_str())
                .collect::<Vec<_>>(),
            ["f1.4", "f1.5", "f1.6"]
        );
        let base = format!("/analyses/{analysis}");
        assert_eq!(
            notes,
            vec![
                NoteElsewhere {
                    source: NoteSource::Flows,
                    label: "Flows: the LLM flagged these lines (2 flags)".to_owned(),
                    count: Some(2),
                    url: format!("{base}#f1.5"),
                },
                NoteElsewhere {
                    source: NoteSource::Files,
                    label: "Files: incidental-change note on these lines".to_owned(),
                    count: Some(1),
                    url: format!(
                        "{base}/files#lines?file=src%2Frpc%2Fclient.rs&side=head&line=98&end=101"
                    ),
                },
                NoteElsewhere {
                    source: NoteSource::Dependencies,
                    label: "Dependencies: 1 note on what changed in crate rpc-core".to_owned(),
                    count: Some(1),
                    url: format!("{base}/deps"),
                },
            ]
        );
    }

    /// Lines without flags or incidental notes get only their crate's notes; without a dependency map, none.
    #[test]
    fn lines_without_notes_get_none() {
        let analysis = AnalysisId(uuid::Uuid::from_u128(
            0x6d3a_2f1e_0482_4c5e_9a7b_0000_0000_0482,
        ));
        let lines = LineRange {
            start_line: 21,
            end_line: 22,
        };
        let (_, notes) = notes_on(
            analysis,
            &tour(),
            &[],
            Some(&deps()),
            "src/rpc/retry.rs",
            lines,
        );
        assert_eq!(
            notes
                .iter()
                .map(|note| note.label.as_str())
                .collect::<Vec<_>>(),
            ["Dependencies: 1 note on what changed in crate rpc-core"]
        );
        let (_, notes) = notes_on(analysis, &tour(), &[], Some(&deps()), "src/lib.rs", lines);
        assert_eq!(
            notes
                .iter()
                .map(|note| note.label.as_str())
                .collect::<Vec<_>>(),
            ["Dependencies: 2 notes on what changed in crate node-client"]
        );
        let (_, notes) = notes_on(analysis, &tour(), &[], None, "src/lib.rs", lines);
        assert!(notes.is_empty(), "{notes:?}");
    }
}
