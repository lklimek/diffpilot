//! Liveness probe (`GET /healthz`): public, no body.

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use contracts::routes::HEALTHZ;

use crate::state::AppState;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new().route(HEALTHZ.path, get(healthz))
}

async fn healthz() -> StatusCode {
    StatusCode::NO_CONTENT
}
