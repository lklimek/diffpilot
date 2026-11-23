//! The review loop (ADR-023, `docs/review-workflow-plan.md`): GitHub reads into items with stable `C<no>`
//! numbers, carry-overs, rounds and PR-level questions, decisions with their effects, and the owner's item actions.
//! GitHub calls live in `github::review_loop`, SQL in `db::repo::review_loop`; the views in [`views`].

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use contracts::api::dashboard::default_known_bots;
use contracts::api::pr::PrSummary;
use contracts::api::review_loop::{
    AuthorKind, BatchVia, CarryKind, CommentItemKind, DecideRequest, Decision, DecisionBatchRow,
    DecisionKind, DecisionVia, EffectKind, EffectState, Gate, GateMark, GateSource,
    ItemGithubFacts, ItemState, OutsideResolution, ReadFailure,
};
use contracts::common::PrRef;
use contracts::ids::CommitSha;
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::review_loop::{
    ActionOutcome, DecideOutcome, ItemRecord, NewCarryEvent, NewDecision, NewEffect, ObservedItem,
    ReadResult, StoredAssessment, StoredDecision, UpsertOutcome,
};
use jiff::SignedDuration;

use crate::auth::{Session, UserTokenError, user_access_token, user_token_rejected};
use crate::github::graphql::{GraphqlError, GraphqlErrorType};
use crate::github::review_loop::{self as reads, ReadError};
use crate::github::{checks, mirror, rest};
use crate::state::AppState;

pub mod assess;
mod badge;
mod carry;
mod effects;
pub mod feed;
mod markers;
pub mod notify;
mod observe;
mod queue;
pub mod views;
pub mod watch;

pub use effects::DecisionError;
use effects::{EffectItem, PrQuestion, PrScope};
use observe::{BotRule, Observed, Snapshot};

/// A page reads GitHub again when its last read is older than this (plan §4.1).
pub const READ_INTERVAL: SignedDuration = SignedDuration::from_secs(60);
/// "Read GitHub again" is refused within this time of the last read (RC3: 429).
pub const MIN_REREAD: SignedDuration = SignedDuration::from_secs(10);

/// Reads of one pull request run one at a time.
static READ_LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Mutex::default);

/// A review-loop request failed.
#[derive(Debug, thiserror::Error)]
pub enum LoopError {
    /// The pull request or item is unknown.
    #[error("the pull request or item was not found")]
    NotFound,
    /// The repository is private (v1 supports public repositories only).
    #[error("the repository is private")]
    PrivateRepository,
    /// The user's GitHub token is not available.
    #[error("the user's GitHub token is not available")]
    Token(#[from] UserTokenError),
    /// GitHub could not be read and nothing was read before.
    #[error("GitHub could not be read ({0:?})")]
    Unreadable(ReadFailure),
    /// "Read GitHub again" within [`MIN_REREAD`] of the last read.
    #[error("the pull request was read moments ago")]
    TooSoon,
    /// These items (`C<no>`) changed since the revisions named.
    #[error("items changed since the decision was prepared")]
    ItemChanged(Vec<u32>),
    /// A decision does not fit its item.
    #[error("the decision for C{no} is not valid")]
    Decision {
        /// The item.
        no: u32,
        /// Why.
        #[source]
        error: DecisionError,
    },
    /// A gate item in a group or bulk batch, or a table batch leaving out a waiting gate item.
    #[error("gate items need their own decision")]
    GateRow(Vec<u32>),
    /// The item action does not apply now (nothing to undo, an effect already claimed).
    #[error("the action does not apply to the item now")]
    NotApplicable,
    /// The requested line range is too large.
    #[error("the line range is too large")]
    RangeTooLarge,
    /// The line range is empty or starts before line 1.
    #[error("the line range is not valid")]
    BadRange,
    /// The page cursor is not one this server gave.
    #[error("the page cursor is not valid")]
    InvalidCursor,
    /// The user watches [`watch::WATCH_LIMIT`] pull requests already.
    #[error("the user watches the most pull requests allowed")]
    WatchLimitReached,
    /// "Assess again" while an assessment pass of the pull request is queued or running.
    #[error("an assessment pass of the pull request is queued or running")]
    AssessmentRunning,
    /// The file could not be read from the mirror.
    #[error("the file could not be read")]
    Mirror(#[from] mirror::MirrorError),
    /// The database failed.
    #[error("the review loop could not be read or changed")]
    Db(#[from] DbError),
}

/// A pull request known to the review loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrHandle {
    /// Owner, name, number.
    pub pr: PrRef,
    /// `pull_requests.id`.
    pub id: i64,
}

fn lock_key(tenant_id: &str, pr: &PrRef) -> String {
    format!(
        "{tenant_id}/{}/{}/{}",
        pr.repo.owner.to_ascii_lowercase(),
        pr.repo.name.to_ascii_lowercase(),
        pr.number
    )
}

async fn read_lock(tenant_id: &str, pr: &PrRef) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let mut locks = READ_LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(locks.entry(lock_key(tenant_id, pr)).or_default())
    };
    lock.lock_owned().await
}

