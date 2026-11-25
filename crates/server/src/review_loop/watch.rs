//! Watch (ADR-023, plan §2.8, RW1–RW2): the scheduler probes watched pull requests without live webhook
//! updates every [`PROBE_INTERVAL`] with the watcher's own token, one alias-batched GraphQL query
//! per [`PROBE_BATCH`] pull requests; only a pull request whose probe values changed is read in full. A change makes
//! an automatic assessment pass due: it waits while thepastaclaw reviews the pull request (at most
//! [`BOT_ROUND_WAIT`]), then goes through [`assess::request_pass`] (one pass per pull request, spacing, daily cap). A
//! plug-in change signal probes its watched pull requests without live updates at once. A pass that leaves undecided questions writes one
//! `questions_ready` user event per round and watcher; delivering it is the notification task's work. A user whose
//! GitHub rate limit is used up is not probed until GitHub's reset or retry time.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::Duration;

use contracts::api::review_loop::{
    ItemState, PrBadgeView, ReadFailure, UserEventKind, WatchList, WatchState, WatchedPr,
};
use contracts::common::PrRef;
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::review_loop::{NewUserEvent, PassCause, PassRecord, WatchOutcome, WatchRecord};
use jiff::SignedDuration;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::Instant;

use super::assess::{self, AssessError, PassQueued, PassReason};
use super::observe::Snapshot;
use super::{Loaded, LoopError, PrHandle, ReadOutcome, queue};
use crate::auth::{Session, UserTokenError};
use crate::dashboard::{installations, plugin::PrKey};
use crate::github::graphql::ReadBudget;
use crate::github::work::{EventBudget, UserTokens, WorkClient, WorkError};
use crate::state::AppState;

/// Most pull requests one user watches (plan §2.8).
// TODO(S13-2 follow-up): confirm the Watch constants against the owner's usage (plan §12).
pub const WATCH_LIMIT: u32 = 20;
/// Least time between two periodic probes of one watch.
pub const PROBE_INTERVAL: SignedDuration = SignedDuration::from_hours(1);
/// Fallback backoff when GitHub does not return a rate limit expiry.
const RATE_LIMIT_BACKOFF: SignedDuration = SignedDuration::from_mins(5);
/// Pull requests per probe query (spike S13-2: 10 or 20 cost one point; 10 keeps a failed request small).
pub const PROBE_BATCH: usize = 10;
/// Longest wait of a due automatic pass for the bot's round to end.
pub const BOT_ROUND_WAIT: SignedDuration = SignedDuration::from_mins(90);
/// Users whose probes wait for GitHub's rate limit, and until when (kept after it passes, so a repeated limit is
/// logged quietly; removed by the next successful probe).
static RATE_LIMITED: LazyLock<Mutex<HashMap<String, Timestamp>>> = LazyLock::new(Mutex::default);

/// Serialises marking a pass due with requesting it: a detection stored while a pass picks its items stays due.
static DETECTION: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(Default::default);

/// Whether `user`'s probes wait for GitHub's rate limit at `now`.
fn waits_for_limit(user: &str, now: Timestamp) -> bool {
    limit_until(user, now).is_some()
}

fn limit_until(user: &str, now: Timestamp) -> Option<Timestamp> {
    RATE_LIMITED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(user)
        .copied()
        .filter(|until| *until > now)
}

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
    /// A live watch needs a later attempt after an incomplete read.
    #[error("GitHub did not complete a watched pull request probe")]
    IncompleteProbe,
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
    let my_work = super::badge::my_work(state, &session.tenant_id, &session.user_id).await?;
    let mut watches = Vec::new();
    for record in rl.watches(&session.user_id).await? {
        let handle = PrHandle {
            pr: record.pr.clone(),
            id: record.pull_request_id,
        };
        let badge = match super::views::badge(state, handle, &my_work).await {
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
        passes_today: assess::DAILY_PASSES_PER_USER
            - state
                .db
                .assessments()
                .quota(
                    &contracts::ids::UserId(session.user_id.clone()),
                    Timestamp::now(),
                )
                .await?
                .remaining,
        daily_limit: assess::DAILY_PASSES_PER_USER,
    })
}

