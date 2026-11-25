//! Dashboard summary, items, refreshes and their events (R1–R5, `docs/contracts/http-api.md` "Dashboard"). Every
//! request reads the signed-in user's own data; another user's refresh id answers `not_found` (T16). Reads take
//! `Session` (a session cookie or a `read` personal access token); writes take the session and CSRF.

use std::borrow::Cow;
use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_TYPE, ETAG, IF_NONE_MATCH, RETRY_AFTER};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use contracts::api::dashboard::{
    CommentTally, DashboardEvent, DashboardSummary, ItemKey, ItemKind, ItemPage, ItemQuery,
    RefreshStatus, RefreshTrigger, ReviewLink, ScheduleStatus, ScheduledRun, StartRefreshRequest,
    StatusCounts, SummaryQuery, WorkItem,
};
use contracts::api::error::ApiErrorCode;
use contracts::common::PrRef;
use contracts::ids::RefreshId;
use contracts::routes::{
    CANCEL_DASHBOARD_REFRESH, DASHBOARD_REFRESH_EVENTS, GET_DASHBOARD, GET_DASHBOARD_REFRESH,
    HIDE_DASHBOARD_MENTION, LIST_DASHBOARD_ITEMS, RANK_DASHBOARD_REFRESH, REJUDGE_DASHBOARD_ITEM,
    START_DASHBOARD_REFRESH,
};
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::dashboard::StoredDashboardSettings;
use futures::stream::{self, Stream, StreamExt};
use tokio::sync::broadcast;
use tokio::time::MissedTickBehavior;

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::api::events::{StreamSlot, holding};
use crate::auth::{Session, SessionCsrf};
use crate::dashboard::refresh::rank::ItemJudgement;
use crate::dashboard::refresh::{self, ControlError, Scope, StartError};
use crate::dashboard::schedule::{Schedule, counted_run_date};
use crate::dashboard::snapshot::{self, CursorError, Snapshot, View};
use crate::dashboard::{installations, judge};
use crate::review_loop::{PrHandle, views};
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
    /// The optional refresh body could not be read.
    #[error("the refresh body is not valid")]
    Json(#[from] JsonRejection),
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
        retry_at: Timestamp,
    },
    /// Cancel of an ended refresh, or Retry ranking of one that cannot be retried.
    #[error("the refresh does not allow this now")]
    NotAllowed,
    /// The item cannot be re-judged in its current state.
    #[error("the item's judgement is not outdated")]
    NotOutdated,
    /// The user's daily ranking cap has been reached.
    #[error("today's ranking limit has been reached")]
    RankingCap,
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
            Self::Path(_)
            | Self::Query(_)
            | Self::Json(_)
            | Self::Cursor(CursorError::Malformed) => ApiErrorCode::InvalidRequest,
            Self::Db(error) => error.code(),
            Self::Running => ApiErrorCode::RefreshRunning,
            Self::RateLimited { .. } => ApiErrorCode::GithubRateLimited,
            Self::NotAllowed | Self::NotOutdated | Self::RankingCap => ApiErrorCode::Conflict,
            Self::Cursor(CursorError::Expired) => ApiErrorCode::CursorExpired,
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Path(rejection) => rejection.public_message(),
            Self::Query(rejection) => rejection.public_message(),
            Self::Json(rejection) => rejection.public_message(),
            Self::Cursor(CursorError::Malformed) => Cow::Borrowed("The cursor is not valid."),
            Self::Db(error) => error.public_message(),
            Self::Running => Cow::Borrowed("A refresh is already running. Wait for it to finish."),
            Self::RateLimited { retry_at } => Cow::Owned(format!(
                "GitHub's rate limit is used up until {} UTC. Refresh after that.",
                retry_at.get().strftime("%H:%M")
            )),
            Self::NotAllowed => Cow::Borrowed(
                "The refresh does not allow this in its current state. Reload and try again.",
            ),
            Self::NotOutdated => Cow::Borrowed("The item's judgement is not outdated."),
            Self::RankingCap => Cow::Borrowed("Today's ranking limit has been reached."),
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
            let wait = (retry_at - Timestamp::now()).as_secs().max(1);
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
        .route(HIDE_DASHBOARD_MENTION.path, post(hide_mention))
        .route(LIST_DASHBOARD_ITEMS.path, get(items))
        .route(REJUDGE_DASHBOARD_ITEM.path, post(rejudge))
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