/// The tenant's stored pull request.
pub async fn find(state: &AppState, tenant_id: &str, pr: &PrRef) -> Result<PrHandle, LoopError> {
    if !rest::is_valid_repo(&pr.repo) || pr.number == 0 {
        return Err(LoopError::NotFound);
    }
    let id = state
        .db
        .pull_requests()
        .find(tenant_id, &pr.repo, pr.number)
        .await?
        .ok_or(LoopError::NotFound)?;
    Ok(PrHandle { pr: pr.clone(), id })
}

/// When the PR's last read was attempted.
async fn last_attempt(state: &AppState, id: i64) -> Result<Option<Timestamp>, DbError> {
    Ok(state.db.review_loop().pr_state(id).await?.and_then(|row| {
        let failed = row.read_failure.map(|(_, at)| at);
        row.read_at.max(failed)
    }))
}

/// The pull request for a page: read from GitHub first when its last read is older than [`READ_INTERVAL`] (or
/// always with `force`). Reads use the reader's GitHub token; a read that finds new comments asks for an on-demand
/// assessment pass. An access-token request is served what is stored and never reads GitHub: a token read leaves
/// the database unchanged (ADR-019).
pub async fn ensure_read(
    state: &AppState,
    session: &Session,
    pr: &PrRef,
    force: bool,
) -> Result<PrHandle, LoopError> {
    if session.is_access_token() {
        return find(state, &session.tenant_id, pr).await;
    }
    let read = read_if_due(state, &session.tenant_id, &session.user_id, pr, force).await?;
    if let ReadOutcome::Read { new } = read.outcome
        && new > 0
        && let Err(error) =
            assess::request_pass(state, read.handle.id, assess::PassReason::NewComments).await
    {
        tracing::warn!(
            ?error,
            pr = read.handle.id,
            "assessment pass for new comments could not be requested"
        );
    }
    Ok(read.handle)
}

/// What [`read_if_due`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadOutcome {
    /// Not read: the last read is recent.
    Fresh,
    /// Read and stored; `new` items appeared.
    Read {
        /// New items.
        new: u32,
    },
    /// GitHub could not be read; the last read stays.
    Failed(ReadFailure),
}

/// The pull request and what its read did.
#[derive(Clone, Debug)]
pub(crate) struct Read {
    pub(crate) handle: PrHandle,
    pub(crate) outcome: ReadOutcome,
}

/// Reads the pull request from GitHub with the user's token when its last read is older than [`READ_INTERVAL`] (or
/// always with `force`); reads of one pull request run one at a time.
pub(crate) async fn read_if_due(
    state: &AppState,
    tenant: &str,
    user: &str,
    pr: &PrRef,
    force: bool,
) -> Result<Read, LoopError> {
    if !rest::is_valid_repo(&pr.repo) || pr.number == 0 {
        return Err(LoopError::NotFound);
    }
    let fresh = |last: Option<Timestamp>, now: Timestamp| {
        last.is_some_and(|at| now.duration_since(at) < READ_INTERVAL)
    };
    let skipped = |handle| Read {
        handle,
        outcome: ReadOutcome::Fresh,
    };
    if !force
        && let Ok(handle) = find(state, tenant, pr).await
        && fresh(last_attempt(state, handle.id).await?, Timestamp::now())
    {
        return Ok(skipped(handle));
    }
    let _guard = read_lock(tenant, pr).await;
    if let Ok(handle) = find(state, tenant, pr).await {
        let last = last_attempt(state, handle.id).await?;
        let now = Timestamp::now();
        if force && last.is_some_and(|at| now.duration_since(at) < MIN_REREAD) {
            return Err(LoopError::TooSoon);
        }
        if !force && fresh(last, now) {
            return Ok(skipped(handle));
        }
    }
    read_now(state, tenant, user, pr).await
}

fn failure_of(error: &ReadError) -> ReadFailure {
    match error {
        ReadError::Graphql(GraphqlError::RateLimited) => ReadFailure::RateLimited,
        ReadError::Graphql(error) if error.kind() == Some(GraphqlErrorType::RateLimited) => {
            ReadFailure::RateLimited
        }
        ReadError::Graphql(GraphqlError::Unauthorized) => ReadFailure::SignInRequired,
        _ => ReadFailure::Unavailable,
    }
}

/// Records a failed read on a stored PR (the last good read stays) or fails without one.
async fn read_failed(
    state: &AppState,
    tenant: &str,
    pr: &PrRef,
    failure: ReadFailure,
) -> Result<Read, LoopError> {
    let Ok(handle) = find(state, tenant, pr).await else {
        return Err(LoopError::Unreadable(failure));
    };
    let rl = state.db.review_loop();
    rl.ensure_pr(tenant, handle.id, Timestamp::now()).await?;
    rl.record_read(handle.id, &ReadResult::Failed(failure), Timestamp::now())
        .await?;
    tracing::warn!(
        pr = handle.id,
        ?failure,
        "review-loop GitHub read failed; the last read stays shown"
    );
    Ok(Read {
        handle,
        outcome: ReadOutcome::Failed(failure),
    })
}

