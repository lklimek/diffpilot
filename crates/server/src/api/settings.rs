//! User and repository settings, LLM status.

use axum::Router;

use crate::state::AppState;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
}
