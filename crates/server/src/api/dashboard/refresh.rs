//! Dashboard summary, items, refreshes and their events (R1–R5, `docs/contracts/http-api.md` "Dashboard"). Every
//! request reads the signed-in user's own data; another user's refresh id answers `not_found` (T16). Reads take
//! `Session` (a session cookie or a `read` personal access token); writes take the session and CSRF.

use std::borrow::Cow;
use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_TYPE, ETAG, IF_NONE_MATCH, RETRY_AFTER};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use contracts::api::dashboard::{
    DashboardEvent, DashboardSummary, ItemKey, ItemKind, ItemPage, ItemQuery, RefreshStatus,
    RefreshTrigger, ReviewLink, ScheduleStatus, ScheduledRun, WorkItem,
};
use contracts::api::error::ApiErrorCode;
use contracts::ids::RefreshId;
use contracts::routes::{
    CANCEL_DASHBOARD_REFRESH, DASHBOARD_REFRESH_EVENTS, GET_DASHBOARD, GET_DASHBOARD_REFRESH,
    LIST_DASHBOARD_ITEMS, RANK_DASHBOARD_REFRESH, START_DASHBOARD_REFRESH,
};
use db::error::DbError;
use db::repo::dashboard::StoredDashboardSettings;
use futures::stream::{self, Stream, StreamExt};
use tokio::sync::broadcast;
use tokio::time::MissedTickBehavior;

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::{Session, SessionCsrf};
use crate::dashboard::refresh::rank::ItemJudgement;
use crate::dashboard::refresh::{self, ControlError, Scope, StartError, status_of};
use crate::dashboard::schedule::Schedule;
use crate::dashboard::snapshot::{self, CursorError, Snapshot, View};
use crate::state::AppState;

/// Interval of the `heartbeat` event, which also re-checks the session.
const HEARTBEAT: Duration = Duration::from_secs(15);

/// A dashboard request failed.
#[derive(Debug, thiserror::Error)]
pub enum DashboardError {
    /// The refresh id in the path is not valid.
    #[error("the refresh id is not valid")]
    Path(#[from] PathRejection),
    /// The item query is not valid.
    #[error("the item query is not valid")]
    Query(#[from] QueryRejection),
    /// The database failed, or the refresh is unknown.
    #[error("the dashboard could not be read or changed")]
    Db(#[from] DbError),
    /// A refresh of the user is scanning (R2) or live (R4).
    #[error("a refresh of the user is running")]
    Running,
    /// The user's GitHub rate limit resets at `retry_at`.
    #[error("the user's GitHub rate limit is exhausted")]
    RateLimited {
        /// Reset time.
        retry_at: DateTime<Utc>,
    },
    /// Cancel of an ended refresh, or Retry ranking of one that cannot be retried.
    #[error("the refresh does not allow this now")]
    NotAllowed,
    /// The cursor is not usable.
    #[error(transparent)]
    Cursor(#[from] CursorError),
}

impl From<StartError> for DashboardError {
    fn from(error: StartError) -> Self {
        match error {
            StartError::Running => Self::Running,
            StartError::RateLimited { retry_at } => Self::RateLimited { retry_at },
            StartError::Db(error) => Self::Db(error),
        }
    }
}

impl From<ControlError> for DashboardError {
    fn from(error: ControlError) -> Self {
        match error {
            ControlError::NotAllowed => Self::NotAllowed,
            ControlError::Running => Self::Running,
            ControlError::Db(error) => Self::Db(error),
        }
    }
}

impl ApiErrorSource for DashboardError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Path(_) | Self::Query(_) | Self::Cursor(CursorError::Malformed) => {
                ApiErrorCode::InvalidRequest
            }
            Self::Db(error) => error.code(),
            Self::Running => ApiErrorCode::RefreshRunning,
            Self::RateLimited { .. } => ApiErrorCode::GithubRateLimited,
            Self::NotAllowed => ApiErrorCode::Conflict,
            Self::Cursor(CursorError::Expired) => ApiErrorCode::CursorExpired,
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Path(rejection) => rejection.public_message(),
            Self::Query(rejection) => rejection.public_message(),
            Self::Cursor(CursorError::Malformed) => Cow::Borrowed("The cursor is not valid."),
            Self::Db(error) => error.public_message(),
            Self::Running => Cow::Borrowed("A refresh is already running. Wait for it to finish."),
            Self::RateLimited { retry_at } => Cow::Owned(format!(
                "GitHub's rate limit is used up until {} UTC. Refresh after that.",
                retry_at.format("%H:%M")
            )),
            Self::NotAllowed => Cow::Borrowed(
                "The refresh does not allow this in its current state. Reload and try again.",
            ),
            Self::Cursor(CursorError::Expired) => Cow::Borrowed(
                "The list changed since this page was read. Read it again from the start.",
            ),
        }
    }
}

impl IntoResponse for DashboardError {
    fn into_response(self) -> Response {
        let retry_at = match &self {
            Self::RateLimited { retry_at } => Some(*retry_at),
            _ => None,
        };
        let mut response = ApiFailure(self).into_response();
        if let Some(retry_at) = retry_at {
            let wait = (retry_at - Utc::now()).num_seconds().max(1);
            if let Ok(value) = HeaderValue::from_str(&wait.to_string()) {
                response.headers_mut().insert(RETRY_AFTER, value);
            }
        }
        response
    }
}

type DashboardResult<T> = Result<T, DashboardError>;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(GET_DASHBOARD.path, get(summary))
        .route(LIST_DASHBOARD_ITEMS.path, get(items))
        .route(START_DASHBOARD_REFRESH.path, post(start))
        .route(GET_DASHBOARD_REFRESH.path, get(refresh_status))
        .route(CANCEL_DASHBOARD_REFRESH.path, post(cancel))
        .route(RANK_DASHBOARD_REFRESH.path, post(rank))
        .route(DASHBOARD_REFRESH_EVENTS.path, get(events))
}

