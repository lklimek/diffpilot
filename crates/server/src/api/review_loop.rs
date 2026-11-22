//! Review-loop routes (ADR-023, `docs/contracts/http-api.md` "Review loop"): the PR page, Comments and its pages,
//! decisions and the owner's item actions. Reads take `Session` (a session or a `read` access token, which is served
//! what is stored: it never reads GitHub, asks for an assessment pass or marks plug-in presence, so a token read
//! leaves the database unchanged); every write takes the session and CSRF (T22). The work lives in
//! `review_loop`.

use std::borrow::Cow;

use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::header::RETRY_AFTER;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use contracts::api::error::ApiErrorCode;
use contracts::api::review_loop::{
    CarryOverPage, CarryOverQuery, CodeContext, CommentItem, CommentLog, CommentsOverview,
    ContextQuery, DecideRequest, DecideRest, Decision, DecisionBatchRequest, DecisionBatchRow,
    DecisionVia, EffectsPreview, EffectsPreviewRequest, GateMarkRequest, ItemActionRequest,
    ItemsQuery, PrPage, ReadFailure,
};
use contracts::common::{PrRef, RepoRef};
use contracts::routes::{
    ASSESS_COMMENT, DECIDE_COMMENT, DECIDE_COMMENT_BATCH, GET_COMMENT_CONTEXT, GET_COMMENT_LOG,
    GET_COMMENTS, GET_DECIDE_REST, GET_PR_PAGE, LIST_CARRY_OVERS, LIST_COMMENT_ITEMS,
    MARK_COMMENT_GATE, PREVIEW_COMMENT_EFFECTS, READ_COMMENTS, REOPEN_COMMENT_ITEM,
    SKIP_COMMENT_ITEM, UNDO_COMMENT_DECISION,
};

use crate::api::dashboard::refresh::mark_present;
use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::{Session, SessionCsrf};
use crate::github::mirror::MirrorError;
use crate::review_loop::assess::{self, PassReason};
use crate::review_loop::{self, ItemAction, LoopError, MIN_REREAD, PrHandle, views};
use crate::state::AppState;

