//! Signed-in payer's plan linking and gateway-to-database status synchronization.

use super::{error::ApiFailure, settings::SettingsApiError};
use crate::{
    auth::{Session, SessionCsrf},
    state::AppState,
};
use axum::{
    Json, Router,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
    routing::{get, post},
};
use contracts::{
    gateway::{CredentialStatus, DeviceLink, LinkCredential},
    ids::UserId,
    routes::{GET_CODEX, GET_CODEX_LINK},
};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Weak},
};
use tokio::sync::Mutex;

type Locks = HashMap<UserId, Weak<Mutex<()>>>;
static USERS: LazyLock<Mutex<Locks>> = LazyLock::new(Mutex::default);

async fn lock(user: &UserId) -> tokio::sync::OwnedMutexGuard<()> {
    let mutex = {
        let mut users = USERS.lock().await;
        users.retain(|_, value| value.strong_count() > 0);
        let mutex = users.get(user).and_then(Weak::upgrade).unwrap_or_default();
        users.insert(user.clone(), Arc::downgrade(&mutex));
        mutex
    };
    mutex.lock_owned().await
}

type Result<T> = std::result::Result<T, ApiFailure<SettingsApiError>>;
fn failure(error: impl Into<SettingsApiError>) -> ApiFailure<SettingsApiError> {
    ApiFailure(error.into())
}
fn admin(state: &AppState) -> Result<&crate::gateway::client::Admin> {
    state
        .config
        .codex_gateway
        .as_ref()
        .ok_or_else(|| failure(SettingsApiError::GatewayNotConfigured))
}

/// Existing W1 common DTOs and session/CSRF authentication; user identity never comes from the body.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(GET_CODEX.path, get(status).delete(unlink))
        .route(GET_CODEX_LINK.path, post(link).get(link_status))
}

async fn sync(state: &AppState, session: &Session) -> Result<CredentialStatus> {
    let status = admin(state)?
        .status(&UserId(session.user_id.clone()), false)
        .await
        .map_err(failure)?;
    state
        .db
        .llm_credentials()
        .put_status(&session.tenant_id, &session.user_id, &status)
        .await
        .map_err(failure)?;
    Ok(status)
}

async fn status(State(state): State<AppState>, session: Session) -> Result<Json<CredentialStatus>> {
    let _guard = lock(&UserId(session.user_id.clone())).await;
    Ok(Json(sync(&state, &session).await?))
}

async fn link_status(
    State(state): State<AppState>,
    session: Session,
) -> Result<Json<CredentialStatus>> {
    let _guard = lock(&UserId(session.user_id.clone())).await;
    // Admission follows usable credentials even while an old usable link is being replaced.
    let staged = admin(&state)?
        .status(&UserId(session.user_id.clone()), true)
        .await
        .map_err(failure)?;
    sync(&state, &session).await?;
    Ok(Json(staged))
}

async fn link(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    body: std::result::Result<Json<LinkCredential>, JsonRejection>,
) -> Result<Json<DeviceLink>> {
    let Json(command) = body.map_err(failure)?;
    let _guard = lock(&UserId(session.user_id.clone())).await;
    let challenge = admin(&state)?
        .link(&UserId(session.user_id.clone()), &command)
        .await
        .map_err(failure)?;
    sync(&state, &session).await?;
    Ok(Json(challenge))
}

async fn unlink(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
) -> Result<StatusCode> {
    let _guard = lock(&UserId(session.user_id.clone())).await;
    admin(&state)?
        .unlink(&UserId(session.user_id.clone()))
        .await
        .map_err(failure)?;
    state
        .db
        .llm_credentials()
        .remove_status(&session.user_id)
        .await
        .map_err(failure)?;
    Ok(StatusCode::NO_CONTENT)
}
