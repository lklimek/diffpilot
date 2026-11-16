//! HTTP routes: every module contributes one `routes()`, merged here exactly once. The runner protocol is
//! served only on the runner listener (`SR_RUNNER_LISTEN_ADDR`), never on the public one.

use axum::Router;

use crate::state::AppState;

pub mod analyses;
pub mod auth;
pub mod comments;
pub mod diff;
pub mod error;
pub mod events;
pub mod feedback;
pub mod lines;
pub mod pr;
pub mod questions;
pub mod review_state;
pub mod runner;
pub mod settings;
pub mod tour;

/// Builds the public router (browser API, auth, web UI) from all route modules except the runner protocol.
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
        .merge(settings::routes())
        .merge(tour::routes())
        .merge(crate::frontend::routes())
        .with_state(state)
}

/// Builds the runner-only router (`/runner/v1/*`), served on the runner listener.
pub fn runner_router(state: AppState) -> Router {
    Router::new().merge(runner::routes()).with_state(state)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::*;
    use crate::state::tests::test_state;

    async fn status_of(app: Router, request: Request<Body>) -> StatusCode {
        app.oneshot(request).await.expect("infallible").status()
    }

    #[tokio::test]
    async fn router_merges_all_modules_and_answers_unknown_path_with_404() {
        let request = Request::get("/api/v1/no-such-route")
            .body(Body::empty())
            .expect("request");
        assert_eq!(
            status_of(router(test_state().await), request).await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn runner_protocol_is_not_served_on_the_public_router() {
        for route in contracts::routes::ROUTES
            .iter()
            .filter(|route| route.listener == contracts::routes::Listener::Runner)
        {
            let path = route
                .path
                .replace("{id}", "00000000-0000-4000-8000-000000000000");
            let request = Request::post(path).body(Body::empty()).expect("request");
            assert_eq!(
                status_of(router(test_state().await), request).await,
                StatusCode::NOT_FOUND,
                "{}",
                route.path
            );
        }
    }

    #[tokio::test]
    async fn browser_api_is_not_served_on_the_runner_router() {
        let request = Request::get("/api/v1/me")
            .body(Body::empty())
            .expect("request");
        assert_eq!(
            status_of(runner_router(test_state().await), request).await,
            StatusCode::NOT_FOUND
        );
    }
}