/// A review-loop request failed.
#[derive(Debug, thiserror::Error)]
pub enum ReviewLoopApiError {
    /// Malformed path.
    #[error("the path is not valid")]
    Path(#[from] PathRejection),
    /// Malformed query.
    #[error("the query is not valid")]
    Query(#[from] QueryRejection),
    /// Malformed body.
    #[error("the request body is not valid")]
    Json(#[from] JsonRejection),
    /// The work failed.
    #[error(transparent)]
    Loop(#[from] LoopError),
}

fn item_list(nos: &[u32]) -> String {
    nos.iter()
        .map(|no| format!("C{no}"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl ApiErrorSource for ReviewLoopApiError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Path(_) | Self::Query(_) | Self::Json(_) => ApiErrorCode::InvalidRequest,
            Self::Loop(error) => match error {
                LoopError::NotFound => ApiErrorCode::NotFound,
                LoopError::PrivateRepository => ApiErrorCode::PrivateRepository,
                LoopError::Token(error) => error.code(),
                LoopError::Unreadable(ReadFailure::RateLimited) => ApiErrorCode::GithubRateLimited,
                LoopError::Unreadable(ReadFailure::SignInRequired) => ApiErrorCode::Unauthenticated,
                LoopError::Unreadable(ReadFailure::Unavailable) => ApiErrorCode::GithubUnavailable,
                LoopError::TooSoon => ApiErrorCode::RateLimited,
                LoopError::ItemChanged(_) => ApiErrorCode::ItemChanged,
                LoopError::Decision { .. }
                | LoopError::GateRow(_)
                | LoopError::InvalidCursor
                | LoopError::BadRange => ApiErrorCode::InvalidRequest,
                LoopError::NotApplicable => ApiErrorCode::Conflict,
                LoopError::RangeTooLarge => ApiErrorCode::RangeTooLarge,
                LoopError::Mirror(MirrorError::BlobTooLarge { .. }) => ApiErrorCode::NotRendered,
                LoopError::Mirror(MirrorError::InvalidRepo) => ApiErrorCode::NotFound,
                LoopError::Mirror(_) => ApiErrorCode::GithubUnavailable,
                LoopError::Db(error) => error.code(),
            },
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Path(rejection) => rejection.public_message(),
            Self::Query(rejection) => rejection.public_message(),
            Self::Json(rejection) => rejection.public_message(),
            Self::Loop(error) => match error {
                LoopError::NotFound => Cow::Borrowed("The pull request or comment was not found."),
                LoopError::PrivateRepository => Cow::Borrowed(
                    "The repository is private; diffpilot supports public repositories.",
                ),
                LoopError::Token(error) => error.public_message(),
                LoopError::Unreadable(ReadFailure::RateLimited) => Cow::Borrowed(
                    "GitHub's rate limit is used up; the pull request could not be read. Try again later.",
                ),
                LoopError::Unreadable(ReadFailure::SignInRequired) => {
                    Cow::Borrowed("Your GitHub sign-in has ended. Sign in again.")
                }
                LoopError::Unreadable(ReadFailure::Unavailable) => {
                    Cow::Borrowed("GitHub could not be read. Try again later.")
                }
                LoopError::TooSoon => Cow::Borrowed(
                    "The pull request was read from GitHub moments ago. Try again in a few seconds.",
                ),
                LoopError::ItemChanged(nos) => Cow::Owned(format!(
                    "These comments changed since you looked: {}. Read them again.",
                    item_list(nos)
                )),
                LoopError::Decision { no, error } => Cow::Owned(format!("C{no}: {error}.")),
                LoopError::GateRow(nos) => Cow::Owned(format!(
                    "Gate items need their own decision: {}.",
                    item_list(nos)
                )),
                LoopError::NotApplicable => Cow::Borrowed(
                    "This does not apply to the comment now: it has no standing decision, or an agent claimed \
                     its effects.",
                ),
                LoopError::RangeTooLarge => Cow::Borrowed("The requested line range is too large."),
                LoopError::BadRange => Cow::Borrowed("The line range is not valid."),
                LoopError::InvalidCursor => {
                    Cow::Borrowed("The page cursor is not valid; start from the first page.")
                }
                LoopError::Mirror(MirrorError::BlobTooLarge { .. }) => {
                    Cow::Borrowed("The file is too large to show.")
                }
                LoopError::Mirror(MirrorError::InvalidRepo) => {
                    Cow::Borrowed("The pull request or comment was not found.")
                }
                LoopError::Mirror(_) => {
                    Cow::Borrowed("The file could not be read from GitHub. Try again later.")
                }
                LoopError::Db(error) => error.public_message(),
            },
        }
    }
}

impl IntoResponse for ReviewLoopApiError {
    fn into_response(self) -> Response {
        let too_soon = matches!(self, Self::Loop(LoopError::TooSoon));
        let mut response = ApiFailure(self).into_response();
        if too_soon && let Ok(value) = HeaderValue::from_str(&MIN_REREAD.as_secs().to_string()) {
            response.headers_mut().insert(RETRY_AFTER, value);
        }
        response
    }
}

type ApiResult<T> = Result<T, ReviewLoopApiError>;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(GET_PR_PAGE.path, get(pr_page))
        .route(GET_COMMENTS.path, get(overview))
        .route(READ_COMMENTS.path, post(read_again))
        .route(LIST_COMMENT_ITEMS.path, get(items))
        .route(GET_COMMENT_CONTEXT.path, get(context))
        .route(ASSESS_COMMENT.path, post(assess_again))
        .route(DECIDE_COMMENT.path, post(decide))
        .route(DECIDE_COMMENT_BATCH.path, post(decide_batch))
        .route(UNDO_COMMENT_DECISION.path, post(undo))
        .route(REOPEN_COMMENT_ITEM.path, post(reopen))
        .route(SKIP_COMMENT_ITEM.path, post(skip))
        .route(MARK_COMMENT_GATE.path, post(mark_gate))
        .route(PREVIEW_COMMENT_EFFECTS.path, post(preview))
        .route(GET_DECIDE_REST.path, get(decide_rest))
        .route(GET_COMMENT_LOG.path, get(log))
        .route(LIST_CARRY_OVERS.path, get(carry_overs))
}

type PrPath = Result<Path<(String, String, u32)>, PathRejection>;
type ItemPath = Result<Path<(String, String, u32, String)>, PathRejection>;

fn pr_ref(owner: String, name: String, number: u32) -> PrRef {
    PrRef {
        repo: RepoRef { owner, name },
        number,
    }
}

/// `C<no>` (or a bare number) of the path.
fn item_no(raw: &str) -> Result<u32, LoopError> {
    let digits = raw.strip_prefix(['C', 'c']).unwrap_or(raw);
    digits
        .parse()
        .ok()
        .filter(|no| *no > 0)
        .ok_or(LoopError::NotFound)
}

async fn handle_of(state: &AppState, session: &Session, path: PrPath) -> ApiResult<PrHandle> {
    let Path((owner, name, number)) = path?;
    Ok(review_loop::ensure_read(state, session, &pr_ref(owner, name, number), false).await?)
}

async fn item_of(
    state: &AppState,
    session: &Session,
    path: ItemPath,
) -> ApiResult<(PrHandle, u32)> {
    let Path((owner, name, number, item)) = path?;
    let no = item_no(&item)?;
    let handle = review_loop::find(state, &session.tenant_id, &pr_ref(owner, name, number)).await?;
    Ok((handle, no))
}

async fn pr_page(
    State(state): State<AppState>,
    session: Session,
    path: PrPath,
) -> ApiResult<Json<PrPage>> {
    mark_present(&state, &session);
    let handle = handle_of(&state, &session, path).await?;
    Ok(Json(views::pr_page(&state, &session, handle).await?))
}

async fn overview(
    State(state): State<AppState>,
    session: Session,
    path: PrPath,
) -> ApiResult<Json<CommentsOverview>> {
    mark_present(&state, &session);
    let handle = handle_of(&state, &session, path).await?;
    if !session.is_access_token()
        && let Err(error) = assess::request_pass(&state, handle.id, PassReason::Opened).await
    {
        tracing::warn!(
            ?error,
            pr = handle.id,
            "assessment pass on opening Comments could not be requested"
        );
    }
    Ok(Json(views::overview(&state, &session, handle).await?))
}

async fn read_again(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: PrPath,
) -> ApiResult<StatusCode> {
    let Path((owner, name, number)) = path?;
    review_loop::ensure_read(&state, &session, &pr_ref(owner, name, number), true).await?;
    Ok(StatusCode::ACCEPTED)
}

async fn items(
    State(state): State<AppState>,
    session: Session,
    path: PrPath,
    query: Result<Query<ItemsQuery>, QueryRejection>,
) -> ApiResult<Json<Vec<CommentItem>>> {
    let Query(query) = query?;
    mark_present(&state, &session);
    let handle = handle_of(&state, &session, path).await?;
    Ok(Json(views::items(&state, handle, &query).await?))
}

async fn context(
    State(state): State<AppState>,
    session: Session,
    path: ItemPath,
    query: Result<Query<ContextQuery>, QueryRejection>,
) -> ApiResult<Json<CodeContext>> {
    let Query(query) = query?;
    let (handle, no) = item_of(&state, &session, path).await?;
    Ok(Json(
        views::context(&state, &session, handle, no, &query).await?,
    ))
}

async fn assess_again(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: ItemPath,
) -> ApiResult<StatusCode> {
    let (handle, no) = item_of(&state, &session, path).await?;
    state
        .db
        .review_loop()
        .item_by_no(handle.id, no)
        .await
        .map_err(LoopError::from)?
        .ok_or(LoopError::NotFound)?;
    // TODO(WP13-JOB): pass the item ("assess again") once the assessment hook takes one.
    if let Err(error) = assess::request_pass(&state, handle.id, PassReason::Opened).await {
        tracing::warn!(
            ?error,
            pr = handle.id,
            item = no,
            "assess again could not be requested"
        );
    }
    Ok(StatusCode::ACCEPTED)
}

async fn decide(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: ItemPath,
    body: Result<Json<DecideRequest>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<Decision>)> {
    let Json(request) = body?;
    let (handle, no) = item_of(&state, &session, path).await?;
    let row = DecisionBatchRow {
        item_no: no,
        decision: request,
    };
    let mut stored = review_loop::decide(
        &state,
        &session,
        handle,
        std::slice::from_ref(&row),
        DecisionVia::Question,
    )
    .await?;
    let decision = stored.pop().ok_or(LoopError::NotFound)?;
    Ok((StatusCode::CREATED, Json(decision)))
}

async fn decide_batch(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: PrPath,
    body: Result<Json<DecisionBatchRequest>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<Vec<Decision>>)> {
    let Json(request) = body?;
    let Path((owner, name, number)) = path?;
    let handle =
        review_loop::find(&state, &session.tenant_id, &pr_ref(owner, name, number)).await?;
    let stored = review_loop::decide(
        &state,
        &session,
        handle,
        &request.rows,
        review_loop::batch_via(request.via),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(stored)))
}

async fn act(
    state: AppState,
    session: Session,
    path: ItemPath,
    body: Result<Json<ItemActionRequest>, JsonRejection>,
    action: ItemAction,
) -> ApiResult<Json<CommentItem>> {
    let Json(request) = body?;
    let (handle, no) = item_of(&state, &session, path).await?;
    review_loop::act(&state, &session, &handle, no, request.item_revision, action).await?;
    item_json(&state, handle, no).await
}

async fn item_json(state: &AppState, handle: PrHandle, no: u32) -> ApiResult<Json<CommentItem>> {
    let mut found = views::items(state, handle, &ItemsQuery::default()).await?;
    found.retain(|item| item.no == no);
    Ok(Json(found.pop().ok_or(LoopError::NotFound)?))
}

async fn undo(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: ItemPath,
    body: Result<Json<ItemActionRequest>, JsonRejection>,
) -> ApiResult<Json<CommentItem>> {
    act(state, session, path, body, ItemAction::Undo).await
}

async fn reopen(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: ItemPath,
    body: Result<Json<ItemActionRequest>, JsonRejection>,
) -> ApiResult<Json<CommentItem>> {
    act(state, session, path, body, ItemAction::Reopen).await
}

async fn skip(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: ItemPath,
    body: Result<Json<ItemActionRequest>, JsonRejection>,
) -> ApiResult<StatusCode> {
    let Json(request) = body?;
    let (handle, no) = item_of(&state, &session, path).await?;
    review_loop::skip(&state, &session, &handle, no, request.item_revision).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn mark_gate(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: ItemPath,
    body: Result<Json<GateMarkRequest>, JsonRejection>,
) -> ApiResult<Json<CommentItem>> {
    let Json(request) = body?;
    let (handle, no) = item_of(&state, &session, path).await?;
    review_loop::mark_gate(
        &state,
        &handle,
        no,
        request.gate,
        request.marked,
        request.item_revision,
    )
    .await?;
    item_json(&state, handle, no).await
}

async fn preview(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: PrPath,
    body: Result<Json<EffectsPreviewRequest>, JsonRejection>,
) -> ApiResult<Json<Vec<EffectsPreview>>> {
    let Json(request) = body?;
    let Path((owner, name, number)) = path?;
    let handle =
        review_loop::find(&state, &session.tenant_id, &pr_ref(owner, name, number)).await?;
    Ok(Json(views::preview(&state, handle, &request.rows).await?))
}

async fn decide_rest(
    State(state): State<AppState>,
    session: Session,
    path: PrPath,
) -> ApiResult<Json<DecideRest>> {
    let Path((owner, name, number)) = path?;
    let handle =
        review_loop::find(&state, &session.tenant_id, &pr_ref(owner, name, number)).await?;
    Ok(Json(views::decide_rest(&state, handle).await?))
}

async fn log(
    State(state): State<AppState>,
    session: Session,
    path: PrPath,
) -> ApiResult<Json<CommentLog>> {
    let Path((owner, name, number)) = path?;
    let handle =
        review_loop::find(&state, &session.tenant_id, &pr_ref(owner, name, number)).await?;
    Ok(Json(views::log(&state, &session, handle).await?))
}

async fn carry_overs(
    State(state): State<AppState>,
    session: Session,
    query: Result<Query<CarryOverQuery>, QueryRejection>,
) -> ApiResult<Json<CarryOverPage>> {
    let Query(query) = query?;
    let pr = match query.pr.as_deref() {
        Some(raw) => {
            let mut parts = raw.split('/');
            let (Some(owner), Some(name), Some(number), None) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                return Err(LoopError::NotFound.into());
            };
            let number = number.parse().map_err(|_| LoopError::NotFound)?;
            Some(
                review_loop::find(
                    &state,
                    &session.tenant_id,
                    &pr_ref(owner.to_owned(), name.to_owned(), number),
                )
                .await?,
            )
        }
        None => None,
    };
    Ok(Json(
        views::carry_overs(&state, &session, pr, query.cursor.as_deref()).await?,
    ))
}

#[cfg(test)]
mod tests;
