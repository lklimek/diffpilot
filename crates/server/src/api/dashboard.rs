//! Dashboard HTTP routes (plan §E.2); filled by WP11-REFRESH.

use axum::Router;

use crate::state::AppState;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
}