/// Reads the pull request from GitHub with the user's token and stores what changed.
async fn read_now(
    state: &AppState,
    tenant: &str,
    user: &str,
    pr: &PrRef,
) -> Result<Read, LoopError> {
    let token = match user_access_token(state, user).await {
        Ok(token) => token,
        Err(UserTokenError::SignInRequired) => {
            return read_failed(state, tenant, pr, ReadFailure::SignInRequired).await;
        }
        Err(error) => return Err(error.into()),
    };
    let config = &state.config.github;
    let read = match reads::read(config, &token, pr).await {
        Ok(read) => read,
        Err(ReadError::NotFound) => return Err(LoopError::NotFound),
        Err(ReadError::Graphql(error)) if error.kind() == Some(GraphqlErrorType::NotFound) => {
            return Err(LoopError::NotFound);
        }
        Err(error) => {
            if matches!(error, ReadError::Graphql(GraphqlError::Unauthorized)) {
                user_token_rejected(state, user, &token).await;
            }
            tracing::debug!(?error, "review-loop read of the pull request failed");
            return read_failed(state, tenant, pr, failure_of(&error)).await;
        }
    };
    if read.private {
        return Err(LoopError::PrivateRepository);
    }
    let repo = state
        .db
        .repos()
        .upsert(tenant, &pr.repo, Some(read.repo_github_id))
        .await?;
    let summary = PrSummary {
        repo: repo.repo.clone(),
        number: pr.number,
        title: read.pr.title.clone(),
        author: read
            .pr
            .author
            .as_ref()
            .map_or_else(|| "ghost".to_owned(), |a| a.login.clone()),
        html_url: crate::github::threads::github_web_url(&read.pr.url).unwrap_or_else(|| {
            format!(
                "https://github.com/{}/{}/pull/{}",
                repo.repo.owner, repo.repo.name, pr.number
            )
        }),
        base_sha: CommitSha(read.pr.base_ref_oid.clone()),
        head_sha: CommitSha(read.pr.head_ref_oid.clone()),
    };
    let now = Timestamp::now();
    let id = state
        .db
        .pull_requests()
        .upsert(tenant, repo.id, &summary, now)
        .await?;
    state.db.review_loop().ensure_pr(tenant, id, now).await?;
    let handle = PrHandle {
        pr: PrRef {
            repo: repo.repo,
            number: pr.number,
        },
        id,
    };
    let ci = match checks::fetch(config, &token, &handle.pr.repo, &summary.head_sha).await {
        Ok(ci) => Some(ci),
        Err(error) => {
            tracing::debug!(
                ?error,
                "CI of the pull request head not read for the review loop"
            );
            None
        }
    };
    let known_bots = state
        .db
        .dashboard()
        .settings(tenant, user)
        .await?
        .map_or_else(default_known_bots, |stored| stored.settings.known_bots);
    let (observed, mut snapshot) = observe::observe(&read, &BotRule::new(&known_bots));
    snapshot.ci = ci;
    let new = apply(state, tenant, &handle, &observed, &snapshot).await?;
    add_pr_questions(state, tenant, &handle, &snapshot).await?;
    let round_count = u32::try_from(snapshot.rounds.len()).unwrap_or(u32::MAX);
    let snapshot = serde_json::to_value(&snapshot).map_err(DbError::from)?;
    state
        .db
        .review_loop()
        .record_read(
            id,
            &ReadResult::Read {
                snapshot,
                round_count,
            },
            Timestamp::now(),
        )
        .await?;
    tracing::info!(pr = id, new, "review-loop read stored");
    Ok(Read {
        handle,
        outcome: ReadOutcome::Read { new },
    })
}

/// thepastaclaw reviews or has queued the pull request (plan §2.8, badge B4): a statement of its status plug-in, or
/// its gate comment in progress or queued on the head of the last read.
pub(crate) fn bot_round_running(state: &AppState, pr: &PrRef, snapshot: Option<&Snapshot>) -> bool {
    let plugin = state
        .plugins
        .overlay(Timestamp::now())
        .notes(&pr.repo, pr.number)
        .iter()
        .any(|note| note.plugin == "thepastaclaw");
    let gate = snapshot.is_some_and(|snapshot| {
        matches!(
            snapshot.gate,
            Some((observe::GateFact::InProgress | observe::GateFact::Queued, ref commit))
                if commit.as_ref().is_none_or(|commit| snapshot.head.starts_with(commit.as_str()))
        )
    });
    plugin || gate
}

/// The standing decision of each item: its newest decision unless superseded or undone.
fn standing(decisions: &[StoredDecision]) -> HashMap<String, &StoredDecision> {
    let superseded: std::collections::HashSet<_> = decisions
        .iter()
        .filter_map(|stored| stored.decision.supersedes)
        .collect();
    decisions
        .iter()
        .filter(|stored| !superseded.contains(&stored.decision.id))
        .filter(|stored| stored.decision.undone_at.is_none())
        .map(|stored| (stored.item.to_string(), stored))
        .collect()
}

/// The item state a decision's effects give (as the repository settles it).
fn state_from(decision: &Decision) -> ItemState {
    let open = decision.effects.iter().any(|effect| {
        matches!(
            effect.state,
            EffectState::Waiting | EffectState::Open | EffectState::Claimed | EffectState::Failed
        )
    });
    if open {
        ItemState::Executing
    } else if !decision.effects.is_empty()
        && decision
            .effects
            .iter()
            .all(|effect| effect.state == EffectState::Done)
    {
        ItemState::Done
    } else {
        ItemState::Decided
    }
}