/// Tells the plug-in host whether any user watches a pull request (ADR-022 §3 presence).
pub async fn presence(state: &AppState) -> Result<(), DbError> {
    let any = !state.db.review_loop().live_watches().await?.is_empty();
    state.plugins.set_watching(any);
    Ok(())
}

fn by_user<'a>(watches: &[&'a WatchRecord]) -> BTreeMap<&'a str, Vec<&'a WatchRecord>> {
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
    let due: Vec<_> = watches
        .iter()
        .filter(|watch| {
            watch.state != WatchState::Ended
                && watch
                    .last_probe_at
                    .is_none_or(|at| now.duration_since(at) >= PROBE_INTERVAL)
        })
        .collect();
    for (user, list) in by_user(&without_live_updates(state, &due).await?) {
        if !waits_for_limit(user, now) {
            probe(state, user, &list, now, ProbeTrigger::Poll).await;
        }
    }
    settle_all(state, None, now).await
}

async fn without_live_updates<'a>(
    state: &AppState,
    watches: &[&'a WatchRecord],
) -> Result<Vec<&'a WatchRecord>, DbError> {
    let mut covered = HashMap::new();
    let mut due = Vec::new();
    for &watch in watches {
        let live = match covered.entry(watch.tenant_id.as_str()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(installations::live(state, watch.tenant_id.as_str()).await?)
            }
        };
        if !live.contains(&installations::key(&watch.pr.repo)) {
            due.push(watch);
        }
    }
    Ok(due)
}

/// A plug-in change signal (a PR Hygiene change mark, thepastaclaw's round end): the watched pull requests among
/// `changed` without live updates are probed at once, and all changed watches are settled.
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
    match without_live_updates(state, &hit).await {
        Ok(watches) => {
            for (user, list) in by_user(&watches) {
                if !waits_for_limit(user, now) {
                    probe(state, user, &list, now, ProbeTrigger::Poll).await;
                }
            }
        }
        Err(error) => {
            tracing::warn!(%error, "watch installation coverage could not be checked after a plug-in change")
        }
    }
    let prs: HashSet<i64> = hit.iter().map(|watch| watch.pull_request_id).collect();
    if let Err(error) = settle_all(state, Some(&prs), now).await {
        tracing::warn!(%error, "watches could not be settled after a plug-in change signal");
    }
}

const EVENT_PROBE_INTERVAL: SignedDuration = SignedDuration::from_secs(60);

/// Probes a live PR using an available watcher, or returns the wait until another attempt.
pub(crate) async fn event_probe(
    state: &AppState,
    pr: i64,
    now: Timestamp,
    review: bool,
    budgets: &mut HashMap<String, EventBudget>,
    instant: Instant,
) -> Result<Option<Duration>, WatchError> {
    let rl = state.db.review_loop();
    let watches = rl.watches_of(pr).await?;
    let Some(first) = watches.first() else {
        return Ok(None);
    };
    if !installations::live(state, &first.tenant_id)
        .await?
        .contains(&installations::key(&first.pr.repo))
    {
        return Ok(None);
    }
    if let Some(due) = watches
        .iter()
        .filter_map(|watch| watch.last_probe_at)
        .max()
        .map(|at| at + EVENT_PROBE_INTERVAL)
        .filter(|due| *due > now)
    {
        return Ok(Some((due - now).try_into().unwrap_or_default()));
    }
    let mut selected = None;
    let mut earliest_wait = None;
    for watch in &watches {
        let budget = budgets.entry(watch.user_id.clone()).or_default();
        let backoff = limit_until(&watch.user_id, now)
            .and_then(|reset| Duration::try_from(reset - now).ok())
            .unwrap_or_default();
        let delay = backoff.max(budget.blocked_for(instant, now).await);
        if !delay.is_zero() {
            earliest_wait = Some(earliest_wait.map_or(delay, |old: Duration| old.min(delay)));
        } else if selected.is_none() || watch.state != WatchState::Paused {
            selected = Some((watch, budget.clone()));
            if watch.state != WatchState::Paused {
                break;
            }
        }
    }
    let Some((watch, budget)) = selected else {
        return Ok(earliest_wait);
    };

    // Failed attempts still consume this PR's interval.
    let previous = watch.probe.as_ref().unwrap_or(&Value::Null);
    match rl.record_probe(&watch.user_id, pr, previous, now).await {
        Ok(()) => (),
        Err(DbError::NotFound) => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    match probe(
        state,
        &watch.user_id,
        &[watch],
        now,
        ProbeTrigger::Webhook {
            review,
            budget: &budget,
            instant,
        },
    )
    .await
    {
        ProbeOutcome::Complete => {
            settle_all(state, Some(&HashSet::from([pr])), now).await?;
            Ok(None)
        }
        ProbeOutcome::Incomplete => Err(WatchError::IncompleteProbe),
        ProbeOutcome::Deferred(delay) => Ok(Some(delay.max(EVENT_PROBE_INTERVAL.unsigned_abs()))),
    }
}

#[derive(Clone, Copy)]
enum ProbeTrigger<'a> {
    Poll,
    Webhook {
        review: bool,
        budget: &'a EventBudget,
        instant: Instant,
    },
}

