//! Watch (ADR-023, plan §2.8, RW1–RW2): a user's watched pull requests (at most [`WATCH_LIMIT`]) are probed every
//! [`PROBE_INTERVAL`] from the ADR-020 scheduler task with the watcher's own token, one alias-batched GraphQL query
//! per [`PROBE_BATCH`] pull requests; only a pull request whose probe values changed is read in full. A change makes
//! an automatic assessment pass due: it waits while thepastaclaw reviews the pull request (at most
//! [`BOT_ROUND_WAIT`]), then goes through [`assess::request_pass`] (one pass per pull request, spacing, daily cap). A
//! plug-in change signal probes its watched pull requests at once. A pass that leaves undecided questions writes one
//! `questions_ready` user event per round and watcher; delivering it is the notification task's work.

use std::collections::{BTreeMap, HashSet};

use contracts::api::review_loop::{
    ItemState, PrBadgeView, UserEventKind, WatchList, WatchState, WatchedPr,
};
use contracts::common::PrRef;
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::review_loop::{NewUserEvent, PassCause, PassRecord, WatchOutcome, WatchRecord};
use jiff::SignedDuration;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::assess::{self, AssessError, PassQueued, PassReason};
use super::observe::Snapshot;
use super::{Loaded, LoopError, PrHandle, ReadOutcome, queue};
use crate::auth::{Session, UserTokenError};
use crate::dashboard::plugin::PrKey;
use crate::github::work::{UserTokens, WorkClient, WorkError};
use crate::state::AppState;

/// Most pull requests one user watches (plan §2.8).
// TODO(S13-2 follow-up): confirm the Watch constants against the owner's usage (plan §12).
pub const WATCH_LIMIT: u32 = 20;
/// Least time between two probes of one watch.
pub const PROBE_INTERVAL: SignedDuration = SignedDuration::from_mins(5);
/// Pull requests per probe query (spike S13-2: 10 or 20 cost one point; 10 keeps a failed request small).
pub const PROBE_BATCH: usize = 10;
/// Longest wait of a due automatic pass for the bot's round to end.
pub const BOT_ROUND_WAIT: SignedDuration = SignedDuration::from_mins(90);
/// The window of the daily cap.
const DAY: SignedDuration = SignedDuration::from_hours(24);