/// The decision resolves the thread through diffpilot (a resolve effect not cancelled).
fn resolves(decision: &Decision) -> bool {
    decision
        .effects
        .iter()
        .any(|effect| effect.kind == EffectKind::Resolve && effect.state != EffectState::Cancelled)
}

/// Stores the observed items; carries or asks again decided items raised again. Returns the new items' count.
async fn apply(
    state: &AppState,
    tenant: &str,
    handle: &PrHandle,
    observed: &[Observed],
    snapshot: &Snapshot,
) -> Result<u32, LoopError> {
    let rl = state.db.review_loop();
    let previous: HashMap<String, ItemRecord> = rl
        .items(handle.id)
        .await?
        .into_iter()
        .map(|item| (item.stable_key.clone(), item))
        .collect();
    let decisions = rl.pr_decisions(handle.id).await?;
    let standing = standing(&decisions);
    let head = CommitSha(snapshot.head.clone());
    let mut diffs: HashMap<String, Option<Vec<analysis::diff::FileDiff>>> = HashMap::new();
    let mut new = 0;
    for entry in observed {
        let mut item = entry.item.clone();
        let before = previous.get(&item.stable_key);
        let decided = before.and_then(|record| standing.get(&record.id.to_string()).copied());
        if let Some((_, true, resolved_by)) = &entry.thread
            && !decided.is_some_and(|stored| resolves(&stored.decision))
        {
            item.outside = Some(OutsideResolution {
                by: resolved_by.clone().unwrap_or_else(|| "ghost".to_owned()),
                seen_at: before
                    .and_then(|record| record.outside.as_ref())
                    .map_or_else(Timestamp::now, |outside| outside.seen_at),
            });
        }
        item.github.edited_after_decision =
            decided.is_some_and(|stored| stored.body_hash != item.body_hash);
        let state_if_new =
            decided.map_or(ItemState::Undecided, |stored| state_from(&stored.decision));
        let verdict = match (before, decided) {
            (Some(record), Some(stored))
                if item.outside.is_none()
                    && !queue::is_open(record.state)
                    && record.state != ItemState::Outside
                    && carry::reraised(record, &item) =>
            {
                let mapping = match (&stored.lines, &record.location) {
                    (Some(lines), Some(location)) if stored.decision.head != head => {
                        let files =
                            diff_since(state, handle, &stored.decision.head, &head, &mut diffs)
                                .await;
                        Some(match files {
                            Some(files) => carry::map_range(files, &location.path, *lines),
                            None => carry::Mapping::Unknown,
                        })
                    }
                    _ => None,
                };
                let decided_facts = carry::Decided {
                    body_hash: &stored.body_hash,
                    head: &stored.decision.head,
                    location: record
                        .location
                        .as_ref()
                        .zip(stored.lines)
                        .map(|(location, lines)| (location.path.as_str(), lines)),
                };
                Some((stored, carry::verdict(&decided_facts, &item, mapping)))
            }
            _ => None,
        };
        let outcome = rl
            .upsert_item(tenant, handle.id, &item, state_if_new, Timestamp::now())
            .await?;
        let record = match outcome {
            UpsertOutcome::New(record) => {
                new += 1;
                record
            }
            UpsertOutcome::Changed(record) | UpsertOutcome::Unchanged(record) => record,
        };
        if let Some((stored, verdict)) = verdict {
            carry_over(state, tenant, handle, snapshot, &record, stored, verdict).await?;
        }
    }
    Ok(new)
}

/// The diff of the PR's repository from `from` to `to` (cached per read); `None` when it cannot be made.
async fn diff_since<'a>(
    state: &AppState,
    handle: &PrHandle,
    from: &CommitSha,
    to: &CommitSha,
    cache: &'a mut HashMap<String, Option<Vec<analysis::diff::FileDiff>>>,
) -> Option<&'a Vec<analysis::diff::FileDiff>> {
    if !cache.contains_key(&from.0) {
        let made = async {
            let mirror =
                mirror::ensure_commits(&state.config, &handle.pr.repo, &[from, to]).await?;
            let (from, to) = (from.clone(), to.clone());
            mirror::in_read_slot(move || analysis::diff::diff(&mirror, &from, &to))
                .await?
                .map_err(|error| mirror::MirrorError::Git {
                    operation: "diff",
                    message: error.to_string(),
                })
        }
        .await;
        let files = match made {
            Ok(files) => Some(files),
            Err(error) => {
                tracing::debug!(
                    ?error,
                    "no diff since the decision; the decided lines count as changed"
                );
                None
            }
        };
        cache.insert(from.0.clone(), files);
    }
    cache.get(&from.0).and_then(Option::as_ref)
}