/// Counts a dashboard read in a browser session as the user's presence for the plug-in poller (ADR-022 §3); a
/// personal access token read is an automated client, not a user in the session.
pub(crate) fn mark_present(state: &AppState, session: &Session) {
    if !session.is_access_token() {
        state
            .plugins
            .mark_active(&session.tenant_id, &session.user_id);
    }
}

/// The current snapshot scored with the judgements present now, and how many of those judgements an older ranking
/// prompt made; `None` without a readable snapshot.
async fn current_view(
    state: &AppState,
    scope: &Scope,
) -> Result<
    Option<(
        String,
        Snapshot,
        View,
        db::repo::dashboard::RefreshRecord,
        u32,
    )>,
    DbError,
> {
    let Some((record, snapshot)) = snapshot::load(state, &scope.tenant_id, &scope.user_id).await?
    else {
        return Ok(None);
    };
    let judgements: HashMap<ItemKey, ItemJudgement> = state
        .dashboard
        .ranker()
        .judgements(state, scope, &snapshot)
        .await?;
    let older = judgements.values().filter(|j| j.older_prompt).count();
    let mut view = snapshot::build(
        &snapshot,
        &judgements,
        state.config.memcan_ui_url.as_deref(),
        &state.plugins.overlay(state.dashboard.now()),
        Some(&record),
    );
    let live = installations::live(state, &scope.tenant_id).await?;
    view.apply_installations(&live);
    Ok(Some((
        snapshot::revision(&record, &snapshot, &view),
        snapshot,
        view,
        record,
        u32::try_from(older).unwrap_or(u32::MAX),
    )))
}

async fn summary(
    State(state): State<AppState>,
    session: Session,
    query: Result<Query<SummaryQuery>, QueryRejection>,
) -> DashboardResult<Json<DashboardSummary>> {
    let Query(query) = query?;
    let list_excluded = query.excluded.unwrap_or(false);
    let scope = scope(&session);
    mark_present(&state, &session);
    let dashboard = state.db.dashboard();
    let latest_refresh = match dashboard
        .latest_refresh(&scope.tenant_id, &scope.user_id)
        .await?
    {
        Some(record) => Some(judge::status(&state, &record).await),
        None => None,
    };
    let (
        revision,
        meta,
        older_prompt_judgements,
        scanned_at,
        mentions,
        mention_coverage,
        (live_repositories, not_live_repositories),
        last_event_at,
        status_counts,
    ) = match current_view(&state, &scope).await? {
        Some((revision, snapshot, view, record, older)) => (
            revision,
            Some(view.meta(&snapshot, &record, list_excluded)),
            older,
            record.scanned_at,
            snapshot.mentions,
            snapshot.mention_coverage,
            view.repository_counts,
            snapshot.last_event_at,
            view.status_counts(),
        ),
        None => (
            snapshot::NO_SNAPSHOT.to_owned(),
            None,
            0,
            None,
            Vec::new(),
            Default::default(),
            (0, 0),
            None,
            StatusCounts {
                outdated: 0,
                not_judged: 0,
                not_live: 0,
                excluded: 0,
            },
        ),
    };
    let stored = dashboard.settings(&scope.tenant_id, &scope.user_id).await?;
    Ok(Json(DashboardSummary {
        status_counts: Some(status_counts),
        live_repositories: Some(live_repositories),
        not_live_repositories: Some(not_live_repositories),
        last_event_at,
        mentions,
        mention_coverage,
        revision,
        latest_refresh,
        snapshot: meta,
        schedule: schedule_status(stored.as_ref(), state.dashboard.now(), scanned_at),
        older_prompt_judgements,
        plugins: state.plugins.statuses(state.dashboard.now()),
    }))
}