fn scope(session: &Session) -> Scope {
    Scope {
        tenant_id: session.tenant_id.clone(),
        user_id: session.user_id.clone(),
    }
}

/// The current snapshot scored with the judgements present now; `None` without a readable snapshot.
async fn current_view(
    state: &AppState,
    scope: &Scope,
) -> Result<Option<(String, Snapshot, View, db::repo::dashboard::RefreshRecord)>, DbError> {
    let Some((record, snapshot)) = snapshot::load(state, &scope.tenant_id, &scope.user_id).await?
    else {
        return Ok(None);
    };
    let judgements: HashMap<ItemKey, ItemJudgement> = state
        .dashboard
        .ranker()
        .judgements(state, scope, &snapshot)
        .await?;
    let view = snapshot::build(
        &snapshot,
        &judgements,
        state.config.memcan_ui_url.as_deref(),
    );
    Ok(Some((snapshot::revision(&record), snapshot, view, record)))
}

async fn summary(
    State(state): State<AppState>,
    session: Session,
) -> DashboardResult<Json<DashboardSummary>> {
    let scope = scope(&session);
    let dashboard = state.db.dashboard();
    let latest_refresh = dashboard
        .latest_refresh(&scope.tenant_id, &scope.user_id)
        .await?
        .as_ref()
        .map(status_of);
    let (revision, meta, scanned_at) = match current_view(&state, &scope).await? {
        Some((revision, snapshot, view, record)) => (
            revision,
            Some(view.meta(&snapshot, &record)),
            record.scanned_at,
        ),
        None => (snapshot::NO_SNAPSHOT.to_owned(), None, None),
    };
    let stored = dashboard.settings(&scope.tenant_id, &scope.user_id).await?;
    Ok(Json(DashboardSummary {
        revision,
        latest_refresh,
        snapshot: meta,
        schedule: schedule_status(stored.as_ref(), state.dashboard.now(), scanned_at),
        // TODO(WP11-JUDGE): count the snapshot's judgements made with an older ranking prompt.
        older_prompt_judgements: 0,
    }))
}