enum ProbeOutcome {
    Complete,
    Incomplete,
    Deferred(Duration),
}

/// Probes `watches` of `user` and handles each answer; a user without a usable token pair is paused.
async fn probe(
    state: &AppState,
    user: &str,
    watches: &[&WatchRecord],
    now: Timestamp,
    trigger: ProbeTrigger<'_>,
) -> ProbeOutcome {
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
            return ProbeOutcome::Incomplete;
        }
    };
    let client = match trigger {
        ProbeTrigger::Poll => client,
        ProbeTrigger::Webhook {
            budget, instant, ..
        } => client.with_event_budget(budget.clone(), instant),
    };
    let answers = match client
        .batched(watches, PROBE_BATCH, "", |watch| lookup(&watch.pr))
        .await
    {
        Ok(answers) => answers,
        Err(WorkError::Token(UserTokenError::SignInRequired) | WorkError::Unauthorized) => {
            pause(state, watches, now).await;
            return ProbeOutcome::Complete;
        }
        Err(WorkError::PrimaryRateLimit { reset }) => {
            return ProbeOutcome::Deferred(backoff_for_rate_limit(user, reset, now));
        }
        Err(WorkError::SecondaryRateLimit { retry_after }) => {
            let reset = retry_after
                .and_then(|wait| SignedDuration::try_from(wait).ok())
                .and_then(|wait| now.checked_add(wait));
            return ProbeOutcome::Deferred(backoff_for_rate_limit(user, reset, now));
        }
        Err(WorkError::EventBudget) => {
            if let ProbeTrigger::Webhook {
                budget, instant, ..
            } = trigger
            {
                return ProbeOutcome::Deferred(budget.blocked_for(instant, now).await);
            }
            return ProbeOutcome::Incomplete;
        }
        Err(error) => {
            tracing::warn!(user, %error, error_chain = ?error,
                "watched pull requests could not be probed; another probe can retry");
            return ProbeOutcome::Incomplete;
        }
    };
    if RATE_LIMITED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(user)
        .is_some()
    {
        tracing::info!(user, "watch probes read GitHub again after its rate limit");
    }
    if !answers.failed.is_empty() {
        tracing::debug!(
            user,
            failed = answers.failed.len(),
            "some watched pull requests were not probed; another probe can retry"
        );
    }
    let mut complete = answers.failed.is_empty();
    for (watch, value) in answers.resolved {
        match observe(state, watch, value, now, trigger).await {
            Ok(Some(delay)) => return ProbeOutcome::Deferred(delay),
            Ok(None) => (),
            Err(error) => {
                complete = false;
                tracing::warn!(pr = watch.pull_request_id, user, %error, error_chain = ?error,
                    "a watched pull request's probe could not be handled");
            }
        }
    }
    if complete {
        ProbeOutcome::Complete
    } else {
        ProbeOutcome::Incomplete
    }
}

fn backoff_for_rate_limit(user: &str, reset: Option<Timestamp>, now: Timestamp) -> Duration {
    let until = reset
        .filter(|reset| *reset > now)
        .unwrap_or(now + RATE_LIMIT_BACKOFF);
    rate_limited(user, until, now);
    (until - now).try_into().unwrap_or_default()
}