/// The schedule as stored (ADR-020): the last run's instant is the configured local time of its date, the next
/// run follows the due rule given the current snapshot's scan time (`last_success`).
fn schedule_status(
    stored: Option<&StoredDashboardSettings>,
    now: Timestamp,
    last_success: Option<Timestamp>,
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
        .map(|schedule| schedule.next_run_at(now, counted_run_date(last_scheduled), last_success));
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
    mark_present(&state, &session);
    let (revision, page, next_cursor, mentions, mention_coverage) =
        match current_view(&state, &scope).await? {
            Some((revision, snapshot, view, _, _)) => {
                let (page, next) = snapshot::page(&view.items, &query, &revision)?;
                (
                    revision,
                    page,
                    next,
                    if query.cursor.is_none() {
                        snapshot.mentions
                    } else {
                        Vec::new()
                    },
                    snapshot.mention_coverage,
                )
            }
            None => {
                let revision = snapshot::NO_SNAPSHOT.to_owned();
                let (page, next) = snapshot::page(&[], &query, &revision)?;
                (revision, page, next, Vec::new(), Default::default())
            }
        };
    let mut items: Vec<WorkItem> = Vec::with_capacity(page.len());
    let mut heads = Vec::with_capacity(page.len());
    for item in page {
        heads.push(item.head_sha);
        items.push(item.item);
    }
    review_links(&state, &session, &mut items, &heads).await?;
    comment_tallies(&state, &session, &mut items).await?;
    let body = ItemPage {
        mentions,
        mention_coverage,
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

/// Fills `review` of the pull requests on the page from the tenant's analyses, read now in one statement (plan
/// §F.1): the analysis a reader is shown, whether the PR head moved since (snapshot head vs analysed head) and
/// whether the reader saved review state. Part of the page's `ETag`, so a new analysis is never hidden by a 304.
async fn review_links(
    state: &AppState,
    session: &Session,
    items: &mut [WorkItem],
    heads: &[Option<String>],
) -> Result<(), DbError> {
    let rows: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.kind == ItemKind::Pr && item.repo.is_some() && item.number.is_some()
        })
        .map(|(row, _)| row)
        .collect();
    if rows.is_empty() {
        return Ok(());
    }
    let wanted: Vec<_> = rows
        .iter()
        .filter_map(|row| Some((items[*row].repo.clone()?, items[*row].number?)))
        .collect();
    let language = state.db.users().get(&session.user_id).await?.language;
    let links = state
        .db
        .pull_requests()
        .review_links(&session.tenant_id, &session.user_id, language, &wanted)
        .await?;
    for link in links {
        let Some(&row) = rows.get(link.index) else {
            continue;
        };
        let head = heads.get(row).and_then(Option::as_deref);
        items[row].review = Some(ReviewLink {
            analysis_id: link.analysis_id,
            head_moved: head.is_some_and(|head| head != link.head_sha.0),
            has_my_state: link.has_review_state,
            analysed_head: link.head_sha,
        });
    }
    Ok(())
}

/// Fills `comments` and `watched` of the pull requests on the page from the tenant's review-loop tables and the
/// user's watches. The queue inputs for read PRs are loaded together.
/// Part of the page's `ETag` like `review`.
async fn comment_tallies(
    state: &AppState,
    session: &Session,
    items: &mut [WorkItem],
) -> Result<(), DbError> {
    let (rows, wanted): (Vec<usize>, Vec<_>) = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.kind == ItemKind::Pr)
        .filter_map(|(row, item)| Some((row, (item.repo.clone()?, item.number?))))
        .unzip();
    if rows.is_empty() {
        return Ok(());
    }
    let tallies = state
        .db
        .review_loop()
        .comment_tallies(&session.tenant_id, &session.user_id, &wanted)
        .await?;
    let read: Vec<_> = tallies
        .iter()
        .filter_map(|row| row.total.map(|_| row.pull_request_id))
        .collect();
    let mut batch = match state
        .db
        .review_loop()
        .comment_tally_views(&session.tenant_id, &read)
        .await
    {
        Ok(batch) => Some(batch),
        Err(error) => {
            if state.dashboard.tally_warning_due(None) {
                tracing::warn!(?error, "dashboard comment tally batch could not be read");
            }
            None
        }
    };
    let mut needs_you = HashMap::new();
    for tally in tallies {
        let Some((&row, (repo, number))) = rows.get(tally.index).zip(wanted.get(tally.index))
        else {
            continue;
        };
        items[row].watched = tally.watched;
        items[row].comments = if let Some(total) = tally.total {
            needs_you
                .entry(tally.pull_request_id)
                .or_insert_with(|| {
                    let batch = batch.as_mut()?;
                    let handle = PrHandle {
                        pr: PrRef {
                            repo: repo.clone(),
                            number: *number,
                        },
                        id: tally.pull_request_id,
                    };
                    match batch
                        .remove(&handle.id)
                        .unwrap_or(Err(DbError::NotFound))
                        .and_then(|view| views::dashboard_needs_you(handle, view))
                    {
                        Ok(needs_you) => Some(needs_you),
                        Err(error) => {
                            if state
                                .dashboard
                                .tally_warning_due(Some(tally.pull_request_id))
                            {
                                tracing::warn!(
                                    ?error,
                                    pr = tally.pull_request_id,
                                    "dashboard comment tally omitted"
                                );
                            }
                            None
                        }
                    }
                })
                .map(|needs_you| CommentTally { total, needs_you })
        } else {
            None
        };
    }
    Ok(())
}

