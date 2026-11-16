//! HTTP routes: every module contributes one `routes()`, merged here exactly once.

use axum::Router;

use crate::state::AppState;

pub mod analyses;
pub mod auth;
pub mod comments;
pub mod diff;
pub mod events;
pub mod feedback;
pub mod lines;
pub mod pr;
pub mod questions;
pub mod review_state;
pub mod runner;
pub mod settings;
pub mod tour;

/// Builds the application router from all route modules and the web UI.
pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(analyses::routes())
        .merge(auth::routes())
        .merge(comments::routes())
        .merge(diff::routes())
        .merge(events::routes())
        .merge(feedback::routes())
        .merge(lines::routes())
        .merge(pr::routes())
        .merge(questions::routes())
        .merge(review_state::routes())
        .merge(runner::routes())
        .merge(settings::routes())
        .merge(tour::routes())
        .merge(crate::frontend::routes())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::*;
    use crate::config::tests::test_config;

    #[tokio::test]
    async fn router_merges_all_modules_and_answers_unknown_path_with_404() {
        let app = router(AppState::new(test_config()));
        let request = Request::get("/api/v1/no-such-route")
            .body(Body::empty())
            .expect("request");
        let response = app.oneshot(request).await.expect("infallible");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
