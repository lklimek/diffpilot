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
    ItemGithubFacts, ItemState, OutsideResolution, PlannedEffect, ReadFailure,
};
use contracts::common::PrRef;
use contracts::ids::CommitSha;
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::review_loop::{
    ActionOutcome, DecideOutcome, ItemRecord, NewCarryEvent, NewDecision, NewEffect, ObservedItem,
    PrView, ReadResult, StoredAssessment, StoredDecision, UpsertOutcome,
};
use jiff::SignedDuration;
use sha2::{Digest, Sha256};

use crate::auth::{Session, UserTokenError, user_access_token, user_token_rejected};
use crate::github::graphql::{GraphqlError, GraphqlErrorType, ReadBudget};
use crate::github::review_loop::{self as reads, ReadError};
use crate::github::{checks, mirror, rest};
use crate::state::AppState;

pub mod assess;
mod badge;
mod carry;
mod clean;
mod context;
mod effects;
pub mod feed;
pub(crate) mod freshness;
mod handin;
mod markers;
pub mod notify;
mod observe;
pub(crate) mod propagation;
mod queue;
pub(crate) mod seen;
pub mod views;
pub mod watch;

pub use effects::DecisionError;
use effects::{EffectItem, PrQuestion, PrScope};
use observe::{BotRule, Observed, Snapshot};

/// Browser polling can read GitHub again at this interval in repositories without webhook coverage.
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
    /// An event read has to wait for its GitHub budget.
    #[error("GitHub webhook read budget reached")]
    EventBudget(std::time::Duration),
    /// These items (`C<no>`) changed since the revisions named.
    #[error("items changed since the decision was prepared")]
    ItemChanged(Vec<u32>),
    /// What the decisions of these items (`C<no>`) would do differs from the plan the owner confirmed.
    #[error("the plan changed since it was confirmed")]
    PlanChanged(Vec<u32>),
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
    /// A follower has a live agent claim.
    #[error("an agent has claimed the effects of follower C{0}")]
    FollowerClaimed(u32),
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
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
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

/// Reads one submitted thread and looks up its exact review card without refreshing Comments.
pub async fn github_thread(
    state: &AppState,
    session: &Session,
    handle: PrHandle,
    id: &str,
) -> Result<contracts::api::review_loop::GithubThreadDetail, LoopError> {
    if id.is_empty() || id.len() > 256 || !id.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(LoopError::NotFound);
    }
    let token = user_access_token(state, &session.user_id).await?;
    let mut detail =
        match crate::github::threads::detail(&state.config.github, &token, &handle.pr, id).await {
            Ok(Some(detail)) => detail,
            Ok(None) => return Err(LoopError::NotFound),
            Err(error)
                if matches!(
                    error.kind(),
                    Some(GraphqlErrorType::NotFound | GraphqlErrorType::Forbidden)
                ) =>
            {
                return Err(LoopError::NotFound);
            }
            Err(error) => {
                if matches!(error, GraphqlError::Unauthorized) {
                    user_token_rejected(state, &session.user_id, &token).await;
                }
                return Err(LoopError::Unreadable(failure_of(&ReadError::Graphql(
                    error,
                ))));
            }
        };
    let view = state.db.review_loop().pr_view(handle.id).await?;
    if let Some(snapshot) = view
        .row
        .and_then(|row| row.github_snapshot)
        .and_then(|data| serde_json::from_value::<Snapshot>(data).ok())
    {
        detail.item_no = view
            .items
            .iter()
            .find(|item| {
                item.kind == CommentItemKind::Thread
                    && item.gone_at.is_none()
                    && snapshot
                        .thread_ids
                        .get(&item.stable_key)
                        .is_some_and(|thread| thread == id)
            })
            .map(|item| item.no);
    }
    Ok(detail)
}

/// When the PR's last read was attempted.
async fn last_attempt(state: &AppState, id: i64) -> Result<Option<Timestamp>, DbError> {
    Ok(state.db.review_loop().pr_state(id).await?.and_then(|row| {
        let failed = row.read_failure.map(|(_, at)| at);
        row.read_at.max(failed)
    }))
}