/// Applies a repeat's verdict: the decision is carried (a new decision `via carried` with the same choice and
/// fresh effects) or the item is asked again; either is logged with its evidence.
async fn carry_over(
    state: &AppState,
    tenant: &str,
    handle: &PrHandle,
    snapshot: &Snapshot,
    record: &ItemRecord,
    stored: &StoredDecision,
    verdict: carry::Verdict,
) -> Result<(), LoopError> {
    let rl = state.db.review_loop();
    let source = stored.decision.id;
    let (kind, reason, evidence) = match verdict {
        carry::Verdict::Carry(evidence) => {
            let request = DecideRequest {
                kind: stored.decision.kind,
                reason: stored.decision.reason.clone(),
                target: stored.decision.target.clone(),
                r#override: stored.decision.r#override.clone(),
                resolve_ticked: stored.decision.resolve_ticked,
                reply_text: None,
                comment: stored.decision.comment.clone(),
                item_revision: record.revision,
            };
            let item = effect_item(record, snapshot, &record_gates(state, record).await?);
            let scope = PrScope {
                number: handle.pr.number,
                ..PrScope::default()
            };
            let planned = match effects::plan(&item, &request, &scope) {
                Ok(planned) => planned,
                Err(error) => {
                    tracing::debug!(
                        ?error,
                        item = record.no,
                        "carried decision no longer fits; asked again"
                    );
                    let event = NewCarryEvent {
                        item: record.id,
                        source_decision: source,
                        kind: CarryKind::AskedAgain,
                        reason: Some(contracts::api::review_loop::ReaskReason::BotEdited),
                        evidence,
                    };
                    rl.record_carry(tenant, &event, Timestamp::now()).await?;
                    return Ok(());
                }
            };
            let user = rl.decision_user(source).await?;
            let new = NewDecision {
                item: record.id,
                item_revision: record.revision,
                user_id: user,
                kind: request.kind,
                reason: request.reason,
                target: request.target,
                r#override: request.r#override,
                resolve_ticked: request.resolve_ticked,
                reply_text: planned.reply_text,
                comment: request.comment,
                recommendation: stored.decision.recommendation.clone(),
                via: DecisionVia::Carried,
                carried_from: Some(source),
                head: CommitSha(snapshot.head.clone()),
                effects: new_effects(planned.effects),
            };
            match rl.decide(tenant, &[new], Timestamp::now()).await? {
                DecideOutcome::Stored(_) => state.feed.wake_sync(),
                DecideOutcome::ItemChanged(_) => {
                    tracing::debug!(
                        item = record.no,
                        "item changed during the read; carry left for the next read"
                    );
                    return Ok(());
                }
            }
            (CarryKind::Carried, None, evidence)
        }
        carry::Verdict::Reask(reason, evidence) => (CarryKind::AskedAgain, Some(reason), evidence),
    };
    rl.record_carry(
        tenant,
        &NewCarryEvent {
            item: record.id,
            source_decision: source,
            kind,
            reason,
            evidence,
        },
        Timestamp::now(),
    )
    .await?;
    tracing::info!(
        pr = handle.id,
        item = record.no,
        ?kind,
        ?reason,
        "repeat of a decided item handled"
    );
    Ok(())
}

fn new_effects(planned: Vec<contracts::api::review_loop::PlannedEffect>) -> Vec<NewEffect> {
    planned
        .into_iter()
        .map(|effect| NewEffect {
            seq: effect.seq,
            after: effect.after,
            kind: effect.kind,
            executor: effect.executor,
            params: effect.params,
        })
        .collect()
}

/// The item's gates: the owner's marks and the gates its latest assessment names.
pub(crate) fn gates_of(
    record: &ItemRecord,
    assessment: Option<&StoredAssessment>,
) -> Vec<GateMark> {
    let mut gates = record.gates.clone();
    for gate in assessment
        .map(|stored| stored.assessment.gates.as_slice())
        .unwrap_or_default()
    {
        if !gates.iter().any(|mark| mark.gate == *gate) {
            gates.push(GateMark {
                gate: *gate,
                source: GateSource::Assessment,
            });
        }
    }
    gates
}

async fn record_gates(state: &AppState, record: &ItemRecord) -> Result<Vec<Gate>, DbError> {
    let assessment = state.db.review_loop().latest_assessment(record.id).await?;
    Ok(gates_of(record, assessment.as_ref())
        .into_iter()
        .map(|mark| mark.gate)
        .collect())
}

/// Which PR-level question an item is, and its round.
pub(crate) fn question_of(record: &ItemRecord) -> Option<(PrQuestion, u32)> {
    if record.kind != CommentItemKind::PrQuestion {
        return None;
    }
    let mut parts = record.stable_key.split(':');
    let (Some("pr"), Some(round), Some(kind)) = (parts.next(), parts.next(), parts.next()) else {
        return None;
    };
    let question = match kind {
        "merge" => PrQuestion::Merge,
        "dismiss" => PrQuestion::Dismiss,
        _ => return None,
    };
    Some((question, round.parse().ok()?))
}

fn effect_item(record: &ItemRecord, snapshot: &Snapshot, gates: &[Gate]) -> EffectItem {
    EffectItem {
        no: record.no,
        kind: record.kind,
        author_kind: record.author_kind,
        thread_id: snapshot
            .thread_ids
            .get(&record.stable_key)
            .cloned()
            .or_else(|| {
                (record.kind == CommentItemKind::Thread && record.stable_key.starts_with("PRRT_"))
                    .then(|| record.stable_key.clone())
            }),
        review_id: snapshot.review_ids.get(&record.stable_key).cloned(),
        gates: gates.to_vec(),
        question: question_of(record).map(|(question, _)| question),
    }
}

