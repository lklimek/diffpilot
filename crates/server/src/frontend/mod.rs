//! Serving the web UI (embedded with feature `embed-ui`).

use axum::Router;

use crate::state::AppState;

/// Web UI routes.
pub fn routes() -> Router<AppState> {
    Router::new()
}