/// The schedule as stored (ADR-020): the last run's instant is the configured local time of its date, the next
/// run follows the due rule given the current snapshot's scan time (`last_success`).
fn schedule_status(
    stored: Option<&StoredDashboardSettings>,
    now: DateTime<Utc>,
    last_success: Option<DateTime<Utc>>,
) -> ScheduleStatus {
    let settings = stored.map(|s| s.settings.clone()).unwrap_or_default();
    let configured = settings
        .time_zone
        .as_deref()
        .and_then(|zone| Schedule::parse(zone, &settings.schedule_time));
    let last_scheduled = stored.and_then(|s| s.last_scheduled.as_ref());
    let last_run = last_scheduled.and_then(|run| {
        Some(ScheduledRun {
            at: configured?.instant(run.on),
            outcome: run.outcome,
            reason: run.reason.clone(),
        })
    });
    let next_run_at = Schedule::of(&settings)
        .map(|schedule| schedule.next_run_at(now, last_scheduled.map(|run| run.on), last_success));
    ScheduleStatus {
        enabled: settings.schedule_enabled,
        time: settings.schedule_time,
        time_zone: settings.time_zone,
        next_run_at,
        last_run,
    }
}

async fn items(
    State(state): State<AppState>,
    session: Session,
    query: Result<Query<ItemQuery>, QueryRejection>,
    headers: HeaderMap,
) -> DashboardResult<Response> {
    let Query(query) = query?;
    let scope = scope(&session);
    let (revision, page, next_cursor) = match current_view(&state, &scope).await? {
        Some((revision, _, view, _)) => {
            let (page, next) = snapshot::page(&view.items, &query, &revision)?;
            (revision, page, next)
        }
        None => {
            let revision = snapshot::NO_SNAPSHOT.to_owned();
            let (page, next) = snapshot::page(&[], &query, &revision)?;
            (revision, page, next)
        }
    };
    let mut items: Vec<WorkItem> = Vec::with_capacity(page.len());
    let mut heads = Vec::with_capacity(page.len());
    for item in page {
        heads.push(item.head_sha);
        items.push(item.item);
    }
    review_links(&state, &session, &mut items, &heads).await?;
    let body = ItemPage {
        revision,
        items,
        next_cursor,
    };
    let bytes = serde_json::to_vec(&body).map_err(DbError::from)?;
    let tag = snapshot::etag(&bytes);
    let unchanged = headers
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| snapshot::etag_matches(value, &tag));
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

/// Fills `review` of the pull requests on the page from the tenant's analyses, read now (plan §F.1): the analysis
/// a reader is shown (`latest_for_pr`), whether the PR head moved since (snapshot head vs analysed head) and
/// whether the reader saved review state.
async fn review_links(
    state: &AppState,
    session: &Session,
    items: &mut [WorkItem],
    heads: &[Option<String>],
) -> Result<(), DbError> {
    if !items.iter().any(|item| item.kind == ItemKind::Pr) {
        return Ok(());
    }
    let language = state.db.users().get(&session.user_id).await?.language;
    for (item, head) in items.iter_mut().zip(heads) {
        let (ItemKind::Pr, Some(repo), Some(number)) = (item.kind, &item.repo, item.number) else {
            continue;
        };
        let Some(pr_id) = state
            .db
            .pull_requests()
            .find(&session.tenant_id, repo, number)
            .await?
        else {
            continue;
        };
        let Some(analysis) = state
            .db
            .analyses()
            .latest_for_pr(pr_id, language)
            .await?
            .filter(|record| record.tenant_id == session.tenant_id)
        else {
            continue;
        };
        item.review = Some(ReviewLink {
            analysis_id: analysis.id,
            head_moved: head
                .as_deref()
                .is_some_and(|head| head != analysis.head_sha.0),
            has_my_state: state
                .db
                .review_states()
                .exists(&session.user_id, analysis.id)
                .await?,
            analysed_head: analysis.head_sha,
        });
    }
    Ok(())
}

async fn start(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
) -> DashboardResult<(StatusCode, Json<RefreshStatus>)> {
    let status = refresh::start(&state, &scope(&session), RefreshTrigger::Manual).await?;
    Ok((StatusCode::CREATED, Json(status)))
}

async fn refresh_status(
    State(state): State<AppState>,
    session: Session,
    path: Result<Path<RefreshId>, PathRejection>,
) -> DashboardResult<Json<RefreshStatus>> {
    let Path(id) = path?;
    let record = state
        .db
        .dashboard()
        .refresh(&session.tenant_id, &session.user_id, id)
        .await?;
    Ok(Json(status_of(&record)))
}

