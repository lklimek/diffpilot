//! Settings → Notifications (ADR-023 decision 11, plan §5.5): RN1 reads the session user's channels with secrets
//! masked; RN2 saves or removes a channel and RN3 sends one test message, both with session and CSRF. Tokens are
//! refused on all of them (RN1 is `session`, not `read`). The work lives in `review_loop::notify`.

use std::borrow::Cow;

use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::header::RETRY_AFTER;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use contracts::api::error::ApiErrorCode;
use contracts::api::review_loop::{
    ChannelKind, ChannelTestResult, NotificationChannel, NotificationSettings, PutChannelRequest,
};
use contracts::routes::{
    GET_NOTIFICATION_SETTINGS, PUT_NOTIFICATION_CHANNEL, TEST_NOTIFICATION_CHANNEL,
};
use contracts::time::Timestamp;

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::{Session, SessionCsrf};
use crate::review_loop::notify::outbound::UrlError;
use crate::review_loop::notify::{self, NotifyError};
use crate::state::AppState;

/// Error code of a notification failure (RN1–RN3, RF5).
pub(crate) fn notify_code(error: &NotifyError) -> ApiErrorCode {
    match error {
        NotifyError::Url(UrlError::Scheme) => ApiErrorCode::SchemeNotAllowed,
        NotifyError::Url(_) | NotifyError::Topic(_) | NotifyError::Secret => {
            ApiErrorCode::InvalidRequest
        }
        NotifyError::SecretRequired => ApiErrorCode::SecretRequired,
        NotifyError::Limited { .. } => ApiErrorCode::RateLimited,
        NotifyError::NotFound => ApiErrorCode::NotFound,
        NotifyError::Cipher(_) => ApiErrorCode::Internal,
        NotifyError::Db(error) => error.code(),
    }
}

/// Plain message of a notification failure.
pub(crate) fn notify_message(error: &NotifyError) -> Cow<'static, str> {
    match error {
        NotifyError::Url(_) | NotifyError::Topic(_) | NotifyError::Secret => {
            Cow::Owned(format!("{error}."))
        }
        NotifyError::SecretRequired => Cow::Borrowed(
            "The kind or the URL's scheme, host or port changed: replace or remove the secret, it is not sent to a \
             new destination.",
        ),
        NotifyError::Limited { .. } => Cow::Owned(format!(
            "{} messages were sent to this channel in the last hour; try again later.",
            notify::MESSAGES_PER_HOUR
        )),
        NotifyError::NotFound => Cow::Borrowed("Set up this channel first."),
        NotifyError::Cipher(_) => {
            Cow::Borrowed("Something went wrong on the server. Try again later.")
        }
        NotifyError::Db(error) => error.public_message(),
    }
}

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

/// A failed notification request; an over-limit test carries `Retry-After`.
#[derive(Debug)]
struct Failed(NotificationsApiError);

impl IntoResponse for Failed {
    fn into_response(self) -> Response {
        let retry_at = match &self.0 {
            NotificationsApiError::Notify(NotifyError::Limited { retry_at }) => Some(*retry_at),
            _ => None,
        };
        let mut response = ApiFailure(self.0).into_response();
        if let Some(retry_at) = retry_at {
            let seconds = retry_at.duration_since(Timestamp::now()).as_secs().max(1);
            if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                response.headers_mut().insert(RETRY_AFTER, value);
            }
        }
        response
    }
}

type ApiResult<T> = Result<T, Failed>;

fn failure(error: impl Into<NotificationsApiError>) -> Failed {
    Failed(error.into())
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
