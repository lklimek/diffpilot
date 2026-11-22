//! Read views of the review loop (RC1, RC2, RC4, RC5, RC10, RC11, RC16, RC17): built from the stored read, items,
//! assessments and decisions; no GitHub call.

use contracts::api::analyses::AnalysisSummary;
use contracts::api::review_loop::{
    AssessmentProgress, AssessmentState, CarryOverPage, CodeContext, CodeLine, CommentCounts,
    CommentItem, CommentLog, CommentsOverview, CommentsSummary, ContextQuery, DecideRest,
    DecideRestRow, DecisionBatchRow, EffectState, EffectsPreview, FeedSubscriber, FlowStepRef,
    GithubRead, ItemState, ItemsQuery, KindCount, NotAssessedReason, NoteElsewhere, NoteSource,
    PrBadgeView, PrPage, Round, WatchedPr,
};
use contracts::api::tour::CiState;
use contracts::common::{LineRange, LineSide, PrRef};
use contracts::ids::{AnalysisId, CarryEventId, CommitSha};
use contracts::time::Timestamp;
use db::repo::review_loop::{ItemRecord, PassRecord, PassState, StoredDecision};

use super::badge::{self, BadgeInputs};
use super::observe::{GateFact, Snapshot};
use super::queue;
use super::{Loaded, LoopError, PrHandle, READ_INTERVAL};
use crate::auth::Session;
use crate::github::mirror;
use crate::state::AppState;

/// Lines served by default around the commented lines.
const DEFAULT_CONTEXT_LINES: u32 = 40;
/// Most lines one context request serves.
const MAX_CONTEXT_LINES: u32 = 400;
/// Carry events per page (RC11).
const CARRY_PAGE: u32 = 50;

fn pass_live(pass: Option<&PassRecord>) -> bool {
    pass.is_some_and(|pass| matches!(pass.state, PassState::Queued | PassState::Running))
}

/// The item as the API shows it.
pub(crate) fn item_view(
    loaded: &Loaded,
    record: &ItemRecord,
    pass: Option<&PassRecord>,
) -> CommentItem {
    let assessment = loaded.assessments.get(&record.no);
    let assessment_state = if assessment.is_some() {
        AssessmentState::Assessed
    } else if record.kind == contracts::api::review_loop::CommentItemKind::PrQuestion {
        AssessmentState::NotAssessed {
            reason: NotAssessedReason::NotRun,
        }
    } else if pass_live(pass) {
        AssessmentState::Running
    } else {
        AssessmentState::NotAssessed {
            reason: match pass.map(|pass| pass.state) {
                Some(PassState::Failed) => NotAssessedReason::Failed,
                Some(PassState::Finished) => NotAssessedReason::Missing,
                Some(PassState::Queued | PassState::Running) | None => NotAssessedReason::NotRun,
            },
        }
    };
    let repeat_of = record.repeat_of.and_then(|id| {
        loaded
            .items
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.no)
    });
    CommentItem {
        id: record.id,
        no: record.no,
        kind: record.kind,
        stable_key: record.stable_key.clone(),
        author: record.author.clone(),
        author_kind: record.author_kind,
        url: record.url.clone(),
        location: record.location.clone(),
        title: record.title.clone(),
        body: record.body.clone(),
        bot_severity: record.bot_severity.clone(),
        gates: loaded.gates(record),
        state: record.state,
        revision: record.revision,
        first_seen_head: record.first_seen_head.clone(),
        first_round: record.first_round,
        github: record.github.clone(),
        outside: record.outside.clone(),
        repeat_of,
        assessment_state,
        assessment: assessment.map(|stored| stored.assessment.clone()),
        decisions: loaded
            .decisions
            .iter()
            .filter(|stored| stored.item == record.id)
            .map(|stored| stored.decision.clone())
            .collect(),
        // TODO(WP13-JOB): count the Ask chat's answers with subject `comment_item` once that subject exists.
        ask_answers: 0,
    }
}