async fn start(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    body: Result<Option<Json<StartRefreshRequest>>, JsonRejection>,
) -> DashboardResult<(StatusCode, Json<RefreshStatus>)> {
    let rejudge_outdated = body?.is_some_and(|Json(request)| request.rejudge_outdated);
    let status = refresh::start(
        &state,
        &scope(&session),
        RefreshTrigger::Manual,
        rejudge_outdated,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(status)))
}

async fn rejudge(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: Result<Path<ItemKey>, PathRejection>,
) -> DashboardResult<StatusCode> {
    let Path(key) = path?;
    refresh::rejudge(&state, &scope(&session), &key)
        .await
        .map_err(|error| match error {
            judge::RejudgeError::NotOutdated => DashboardError::NotOutdated,
            judge::RejudgeError::DailyCap => DashboardError::RankingCap,
            judge::RejudgeError::Db(error) => DashboardError::Db(error),
        })?;
    Ok(StatusCode::ACCEPTED)
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
    Ok(Json(judge::status(&state, &record).await))
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
) -> Response {
    let slot = match StreamSlot::open(&session) {
        Ok(slot) => slot,
        Err(full) => return full.into_response(),
    };
    let opened: DashboardResult<_> = async {
        let Path(id) = path?;
        Ok(refresh_events(state, session, id, HEARTBEAT).await?)
    }
    .await;
    match opened {
        Ok(stream) => Sse::new(
            holding(slot, stream)
                .filter_map(|event| async move { sse_event(&event) })
                .map(Ok::<_, Infallible>),
        )
        .into_response(),
        Err(error) => error.into_response(),
    }
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
    let first = Next::Status(Box::new(judge::status(&state, &record).await));
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
                    Next::Reread => match state
                        .db
                        .dashboard()
                        .refresh(&session.tenant_id, &session.user_id, id)
                        .await
                    {
                        Ok(record) => Some(DashboardEvent::Refresh {
                            status: judge::status(&state, &record).await,
                        }),
                        Err(error) => {
                            tracing::debug!(%error, "dashboard SSE status re-read failed; closing stream");
                            None
                        }
                    },
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

/// Session + CSRF only; the identity must belong to this user's current snapshot.
async fn hide_mention(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    Path(id): Path<String>,
) -> DashboardResult<StatusCode> {
    hide_mention_inner(&state, &session, &id)
        .await
        .inspect_err(|error| {
            tracing::warn!(%error, user_id = %session.user_id, "dashboard mention hide failed");
        })
}

async fn hide_mention_inner(
    state: &AppState,
    session: &Session,
    id: &str,
) -> DashboardResult<StatusCode> {
    let Some((_, snapshot)) = snapshot::load(state, &session.tenant_id, &session.user_id).await?
    else {
        return Err(DbError::NotFound.into());
    };
    if !snapshot.mentions.iter().any(|m| m.id == id) {
        if state
            .db
            .dashboard()
            .hidden_mentions(&session.tenant_id, &session.user_id)
            .await?
            .iter()
            .any(|hidden| hidden == id)
        {
            return Ok(StatusCode::NO_CONTENT);
        }
        return Err(DbError::NotFound.into());
    }
    state
        .db
        .dashboard()
        .hide_mention(
            &session.tenant_id,
            &session.user_id,
            id,
            state.dashboard.now(),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