/// The pull request for a page: read from GitHub initially, periodically without coverage, or on a manual refresh.
/// Browser reads use the reader's GitHub token without scheduling assessment.
/// Access-token requests read stored data without updating the database (ADR-019).
pub async fn ensure_read(
    state: &AppState,
    session: &Session,
    pr: &PrRef,
    force: bool,
) -> Result<PrHandle, LoopError> {
    if session.is_access_token() {
        return find(state, &session.tenant_id, pr).await;
    }
    if !force
        && let Ok(handle) = find(state, &session.tenant_id, pr).await
        && state
            .db
            .review_loop()
            .pr_state(handle.id)
            .await?
            .is_some_and(|row| row.github_snapshot.is_some())
        && freshness::covered(state, &session.tenant_id, &pr.repo).await?
    {
        return Ok(handle);
    }
    Ok(
        read_if_due(state, &session.tenant_id, &session.user_id, pr, force)
            .await?
            .handle,
    )
}

/// Refreshes an invalidated snapshot when a browser reader enters or resumes Comments.
pub(crate) async fn refresh_invalidated(
    state: &AppState,
    session: &Session,
    handle: &PrHandle,
) -> Result<(), LoopError> {
    if !freshness::covered(state, &session.tenant_id, &handle.pr.repo).await? {
        return Ok(());
    }
    let _guard = read_lock(&session.tenant_id, &handle.pr).await;
    if state
        .db
        .review_loop()
        .pr_state(handle.id)
        .await?
        .is_some_and(|row| row.github_dirty_at.is_some())
    {
        read_now(
            state,
            &session.tenant_id,
            &session.user_id,
            &handle.pr,
            None,
        )
        .await?;
    }
    Ok(())
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
    read_if_due_with_budget(state, tenant, user, pr, force, None).await
}

