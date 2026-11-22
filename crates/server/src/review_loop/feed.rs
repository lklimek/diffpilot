//! The decision feed for external implementing agents (plan §5.2, ADR-023 decision 7): claimable decisions, live
//! streams and their presence, claims under a lease, agent reports, the `round_executed` user event, and the
//! wake-ups that announce a newly claimable decision (an SSE event for streams, a push for agents without one).
//!
//! The feed is recomputed from the database by one task ([`sync`]): after a decision, claim or report (woken) and
//! every [`SYNC_INTERVAL`] (so a lapsed lease returns its decision). Comparing with the previous computation gives
//! `decision_available` and `decision_cancelled`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use contracts::api::access_tokens::{AccessTokenId, TokenScope};
use contracts::api::review_loop::{
    Claim, DecisionFeed, EffectKind, EffectReportRequest, EffectState, FeedDecision, FeedEvent,
    ReportOutcome, UserEventKind,
};
use contracts::common::{PrRef, RepoRef};
use contracts::ids::{DecisionId, EffectId};
use contracts::routes::GET_FEED_DECISION;
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::review_loop::{
    ClaimOutcome, DecisionOwner, DeliveryTarget, NewUserEvent, OpenFeedEntry, ReportOutcomeRecord,
    ReportRequest, SubscriptionRecord,
};
use jiff::SignedDuration;
use tokio::sync::{Notify, broadcast};

use crate::auth::TokenPrincipal;
use crate::github::rest;
use crate::state::AppState;

/// A claim's lease; each report renews it.
pub const LEASE: SignedDuration = SignedDuration::from_mins(30);
/// The feed is recomputed at least this often.
pub const SYNC_INTERVAL: Duration = Duration::from_secs(15);
/// Decisions per page of RF1.
pub const PAGE_SIZE: usize = 100;
/// Longest `reference` or `message` of a report.
pub const MAX_REPORT_TEXT: usize = 2000;
/// Messages per hour to one channel or push target.
pub const MESSAGES_PER_HOUR: usize = 30;
const HOUR: Duration = Duration::from_secs(3600);
/// Buffered feed events per stream; a stream that falls further behind closes and reconnects.
const SIGNAL_BUFFER: usize = 256;

/// A feed event and whom it concerns.
#[derive(Clone, Debug)]
pub struct FeedSignal {
    /// Tenant of the decision.
    pub tenant_id: String,
    /// Its pull request (a stream's `repo` filter).
    pub pr: PrRef,
    /// Kinds of its open agent effects: `decision_available` reaches a stream only within its capabilities.
    pub kinds: Vec<EffectKind>,
    /// The event.
    pub event: FeedEvent,
}

/// In-process state of the feed and of notification deliveries (one server process serves every stream).
#[derive(Clone)]
pub struct FeedHub(Arc<Hub>);

struct Hub {
    events: broadcast::Sender<FeedSignal>,
    /// Open streams per access token (FeedSubscriber.connected; no push while connected).
    connected: Mutex<HashMap<AccessTokenId, u32>>,
    sync_wanted: Notify,
    deliveries_wanted: Notify,
    /// The claimable decisions of the last [`sync`].
    open: tokio::sync::Mutex<Vec<OpenFeedEntry>>,
    /// Send times of the last hour per channel or push target.
    sent: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl Default for FeedHub {
    fn default() -> Self {
        Self(Arc::new(Hub {
            events: broadcast::channel(SIGNAL_BUFFER).0,
            connected: Mutex::default(),
            sync_wanted: Notify::new(),
            deliveries_wanted: Notify::new(),
            open: tokio::sync::Mutex::default(),
            sent: Mutex::default(),
        }))
    }
}

impl fmt::Debug for FeedHub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FeedHub").finish_non_exhaustive()
    }
}

/// An open feed stream of an access token; the token counts as connected until every one is dropped.
#[derive(Debug)]
pub struct Presence {
    hub: FeedHub,
    token: AccessTokenId,
}