/// GitHub's rate limit stops `user`'s probes until `until`: logged at `warn` when it starts, quietly while it lasts.
fn rate_limited(user: &str, until: Timestamp, now: Timestamp) {
    let was = RATE_LIMITED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(user.to_owned(), until);
    if was.is_some_and(|was| was > now - RATE_LIMIT_BACKOFF) {
        tracing::debug!(user, %until, "GitHub's rate limit still stops this user's watch probes");
    } else {
        tracing::warn!(user, %until,
            "GitHub's rate limit stops this user's watch probes; they resume after the limit resets");
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
/// and make an automatic pass due. The values are stored only after a read covering them: when the read fails or was
/// skipped as too soon after another, the old values stay, so the next probe (after the interval) reads again.
async fn observe(
    state: &AppState,
    watch: &WatchRecord,
    value: Value,
    now: Timestamp,
    trigger: ProbeTrigger<'_>,
) -> Result<Option<Duration>, WatchError> {
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
        return keep_probe(state, watch, now).await.map(|()| None);
    };
    if let Some(closed) = pull.state.as_deref().filter(|state| *state != "OPEN") {
        rl.end_pr_watches(pr, now).await?;
        tracing::info!(
            pr,
            user,
            state = closed,
            "watched pull request merged or closed; watching ended"
        );
        return Ok(None);
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
    let changed = stored.as_ref() != Some(&probe);
    // Editing a review or resolving a thread may leave all probe values unchanged.
    if changed || matches!(trigger, ProbeTrigger::Webhook { review: true, .. }) {
        let budget = match trigger {
            ProbeTrigger::Poll => None,
            ProbeTrigger::Webhook {
                budget, instant, ..
            } => Some(ReadBudget { budget, instant }),
        };
        let read = match super::read_if_due_with_budget(
            state,
            &watch.tenant_id,
            user,
            &watch.pr,
            true,
            budget,
        )
        .await
        {
            Ok(read) => read.outcome,
            Err(LoopError::EventBudget(delay)) => return Ok(Some(delay)),
            Err(LoopError::TooSoon) => {
                tracing::debug!(
                    pr,
                    "a changed watched pull request was read moments ago; the next probe reads it"
                );
                keep_probe(state, watch, now).await?;
                return if matches!(trigger, ProbeTrigger::Webhook { .. }) {
                    Err(WatchError::IncompleteProbe)
                } else {
                    Ok(None)
                };
            }
            Err(LoopError::Unreadable(ReadFailure::RateLimited)) => {
                ReadOutcome::Failed(ReadFailure::RateLimited)
            }
            Err(error) => return Err(error.into()),
        };
        if let ReadOutcome::Failed(failure) = read {
            if failure == ReadFailure::RateLimited {
                return Ok(Some(backoff_for_rate_limit(user, None, now)));
            }
            tracing::debug!(
                pr,
                ?failure,
                "a changed watched pull request could not be read; the next probe tries again"
            );
            keep_probe(state, watch, now).await?;
            return if matches!(trigger, ProbeTrigger::Webhook { .. }) {
                Err(WatchError::IncompleteProbe)
            } else {
                Ok(None)
            };
        }
        if changed || matches!(read, ReadOutcome::Read { new } if new > 0) {
            let _detection = DETECTION.lock().await;
            rl.set_pass_due(user, pr, Some(now)).await?;
            tracing::debug!(
                pr,
                user,
                "watched pull request changed; an automatic assessment pass is due"
            );
        }
    }
    rl.record_probe(user, pr, &serde_json::to_value(&probe)?, now)
        .await?;
    if watch.state == WatchState::Paused {
        rl.set_watch_state(user, pr, WatchState::Active, now)
            .await?;
        tracing::info!(pr, user, "watching resumed: the probe read GitHub again");
    }
    Ok(None)
}

/// Records the probe time with the watch's old values: the change stays unconfirmed for the next probe.
async fn keep_probe(
    state: &AppState,
    watch: &WatchRecord,
    now: Timestamp,
) -> Result<(), WatchError> {
    let kept = watch.probe.clone().unwrap_or(Value::Null);
    state
        .db
        .review_loop()
        .record_probe(&watch.user_id, watch.pull_request_id, &kept, now)
        .await?;
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

/// A report read found more comments. Use the normal watch admission rules for that PR.
pub(super) async fn comments_found_after_report(
    state: &AppState,
    pr: i64,
) -> Result<(), WatchError> {
    let rl = state.db.review_loop();
    let now = Timestamp::now();
    {
        let _detection = DETECTION.lock().await;
        for watch in rl
            .live_watches()
            .await?
            .iter()
            .filter(|watch| watch.pull_request_id == pr)
        {
            rl.set_pass_due(&watch.user_id, pr, Some(now)).await?;
        }
    }
    settle_all(state, Some(&HashSet::from([pr])), now).await?;
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
    // Held until the due marks are cleared: a detection stored meanwhile stays due for the next pass.
    let _detection = DETECTION.lock().await;
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
                // Nothing new to assess (e.g. an on-demand pass assessed the change): the watcher still hears of
                // the questions that wait.
                let loaded = Loaded::of(
                    state,
                    PrHandle {
                        pr: watch.pr.clone(),
                        id: pr,
                    },
                )
                .await?;
                let head = state.db.pull_requests().get(pr).await?.pr.head_sha.0;
                let round = round_key(loaded.snapshot.as_ref(), &head);
                questions_ready(state, watch, ready_questions(&loaded), &round, now).await?;
                WatchState::Active
            }
        },
    )
}

