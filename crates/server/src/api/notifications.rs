//! Settings → Notifications (ADR-023 decision 11, plan §5.5): RN1 reads the session user's channels with secrets
//! masked; RN2 saves or removes a channel and RN3 sends one test message, both with session and CSRF. Tokens are
//! refused on all of them (RN1 is `session`, not `read`). The work lives in `review_loop::notify`.

use std::borrow::Cow;

use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use contracts::api::error::ApiErrorCode;
use contracts::api::review_loop::{
    ChannelKind, ChannelTestResult, NotificationChannel, NotificationSettings, PutChannelRequest,
};
use contracts::routes::{
    GET_NOTIFICATION_SETTINGS, PUT_NOTIFICATION_CHANNEL, TEST_NOTIFICATION_CHANNEL,
};

use crate::api::decision_feed::{notify_code, notify_message};
use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::{Session, SessionCsrf};
use crate::review_loop::notify::{self, NotifyError};
use crate::state::AppState;

/// A notification-settings request failed.
#[derive(Debug, thiserror::Error)]
pub enum NotificationsApiError {
    /// Malformed path (`{kind}` is not `ntfy` or `webhook`).
    #[error("the path is not valid")]
    Path(#[from] PathRejection),
    /// Malformed body.
    #[error("the request body is not valid")]
    Json(#[from] JsonRejection),
    /// The work failed.
    #[error(transparent)]
    Notify(#[from] NotifyError),
}

impl ApiErrorSource for NotificationsApiError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Path(_) | Self::Json(_) => ApiErrorCode::InvalidRequest,
            Self::Notify(error) => notify_code(error),
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Path(rejection) => rejection.public_message(),
            Self::Json(rejection) => rejection.public_message(),
            Self::Notify(error) => notify_message(error),
        }
    }
}

type ApiResult<T> = Result<T, ApiFailure<NotificationsApiError>>;

fn failure(error: impl Into<NotificationsApiError>) -> ApiFailure<NotificationsApiError> {
    ApiFailure(error.into())
}

type KindPath = Result<Path<ChannelKind>, PathRejection>;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(GET_NOTIFICATION_SETTINGS.path, get(settings))
        .route(
            PUT_NOTIFICATION_CHANNEL.path,
            put(put_channel).delete(delete_channel),
        )
        .route(TEST_NOTIFICATION_CHANNEL.path, post(test_channel))
}

async fn settings(
    State(state): State<AppState>,
    session: Session,
) -> ApiResult<Json<NotificationSettings>> {
    Ok(Json(
        notify::settings(&state, &session).await.map_err(failure)?,
    ))
}

async fn put_channel(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: KindPath,
    body: Result<Json<PutChannelRequest>, JsonRejection>,
) -> ApiResult<Json<NotificationChannel>> {
    let Path(kind) = path.map_err(failure)?;
    let Json(request) = body.map_err(failure)?;
    Ok(Json(
        notify::put_channel(&state, &session, kind, &request)
            .await
            .map_err(failure)?,
    ))
}

async fn delete_channel(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: KindPath,
) -> ApiResult<StatusCode> {
    let Path(kind) = path.map_err(failure)?;
    notify::delete_channel(&state, &session, kind)
        .await
        .map_err(failure)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn test_channel(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: KindPath,
) -> ApiResult<Json<ChannelTestResult>> {
    let Path(kind) = path.map_err(failure)?;
    Ok(Json(
        notify::test_channel(&state, &session, kind)
            .await
            .map_err(failure)?,
    ))
}