impl Drop for Presence {
    fn drop(&mut self) {
        let mut connected = self
            .hub
            .0
            .connected
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = connected.get_mut(&self.token) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                connected.remove(&self.token);
            }
        }
    }
}

impl FeedHub {
    /// Live feed events from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<FeedSignal> {
        self.0.events.subscribe()
    }

    /// Marks `token` connected while the returned guard lives.
    pub fn connect(&self, token: &AccessTokenId) -> Presence {
        *self
            .0
            .connected
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(token.clone())
            .or_insert(0) += 1;
        Presence {
            hub: self.clone(),
            token: token.clone(),
        }
    }

    /// Whether `token` holds a live feed stream now.
    pub fn is_connected(&self, token: &AccessTokenId) -> bool {
        self.0
            .connected
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(token)
    }

    /// Asks the feed task to recompute the feed now.
    pub fn wake_sync(&self) {
        self.0.sync_wanted.notify_one();
    }

    /// Asks the delivery task to look for due deliveries now.
    pub fn wake_deliveries(&self) {
        self.0.deliveries_wanted.notify_one();
    }

    pub(crate) async fn sync_wanted(&self) {
        self.0.sync_wanted.notified().await;
    }

    pub(crate) async fn deliveries_wanted(&self) {
        self.0.deliveries_wanted.notified().await;
    }

    /// Takes one of the hourly messages of `target`; `Err(wait)` when [`MESSAGES_PER_HOUR`] were sent within the
    /// last hour.
    pub(crate) fn take_message(&self, target: &str, now: Instant) -> Result<(), Duration> {
        let mut sent = self.0.sent.lock().unwrap_or_else(PoisonError::into_inner);
        let times = sent.entry(target.to_owned()).or_default();
        while times
            .front()
            .is_some_and(|first| now.duration_since(*first) >= HOUR)
        {
            times.pop_front();
        }
        if times.len() >= MESSAGES_PER_HOUR {
            let first = times.front().copied().unwrap_or(now);
            return Err(HOUR.saturating_sub(now.duration_since(first)));
        }
        times.push_back(now);
        Ok(())
    }

    fn publish(&self, signal: FeedSignal) {
        // No receiver (no open stream) is not an error.
        let _ = self.0.events.send(signal);
    }
}

