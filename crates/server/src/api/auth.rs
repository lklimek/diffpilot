//! Sign-in, callback, logout and `/api/v1/me`.

use axum::Router;

use crate::state::AppState;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
}