/// Asks the PR-level questions a read made due: "merge now?" from round 3 on, once per round (RQ10); "dismiss
/// stale bot reviews?" once per round when a bot's change request has only decided items (RQ11).
async fn add_pr_questions(
    state: &AppState,
    tenant: &str,
    handle: &PrHandle,
    snapshot: &Snapshot,
) -> Result<(), LoopError> {
    let Ok(round) = u32::try_from(snapshot.rounds.len()) else {
        return Ok(());
    };
    if round == 0 {
        return Ok(());
    }
    let rl = state.db.review_loop();
    let items = rl.items(handle.id).await?;
    let has = |key: &str| items.iter().any(|item| item.stable_key == key);
    let question = |key: String, title: &str, body: String| ObservedItem {
        kind: CommentItemKind::PrQuestion,
        stable_key: key,
        author: "diffpilot".to_owned(),
        author_kind: AuthorKind::Diffpilot,
        url: None,
        location: None,
        title: title.to_owned(),
        body_hash: markers::body_hash(&body),
        body,
        head: CommitSha(snapshot.head.clone()),
        round: Some(round),
        bot_severity: None,
        github: ItemGithubFacts {
            resolved: None,
            resolved_by: None,
            outdated: None,
            bot_status: None,
            edited_after_decision: false,
            replies: 0,
            marker: None,
        },
        outside: None,
    };
    let mut asks = Vec::new();
    let merge = format!("pr:{round}:merge");
    if round >= 3 && !has(&merge) {
        asks.push(question(
            merge,
            "Merge now, or another round?",
            format!(
                "Bot round {round} has ended. Merge with the remaining items as follow-ups (one follow-up pull \
                 request), or another round."
            ),
        ));
    }
    let dismiss = format!("pr:{round}:dismiss");
    if !has(&dismiss) {
        let stale = stale_reviews(state, handle, snapshot, &items).await?;
        if !stale.is_empty() {
            let list: Vec<String> = stale
                .iter()
                .map(|request| format!("- {}: {}", request.login, request.url))
                .collect();
            asks.push(question(
                dismiss,
                "Dismiss stale bot reviews?",
                format!(
                    "Every item these reviews raised has a decision:\n{}",
                    list.join("\n")
                ),
            ));
        }
    }
    for ask in asks {
        rl.upsert_item(
            tenant,
            handle.id,
            &ask,
            ItemState::Undecided,
            Timestamp::now(),
        )
        .await?;
        tracing::info!(pr = handle.id, key = %ask.stable_key, "PR-level question asked");
    }
    Ok(())
}

/// Bots whose latest review requests changes while every item they raised (not resolved outside) has a standing
/// decision.
async fn stale_reviews<'a>(
    state: &AppState,
    handle: &PrHandle,
    snapshot: &'a Snapshot,
    items: &[ItemRecord],
) -> Result<Vec<&'a observe::BotChangeRequest>, LoopError> {
    let decisions = state.db.review_loop().pr_decisions(handle.id).await?;
    let standing = standing(&decisions);
    Ok(snapshot
        .bot_change_requests
        .iter()
        .filter(|request| {
            let raised: Vec<&ItemRecord> = items
                .iter()
                .filter(|item| item.author.eq_ignore_ascii_case(&request.login))
                .filter(|item| item.state != ItemState::Outside)
                .collect();
            !raised.is_empty()
                && raised
                    .iter()
                    .all(|item| standing.contains_key(&item.id.to_string()))
        })
        .collect())
}

/// Everything the decision and view code needs of one PR.
pub(crate) struct Loaded {
    pub(crate) handle: PrHandle,
    pub(crate) row: db::repo::review_loop::CommentPrRecord,
    pub(crate) snapshot: Option<Snapshot>,
    pub(crate) items: Vec<ItemRecord>,
    pub(crate) decisions: Vec<StoredDecision>,
    pub(crate) assessments: BTreeMap<u32, StoredAssessment>,
}

impl Loaded {
    pub(crate) async fn of(state: &AppState, handle: PrHandle) -> Result<Self, LoopError> {
        let rl = state.db.review_loop();
        let row = rl.pr_state(handle.id).await?.ok_or(LoopError::NotFound)?;
        let snapshot = row
            .github_snapshot
            .clone()
            .and_then(|value| serde_json::from_value(value).ok());
        let items = rl.items(handle.id).await?;
        let mut assessments = BTreeMap::new();
        for item in &items {
            if let Some(stored) = rl.latest_assessment(item.id).await? {
                assessments.insert(item.no, stored);
            }
        }
        Ok(Self {
            decisions: rl.pr_decisions(handle.id).await?,
            handle,
            row,
            snapshot,
            items,
            assessments,
        })
    }

    pub(crate) fn snapshot(&self) -> Result<&Snapshot, LoopError> {
        self.snapshot.as_ref().ok_or(LoopError::Unreadable(
            self.row
                .read_failure
                .map_or(ReadFailure::Unavailable, |(failure, _)| failure),
        ))
    }