async fn read_if_due_with_budget(
    state: &AppState,
    tenant: &str,
    user: &str,
    pr: &PrRef,
    force: bool,
    budget: Option<ReadBudget<'_>>,
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
    read_now(state, tenant, user, pr, budget).await
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
    budget: Option<ReadBudget<'_>>,
) -> Result<Read, LoopError> {
    let token = match user_access_token(state, user).await {
        Ok(token) => token,
        Err(UserTokenError::SignInRequired) => {
            return read_failed(state, tenant, pr, ReadFailure::SignInRequired).await;
        }
        Err(error) => return Err(error.into()),
    };
    let config = &state.config.github;
    let started_at = Timestamp::now();
    if let Some(id) = state
        .db
        .pull_requests()
        .find(tenant, &pr.repo, pr.number)
        .await?
    {
        state
            .db
            .review_loop()
            .ensure_pr(tenant, id, started_at)
            .await?;
    }
    let read = match reads::read(config, &token, pr, budget).await {
        Ok(read) => read,
        Err(ReadError::Graphql(GraphqlError::EventBudget(delay))) => {
            return Err(LoopError::EventBudget(delay));
        }
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
    let (ci, pending_checks) = match checks::fetch_with_pending(
        config,
        &token,
        &handle.pr.repo,
        &summary.head_sha,
    )
    .await
    {
        Ok((ci, pending)) => (Some(ci), pending),
        Err(error) => {
            tracing::debug!(
                ?error,
                "CI of the pull request head not read for the review loop"
            );
            (None, Vec::new())
        }
    };
    let known_bots = state
        .db
        .dashboard()
        .settings(tenant, user)
        .await?
        .map_or_else(default_known_bots, |stored| stored.settings.known_bots);
    let bots = BotRule::new(&known_bots);
    let (mut observed, mut snapshot) = observe::observe(&read, &bots);
    let (mut pr_data, sources) = handin::build(
        &read,
        &format!("{}/{}", summary.repo.owner, summary.repo.name),
        &known_bots,
        state.db.users().member_logins(tenant).await?,
    );
    observed.retain(|entry| {
        entry.item.kind == CommentItemKind::Thread
            || entry.item.url.as_ref().is_some_and(|url| {
                sources.iter().any(|s| {
                    &s.url == url
                        && (s.role == contracts::api::review_loop::SourceRole::Source
                            || (s.role == contracts::api::review_loop::SourceRole::Context
                                && entry.item.author_kind == AuthorKind::Bot))
                })
            })
    });
    for source in &sources {
        state.db.comment_sources().put(tenant, id, source).await?;
        if source.role != contracts::api::review_loop::SourceRole::Source
            || observed
                .iter()
                .any(|entry| entry.item.url.as_ref() == Some(&source.url))
        {
            continue;
        }
        let raw = pr_data.reviews.iter().chain(&pr_data.comments).find(|raw| {
            raw.get("html_url")
                .or_else(|| raw.get("url"))
                .and_then(serde_json::Value::as_str)
                == Some(source.url.as_str())
        });
        let Some(raw) = raw else {
            continue;
        };
        let raw_id = raw["id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| raw["id"].to_string());
        let review = source.kind == contracts::api::review_loop::CommentSourceKind::ReviewBody;
        let comment = if review {
            None
        } else {
            read.comments
                .iter()
                .find(|comment| comment.url == source.url)
        };
        let (author_kind, status_comment) = if let Some(comment) = comment {
            bots.pr_comment(comment)
        } else {
            let kind = if known_bots
                .iter()
                .any(|bot| bot.eq_ignore_ascii_case(&source.author))
                || source.author.ends_with("[bot]")
            {
                AuthorKind::Bot
            } else {
                AuthorKind::Human
            };
            (kind, false)
        };
        if status_comment {
            pr_data.scope.sources.retain(|url| *url != source.url);
        }
        observed.push(Observed {
            thread: None,
            item: ObservedItem {
                kind: if review {
                    CommentItemKind::ReviewBody
                } else {
                    CommentItemKind::PrComment
                },
                stable_key: format!("{}:{raw_id}:1", if review { "review" } else { "comment" }),
                author: source.author.clone(),
                author_kind,
                status_comment,
                url: Some(source.url.clone()),
                location: None,
                title: markers::title_of(&source.body),
                body: source.body.clone(),
                body_hash: markers::body_hash(&source.body),
                head: summary.head_sha.clone(),
                round: None,
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
            },
        });
    }
    let versions = state.db.comment_sources().list(tenant, id).await?;
    let previous = state.db.review_loop().items(id).await?;
    let zero_keys: std::collections::HashSet<_> = previous
        .iter()
        .filter(|item| {
            item.gone_at.is_some()
                && item.source_id.is_none()
                && item.kind != CommentItemKind::Thread
                && sources.iter().any(|source| {
                    item.url.as_ref() == Some(&source.url)
                        && versions.iter().any(|v| {
                            v.source.url == source.url
                                && v.source.body_hash == source.body_hash
                                && v.source.read_at.is_some()
                        })
                })
        })
        .map(|item| item.stable_key.as_str())
        .collect();
    observed.retain(|entry| {
        entry.item.status_comment || !zero_keys.contains(entry.item.stable_key.as_str())
    });
    snapshot.pr_data = Some(pr_data);
    snapshot.ci = ci;
    snapshot.pending_checks = pending_checks;
    let new = apply(state, tenant, &handle, &observed, &snapshot).await?;
    if snapshot.complete {
        let read_keys: Vec<&str> = observed
            .iter()
            .map(|entry| entry.item.stable_key.as_str())
            .collect();
        let gone = state
            .db
            .review_loop()
            .mark_gone(id, &read_keys, Timestamp::now())
            .await?;
        if !gone.is_empty() {
            tracing::info!(
                pr = id,
                ?gone,
                latest_round = snapshot.rounds.len(),
                "items the read no longer produced leave the queue"
            );
        }
    }
    add_pr_questions(state, tenant, &handle, &snapshot).await?;
    seen::mark(state, &handle, &read).await?;
    let round_count = u32::try_from(snapshot.rounds.len()).unwrap_or(u32::MAX);
    let snapshot = serde_json::to_value(&snapshot).map_err(DbError::from)?;
    state
        .db
        .review_loop()
        .record_read_started(
            id,
            &ReadResult::Read {
                snapshot,
                round_count,
            },
            started_at,
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
        .any(|note| note.plugin == crate::dashboard::plugin::thepastaclaw::ID);
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

/// Items whose threads an agent resolves for diffpilot: a resolve effect of a standing decision that an agent claimed
/// or reported done (a planned resolve nobody holds is not diffpilot's resolution).
fn resolving(standing: &HashMap<String, &StoredDecision>) -> Vec<u32> {
    standing
        .values()
        .flat_map(|stored| &stored.decision.effects)
        .filter(|effect| {
            effect.kind == EffectKind::Resolve
                && matches!(effect.state, EffectState::Claimed | EffectState::Done)
        })
        .flat_map(|effect| effect.params.item_nos.iter().copied())
        .collect()
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
    let resolving = resolving(&standing);
    let head = CommitSha(snapshot.head.clone());
    let mut diffs: HashMap<String, Option<Vec<analysis::diff::FileDiff>>> = HashMap::new();
    let mut new = 0;
    for entry in observed {
        let mut item = entry.item.clone();
        let before = previous.get(&item.stable_key);
        let decided = before.and_then(|record| standing.get(&record.id.to_string()).copied());
        if let Some((_, true, resolved_by)) = &entry.thread
            && !before.is_some_and(|record| resolving.contains(&record.no))
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
        // A person's thread unresolved after a resolution outside diffpilot is an objection again; a bot's is not
        // carried from there.
        let raised = before
            .filter(|record| {
                !item.status_comment
                    && item.outside.is_none()
                    && !queue::is_open(record.state)
                    && (record.state != ItemState::Outside || record.author_kind != AuthorKind::Bot)
            })
            .and_then(|record| carry::raised(record, &item));
        let verdict = match (before, decided, raised) {
            (Some(_), Some(_), Some(carry::Raised::Reopened)) => Some(Repeat::AskAgain),
            (Some(record), Some(stored), Some(carry::Raised::Repeat)) => {
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
                Some(Repeat::Verdict(
                    stored,
                    carry::verdict(&decided_facts, &item, mapping),
                ))
            }
            _ => None,
        };
        let outcome = match verdict {
            // The observation and the owner's new question commit together (plan §2.5): nothing in between.
            Some(Repeat::AskAgain) => {
                rl.upsert_asking_again(tenant, handle.id, &item, state_if_new, Timestamp::now())
                    .await?
            }
            _ => {
                rl.upsert_item(tenant, handle.id, &item, state_if_new, Timestamp::now())
                    .await?
            }
        };
        let record = match outcome {
            UpsertOutcome::New(record) => {
                new += 1;
                record
            }
            UpsertOutcome::Changed(record) | UpsertOutcome::Unchanged(record) => record,
        };
        match verdict {
            Some(Repeat::Verdict(stored, verdict)) => {
                carry_over(state, tenant, handle, snapshot, &record, stored, verdict).await?;
            }
            Some(Repeat::AskAgain) => {
                state.feed.wake_sync();
                tracing::info!(
                    pr = handle.id,
                    item = record.no,
                    "a decided item raised again outside the carry-over rules is asked again"
                );
            }
            None => {}
        }
    }
    Ok(new)
}

/// What a decided item raised again gets.
enum Repeat<'a> {
    /// A bot's repeat: the carry-over rules decide (plan §2.5).
    Verdict(&'a StoredDecision, carry::Verdict),
    /// A person's thread unresolved or raised again: the owner is asked again, nothing is carried.
    AskAgain,
}

/// Asks a decided item whose carried decision no longer fits again, as Reopen does: the standing decision's effects
/// no agent holds are cancelled.
async fn ask_again(
    state: &AppState,
    handle: &PrHandle,
    record: &ItemRecord,
) -> Result<(), LoopError> {
    match state
        .db
        .review_loop()
        .reopen(record.id, record.revision, Timestamp::now())
        .await?
    {
        ActionOutcome::Done(_) => {
            state.feed.wake_sync();
            tracing::info!(
                pr = handle.id,
                item = record.no,
                "a decided item whose carried decision no longer fits is asked again"
            );
        }
        ActionOutcome::ItemChanged
        | ActionOutcome::MarkUndone(_)
        | ActionOutcome::NoDecision
        | ActionOutcome::EffectClaimed
        | ActionOutcome::FollowerClaimed(_) => {
            tracing::debug!(
                item = record.no,
                "item changed during the read; asking again is left for the next read"
            );
        }
    }
    Ok(())
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
    let automatic = matches!(
        stored.decision.via,
        DecisionVia::ImportedGithub | DecisionVia::FollowsPrimary
    );
    let (kind, reason, evidence) = match verdict {
        // Keep the original provenance when an automatic decision's point is repeated.
        carry::Verdict::Carry(evidence) if automatic => (CarryKind::Carried, None, evidence),
        carry::Verdict::Carry(evidence) => {
            let request = DecideRequest {
                kind: stored.decision.kind,
                reason: stored.decision.reason.clone(),
                target: stored.decision.target.clone(),
                r#override: stored.decision.r#override.clone(),
                resolve_ticked: stored.decision.resolve_ticked,
                reply_text: stored.decision.reply_text.clone(),
                comment: stored.decision.comment.clone(),
                item_revision: record.revision,
                plan_digest: None,
            };
            let item = effect_item(record, snapshot, &record_gates(state, record).await?);
            let scope = PrScope {
                number: handle.pr.number,
                ..PrScope::default()
            };
            let planned = match effects::plan(&item, &request, &scope) {
                Ok(planned) => planned,
                Err(error) => {
                    // Neither carry-over rule applies (text and code are unchanged), so no carry event names one.
                    tracing::debug!(?error, item = record.no, "carried decision no longer fits");
                    return ask_again(state, handle, record).await;
                }
            };
            let user = rl.decision_user(source).await?;
            let new = NewDecision {
                origin: None,
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
                DecideOutcome::ItemChanged(_) | DecideOutcome::FollowerClaimed(_) => {
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
    let recorded = rl
        .record_carry(
            tenant,
            &NewCarryEvent {
                item: record.id,
                source_decision: source,
                kind,
                reason,
                evidence,
            },
            automatic.then_some(record.revision),
            Timestamp::now(),
        )
        .await;
    if automatic && matches!(recorded, Err(DbError::Conflict)) {
        return Ok(());
    }
    recorded?;
    if kind == CarryKind::AskedAgain {
        // The earlier decision's unclaimed effects were cancelled: they leave the feed.
        state.feed.wake_sync();
    }
    tracing::info!(
        pr = handle.id,
        item = record.no,
        ?kind,
        ?reason,
        "repeat of a decided item handled"
    );
    Ok(())
}

fn new_effects(planned: Vec<PlannedEffect>) -> Vec<NewEffect> {
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
        quote: record.quote.clone().or_else(|| Some(record.title.clone())),
        no: record.no,
        kind: record.kind,
        author: record.author.clone(),
        author_kind: record.author_kind,
        url: record.url.clone(),
        thread_id: snapshot
            .thread_ids
            .get(&record.stable_key)
            .cloned()
            .or_else(|| {
                (record.kind == CommentItemKind::Thread && record.stable_key.starts_with("PRRT_"))
                    .then(|| record.stable_key.clone())
            }),
        review_id: snapshot
            .review_ids
            .get(&record.stable_key)
            .cloned()
            .or_else(|| {
                snapshot
                    .pr_data
                    .as_ref()?
                    .reviews
                    .iter()
                    .find(|raw| raw["html_url"].as_str() == record.url.as_deref())?
                    .get("node_id")?
                    .as_str()
                    .map(str::to_owned)
            }),
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
        status_comment: false,
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
        let decisions = rl.pr_decisions(handle.id).await?;
        let stale = stale_reviews(snapshot, &items, &standing(&decisions));
        if !stale.is_empty() {
            asks.push(question(
                dismiss,
                "Dismiss stale bot reviews?",
                dismiss_body(&stale),
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

/// Bot change requests whose remaining review points all have standing decisions.
fn stale_reviews<'a>(
    snapshot: &'a Snapshot,
    items: &[ItemRecord],
    standing: &HashMap<String, &StoredDecision>,
) -> Vec<&'a observe::BotChangeRequest> {
    snapshot
        .bot_change_requests
        .iter()
        .filter(|request| {
            let raised: Vec<&ItemRecord> = items
                .iter()
                .filter(|item| item.author.eq_ignore_ascii_case(&request.login))
                .filter(|item| {
                    item.state != ItemState::Outside
                        && item.gone_at.is_none()
                        && !item.status_comment
                        && item.active_mark.is_none()
                })
                .collect();
            !raised.is_empty()
                && raised.iter().all(|item| {
                    standing.contains_key(&item.id.to_string()) && !queue::is_open(item.state)
                })
        })
        .collect()
}

/// First line of the "Dismiss stale bot reviews?" text; one line `- <login>: <review link>` per listed review follows.
const DISMISS_LEAD: &str = "Every item these reviews raised has a decision:";

/// The "Dismiss stale bot reviews?" text listing `stale`; the list is what deciding P3 dismisses.
fn dismiss_body(stale: &[&observe::BotChangeRequest]) -> String {
    let mut body = DISMISS_LEAD.to_owned();
    for request in stale {
        body.push_str(&format!("\n- {}: {}", request.login, request.url));
    }
    body
}

/// The reviews a "Dismiss stale bot reviews?" text lists: login and review link per line.
fn listed_reviews(body: &str) -> Vec<(&str, &str)> {
    body.lines()
        .filter_map(|line| line.strip_prefix("- ")?.split_once(": "))
        .collect()
}

/// Everything the decision and view code needs of one PR.
pub(crate) struct Loaded {
    pub(crate) handle: PrHandle,
    pub(crate) row: db::repo::review_loop::CommentPrRecord,
    pub(crate) snapshot: Option<Snapshot>,
    pub(crate) items: Vec<ItemRecord>,
    item_numbers: HashMap<contracts::ids::CommentItemId, u32>,
    pub(crate) decisions: Vec<StoredDecision>,
    pub(crate) assessments: BTreeMap<u32, StoredAssessment>,
}

impl Loaded {
    pub(crate) async fn of(state: &AppState, handle: PrHandle) -> Result<Self, LoopError> {
        let view = state.db.review_loop().pr_view(handle.id).await?;
        Self::from_view(handle, view).ok_or(LoopError::NotFound)
    }

    /// The pull request as `view` read it.
    fn from_view(handle: PrHandle, view: PrView) -> Option<Self> {
        let row = view.row?;
        let snapshot = row
            .github_snapshot
            .clone()
            .and_then(|value| serde_json::from_value(value).ok());
        let numbers: HashMap<_, _> = view.items.iter().map(|item| (item.id, item.no)).collect();
        let assessments = view
            .assessments
            .into_iter()
            .filter_map(|(item, mut stored)| {
                assess::guard_posted(&mut stored.assessment);
                numbers.get(&item).map(|no| (*no, stored))
            })
            .collect();
        Some(Self {
            decisions: view.decisions,
            handle,
            row,
            snapshot,
            items: view.items,
            item_numbers: numbers,
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

    fn number_of(&self, id: contracts::ids::CommentItemId) -> Option<u32> {
        self.item_numbers.get(&id).copied()
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
            // An item the read no longer produces waits for nobody (plan §2.1).
            .filter(|record| {
                record.gone_at.is_none() && !record.status_comment && record.active_mark.is_none()
            })
            .map(|record| queue::QueueItem {
                no: record.no,
                state: record.state,
                revision: record.revision,
                gated: !self.gates(record).is_empty(),
                blocking: record.bot_severity.as_deref() == Some("Blocking"),
                review_body: record.kind == CommentItemKind::ReviewBody,
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

    /// What a PR-level decision acts on now: P1 the waiting items except gate items, which keep their own decision
    /// (RQ4, RQ5).
    fn scope(&self, deciding: &[u32]) -> Result<PrScope, LoopError> {
        let queue_items = self.queue_items(&[]);
        let waiting: Vec<u32> = queue::waiting(&queue_items)
            .into_iter()
            .filter(|item| item.question.is_none() && !item.gated && !deciding.contains(&item.no))
            .map(|item| item.no)
            .collect();
        let mut remaining = Vec::new();
        for no in waiting {
            remaining.push(self.effect_item(self.item(no)?)?);
        }
        Ok(PrScope {
            number: self.handle.pr.number,
            remaining,
            stale_reviews: Vec::new(),
        })
    }

    /// The reviews deciding P3 on `question` dismisses: those its text lists, each still stale now (RQ11).
    fn dismissal(&self, question: &ItemRecord) -> Result<Vec<String>, LoopError> {
        let standing = self.standing();
        let stale = stale_reviews(self.snapshot()?, &self.items, &standing);
        listed_reviews(&question.body)
            .into_iter()
            .map(|(login, url)| {
                stale
                    .iter()
                    .find(|request| request.login.eq_ignore_ascii_case(login) && request.url == url)
                    .map(|request| request.review_id.clone())
                    .ok_or(LoopError::Decision {
                        no: question.no,
                        error: DecisionError::ReviewsChanged,
                    })
            })
            .collect()
    }

    /// The digest of a PR-level decision's plan (RC16 `plan_digest`): every effect it plans (kind, executor, order,
    /// targets and text) and what it rests on, "Merge now" the items it follows up with their revisions, the
    /// dismissal the reviews it dismisses, each still stale. `None` for other rows.
    pub(crate) fn plan_digest(
        &self,
        row: &DecisionBatchRow,
        deciding: &[u32],
        effects: &[PlannedEffect],
    ) -> Result<Option<String>, LoopError> {
        let record = self.item(row.item_no)?;
        let rests_on: Vec<String> = match (row.decision.kind, question_of(record)) {
            (DecisionKind::MergeWithFollowUps, _) => self
                .scope(deciding)?
                .remaining
                .iter()
                .map(|item| {
                    self.item(item.no)
                        .map(|item| format!("follow-up C{} r{}", item.no, item.revision))
                })
                .collect::<Result<_, _>>()?,
            (DecisionKind::DismissStaleReviews, Some((PrQuestion::Dismiss, _))) => self
                .dismissal(record)?
                .into_iter()
                .map(|review| format!("dismiss {review}"))
                .collect(),
            _ => return Ok(None),
        };
        let planned = effects.iter().map(|effect| {
            let params = &effect.params;
            format!(
                "effect {} after {:?} {:?} by {:?} thread {:?} review {:?} items {:?} url {:?} title {:?} text {:?}",
                effect.seq,
                effect.after,
                effect.kind,
                effect.executor,
                params.thread_id,
                params.review_id,
                params.item_nos,
                params.url,
                params.title,
                params.text
            )
        });
        let mut hash = Sha256::new();
        for line in rests_on.into_iter().chain(planned) {
            hash.update(line.as_bytes());
            hash.update(b"\n");
        }
        Ok(Some(
            hash.finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        ))
    }

    /// Plans the effects of `rows` (validated against their items).
    pub(crate) fn plan<K: Copy + Into<DecisionKind>>(
        &self,
        rows: &[DecisionBatchRow<K>],
    ) -> Result<Vec<(&ItemRecord, effects::Planned)>, LoopError> {
        let deciding: Vec<u32> = rows.iter().map(|row| row.item_no).collect();
        let scope = self.scope(&deciding)?;
        rows.iter()
            .map(|row| {
                let record = self.item(row.item_no)?;
                let kind = row.decision.kind.into();
                if kind == DecisionKind::NotAReviewPoint {
                    if record.revision != row.decision.item_revision {
                        return Err(LoopError::ItemChanged(vec![record.no]));
                    }
                    if record.kind == CommentItemKind::PrQuestion
                        || record.status_comment
                        || record.active_mark.is_some()
                        || !matches!(
                            record.state,
                            ItemState::Undecided | ItemState::Reopened | ItemState::AskedBack
                        )
                        || record.gone_at.is_some()
                    {
                        return Err(LoopError::Decision {
                            no: record.no,
                            error: DecisionError::KindNotOffered,
                        });
                    }
                }
                let dismissing = kind == DecisionKind::DismissStaleReviews
                    && matches!(question_of(record), Some((PrQuestion::Dismiss, _)));
                let listed;
                let scope = if dismissing {
                    listed = PrScope {
                        stale_reviews: self.dismissal(record)?,
                        ..scope.clone()
                    };
                    &listed
                } else {
                    &scope
                };
                let planned = effects::plan(&self.effect_item(record)?, &row.decision, scope)
                    .map_err(|error| LoopError::Decision {
                        no: row.item_no,
                        error,
                    })?;
                Ok((record, planned))
            })
            .collect()
    }
}

/// Stores the decisions of `rows` (all or nothing) made by the session's user. They are planned on the pull
/// request's state inside the write transaction that stores them, so a concurrent change can never sweep an item
/// into a PR-level decision; a row carrying the `plan_digest` the owner confirmed is refused when its plan changed.
pub async fn decide(
    state: &AppState,
    session: &Session,
    handle: PrHandle,
    rows: &[DecisionBatchRow],
    via: DecisionVia,
) -> Result<Vec<Decision>, LoopError> {
    let pr = handle.id;
    let outcome = state
        .db
        .review_loop()
        .decide_planned(&session.tenant_id, pr, Timestamp::now(), |view| {
            let loaded = Loaded::from_view(handle, view).ok_or(LoopError::NotFound)?;
            let mut decisions = loaded.new_decisions(&session.user_id, rows, via)?;
            loaded.append_propagated(&mut decisions, false);
            Ok::<_, LoopError>(decisions)
        })
        .await?;
    match outcome {
        DecideOutcome::Stored(stored) => {
            let real = stored
                .iter()
                .filter(|d| d.kind != DecisionKind::NotAReviewPoint)
                .count();
            if real != 0 {
                tracing::info!(pr, count = real, ?via, "decisions stored");
                state.feed.wake_sync();
            }
            Ok(stored.into_iter().take(rows.len()).collect())
        }
        DecideOutcome::ItemChanged(nos) => Err(LoopError::ItemChanged(nos)),
        DecideOutcome::FollowerClaimed(no) => Err(LoopError::FollowerClaimed(no)),
    }
}

impl Loaded {
    /// The decisions of `rows` by `user`, validated and planned on this state.
    fn new_decisions(
        &self,
        user: &str,
        rows: &[DecisionBatchRow],
        via: DecisionVia,
    ) -> Result<Vec<NewDecision>, LoopError> {
        let mut seen = Vec::new();
        for row in rows {
            if seen.contains(&row.item_no) {
                return Err(LoopError::Decision {
                    no: row.item_no,
                    error: DecisionError::KindNotOffered,
                });
            }
            if row.decision.kind == DecisionKind::NotAReviewPoint && via == DecisionVia::Bulk {
                return Err(LoopError::Decision {
                    no: row.item_no,
                    error: DecisionError::KindNotOffered,
                });
            }
            seen.push(row.item_no);
        }
        let gated: Vec<u32> = rows
            .iter()
            .filter_map(|row| self.item(row.item_no).ok())
            .filter(|record| !self.gates(record).is_empty())
            .map(|record| record.no)
            .collect();
        // Items asked again or no longer read need their own decision; "Decide the rest" leaves them out too (RC17).
        if via == DecisionVia::Bulk
            && let Some((no, error)) = rows
                .iter()
                .filter_map(|row| self.item(row.item_no).ok())
                .find_map(|record| {
                    if record.gone_at.is_some() {
                        Some((record.no, DecisionError::NotRead))
                    } else if record.state == ItemState::Reopened {
                        Some((record.no, DecisionError::AskedAgain))
                    } else {
                        None
                    }
                })
        {
            return Err(LoopError::Decision { no, error });
        }
        if rows.len() > 1 {
            let gated_marks: Vec<_> = rows
                .iter()
                .filter(|row| {
                    row.decision.kind == DecisionKind::NotAReviewPoint
                        && gated.contains(&row.item_no)
                })
                .map(|row| row.item_no)
                .collect();
            if !gated_marks.is_empty() {
                return Err(LoopError::GateRow(gated_marks));
            }
        }
        let single_gate_mark = matches!(rows, [row]
            if row.decision.kind == DecisionKind::NotAReviewPoint && gated.contains(&row.item_no));
        match via {
            DecisionVia::Group | DecisionVia::Bulk if !gated.is_empty() => {
                return Err(LoopError::GateRow(gated));
            }
            DecisionVia::Table if !single_gate_mark => {
                let queue_items = self.queue_items(&[]);
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
        let planned = self.plan(rows)?;
        let mut changed = Vec::new();
        for (row, (_, plan)) in rows.iter().zip(&planned) {
            if let Some(confirmed) = &row.decision.plan_digest
                && self.plan_digest(row, &seen, &plan.effects)?.as_ref() != Some(confirmed)
            {
                changed.push(row.item_no);
            }
        }
        if !changed.is_empty() {
            return Err(LoopError::PlanChanged(changed));
        }
        let head = CommitSha(self.snapshot()?.head.clone());
        Ok(rows
            .iter()
            .zip(planned)
            .map(|(row, (record, planned))| NewDecision {
                origin: None,
                item: record.id,
                item_revision: row.decision.item_revision,
                user_id: user.to_owned(),
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
                recommendation: self
                    .assessments
                    .get(&record.no)
                    .map(|stored| stored.assessment.recommendation.clone()),
                via,
                carried_from: None,
                head: head.clone(),
                effects: new_effects(planned.effects),
            })
            .collect())
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
        ActionOutcome::MarkUndone(record) => Ok(*record),
        ActionOutcome::ItemChanged => Err(LoopError::ItemChanged(vec![no])),
        ActionOutcome::NoDecision | ActionOutcome::EffectClaimed => Err(LoopError::NotApplicable),
        ActionOutcome::FollowerClaimed(no) => Err(LoopError::FollowerClaimed(no)),
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
    session: &Session,
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
        .set_gates(
            (&session.tenant_id, &session.user_id),
            record.id,
            revision,
            &gates,
            Timestamp::now(),
        )
        .await
    {
        Ok(updated) => {
            if updated.state == ItemState::Reopened && record.state != ItemState::Reopened {
                state.feed.wake_sync();
            }
            tracing::info!(
                pr = handle.id,
                item = no,
                ?gate,
                marked,
                "gate mark changed by the owner"
            );
            Ok(updated)
        }
        Err(DbError::Conflict) => Err(LoopError::ItemChanged(vec![no])),
        Err(error) => Err(error.into()),
    }
}