/// A feed request failed.
#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    /// Unknown decision or effect, or another tenant's.
    #[error("the decision or effect was not found")]
    NotFound,
    /// `capabilities` names no effect kind, or an unknown one.
    #[error("the capabilities are not valid: {0}")]
    Capabilities(String),
    /// `repo` is not `owner/name`.
    #[error("the repository filter is not owner/name")]
    Repo,
    /// The cursor's decision left the feed.
    #[error("the page cursor is no longer in the feed")]
    CursorExpired,
    /// Another token holds a live claim (RF3), or the effect is not claimed by this token (RF4).
    #[error("another agent holds the claim")]
    Claimed,
    /// An open effect is outside the capabilities.
    #[error("an open effect is outside the capabilities")]
    CapabilityMissing,
    /// Nothing is left for an agent (superseded, done, or no agent effect).
    #[error("the decision has nothing left to claim")]
    NothingOpen,
    /// The claim's lease ended before the report.
    #[error("the claim's lease ended")]
    LeaseExpired,
    /// A report without its required field, or too long.
    #[error("the report is not valid: {0}")]
    Report(&'static str),
    /// The database failed.
    #[error("the decision feed could not be read or changed")]
    Db(#[from] DbError),
}

/// The fetch path of a decision (RF1b), as sent in `decision_available` and push messages.
pub fn fetch_path(decision: DecisionId) -> String {
    GET_FEED_DECISION
        .path
        .replace("{id}", &decision.to_string())
}

/// The capabilities of a `FeedQuery`: comma-separated `EffectKind` names, at least one.
pub fn parse_capabilities(raw: &str) -> Result<Vec<EffectKind>, FeedError> {
    let mut kinds = Vec::new();
    for name in raw
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        let kind: EffectKind =
            serde_json::from_value(serde_json::Value::String(name.to_owned()))
                .map_err(|_| FeedError::Capabilities(format!("unknown effect kind \"{name}\"")))?;
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    if kinds.is_empty() {
        return Err(FeedError::Capabilities(
            "name at least one effect kind".into(),
        ));
    }
    Ok(kinds)
}

/// The `repo` filter of a `FeedQuery` (`owner/name`).
pub fn parse_repo(raw: Option<&str>) -> Result<Option<RepoRef>, FeedError> {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    let (owner, name) = raw.split_once('/').ok_or(FeedError::Repo)?;
    let repo = RepoRef {
        owner: owner.to_owned(),
        name: name.to_owned(),
    };
    if !rest::is_valid_repo(&repo) {
        return Err(FeedError::Repo);
    }
    Ok(Some(repo))
}

/// Whether `pr` passes a stream's `repo` filter (GitHub names are case-insensitive).
pub fn in_repo(pr: &PrRef, repo: Option<&RepoRef>) -> bool {
    repo.is_none_or(|repo| {
        repo.owner.eq_ignore_ascii_case(&pr.repo.owner)
            && repo.name.eq_ignore_ascii_case(&pr.repo.name)
    })
}

/// Whether every kind is within the capabilities.
pub fn within(kinds: &[EffectKind], capabilities: &[EffectKind]) -> bool {
    kinds.iter().all(|kind| capabilities.contains(kind))
}

/// Records an agent's feed activity and capabilities (RF1, RF2): only for a token with `review_loop_execute`.
pub async fn record_subscription(
    state: &AppState,
    principal: &TokenPrincipal,
    capabilities: &[EffectKind],
) -> Result<(), FeedError> {
    if !principal.scopes.contains(&TokenScope::ReviewLoopExecute) {
        return Ok(());
    }
    state
        .db
        .review_loop()
        .subscribe(
            &principal.tenant_id,
            &principal.token_id,
            capabilities,
            Timestamp::now(),
        )
        .await?;
    Ok(())
}

/// RF1: one page of the claimable decisions within `capabilities`, oldest first.
pub async fn page(
    state: &AppState,
    tenant_id: &str,
    capabilities: &[EffectKind],
    repo: Option<&RepoRef>,
    cursor: Option<&str>,
) -> Result<DecisionFeed, FeedError> {
    let all = state
        .db
        .review_loop()
        .claimable(tenant_id, capabilities, repo, Timestamp::now())
        .await?;
    let start = match cursor {
        None => 0,
        Some(cursor) => {
            all.iter()
                .position(|entry| entry.decision.id.to_string() == cursor)
                .ok_or(FeedError::CursorExpired)?
                + 1
        }
    };
    let rest: Vec<FeedDecision> = all.into_iter().skip(start).collect();
    let next_cursor = (rest.len() > PAGE_SIZE).then(|| rest[PAGE_SIZE - 1].decision.id.to_string());
    Ok(DecisionFeed {
        decisions: rest.into_iter().take(PAGE_SIZE).collect(),
        next_cursor,
    })
}

async fn owned_decision(
    state: &AppState,
    tenant_id: &str,
    decision: DecisionId,
) -> Result<DecisionOwner, FeedError> {
    state
        .db
        .review_loop()
        .decision_owner(decision)
        .await?
        .filter(|owner| owner.tenant_id == tenant_id)
        .ok_or(FeedError::NotFound)
}

/// RF1b: one decision of the tenant, any state.
pub async fn decision(
    state: &AppState,
    tenant_id: &str,
    decision: DecisionId,
) -> Result<FeedDecision, FeedError> {
    owned_decision(state, tenant_id, decision).await?;
    state
        .db
        .review_loop()
        .feed_decision(decision)
        .await?
        .ok_or(FeedError::NotFound)
}

/// RF3: claims every open agent effect of a current decision for the token until the lease ends.
pub async fn claim(
    state: &AppState,
    principal: &TokenPrincipal,
    decision: DecisionId,
    capabilities: &[EffectKind],
) -> Result<Claim, FeedError> {
    owned_decision(state, &principal.tenant_id, decision).await?;
    let rl = state.db.review_loop();
    let outcome = rl
        .claim(
            decision,
            &principal.token_id,
            capabilities,
            LEASE,
            Timestamp::now(),
        )
        .await;
    let lease_until = match outcome {
        Ok(ClaimOutcome::Claimed(until)) => until,
        Ok(ClaimOutcome::Taken) => return Err(FeedError::Claimed),
        Ok(ClaimOutcome::CapabilityMissing) => return Err(FeedError::CapabilityMissing),
        Ok(ClaimOutcome::NothingOpen) => return Err(FeedError::NothingOpen),
        Err(DbError::NotFound) => return Err(FeedError::NotFound),
        Err(error) => return Err(error.into()),
    };
    state.feed.wake_sync();
    tracing::info!(
        target: "audit",
        %decision,
        token_id = %principal.token_id,
        "an agent claimed a decision of the feed"
    );
    Ok(Claim {
        decision: rl
            .feed_decision(decision)
            .await?
            .ok_or(FeedError::NotFound)?,
        lease_until,
    })
}

fn report_text(value: Option<&str>) -> Result<Option<String>, FeedError> {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    if value.is_some_and(|value| value.chars().count() > MAX_REPORT_TEXT) {
        return Err(FeedError::Report(
            "reference and message are at most 2000 characters",
        ));
    }
    Ok(value.map(str::to_owned))
}

/// RF4: records the claimer's report on one effect; a repeated idempotency key changes nothing.
pub async fn report(
    state: &AppState,
    principal: &TokenPrincipal,
    effect: EffectId,
    request: &EffectReportRequest,
) -> Result<(), FeedError> {
    let reference = report_text(request.reference.as_deref())?;
    let message = report_text(request.message.as_deref())?;
    match request.outcome {
        ReportOutcome::Done if reference.is_none() => {
            return Err(FeedError::Report("done needs a reference"));
        }
        ReportOutcome::Failed if message.is_none() => {
            return Err(FeedError::Report("failed needs a message"));
        }
        _ => {}
    }
    let rl = state.db.review_loop();
    let owner = rl
        .effect_owner(effect)
        .await?
        .filter(|owner| owner.tenant_id == principal.tenant_id)
        .ok_or(FeedError::NotFound)?;
    let now = Timestamp::now();
    let recorded = rl
        .report(
            &owner.tenant_id,
            effect,
            &principal.token_id,
            &ReportRequest {
                outcome: request.outcome,
                reference,
                message,
                idempotency_key: request.idempotency_key.to_string(),
            },
            LEASE,
            now,
        )
        .await;
    match recorded {
        Ok(ReportOutcomeRecord::Recorded) => {}
        Ok(ReportOutcomeRecord::Duplicate) => {
            tracing::debug!(%effect, "repeated agent report ignored (same idempotency key)");
            return Ok(());
        }
        Ok(ReportOutcomeRecord::NotClaimer) => return Err(FeedError::Claimed),
        Ok(ReportOutcomeRecord::LeaseExpired) => return Err(FeedError::LeaseExpired),
        Err(DbError::NotFound) => return Err(FeedError::NotFound),
        Err(error) => return Err(error.into()),
    }
    tracing::info!(
        target: "audit",
        %effect,
        decision = %owner.decision,
        token_id = %principal.token_id,
        outcome = ?request.outcome,
        "an agent reported an effect"
    );
    state.feed.wake_sync();
    if request.outcome == ReportOutcome::Done {
        round_executed(state, &owner, now).await?;
    }
    Ok(())
}

/// Round checks run one at a time, so two last reports cannot both announce the same round.
static ROUND_CHECK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(Default::default);

/// After a `done` report: when every standing decision of the PR with effects is executed (each effect done or
/// cancelled), the decisions executed since the PR's last `round_executed` event make one round; each of their
/// deciders gets one `round_executed` event (RQ18).
async fn round_executed(
    state: &AppState,
    owner: &DecisionOwner,
    now: Timestamp,
) -> Result<(), FeedError> {
    let _serial = ROUND_CHECK.lock().await;
    let rl = state.db.review_loop();
    let decisions = rl.pr_decisions(owner.pull_request_id).await?;
    let standing = super::standing(&decisions);
    let executed = |state: EffectState| matches!(state, EffectState::Done | EffectState::Cancelled);
    let acted: Vec<_> = standing
        .values()
        .filter(|stored| {
            stored
                .decision
                .effects
                .iter()
                .any(|effect| effect.state == EffectState::Done)
        })
        .collect();
    let all_executed = standing
        .values()
        .all(|stored| stored.decision.effects.iter().all(|e| executed(e.state)));
    if acted.is_empty() || !all_executed {
        return Ok(());
    }
    let last = rl
        .last_user_event_at(owner.pull_request_id, UserEventKind::RoundExecuted)
        .await?;
    let round: Vec<DecisionId> = acted
        .iter()
        .filter(|stored| {
            let finished = stored.decision.effects.iter().map(|e| e.updated_at).max();
            last.is_none_or(|last| finished.is_some_and(|finished| finished > last))
        })
        .map(|stored| stored.decision.id)
        .collect();
    if round.is_empty() {
        return Ok(());
    }
    let pr = rl
        .feed_decision(owner.decision)
        .await?
        .ok_or(FeedError::NotFound)?
        .pr;
    let count = u32::try_from(round.len()).unwrap_or(u32::MAX);
    let mut users = Vec::new();
    for decision in &round {
        let user = rl.decision_user(*decision).await?;
        if !users.contains(&user) {
            users.push(user);
        }
    }
    let text = format!(
        "{}/{} #{}: {count} decision{} executed",
        pr.repo.owner,
        pr.repo.name,
        pr.number,
        if count == 1 { "" } else { "s" }
    );
    let url = comments_url(state, &pr);
    for user_id in users {
        rl.add_user_event(
            &owner.tenant_id,
            &NewUserEvent {
                user_id,
                kind: UserEventKind::RoundExecuted,
                pull_request_id: Some(owner.pull_request_id),
                count,
                text: text.clone(),
                url: url.clone(),
            },
            now,
        )
        .await?;
    }
    tracing::info!(
        pr = owner.pull_request_id,
        count,
        "agents executed every decision of a round; its deciders are notified"
    );
    state.feed.wake_deliveries();
    Ok(())
}

/// The absolute URL of a PR's Comments page.
pub fn comments_url(state: &AppState, pr: &PrRef) -> Option<String> {
    state
        .config
        .public_url
        .join(&format!(
            "pr/{}/{}/{}/comments",
            pr.repo.owner, pr.repo.name, pr.number
        ))
        .ok()
        .map(String::from)
}

/// Recomputes the feed: returns lapsed claims, then announces decisions that became claimable (SSE, and a push
/// for each subscribed token with a push target, the capabilities and no live stream) and those that left.
pub async fn sync(state: &AppState) -> Result<(), FeedError> {
    let now = Timestamp::now();
    let rl = state.db.review_loop();
    let returned = rl.expire_leases(now).await?;
    if !returned.is_empty() {
        tracing::info!(
            count = returned.len(),
            "claims whose lease ended returned their decisions to the feed"
        );
    }
    let open = rl.open_feed(now).await?;
    let mut known = state.feed.0.open.lock().await;
    let before: HashSet<DecisionId> = known.iter().map(|entry| entry.decision).collect();
    let after: HashSet<DecisionId> = open.iter().map(|entry| entry.decision).collect();
    let mut subscriptions: HashMap<String, Vec<SubscriptionRecord>> = HashMap::new();
    let mut pushes = 0;
    for entry in open
        .iter()
        .filter(|entry| !before.contains(&entry.decision))
    {
        state.feed.publish(FeedSignal {
            tenant_id: entry.tenant_id.clone(),
            pr: entry.pr.clone(),
            kinds: entry.kinds.clone(),
            event: FeedEvent::DecisionAvailable {
                decision: entry.decision,
                pr: entry.pr.clone(),
                fetch: fetch_path(entry.decision),
            },
        });
        if !subscriptions.contains_key(&entry.tenant_id) {
            let found = rl.subscriptions(&entry.tenant_id, now).await?;
            subscriptions.insert(entry.tenant_id.clone(), found);
        }
        for subscription in subscriptions.get(&entry.tenant_id).into_iter().flatten() {
            let wanted = subscription.push.is_some()
                && within(&entry.kinds, &subscription.capabilities)
                && !state.feed.is_connected(&subscription.token_id);
            let target = DeliveryTarget::Push {
                decision: entry.decision,
                token: subscription.token_id.clone(),
            };
            if wanted && rl.enqueue_delivery(&entry.tenant_id, &target, now).await? {
                pushes += 1;
            }
        }
    }
    for entry in known
        .iter()
        .filter(|entry| !after.contains(&entry.decision))
    {
        state.feed.publish(FeedSignal {
            tenant_id: entry.tenant_id.clone(),
            pr: entry.pr.clone(),
            kinds: Vec::new(),
            event: FeedEvent::DecisionCancelled {
                decision: entry.decision,
                pr: entry.pr.clone(),
            },
        });
    }
    *known = open;
    if pushes > 0 {
        tracing::debug!(
            pushes,
            "push wake-ups queued for agents without a live stream"
        );
        state.feed.wake_deliveries();
    }
    Ok(())
}

/// Starts the feed task and the delivery task; both stop with the server.
pub fn spawn(state: AppState) {
    tokio::spawn(run_sync(state.clone()));
    tokio::spawn(super::notify::run_deliveries(state));
}

async fn run_sync(state: AppState) {
    let mut stopping = state.stopping.subscribe();
    tracing::info!("decision feed task started");
    loop {
        if let Err(error) = sync(&state).await {
            tracing::warn!(%error, "the decision feed could not be recomputed; retried shortly");
        }
        tokio::select! {
            () = tokio::time::sleep(SYNC_INTERVAL) => {}
            () = state.feed.sync_wanted() => {}
            _ = stopping.wait_for(|stop| *stop) => break,
        }
    }
    tracing::info!("decision feed task stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_parse_known_kinds_once() {
        assert_eq!(
            parse_capabilities("fix, reply,fix,,resolve").expect("valid"),
            [EffectKind::Fix, EffectKind::Reply, EffectKind::Resolve]
        );
        assert!(matches!(
            parse_capabilities(" , "),
            Err(FeedError::Capabilities(_))
        ));
        assert!(matches!(
            parse_capabilities("fix,merge"),
            Err(FeedError::Capabilities(message)) if message.contains("merge")
        ));
    }

    #[test]
    fn repo_filter_is_owner_and_name() {
        assert_eq!(parse_repo(None).expect("none"), None);
        let repo = parse_repo(Some("acme/node-client"))
            .expect("valid")
            .expect("repo");
        let pr = PrRef {
            repo: RepoRef {
                owner: "ACME".into(),
                name: "Node-Client".into(),
            },
            number: 1,
        };
        assert!(in_repo(&pr, Some(&repo)));
        assert!(in_repo(&pr, None));
        for bad in ["acme", "acme/node/client", "/x", "a b/c"] {
            assert!(parse_repo(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn hourly_message_limit_per_target() {
        let hub = FeedHub::default();
        let start = Instant::now();
        for _ in 0..MESSAGES_PER_HOUR {
            assert!(hub.take_message("channel-a", start).is_ok());
        }
        let wait = hub
            .take_message("channel-a", start + Duration::from_secs(60))
            .expect_err("limit");
        assert_eq!(wait, Duration::from_secs(3540));
        assert!(hub.take_message("channel-b", start).is_ok());
        assert!(hub.take_message("channel-a", start + HOUR).is_ok());
    }

    #[test]
    fn presence_lasts_while_a_stream_is_open() {
        let hub = FeedHub::default();
        let token = AccessTokenId("agentAAA".into());
        let first = hub.connect(&token);
        let second = hub.connect(&token);
        assert!(hub.is_connected(&token));
        drop(first);
        assert!(hub.is_connected(&token));
        drop(second);
        assert!(!hub.is_connected(&token));
    }
}