    pub(crate) fn item(&self, no: u32) -> Result<&ItemRecord, LoopError> {
        self.items
            .iter()
            .find(|item| item.no == no)
            .ok_or(LoopError::NotFound)
    }

    pub(crate) fn standing(&self) -> HashMap<String, &StoredDecision> {
        standing(&self.decisions)
    }

    pub(crate) fn gates(&self, record: &ItemRecord) -> Vec<GateMark> {
        gates_of(record, self.assessments.get(&record.no))
    }

    /// Items listed in a standing "merge now" decision's follow-up PR.
    pub(crate) fn followed_up(&self) -> Vec<u32> {
        self.standing()
            .values()
            .filter(|stored| stored.decision.kind == DecisionKind::MergeWithFollowUps)
            .flat_map(|stored| {
                stored
                    .decision
                    .effects
                    .iter()
                    .filter(|effect| {
                        effect.kind == EffectKind::FollowUpPr
                            && effect.state != EffectState::Cancelled
                    })
                    .flat_map(|effect| effect.params.item_nos.clone())
            })
            .collect()
    }

    /// The queue view of every item for `skips` of the reader.
    pub(crate) fn queue_items(
        &self,
        skips: &[(contracts::ids::CommentItemId, u32)],
    ) -> Vec<queue::QueueItem> {
        let followed_up = self.followed_up();
        self.items
            .iter()
            .map(|record| queue::QueueItem {
                no: record.no,
                state: record.state,
                revision: record.revision,
                gated: !self.gates(record).is_empty(),
                blocking: record.bot_severity.as_deref() == Some("Blocking"),
                first_round: record.first_round,
                recommendation: self
                    .assessments
                    .get(&record.no)
                    .map(|stored| stored.assessment.recommendation.clone()),
                question: question_of(record),
                followed_up: followed_up.contains(&record.no),
                skipped: skips
                    .iter()
                    .any(|(id, revision)| *id == record.id && *revision == record.revision),
            })
            .collect()
    }

    fn effect_item(&self, record: &ItemRecord) -> Result<EffectItem, LoopError> {
        let gates: Vec<Gate> = self
            .gates(record)
            .into_iter()
            .map(|mark| mark.gate)
            .collect();
        Ok(effect_item(record, self.snapshot()?, &gates))
    }

    /// What a PR-level decision acts on now.
    fn scope(&self, deciding: &[u32]) -> Result<PrScope, LoopError> {
        let snapshot = self.snapshot()?;
        let queue_items = self.queue_items(&[]);
        let waiting: Vec<u32> = queue::waiting(&queue_items)
            .into_iter()
            .filter(|item| item.question.is_none() && !deciding.contains(&item.no))
            .map(|item| item.no)
            .collect();
        let mut remaining = Vec::new();
        for no in waiting {
            remaining.push(self.effect_item(self.item(no)?)?);
        }
        Ok(PrScope {
            number: self.handle.pr.number,
            remaining,
            stale_reviews: snapshot
                .bot_change_requests
                .iter()
                .map(|request| request.review_id.clone())
                .collect(),
        })
    }

    /// Plans the effects of `rows` (validated against their items).
    pub(crate) fn plan(
        &self,
        rows: &[DecisionBatchRow],
    ) -> Result<Vec<(&ItemRecord, effects::Planned)>, LoopError> {
        let deciding: Vec<u32> = rows.iter().map(|row| row.item_no).collect();
        let scope = self.scope(&deciding)?;
        rows.iter()
            .map(|row| {
                let record = self.item(row.item_no)?;
                let planned = effects::plan(&self.effect_item(record)?, &row.decision, &scope)
                    .map_err(|error| LoopError::Decision {
                        no: row.item_no,
                        error,
                    })?;
                Ok((record, planned))
            })
            .collect()
    }
}

