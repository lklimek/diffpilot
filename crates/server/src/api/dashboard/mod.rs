//! Dashboard HTTP routes (plan §E.2): one sub-router per owning work package, merged here.

use axum::Router;

use crate::state::AppState;

pub mod refresh;
pub mod settings;
pub mod snooze;
pub mod tokens;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(refresh::routes())
        .merge(settings::routes())
        .merge(snooze::routes())
        .merge(tokens::routes())
}