async fn cancel(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: Result<Path<RefreshId>, PathRejection>,
) -> DashboardResult<Json<RefreshStatus>> {
    let Path(id) = path?;
    Ok(Json(refresh::cancel(&state, &scope(&session), id).await?))
}

async fn rank(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: Result<Path<RefreshId>, PathRejection>,
) -> DashboardResult<(StatusCode, Json<RefreshStatus>)> {
    let Path(id) = path?;
    let status = refresh::rank_again(&state, &scope(&session), id).await?;
    Ok((StatusCode::ACCEPTED, Json(status)))
}

async fn events(
    State(state): State<AppState>,
    session: Session,
    path: Result<Path<RefreshId>, PathRejection>,
) -> DashboardResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let Path(id) = path?;
    let stream = refresh_events(state, session, id, HEARTBEAT).await?;
    Ok(Sse::new(
        stream
            .filter_map(|event| async move { sse_event(&event) })
            .map(Ok),
    ))
}

/// What the stream sends next.
enum Next {
    Status(Box<RefreshStatus>),
    /// The subscriber fell behind: read the stored status.
    Reread,
    Heartbeat,
}

/// The refresh's current status, then each change of it and a heartbeat every `heartbeat` that re-checks the
/// session; ends when the session is gone or the server stops.
async fn refresh_events(
    state: AppState,
    session: Session,
    id: RefreshId,
    heartbeat: Duration,
) -> Result<impl Stream<Item = DashboardEvent>, DbError> {
    // Subscribe first: a change written after the status read is buffered, never lost.
    let live = state.dashboard.subscribe();
    let record = state
        .db
        .dashboard()
        .refresh(&session.tenant_id, &session.user_id, id)
        .await?;
    let first = Next::Status(Box::new(status_of(&record)));
    let changes = stream::unfold(live, move |mut live| async move {
        loop {
            match live.recv().await {
                Ok(status) if status.id == id => {
                    return Some((Next::Status(Box::new(status)), live));
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => return Some((Next::Reread, live)),
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let heartbeats = stream::unfold(ticks, |mut ticks| async move {
        ticks.tick().await;
        Some((Next::Heartbeat, ticks))
    });
    let mut stopping = state.stopping.subscribe();
    let stopped = async move {
        // An error means the sender is gone, which also ends the stream.
        let _ = stopping.wait_for(|stop| *stop).await;
    };
    Ok(stream::once(async { first })
        .chain(stream::select(changes, heartbeats))
        .then(move |next| {
            let state = state.clone();
            let session = session.clone();
            async move {
                match next {
                    Next::Status(status) => Some(DashboardEvent::Refresh { status: *status }),
                    Next::Reread => state
                        .db
                        .dashboard()
                        .refresh(&session.tenant_id, &session.user_id, id)
                        .await
                        .inspect_err(|error| {
                            tracing::debug!(%error, "dashboard SSE status re-read failed; closing stream");
                        })
                        .ok()
                        .map(|record| DashboardEvent::Refresh {
                            status: status_of(&record),
                        }),
                    Next::Heartbeat => session_live(&state, &session)
                        .await
                        .then(|| DashboardEvent::Heartbeat {
                            at: state.dashboard.now(),
                        }),
                }
            }
        })
        .take_while(|event| std::future::ready(event.is_some()))
        .filter_map(std::future::ready)
        .take_until(stopped))
}

/// Whether the stream's session or access token still passes the checks of a new request; a failed read ends the
/// stream too.
async fn session_live(state: &AppState, session: &Session) -> bool {
    match session.is_live(state).await {
        Ok(live) => {
            if !live {
                tracing::debug!(user = %session.user_id, "dashboard SSE stream closed: its session ended");
            }
            live
        }
        Err(error) => {
            tracing::warn!(%error, "dashboard SSE session check failed; closing stream");
            false
        }
    }
}

fn sse_event(event: &DashboardEvent) -> Option<Event> {
    match Event::default().event(event.name()).json_data(event) {
        Ok(sse) => Some(sse),
        Err(error) => {
            tracing::error!(%error, "could not encode a dashboard event for SSE; the event was skipped");
            None
        }
    }
}