/// Stores the decisions of `rows` (all or nothing) made by the session's user.
pub async fn decide(
    state: &AppState,
    session: &Session,
    handle: PrHandle,
    rows: &[DecisionBatchRow],
    via: DecisionVia,
) -> Result<Vec<Decision>, LoopError> {
    let loaded = Loaded::of(state, handle).await?;
    let mut seen = Vec::new();
    for row in rows {
        if seen.contains(&row.item_no) {
            return Err(LoopError::Decision {
                no: row.item_no,
                error: DecisionError::KindNotOffered,
            });
        }
        seen.push(row.item_no);
    }
    let gated: Vec<u32> = rows
        .iter()
        .filter_map(|row| loaded.item(row.item_no).ok())
        .filter(|record| !loaded.gates(record).is_empty())
        .map(|record| record.no)
        .collect();
    match via {
        DecisionVia::Group | DecisionVia::Bulk if !gated.is_empty() => {
            return Err(LoopError::GateRow(gated));
        }
        DecisionVia::Table => {
            let queue_items = loaded.queue_items(&[]);
            let missing: Vec<u32> = queue::waiting(&queue_items)
                .into_iter()
                .filter(|item| item.gated && !seen.contains(&item.no))
                .map(|item| item.no)
                .collect();
            if !missing.is_empty() {
                return Err(LoopError::GateRow(missing));
            }
        }
        _ => {}
    }
    let planned = loaded.plan(rows)?;
    let head = CommitSha(loaded.snapshot()?.head.clone());
    let decisions: Vec<NewDecision> = rows
        .iter()
        .zip(planned)
        .map(|(row, (record, planned))| NewDecision {
            item: record.id,
            item_revision: row.decision.item_revision,
            user_id: session.user_id.clone(),
            kind: row.decision.kind,
            reason: row
                .decision
                .reason
                .as_deref()
                .map(str::trim)
                .filter(|reason| !reason.is_empty())
                .map(str::to_owned),
            target: row.decision.target.clone(),
            r#override: row.decision.r#override.clone(),
            resolve_ticked: row.decision.resolve_ticked,
            reply_text: planned.reply_text,
            comment: row
                .decision
                .comment
                .as_deref()
                .map(str::trim)
                .filter(|comment| !comment.is_empty())
                .map(str::to_owned),
            recommendation: loaded
                .assessments
                .get(&record.no)
                .map(|stored| stored.assessment.recommendation.clone()),
            via,
            carried_from: None,
            head: head.clone(),
            effects: new_effects(planned.effects),
        })
        .collect();
    match state
        .db
        .review_loop()
        .decide(&session.tenant_id, &decisions, Timestamp::now())
        .await?
    {
        DecideOutcome::Stored(stored) => {
            tracing::info!(
                pr = loaded.handle.id,
                count = stored.len(),
                ?via,
                "decisions stored"
            );
            state.feed.wake_sync();
            Ok(stored)
        }
        DecideOutcome::ItemChanged(nos) => Err(LoopError::ItemChanged(nos)),
    }
}

/// The `via` of a batch.
pub fn batch_via(via: BatchVia) -> DecisionVia {
    match via {
        BatchVia::Group => DecisionVia::Group,
        BatchVia::Bulk => DecisionVia::Bulk,
        BatchVia::Table => DecisionVia::Table,
    }
}

/// An owner action on an item besides deciding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemAction {
    /// Undo the standing decision while no effect is claimed.
    Undo,
    /// Ask a decided item again.
    Reopen,
}

/// Undo or Reopen; returns the item as it is now.
pub async fn act(
    state: &AppState,
    session: &Session,
    handle: &PrHandle,
    no: u32,
    revision: u32,
    action: ItemAction,
) -> Result<ItemRecord, LoopError> {
    let rl = state.db.review_loop();
    let record = rl
        .item_by_no(handle.id, no)
        .await?
        .ok_or(LoopError::NotFound)?;
    let now = Timestamp::now();
    let outcome = match action {
        ItemAction::Undo => {
            rl.undo(
                &session.tenant_id,
                record.id,
                revision,
                &session.user_id,
                now,
            )
            .await?
        }
        ItemAction::Reopen => rl.reopen(record.id, revision, now).await?,
    };
    match outcome {
        ActionOutcome::Done(record) => {
            tracing::info!(pr = handle.id, item = no, ?action, "item action applied");
            state.feed.wake_sync();
            Ok(*record)
        }
        ActionOutcome::ItemChanged => Err(LoopError::ItemChanged(vec![no])),
        ActionOutcome::NoDecision | ActionOutcome::EffectClaimed => Err(LoopError::NotApplicable),
    }
}

/// "Skip for now" of the session's user.
pub async fn skip(
    state: &AppState,
    session: &Session,
    handle: &PrHandle,
    no: u32,
    revision: u32,
) -> Result<(), LoopError> {
    let rl = state.db.review_loop();
    let record = rl
        .item_by_no(handle.id, no)
        .await?
        .ok_or(LoopError::NotFound)?;
    if record.revision != revision {
        return Err(LoopError::ItemChanged(vec![no]));
    }
    rl.skip(
        &session.tenant_id,
        &session.user_id,
        record.id,
        revision,
        Timestamp::now(),
    )
    .await?;
    Ok(())
}

/// The owner marks (or clears his mark of) a gate.
pub async fn mark_gate(
    state: &AppState,
    handle: &PrHandle,
    no: u32,
    gate: Gate,
    marked: bool,
    revision: u32,
) -> Result<ItemRecord, LoopError> {
    let rl = state.db.review_loop();
    let record = rl
        .item_by_no(handle.id, no)
        .await?
        .ok_or(LoopError::NotFound)?;
    if record.kind == CommentItemKind::PrQuestion {
        return Err(LoopError::NotApplicable);
    }
    let mut gates: Vec<GateMark> = record
        .gates
        .iter()
        .filter(|mark| !(mark.gate == gate && mark.source == GateSource::Owner))
        .cloned()
        .collect();
    if marked {
        gates.push(GateMark {
            gate,
            source: GateSource::Owner,
        });
    }
    match rl
        .set_gates(record.id, revision, &gates, Timestamp::now())
        .await
    {
        Ok(record) => {
            tracing::info!(
                pr = handle.id,
                item = no,
                ?gate,
                marked,
                "gate mark changed by the owner"
            );
            Ok(record)
        }
        Err(DbError::Conflict) => Err(LoopError::ItemChanged(vec![no])),
        Err(error) => Err(error.into()),
    }
}
