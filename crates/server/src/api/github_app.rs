//! GitHub App configuration and delivery audit status.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use contracts::api::settings::{GithubWebhook, RepositoryInstallations};
use contracts::routes::{GET_GITHUB_WEBHOOK, GET_REPOSITORY_INSTALLATIONS};
use contracts::time::Timestamp;
use db::error::DbError;

use super::error::ApiFailure;
use crate::auth::Session;
use crate::dashboard::installations;
use crate::state::AppState;

/// GitHub App settings routes.
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(GET_GITHUB_WEBHOOK.path, get(read))
        .route(
            GET_REPOSITORY_INSTALLATIONS.path,
            get(repository_installations),
        )
}

async fn read(
    State(state): State<AppState>,
    _: Session,
) -> Result<Json<GithubWebhook>, ApiFailure<DbError>> {
    let (last_delivery_at, dropped_deliveries) = state
        .webhook_metrics
        .status(&state.db, Timestamp::now())
        .await?;
    Ok(Json(GithubWebhook {
        configured: state.config.github_webhook_secret.is_some(),
        webhook_url: state
            .config
            .github_webhook_url
            .as_ref()
            .map(ToString::to_string),
        last_delivery_at,
        dropped_deliveries,
        refused_connections: state.webhook_metrics.refused_connections(),
    }))
}

async fn repository_installations(
    State(state): State<AppState>,
    session: Session,
) -> Result<Json<RepositoryInstallations>, ApiFailure<DbError>> {
    let repositories = installations::for_tenant(&state, &session.tenant_id).await?;
    Ok(Json(RepositoryInstallations { repositories }))
}
