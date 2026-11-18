//! Personal access tokens (R8–R10, ADR-019); filled by WP11-TOKENS.

use axum::Router;

use crate::state::AppState;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
}
