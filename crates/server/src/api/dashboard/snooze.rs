//! Snooze and Wake of a dashboard item (R12, R13; owner SN1): the signed-in browser user's own snoozes only. Session
//! and CSRF, never a personal access token. A snooze changes the summary revision and leaves the snapshot, ranking
//! and judgements alone; nothing is written to GitHub.

use std::borrow::Cow;

use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, put};
use axum::{Json, Router};
use contracts::api::dashboard::{ItemKey, MAX_SNOOZE_DAYS, SnoozeRequest};
use contracts::api::error::ApiErrorCode;
use contracts::routes::{SNOOZE_DASHBOARD_ITEM, WAKE_DASHBOARD_ITEM};
use db::error::DbError;
use jiff::SignedDuration;

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::SessionCsrf;
use crate::dashboard::{snapshot, snooze};
use crate::state::AppState;

/// A snooze request failed.
#[derive(Debug, thiserror::Error)]
pub enum SnoozeError {
    /// The item key in the path is not valid.
    #[error("the item key is not valid")]
    Path(#[from] PathRejection),
    /// Malformed JSON body.
    #[error("the request body is not valid")]
    Json(#[from] JsonRejection),
    /// The number of days is outside 1 to [`MAX_SNOOZE_DAYS`].
    #[error("the snooze length is outside the allowed days")]
    Days,
    /// The database failed, or the item is not in the user's current snapshot.
    #[error("the snooze could not be read or changed")]
    Db(#[from] DbError),
}

impl ApiErrorSource for SnoozeError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Path(rejection) => rejection.code(),
            Self::Json(rejection) => rejection.code(),
            Self::Days => ApiErrorCode::InvalidRequest,
            Self::Db(error) => error.code(),
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Path(rejection) => rejection.public_message(),
            Self::Json(rejection) => rejection.public_message(),
            Self::Days => Cow::Owned(format!("Snooze for 1 to {MAX_SNOOZE_DAYS} days.")),
            Self::Db(error) => error.public_message(),
        }
    }
}

type SnoozeResult<T> = Result<T, ApiFailure<SnoozeError>>;

fn failure(error: impl Into<SnoozeError>) -> ApiFailure<SnoozeError> {
    ApiFailure(error.into())
}

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(SNOOZE_DASHBOARD_ITEM.path, put(snooze))
        .route(WAKE_DASHBOARD_ITEM.path, delete(wake))
}

/// R12: snoozes an item of the user's current snapshot for `days` from now (a new snooze replaces the old one);
/// 404 for an item outside the snapshot.
async fn snooze(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    key: Result<Path<ItemKey>, PathRejection>,
    body: Result<Json<SnoozeRequest>, JsonRejection>,
) -> SnoozeResult<StatusCode> {
    let Path(key) = key.map_err(failure)?;
    let Json(request) = body.map_err(failure)?;
    if !(1..=MAX_SNOOZE_DAYS).contains(&request.days) {
        return Err(failure(SnoozeError::Days));
    }
    let Some((_, current)) = snapshot::load(&state, &session.tenant_id, &session.user_id)
        .await
        .map_err(failure)?
    else {
        return Err(failure(DbError::NotFound));
    };
    if !snooze::holds(&current, &key) {
        return Err(failure(DbError::NotFound));
    }
    let now = state.dashboard.now();
    let until = now
        .checked_add(SignedDuration::from_hours(24 * i64::from(request.days)))
        .ok_or_else(|| failure(SnoozeError::Days))?;
    state
        .db
        .dashboard()
        .snooze(&session.tenant_id, &session.user_id, &key, now, until)
        .await
        .map_err(failure)?;
    tracing::info!(user_id = %session.user_id, item = %key.as_str(), days = request.days, "dashboard item snoozed");
    Ok(StatusCode::NO_CONTENT)
}

/// R13: ends the item's snooze now; 204 also when it was not snoozed.
async fn wake(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    key: Result<Path<ItemKey>, PathRejection>,
) -> SnoozeResult<StatusCode> {
    let Path(key) = key.map_err(failure)?;
    let woken = state
        .db
        .dashboard()
        .wake(
            &session.tenant_id,
            &session.user_id,
            std::slice::from_ref(&key),
        )
        .await
        .map_err(failure)?;
    if woken > 0 {
        tracing::info!(user_id = %session.user_id, item = %key.as_str(), "dashboard item woken by the user");
    }
    Ok(StatusCode::NO_CONTENT)
}
