//! Public-listener webhook paths drain the body before returning 404.

use axum::Router;
use axum::http::StatusCode;
use axum::middleware::from_fn;
use axum::routing::any;

use crate::state::AppState;
use crate::webhooks::drain_unread_body;

pub(super) fn routes() -> Router<AppState> {
    let not_found = || async { StatusCode::NOT_FOUND };
    Router::new()
        .route("/webhooks", any(not_found))
        .route("/webhooks/", any(not_found))
        .route("/webhooks/{*path}", any(not_found))
        .layer(from_fn(drain_unread_body))
}