/// RC4.
pub async fn items(
    state: &AppState,
    handle: PrHandle,
    query: &ItemsQuery,
) -> Result<Vec<CommentItem>, LoopError> {
    let loaded = Loaded::of(state, handle).await?;
    let pass = state.db.review_loop().latest_pass(loaded.handle.id).await?;
    Ok(loaded
        .items
        .iter()
        .filter(|record| query.state.is_none_or(|wanted| record.state == wanted))
        .filter(|record| query.kind.is_none_or(|wanted| record.kind == wanted))
        .map(|record| item_view(&loaded, record, pass.as_ref()))
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

fn counts(loaded: &Loaded, waiting: usize) -> CommentCounts {
    let standing = loaded.standing();
    CommentCounts {
        unresolved_threads: loaded
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.unresolved_threads),
        undecided: count(waiting),
        asked_again: count(
            loaded
                .items
                .iter()
                .filter(|item| item.state == ItemState::Reopened)
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

fn progress(loaded: &Loaded, pass: Option<&PassRecord>) -> AssessmentProgress {
    let needing: Vec<&ItemRecord> = loaded
        .items
        .iter()
        .filter(|item| {
            item.kind != contracts::api::review_loop::CommentItemKind::PrQuestion
                && item.state != ItemState::Outside
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
    AssessmentProgress {
        running: pass_live(pass),
        assessed: count(assessed),
        total: count(needing.len()),
        limited: 0,
        head: latest.map(|stored| stored.assessment.head.clone()),
        at: pass.and_then(|pass| pass.finished_at),
    }
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
                        .filter(|item| queue::is_open(item.state))
                        .count(),
                ),
                carried: carried(contracts::api::review_loop::CarryKind::Carried),
                asked_again: carried(contracts::api::review_loop::CarryKind::AskedAgain),
                undecided: count(
                    of_round
                        .iter()
                        .filter(|item| queue::is_open(item.state))
                        .count(),
                ),
                decisions: kinds,
            }
        })
        .collect()
}

/// The badge of a loaded PR.
fn badge_of(state: &AppState, loaded: &Loaded, waiting: usize) -> PrBadgeView {
    let standing = loaded.standing();
    let snapshot = loaded.snapshot.as_ref();
    let now = Timestamp::now();
    let overlay = state.plugins.overlay(now);
    let pr = &loaded.handle.pr;
    let plugin_round = overlay
        .notes(&pr.repo, pr.number)
        .iter()
        .any(|note| note.plugin == "thepastaclaw");
    let gate_round = snapshot.is_some_and(|snapshot| {
        matches!(
            snapshot.gate,
            Some((GateFact::InProgress | GateFact::Queued, ref commit))
                if commit.as_ref().is_none_or(|commit| snapshot.head.starts_with(commit.as_str()))
        )
    });
    badge::badge(&BadgeInputs {
        open: snapshot.is_none_or(|snapshot| snapshot.state == "OPEN"),
        read_failed: loaded.row.read_failure.is_some() || snapshot.is_none(),
        ci: snapshot
            .and_then(|snapshot| snapshot.ci.as_ref())
            .map(|ci| ci.state),
        failed_effects: standing.values().map(|s| failed(s)).sum(),
        undecided: count(waiting),
        executing: count(standing.values().filter(|s| effects_open(s)).count()),
        bot_round: plugin_round || gate_round,
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

/// RC2 (the caller asks for the opening assessment pass).
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
    let waiting = queue::waiting(&queue_items).len();
    let pass = rl.latest_pass(loaded.handle.id).await?;
    let events = rl
        .carry_events(&session.tenant_id, Some(loaded.handle.id), None, u32::MAX)
        .await?;
    Ok(CommentsOverview {
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
        counts: counts(&loaded, waiting),
        rounds: rounds(&loaded, &events),
        assessment: progress(&loaded, pass.as_ref()),
        queue: queue::queue(&queue_items),
        badge: badge_of(state, &loaded, waiting),
        analysis: latest_analysis(state, session, loaded.handle.id)
            .await?
            .map(|summary| summary.id),
    })
}

/// RC1.
pub async fn pr_page(
    state: &AppState,
    session: &Session,
    handle: PrHandle,
) -> Result<PrPage, LoopError> {
    let loaded = Loaded::of(state, handle).await?;
    let snapshot = loaded.snapshot()?;
    let rl = state.db.review_loop();
    let queue_items = loaded.queue_items(&[]);
    let waiting = queue::waiting(&queue_items).len();
    let pass = rl.latest_pass(loaded.handle.id).await?;
    let badge = badge_of(state, &loaded, waiting);
    let pr = &loaded.handle.pr;
    let watch = rl
        .watches(&session.user_id)
        .await?
        .into_iter()
        .find(|watch| watch.pull_request_id == loaded.handle.id)
        .map(|watch| WatchedPr {
            pr: watch.pr,
            title: watch.title,
            since: watch.since,
            last_probe_at: watch.last_probe_at,
            last_auto_pass: watch.last_auto_pass,
            state: watch.state,
            ended_at: watch.ended_at,
            badge: badge.clone(),
        });
    Ok(PrPage {
        pr: pr.clone(),
        title: snapshot.title.clone(),
        url: if snapshot.url.is_empty() {
            pr_url(pr)
        } else {
            snapshot.url.clone()
        },
        state: snapshot.state.clone(),
        draft: snapshot.draft,
        base: snapshot.base.clone(),
        // TODO(WP13-WEB-S): name the PR whose head branch is the base (stacked PRs); needs a GitHub lookup.
        stack_parent: None,
        head: CommitSha(snapshot.head.clone()),
        ci: snapshot
            .ci
            .clone()
            .filter(|ci| ci.state != CiState::NotChecked),
        plugin_notes: state
            .plugins
            .overlay(Timestamp::now())
            .notes(&pr.repo, pr.number),
        watch,
        badge: badge.clone(),
        analysis: latest_analysis(state, session, loaded.handle.id).await?,
        comments: CommentsSummary {
            counts: counts(&loaded, waiting),
            rounds: count(snapshot.rounds.len()),
            assessment: progress(&loaded, pass.as_ref()),
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
            token_name: subscription.token_name,
            capabilities: subscription.capabilities,
            // TODO(WP13-FEED): live stream presence and the masked push target come with the feed.
            connected: false,
            last_seen_at: subscription.last_seen_at,
            push: None,
        })
        .collect();
    Ok(CommentLog {
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
    Ok(loaded
        .plan(rows)?
        .into_iter()
        .map(|(record, planned)| EffectsPreview {
            item_no: record.no,
            effects: planned.effects,
        })
        .collect())
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
    let (analysis, analysis_head, in_flows) = flows_containing(
        state,
        session,
        loaded.handle.id,
        &location.path,
        location.lines,
    )
    .await;
    let notes = state
        .plugins
        .overlay(Timestamp::now())
        .notes(&loaded.handle.pr.repo, loaded.handle.pr.number)
        .into_iter()
        .filter_map(|note| {
            let source = match note.plugin.as_str() {
                "pr-hygiene" => NoteSource::PrHygiene,
                "thepastaclaw" => NoteSource::Thepastaclaw,
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
        })
        .collect();
    // TODO(WP13-WEB-C): Flows flags, Files incidental notes and Dependencies callers as notes elsewhere.
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

/// The flow steps of the PR's latest analysis whose head lines contain `lines`; nothing without a review-ready
/// analysis.
async fn flows_containing(
    state: &AppState,
    session: &Session,
    pr_id: i64,
    path: &str,
    lines: Option<LineRange>,
) -> (Option<AnalysisId>, Option<CommitSha>, Vec<FlowStepRef>) {
    let summary = match latest_analysis(state, session, pr_id).await {
        Ok(Some(summary)) => summary,
        Ok(None) => return (None, None, Vec::new()),
        Err(error) => {
            tracing::debug!(?error, "no analysis read for the comment context");
            return (None, None, Vec::new());
        }
    };
    let steps = match (lines, crate::pipeline::tour::load(state, &summary).await) {
        (Some(lines), Ok(tour)) => tour
            .flows
            .iter()
            .flat_map(|flow| {
                flow.steps
                    .iter()
                    .filter(move |step| {
                        step.facts.side == LineSide::Head
                            && step.facts.file == path
                            && step.facts.range.start_line <= lines.end_line
                            && lines.start_line <= step.facts.range.end_line
                    })
                    .map(move |step| FlowStepRef {
                        flow: flow.id.clone(),
                        flow_title: flow.llm.title.clone(),
                        step: step.id.clone(),
                    })
            })
            .collect(),
        _ => Vec::new(),
    };
    (Some(summary.id), Some(summary.pr.head_sha.clone()), steps)
}