/// The round a "questions ready" event is for: the bot round, or the head while the pull request has none.
fn round_key(snapshot: Option<&Snapshot>, head: &str) -> String {
    match snapshot {
        Some(snapshot) if !snapshot.rounds.is_empty() => format!("round:{}", snapshot.rounds.len()),
        Some(snapshot) => format!("head:{}", snapshot.head),
        None => format!("head:{head}"),
    }
}

/// Questions ready for a decision: waiting items other than those a pass still assesses.
fn ready_questions(loaded: &Loaded) -> u32 {
    let ready = queue::waiting(&loaded.queue_items(&[]))
        .iter()
        .filter(|item| item.state != ItemState::Assessing)
        .count();
    u32::try_from(ready).unwrap_or(u32::MAX)
}

/// One `questions_ready` event for the watcher per round while questions wait.
async fn questions_ready(
    state: &AppState,
    watch: &WatchRecord,
    questions: u32,
    round: &str,
    now: Timestamp,
) -> Result<(), WatchError> {
    if questions == 0 || watch.notified_for.as_deref() == Some(round) {
        return Ok(());
    }
    let (pr, pr_ref) = (watch.pull_request_id, &watch.pr);
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
        url: Some(super::notify::comments_path(pr_ref)),
    };
    // The database writes it once per watcher and round, however many settles race to it.
    let written = state
        .db
        .review_loop()
        .questions_ready(&watch.tenant_id, &event, round, now)
        .await?;
    if written {
        state.notify.wake_deliveries();
        tracing::info!(pr, user = %watch.user_id, questions, %round, "questions ready event written");
    }
    Ok(())
}

/// After a pass: every watcher of its pull request gets one `questions_ready` event per round while questions wait
/// (also after an on-demand pass, which the daily cap does not hold back); after an automatic pass each also records
/// it with the undecided questions now, and a watch showing the job returns to active.
pub(crate) async fn pass_ended(state: &AppState, pass: &PassRecord) -> Result<(), WatchError> {
    let automatic = pass.cause == PassCause::Watch;
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
    let questions = ready_questions(&loaded);
    let round = round_key(loaded.snapshot.as_ref(), &pass.head.0);
    let now = Timestamp::now();
    for watch in &watchers {
        if automatic {
            rl.record_auto_pass(&watch.user_id, pr, questions, now)
                .await?;
        }
        questions_ready(state, watch, questions, &round, now).await?;
        if automatic && watch.state == WatchState::JobRunning {
            rl.set_watch_state(&watch.user_id, pr, WatchState::Active, now)
                .await?;
        }
    }
    Ok(())
}