/// A watch operation failed.
#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    /// The database failed.
    #[error("the watch state could not be read or stored")]
    Db(#[from] DbError),
    /// The pull request could not be loaded or read.
    #[error("the watched pull request could not be read")]
    Loop(#[from] LoopError),
    /// The automatic pass could not be requested.
    #[error("the automatic assessment pass could not be requested")]
    Assess(#[from] AssessError),
    /// GitHub could not be probed.
    #[error("the watched pull requests could not be probed on GitHub")]
    Github(#[from] WorkError),
    /// The probe values could not be stored.
    #[error("the probe values could not be stored")]
    Json(#[from] serde_json::Error),
}

/// What a probe compares: a bot's in-place edit bumps `updated_at` (spike S13-2 §3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Probe {
    head: String,
    updated_at: String,
    threads: u32,
    comments: u32,
    last_review: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProbedRepository {
    pull_request: Option<ProbedPull>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProbedPull {
    /// `OPEN`, `CLOSED`, `MERGED`; absent in answers recorded without it (spike S13-2), read as open.
    #[serde(default)]
    state: Option<String>,
    head_ref_oid: String,
    updated_at: String,
    review_threads: Count,
    comments: Count,
    reviews: Nodes,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Count {
    total_count: u32,
}

#[derive(Deserialize)]
struct Nodes {
    nodes: Vec<NodeId>,
}

#[derive(Deserialize)]
struct NodeId {
    id: String,
}

/// The aliased lookup of one pull request (a GraphQL string literal is a JSON string).
fn lookup(pr: &PrRef) -> String {
    format!(
        "repository(owner: {}, name: {}) {{ pullRequest(number: {}) {{ state headRefOid updatedAt \
         reviewThreads {{ totalCount }} comments {{ totalCount }} reviews(last: 1) {{ nodes {{ id }} }} }} }}",
        Value::from(pr.repo.owner.as_str()),
        Value::from(pr.repo.name.as_str()),
        pr.number
    )
}

/// RW1: the session's user watches the pull request.
pub async fn watch(
    state: &AppState,
    session: &Session,
    handle: &PrHandle,
) -> Result<(), LoopError> {
    let outcome = state
        .db
        .review_loop()
        .watch(
            &session.tenant_id,
            &session.user_id,
            handle.id,
            WATCH_LIMIT,
            Timestamp::now(),
        )
        .await?;
    presence(state).await?;
    match outcome {
        WatchOutcome::Watching => {
            tracing::info!(pr = handle.id, user = %session.user_id, "pull request watched");
            Ok(())
        }
        WatchOutcome::LimitReached => Err(LoopError::WatchLimitReached),
    }
}

/// RW1: the session's user stops watching the pull request (nothing when not watched).
pub async fn unwatch(
    state: &AppState,
    session: &Session,
    handle: &PrHandle,
) -> Result<(), LoopError> {
    if state
        .db
        .review_loop()
        .unwatch(&session.user_id, handle.id)
        .await?
    {
        tracing::info!(pr = handle.id, user = %session.user_id, "pull request no longer watched");
    }
    presence(state).await?;
    Ok(())
}

/// The watch as the API shows it.
pub(crate) fn view(record: WatchRecord, badge: PrBadgeView) -> WatchedPr {
    WatchedPr {
        pr: record.pr,
        title: record.title,
        since: record.since,
        last_probe_at: record.last_probe_at,
        last_auto_pass: record.last_auto_pass,
        state: record.state,
        ended_at: record.ended_at,
        badge,
    }
}

/// RW2: the session's watches with their badges and the reader's passes of the last 24 hours.
pub async fn list(state: &AppState, session: &Session) -> Result<WatchList, LoopError> {
    let rl = state.db.review_loop();
    let mut watches = Vec::new();
    for record in rl.watches(&session.user_id).await? {
        let handle = PrHandle {
            pr: record.pr.clone(),
            id: record.pull_request_id,
        };
        let badge = match super::views::badge(state, handle).await {
            Ok(badge) => badge,
            Err(error) => {
                tracing::debug!(
                    ?error,
                    pr = record.pull_request_id,
                    "no badge for a watched pull request"
                );
                PrBadgeView {
                    badge: None,
                    facts: Vec::new(),
                    missing: vec!["GitHub read".to_owned()],
                }
            }
        };
        watches.push(view(record, badge));
    }
    Ok(WatchList {
        watches,
        limit: WATCH_LIMIT,
        passes_today: rl
            .charged_passes(&session.user_id, Timestamp::now() - DAY)
            .await?,
        daily_limit: assess::DAILY_PASSES_PER_USER,
    })
}

/// Tells the plug-in host whether any user watches a pull request (ADR-022 §3 presence).
pub async fn presence(state: &AppState) -> Result<(), DbError> {
    let any = !state.db.review_loop().live_watches().await?.is_empty();
    state.plugins.set_watching(any);
    Ok(())
}

fn by_user<'a>(
    watches: impl IntoIterator<Item = &'a WatchRecord>,
) -> BTreeMap<&'a str, Vec<&'a WatchRecord>> {
    let mut users: BTreeMap<&str, Vec<&WatchRecord>> = BTreeMap::new();
    for watch in watches {
        users.entry(watch.user_id.as_str()).or_default().push(watch);
    }
    users
}

/// One evaluation of the scheduler task: probes the watches whose last probe is [`PROBE_INTERVAL`] old, then settles
/// every watch (a due pass is requested, a finished job ends its state). Failures of one user or watch are logged.
pub async fn tick(state: AppState) -> Result<(), DbError> {
    tick_at(&state, Timestamp::now()).await
}

/// [`tick`] at `now`.
pub(crate) async fn tick_at(state: &AppState, now: Timestamp) -> Result<(), DbError> {
    let watches = state.db.review_loop().live_watches().await?;
    state.plugins.set_watching(!watches.is_empty());
    let due = watches
        .iter()
        .filter(|watch| {
            watch
                .last_probe_at
                .is_none_or(|at| now.duration_since(at) >= PROBE_INTERVAL)
        })
        .filter(|watch| watch.state != WatchState::Ended);
    for (user, list) in by_user(due) {
        probe(state, user, &list, now).await;
    }
    settle_all(state, None, now).await
}

/// A plug-in change signal (a PR Hygiene change mark, thepastaclaw's round end): the watched pull requests among
/// `changed` are probed and settled at once.
pub async fn plugin_changed(state: &AppState, changed: &HashSet<PrKey>) {
    let watches = match state.db.review_loop().live_watches().await {
        Ok(watches) => watches,
        Err(error) => {
            tracing::warn!(%error, "watches could not be read after a plug-in change signal");
            return;
        }
    };
    let hit: Vec<&WatchRecord> = watches
        .iter()
        .filter(|watch| changed.contains(&PrKey::new(&watch.pr.repo, watch.pr.number)))
        .collect();
    if hit.is_empty() {
        return;
    }
    let now = Timestamp::now();
    for (user, list) in by_user(hit.iter().copied()) {
        probe(state, user, &list, now).await;
    }
    let prs: HashSet<i64> = hit.iter().map(|watch| watch.pull_request_id).collect();
    if let Err(error) = settle_all(state, Some(&prs), now).await {
        tracing::warn!(%error, "watches could not be settled after a plug-in change signal");
    }
    tracing::debug!(
        watches = hit.len(),
        "watched pull requests probed after a plug-in change signal"
    );
}

/// Probes `watches` of `user` and handles each answer; a user without a usable token pair is paused.
async fn probe(state: &AppState, user: &str, watches: &[&WatchRecord], now: Timestamp) {
    let client = match WorkClient::new(
        state.config.github.api_url.clone(),
        UserTokens {
            state: state.clone(),
            user_id: user.to_owned(),
        },
    ) {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(%error, "the GitHub client for the watch probe could not be created");
            return;
        }
    };
    let answers = match client
        .batched(watches, PROBE_BATCH, "", |watch| lookup(&watch.pr))
        .await
    {
        Ok(answers) => answers,
        Err(WorkError::Token(UserTokenError::SignInRequired) | WorkError::Unauthorized) => {
            pause(state, watches, now).await;
            return;
        }
        Err(error) => {
            tracing::warn!(user, %error, error_chain = ?error,
                "watched pull requests could not be probed; the next tick tries again");
            return;
        }
    };
    if !answers.failed.is_empty() {
        tracing::debug!(
            user,
            failed = answers.failed.len(),
            "some watched pull requests were not probed; the next tick tries again"
        );
    }
    for (watch, value) in answers.resolved {
        if let Err(error) = observe(state, watch, value, now).await {
            tracing::warn!(pr = watch.pull_request_id, user, %error, error_chain = ?error,
                "a watched pull request's probe could not be handled");
        }
    }
}

/// No usable token pair: the user's watches pause until a probe works again ("Watching paused: sign in again").
async fn pause(state: &AppState, watches: &[&WatchRecord], now: Timestamp) {
    for watch in watches
        .iter()
        .filter(|watch| watch.state != WatchState::Paused)
    {
        match state
            .db
            .review_loop()
            .set_watch_state(
                &watch.user_id,
                watch.pull_request_id,
                WatchState::Paused,
                now,
            )
            .await
        {
            Ok(()) => tracing::info!(pr = watch.pull_request_id, user = %watch.user_id,
                "watching paused: the user's GitHub sign-in is not available"),
            Err(error) => tracing::warn!(%error, pr = watch.pull_request_id,
                "a watch could not be marked paused"),
        }
    }
}

/// One probe answer: a closed or merged pull request ends the watch; changed values read the pull request in full
/// and make an automatic pass due. The values are stored only after a successful read, so a failed read is tried
/// again by the next probe.
async fn observe(
    state: &AppState,
    watch: &WatchRecord,
    value: Value,
    now: Timestamp,
) -> Result<(), WatchError> {
    let rl = state.db.review_loop();
    let (user, pr) = (watch.user_id.as_str(), watch.pull_request_id);
    let Some(pull) = serde_json::from_value::<Option<ProbedRepository>>(value)
        .ok()
        .flatten()
        .and_then(|repository| repository.pull_request)
    else {
        tracing::debug!(
            pr,
            "a watched pull request was not found or not readable; it is probed again next time"
        );
        let kept = watch.probe.clone().unwrap_or(Value::Null);
        rl.record_probe(user, pr, &kept, now).await?;
        return Ok(());
    };
    if let Some(closed) = pull.state.as_deref().filter(|state| *state != "OPEN") {
        rl.set_watch_state(user, pr, WatchState::Ended, now).await?;
        tracing::info!(
            pr,
            user,
            state = closed,
            "watched pull request merged or closed; watching ended"
        );
        return Ok(());
    }
    let probe = Probe {
        head: pull.head_ref_oid,
        updated_at: pull.updated_at,
        threads: pull.review_threads.total_count,
        comments: pull.comments.total_count,
        last_review: pull.reviews.nodes.into_iter().next().map(|node| node.id),
    };
    let stored: Option<Probe> = watch
        .probe
        .clone()
        .and_then(|value| serde_json::from_value(value).ok());
    if stored.as_ref() != Some(&probe) {
        let read = match super::read_if_due(state, &watch.tenant_id, user, &watch.pr, true).await {
            Ok(read) => read.outcome,
            Err(LoopError::TooSoon) => ReadOutcome::Fresh,
            Err(error) => return Err(error.into()),
        };
        if let ReadOutcome::Failed(failure) = read {
            tracing::debug!(
                pr,
                ?failure,
                "a changed watched pull request could not be read; the next probe tries again"
            );
            return Ok(());
        }
        rl.set_pass_due(user, pr, Some(now)).await?;
        tracing::debug!(
            pr,
            user,
            "watched pull request changed; an automatic assessment pass is due"
        );
    }
    rl.record_probe(user, pr, &serde_json::to_value(&probe)?, now)
        .await?;
    if watch.state == WatchState::Paused {
        rl.set_watch_state(user, pr, WatchState::Active, now)
            .await?;
        tracing::info!(pr, user, "watching resumed: the probe read GitHub again");
    }
    Ok(())
}

/// Settles every live watch (or those of `only`); failures are logged per watch.
async fn settle_all(
    state: &AppState,
    only: Option<&HashSet<i64>>,
    now: Timestamp,
) -> Result<(), DbError> {
    for watch in state.db.review_loop().live_watches().await? {
        if only.is_some_and(|only| !only.contains(&watch.pull_request_id)) {
            continue;
        }
        if let Err(error) = settle(state, &watch, now).await {
            tracing::warn!(pr = watch.pull_request_id, user = %watch.user_id, %error, error_chain = ?error,
                "a watch could not be settled; the next tick tries again");
        }
    }
    Ok(())
}

/// The watch's next state: a due pass is requested (or waits for the bot's round, the spacing or the daily cap);
/// without one the watch shows whether a pass of its pull request runs. Paused and ended watches stay.
async fn settle(state: &AppState, watch: &WatchRecord, now: Timestamp) -> Result<(), WatchError> {
    if matches!(watch.state, WatchState::Paused | WatchState::Ended) {
        return Ok(());
    }
    let next = match watch.pass_due_at {
        Some(due) => try_pass(state, watch, due, now).await?,
        None if live_pass(state, watch.pull_request_id).await? => WatchState::JobRunning,
        None => WatchState::Active,
    };
    if next != watch.state {
        state
            .db
            .review_loop()
            .set_watch_state(&watch.user_id, watch.pull_request_id, next, now)
            .await?;
        tracing::debug!(pr = watch.pull_request_id, user = %watch.user_id, ?next, "watch state changed");
    }
    Ok(())
}

async fn live_pass(state: &AppState, pr: i64) -> Result<bool, DbError> {
    Ok(state
        .db
        .review_loop()
        .live_passes()
        .await?
        .iter()
        .any(|pass| pass.pull_request_id == pr))
}

/// Requests the due automatic pass unless thepastaclaw still reviews the pull request (for at most
/// [`BOT_ROUND_WAIT`] since the pass became due); returns the watch's state.
async fn try_pass(
    state: &AppState,
    watch: &WatchRecord,
    due: Timestamp,
    now: Timestamp,
) -> Result<WatchState, WatchError> {
    let (user, pr) = (watch.user_id.as_str(), watch.pull_request_id);
    let snapshot: Option<Snapshot> = state
        .db
        .review_loop()
        .pr_state(pr)
        .await?
        .and_then(|row| row.github_snapshot)
        .and_then(|value| serde_json::from_value(value).ok());
    if super::bot_round_running(state, &watch.pr, snapshot.as_ref()) {
        if now.duration_since(due) < BOT_ROUND_WAIT {
            return Ok(WatchState::WaitingForBotRound);
        }
        tracing::info!(
            pr,
            "the bot's round runs past the wait; the automatic assessment pass is requested anyway"
        );
    }
    let rl = state.db.review_loop();
    Ok(
        match assess::request_pass(state, pr, PassReason::Watch).await? {
            PassQueued::Queued => {
                // The pass covers the change for every watcher of the pull request.
                for other in rl.live_watches().await? {
                    if other.pull_request_id == pr && other.pass_due_at.is_some() {
                        rl.set_pass_due(&other.user_id, pr, None).await?;
                    }
                }
                WatchState::JobRunning
            }
            PassQueued::AlreadyRunning => WatchState::JobRunning,
            PassQueued::Spaced => WatchState::Active,
            PassQueued::DailyLimit => WatchState::LimitReached,
            PassQueued::Skipped => {
                rl.set_pass_due(user, pr, None).await?;
                WatchState::Active
            }
        },
    )
}

/// The round a "questions ready" event is for: the bot round, or the head while the pull request has none.
fn round_key(snapshot: Option<&Snapshot>, pass: &PassRecord) -> String {
    match snapshot {
        Some(snapshot) if !snapshot.rounds.is_empty() => format!("round:{}", snapshot.rounds.len()),
        Some(snapshot) => format!("head:{}", snapshot.head),
        None => format!("head:{}", pass.head.0),
    }
}

/// After an automatic pass: every watcher of its pull request records it with the undecided questions now, and
/// gets one `questions_ready` event per round while questions wait; a watch showing the job returns to active.
pub(crate) async fn pass_ended(state: &AppState, pass: &PassRecord) -> Result<(), WatchError> {
    if pass.cause != PassCause::Watch {
        return Ok(());
    }
    let pr = pass.pull_request_id;
    let rl = state.db.review_loop();
    let watchers: Vec<WatchRecord> = rl
        .live_watches()
        .await?
        .into_iter()
        .filter(|watch| watch.pull_request_id == pr)
        .collect();
    let Some(first) = watchers.first() else {
        return Ok(());
    };
    let loaded = Loaded::of(
        state,
        PrHandle {
            pr: first.pr.clone(),
            id: pr,
        },
    )
    .await?;
    // Questions ready for a decision: waiting items other than those another pass still assesses.
    let ready = queue::waiting(&loaded.queue_items(&[]))
        .iter()
        .filter(|item| item.state != ItemState::Assessing)
        .count();
    let questions = u32::try_from(ready).unwrap_or(u32::MAX);
    let round = round_key(loaded.snapshot.as_ref(), pass);
    let now = Timestamp::now();
    for watch in &watchers {
        rl.record_auto_pass(&watch.user_id, pr, questions, now)
            .await?;
        if questions > 0 && watch.notified_for.as_deref() != Some(round.as_str()) {
            let pr_ref = &watch.pr;
            let event = NewUserEvent {
                user_id: watch.user_id.clone(),
                kind: UserEventKind::QuestionsReady,
                pull_request_id: Some(pr),
                count: questions,
                text: format!(
                    "{}/{} #{}: {questions} {} ready",
                    pr_ref.repo.owner,
                    pr_ref.repo.name,
                    pr_ref.number,
                    if questions == 1 {
                        "question"
                    } else {
                        "questions"
                    }
                ),
                url: Some(format!(
                    "/pr/{}/{}/{}/comments",
                    pr_ref.repo.owner, pr_ref.repo.name, pr_ref.number
                )),
            };
            rl.add_user_event(&watch.tenant_id, &event, now).await?;
            rl.set_notified(&watch.user_id, pr, &round).await?;
            tracing::info!(pr, user = %watch.user_id, questions, %round, "questions ready event written");
        }
        if watch.state == WatchState::JobRunning {
            rl.set_watch_state(&watch.user_id, pr, WatchState::Active, now)
                .await?;
        }
    }
    Ok(())
}
