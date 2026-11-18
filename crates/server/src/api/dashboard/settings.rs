//! Dashboard settings (R6, R6b); filled by WP11-SCOPE.

use axum::Router;

use crate::state::AppState;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
}
