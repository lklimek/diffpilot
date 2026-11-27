//! The decision feed for external implementing agents (ADR-023, plan §5.2, `docs/contracts/http-api.md` "Review
//! loop"). RF1, RF1b and RF2 are reads (a session or a `read` token; an agent token with `review_loop_execute`
//! also records its subscription); RF3–RF5 take only an access token with `review_loop_execute` (`TokenOnly`),
//! never a session. No route here decides (T22, T25). The work lives in `review_loop::feed` and
//! `review_loop::notify`.

use std::borrow::Cow;
use std::convert::Infallible;
use std::time::Duration;

use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Extension, Path, Query, State};
use axum::http::header::{CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use contracts::api::error::ApiErrorCode;
use contracts::api::review_loop::{
    Claim, ClaimRequest, EffectKind, EffectReportRequest, FeedDecision, FeedEvent, FeedQuery,
    IssueAttempt, IssueAttemptRequest, PushTarget, PutPushTargetRequest,
};
use contracts::common::RepoRef;
use contracts::ids::{DecisionId, EffectId};
use contracts::routes::{
    CLAIM_DECISION, DECISION_FEED_EVENTS, GET_DECISION_FEED, GET_FEED_DECISION, PUT_PUSH_TARGET,
    REPORT_EFFECT, RESERVE_ISSUE_ATTEMPT,
};
use contracts::time::Timestamp;
use db::error::DbError;
use futures::stream::{self, Stream, StreamExt};
use tokio::sync::broadcast;
use tokio::time::MissedTickBehavior;

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::api::events::{HEARTBEAT, StreamSlot, holding, session_live};
use crate::api::notifications::{notify_code, notify_message};
use crate::auth::access_tokens::TokenOnly;
use crate::auth::{Session, TokenPrincipal};
use crate::dashboard::snapshot::{etag, etag_matches};
use crate::review_loop::feed::{self, FeedError, FeedSignal, Presence};
use crate::review_loop::notify::{self, NotifyError};
use crate::state::AppState;

/// A stream records the token's activity every this many heartbeats (about once a minute).
const SEEN_EVERY: u32 = 4;

/// A feed request failed.
#[derive(Debug, thiserror::Error)]
pub enum FeedApiError {
    /// Malformed path.
    #[error("the path is not valid")]
    Path(#[from] PathRejection),
    /// Malformed query.
    #[error("the query is not valid")]
    Query(#[from] QueryRejection),
    /// Malformed body.
    #[error("the request body is not valid")]
    Json(#[from] JsonRejection),
    /// The feed work failed.
    #[error(transparent)]
    Feed(#[from] FeedError),
    /// The push target could not be saved.
    #[error(transparent)]
    Notify(#[from] NotifyError),
}

impl ApiErrorSource for FeedApiError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Path(_) | Self::Query(_) | Self::Json(_) => ApiErrorCode::InvalidRequest,
            Self::Feed(error) => match error {
                FeedError::NotFound => ApiErrorCode::NotFound,
                FeedError::Capabilities(_) | FeedError::Repo | FeedError::Report(_) => {
                    ApiErrorCode::InvalidRequest
                }
                FeedError::CursorExpired => ApiErrorCode::CursorExpired,
                FeedError::Claimed => ApiErrorCode::Claimed,
                FeedError::CapabilityMissing => ApiErrorCode::CapabilityMissing,
                FeedError::NothingOpen => ApiErrorCode::Conflict,
                FeedError::LeaseExpired => ApiErrorCode::LeaseExpired,
                FeedError::Db(error) => error.code(),
            },
            Self::Notify(error) => notify_code(error),
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Path(rejection) => rejection.public_message(),
            Self::Query(rejection) => rejection.public_message(),
            Self::Json(rejection) => rejection.public_message(),
            Self::Feed(error) => match error {
                FeedError::NotFound => Cow::Borrowed("The decision or effect was not found."),
                FeedError::Capabilities(why) => Cow::Owned(format!(
                    "capabilities: {why} (fix, reply, resolve, pr_comment, follow_up_pr, upstream_issue, todo, \
                     dismiss_review)."
                )),
                FeedError::Repo => Cow::Borrowed("repo must be owner/name."),
                FeedError::CursorExpired => Cow::Borrowed(
                    "The page cursor left the feed meanwhile; start again from the first page.",
                ),
                FeedError::Claimed => {
                    Cow::Borrowed("Another agent holds the claim on this decision.")
                }
                FeedError::CapabilityMissing => Cow::Borrowed(
                    "An open effect of this decision is outside your capabilities; it is not claimable for you.",
                ),
                FeedError::NothingOpen => Cow::Borrowed(
                    "This decision has nothing left for an agent (superseded or done).",
                ),
                FeedError::LeaseExpired => Cow::Borrowed(
                    "Your claim's lease ended; the decision went back to the feed. Claim it again.",
                ),
                FeedError::Report(why) => Cow::Owned(format!("The report is not valid: {why}.")),
                FeedError::Db(error) => error.public_message(),
            },
            Self::Notify(error) => notify_message(error),
        }
    }
}

type ApiResult<T> = Result<T, ApiFailure<FeedApiError>>;

fn failure(error: impl Into<FeedApiError>) -> ApiFailure<FeedApiError> {
    ApiFailure(error.into())
}

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(GET_DECISION_FEED.path, get(list))
        .route(GET_FEED_DECISION.path, get(one))
        .route(DECISION_FEED_EVENTS.path, get(events))
        .route(CLAIM_DECISION.path, post(claim))
        .route(REPORT_EFFECT.path, post(report))
        .route(RESERVE_ISSUE_ATTEMPT.path, post(reserve_issue_attempt))
        .route(PUT_PUSH_TARGET.path, put(put_push).delete(delete_push))
}

type Principal = Option<Extension<TokenPrincipal>>;

/// The query's capabilities and repository filter.
fn parsed(query: &FeedQuery) -> Result<(Vec<EffectKind>, Option<RepoRef>), FeedError> {
    Ok((
        feed::parse_capabilities(&query.capabilities)?,
        feed::parse_repo(query.repo.as_deref())?,
    ))
}

/// RF1: claimable decisions within the capabilities, with a weak `ETag` (304 when unchanged).
async fn list(
    State(state): State<AppState>,
    session: Session,
    principal: Principal,
    headers: HeaderMap,
    query: Result<Query<FeedQuery>, QueryRejection>,
) -> ApiResult<Response> {
    let Query(query) = query.map_err(failure)?;
    let (capabilities, repo) = parsed(&query).map_err(failure)?;
    if let Some(Extension(principal)) = &principal {
        feed::record_subscription(&state, principal, &capabilities)
            .await
            .map_err(failure)?;
    }
    let page = feed::page(
        &state,
        &session.tenant_id,
        &capabilities,
        repo.as_ref(),
        query.cursor.as_deref(),
    )
    .await
    .map_err(failure)?;
    let bytes =
        serde_json::to_vec(&page).map_err(|error| failure(FeedError::Db(DbError::from(error))))?;
    let tag = etag(&bytes);
    let unchanged = headers
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| etag_matches(value, &tag));
    let mut response = if unchanged {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let mut response = bytes.into_response();
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        response
    };
    if let Ok(value) = HeaderValue::from_str(&tag) {
        response.headers_mut().insert(ETAG, value);
    }
    Ok(response)
}

/// RF1b: one decision of the reader's tenant.
async fn one(
    State(state): State<AppState>,
    session: Session,
    path: Result<Path<DecisionId>, PathRejection>,
) -> ApiResult<Json<FeedDecision>> {
    let Path(decision) = path.map_err(failure)?;
    Ok(Json(
        feed::decision(&state, &session.tenant_id, decision)
            .await
            .map_err(failure)?,
    ))
}

/// RF3: claims a decision under a lease.
async fn claim(
    State(state): State<AppState>,
    TokenOnly(principal): TokenOnly,
    path: Result<Path<DecisionId>, PathRejection>,
    body: Result<Json<ClaimRequest>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<Claim>)> {
    let Path(decision) = path.map_err(failure)?;
    let Json(request) = body.map_err(failure)?;
    let claimed = feed::claim(&state, &principal, decision, &request.capabilities)
        .await
        .map_err(failure)?;
    Ok((StatusCode::CREATED, Json(claimed)))
}

/// RF4: reports one effect of the token's claim.
async fn report(
    State(state): State<AppState>,
    TokenOnly(principal): TokenOnly,
    path: Result<Path<EffectId>, PathRejection>,
    body: Result<Json<EffectReportRequest>, JsonRejection>,
) -> ApiResult<StatusCode> {
    let Path(effect) = path.map_err(failure)?;
    let Json(request) = body.map_err(failure)?;
    feed::report(&state, &principal, effect, &request)
        .await
        .map_err(failure)?;
    Ok(StatusCode::CREATED)
}

/// RF4b: grants one attempt while this token holds the effect lease.
async fn reserve_issue_attempt(
    State(state): State<AppState>,
    TokenOnly(principal): TokenOnly,
    path: Result<Path<EffectId>, PathRejection>,
    body: Result<Json<IssueAttemptRequest>, JsonRejection>,
) -> ApiResult<Json<IssueAttempt>> {
    let Path(effect) = path.map_err(failure)?;
    let Json(request) = body.map_err(failure)?;
    Ok(Json(
        feed::reserve_issue_attempt(&state, &principal, effect, request)
            .await
            .map_err(failure)?,
    ))
}

/// RF5 `PUT`: sets the calling token's push target.
async fn put_push(
    State(state): State<AppState>,
    TokenOnly(principal): TokenOnly,
    body: Result<Json<PutPushTargetRequest>, JsonRejection>,
) -> ApiResult<Json<PushTarget>> {
    let Json(request) = body.map_err(failure)?;
    Ok(Json(
        notify::put_push_target(&state, &principal, &request)
            .await
            .map_err(failure)?,
    ))
}

/// RF5 `DELETE`: removes the calling token's push target.
async fn delete_push(
    State(state): State<AppState>,
    TokenOnly(principal): TokenOnly,
) -> ApiResult<StatusCode> {
    notify::delete_push_target(&state, &principal)
        .await
        .map_err(failure)?;
    Ok(StatusCode::NO_CONTENT)
}

/// RF2: the claimable decisions within the capabilities now, then `decision_available` / `decision_cancelled` as
/// the feed changes and a `heartbeat` every 15 s. An agent token with `review_loop_execute` counts as connected
/// (no push) while the stream is open.
async fn events(
    State(state): State<AppState>,
    session: Session,
    principal: Principal,
    query: Result<Query<FeedQuery>, QueryRejection>,
) -> Response {
    let slot = match StreamSlot::open(&session) {
        Ok(slot) => slot,
        Err(full) => return full.into_response(),
    };
    let opened = async {
        let Query(query) = query.map_err(failure)?;
        let (capabilities, repo) = parsed(&query).map_err(failure)?;
        let agent = principal.map(|Extension(principal)| principal);
        if let Some(principal) = &agent {
            feed::record_subscription(&state, principal, &capabilities)
                .await
                .map_err(failure)?;
        }
        feed_stream(state, session, agent, capabilities, repo, HEARTBEAT)
            .await
            .map_err(failure)
    }
    .await;
    match opened {
        Ok(events) => Sse::new(
            holding(slot, events)
                .filter_map(|event| async move { sse_event(&event) })
                .map(Ok::<_, Infallible>),
        )
        .into_response(),
        Err(error) => error.into_response(),
    }
}

fn sse_event(event: &FeedEvent) -> Option<Event> {
    let name = match event {
        FeedEvent::DecisionAvailable { .. } => "decision_available",
        FeedEvent::DecisionCancelled { .. } => "decision_cancelled",
        FeedEvent::Heartbeat { .. } => "heartbeat",
    };
    match Event::default().event(name).json_data(event) {
        Ok(sse) => Some(sse),
        Err(error) => {
            tracing::error!(%error, "could not encode a feed event for SSE; the event was skipped");
            None
        }
    }
}

/// What the stream of one reader shows of a signal.
fn visible(
    signal: &FeedSignal,
    tenant_id: &str,
    capabilities: &[EffectKind],
    repo: Option<&RepoRef>,
) -> bool {
    signal.tenant_id == tenant_id
        && feed::in_repo(&signal.pr, repo)
        && signal.concerns(capabilities)
}

/// Snapshot, live signals and heartbeats; ends when the session or token is no longer valid, the reader falls
/// behind, or the server stops.
pub(crate) async fn feed_stream(
    state: AppState,
    session: Session,
    agent: Option<TokenPrincipal>,
    capabilities: Vec<EffectKind>,
    repo: Option<RepoRef>,
    heartbeat: Duration,
) -> Result<impl Stream<Item = FeedEvent>, FeedError> {
    // Subscribe first: a change after the snapshot read is buffered, never lost.
    let live = state.feed.subscribe();
    let presence: Option<Presence> = agent
        .as_ref()
        .filter(|principal| {
            principal
                .scopes
                .contains(&contracts::api::access_tokens::TokenScope::ReviewLoopExecute)
        })
        .map(|principal| state.feed.connect(&principal.token_id));
    let snapshot: Vec<FeedEvent> = state
        .db
        .review_loop()
        .claimable(
            &session.tenant_id,
            &capabilities,
            repo.as_ref(),
            Timestamp::now(),
        )
        .await?
        .into_iter()
        .map(|entry| FeedEvent::DecisionAvailable {
            decision: entry.decision.id,
            fetch: feed::fetch_path(entry.decision.id),
            pr: entry.pr,
        })
        .collect();
    let tenant = session.tenant_id.clone();
    let declared = capabilities.clone();
    let live = stream::unfold(live, |mut live| async move {
        match live.recv().await {
            Ok(signal) => Some((Some(signal), live)),
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::debug!(
                    skipped,
                    "feed stream fell behind; closing so the agent reconnects"
                );
                None
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    })
    .filter_map(move |signal| {
        let keep = signal.filter(|signal| visible(signal, &tenant, &capabilities, repo.as_ref()));
        std::future::ready(keep.map(|signal| Some(signal.event)))
    })
    .chain(stream::once(async { None }));
    let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let heartbeats = stream::unfold(ticks, |mut ticks| async move {
        ticks.tick().await;
        Some((
            Some(FeedEvent::Heartbeat {
                at: Timestamp::now(),
            }),
            ticks,
        ))
    });
    let mut stopping = state.stopping.subscribe();
    let stopped = async move {
        // An error means the sender is gone, which also ends the stream.
        let _ = stopping.wait_for(|stop| *stop).await;
    };
    let mut beats = 0u32;
    Ok(stream::iter(snapshot)
        .chain(
            stream::select(live, heartbeats)
                .take_while(|event| std::future::ready(event.is_some()))
                .filter_map(std::future::ready),
        )
        .then(move |event| {
            let state = state.clone();
            let session = session.clone();
            let agent = agent.clone();
            let declared = declared.clone();
            let _held = &presence;
            let heartbeat = matches!(event, FeedEvent::Heartbeat { .. });
            if heartbeat {
                beats = beats.wrapping_add(1);
            }
            let touch = heartbeat && beats.is_multiple_of(SEEN_EVERY);
            async move {
                if !heartbeat {
                    return Some(event);
                }
                if !session_live(&state, &session).await {
                    return None;
                }
                if touch && let Some(principal) = agent.as_ref() {
                    let seen = feed::record_subscription(&state, principal, &declared).await;
                    if let Err(error) = seen {
                        tracing::debug!(%error, "feed stream activity could not be recorded");
                    }
                }
                Some(event)
            }
        })
        .take_while(|event| std::future::ready(event.is_some()))
        .filter_map(std::future::ready)
        .take_until(stopped))
}
