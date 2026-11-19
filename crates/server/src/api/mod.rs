//! HTTP routes: every module contributes one `routes()`, merged here exactly once. The runner protocol is
//! served only on the runner listener (`SR_RUNNER_LISTEN_ADDR`), never on the public one.

use axum::Router;
use axum::http::HeaderValue;
use axum::middleware::{from_fn_with_state, map_response};
use axum::response::Response;

use crate::auth::{TokenGate, access_tokens};
use crate::frontend::SECURITY_HEADERS;
use crate::state::AppState;

pub mod analyses;
pub mod auth;
pub mod comments;
pub mod dashboard;
pub mod diff;
pub mod error;
pub mod events;
pub mod feedback;
pub mod files;
pub mod flow_work;
pub mod health;
pub mod lines;
pub mod pr;
pub mod questions;
pub mod review_state;
pub mod runner;
pub mod settings;
pub mod tour;

/// Headers of API and auth responses besides [`SECURITY_HEADERS`]: JSON and event streams load nothing and
/// are never framed; responses carry session data, so no cache keeps them.
const API_HEADERS: [(&str, &str); 2] = [
    (
        "content-security-policy",
        "default-src 'none'; frame-ancestors 'none'",
    ),
    ("cache-control", "no-store"),
];

/// Builds the public router (browser API, auth, web UI) from all route modules except the runner protocol.
pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(analyses::routes())
        .merge(flow_work::routes())
        .merge(auth::routes())
        .merge(comments::routes())
        .merge(dashboard::routes())
        .merge(diff::routes())
        .merge(events::routes())
        .merge(feedback::routes())
        .merge(files::routes())
        .merge(health::routes())
        .merge(lines::routes())
        .merge(pr::routes())
        .merge(questions::routes())
        .merge(review_state::routes())
        .merge(settings::routes())
        .merge(tour::routes())
        .route_layer(from_fn_with_state(
            TokenGate::new(state.clone()),
            access_tokens::check,
        ))
        .layer(map_response(api_security_headers))
        .merge(crate::frontend::routes())
        .with_state(state)
}

/// Sets the API security headers; touches headers only, so streamed bodies (SSE) pass unbuffered.
async fn api_security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    for (name, value) in SECURITY_HEADERS.into_iter().chain(API_HEADERS) {
        headers.insert(name, HeaderValue::from_static(value));
    }
    response
}

/// Builds the runner-only router (`/runner/v1/*`), served on the runner listener.
pub fn runner_router(state: AppState) -> Router {
    Router::new().merge(runner::routes()).with_state(state)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::response::Response;
    use contracts::ids::AnalysisId;
    use contracts::routes::{ANALYSIS_EVENTS, GET_ANALYSIS, HEALTHZ};
    use db::repo::DEFAULT_TENANT;
    use db::repo::analyses::testing::seed_analysis;
    use futures::StreamExt;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::state::tests::test_state;
    use crate::test_support::session_cookie;

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

    /// Headers every API response carries (threat model T7b; JSON and event streams load nothing).
    const EXPECTED_API_HEADERS: [(&str, &str); 5] = [
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "same-origin"),
        ("x-frame-options", "DENY"),
        (
            "content-security-policy",
            "default-src 'none'; frame-ancestors 'none'",
        ),
        ("cache-control", "no-store"),
    ];

    fn assert_api_headers(response: &Response, case: &str) {
        for (name, value) in EXPECTED_API_HEADERS {
            assert_eq!(
                response.headers().get(name).and_then(|v| v.to_str().ok()),
                Some(value),
                "{case}: {name}"
            );
        }
    }

    fn get_analysis(analysis: AnalysisId, cookie: Option<&str>) -> Request<Body> {
        let mut request = Request::get(GET_ANALYSIS.path.replace("{id}", &analysis.to_string()));
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        request.body(Body::empty()).expect("request")
    }

    #[tokio::test]
    async fn api_security_headers_on_authenticated_json_response() {
        let state = test_state().await;
        let analysis = seed_analysis(&state.db).await.expect("seed");
        let cookie = session_cookie(&state, DEFAULT_TENANT).await;
        let response = router(state)
            .oneshot(get_analysis(analysis, Some(&cookie)))
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "application/json");
        assert_api_headers(&response, "analysis");
    }

    #[tokio::test]
    async fn api_security_headers_on_error_responses() {
        let state = test_state().await;
        let analysis = seed_analysis(&state.db).await.expect("seed");
        let cookie = session_cookie(&state, DEFAULT_TENANT).await;
        for (request, status, case) in [
            (
                get_analysis(analysis, None),
                StatusCode::UNAUTHORIZED,
                "no session",
            ),
            (
                get_analysis(AnalysisId::new_random(), Some(&cookie)),
                StatusCode::NOT_FOUND,
                "unknown analysis",
            ),
        ] {
            let response = router(state.clone())
                .oneshot(request)
                .await
                .expect("infallible");
            assert_eq!(response.status(), status, "{case}");
            assert_api_headers(&response, case);
        }
    }

    #[tokio::test]
    async fn api_security_headers_keep_the_event_stream_unbuffered() {
        let state = test_state().await;
        let analysis = seed_analysis(&state.db).await.expect("seed");
        let cookie = session_cookie(&state, DEFAULT_TENANT).await;
        let request = Request::get(ANALYSIS_EVENTS.path.replace("{id}", &analysis.to_string()))
            .header("cookie", &cookie)
            .body(Body::empty())
            .expect("request");
        let response = router(state).oneshot(request).await.expect("infallible");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        assert_api_headers(&response, "events");
        // The stream stays open; the snapshot frame arrives before it ends, so nothing buffers the body.
        let first = tokio::time::timeout(
            Duration::from_secs(10),
            response.into_body().into_data_stream().next(),
        )
        .await
        .expect("snapshot frame in time")
        .expect("stream open")
        .expect("frame");
        assert!(first.starts_with(b"event: analysis"), "{first:?}");
    }

    #[tokio::test]
    async fn healthz_answers_204_without_session() {
        let request = Request::get(HEALTHZ.path)
            .body(Body::empty())
            .expect("request");
        let response = router(test_state().await)
            .oneshot(request)
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_api_headers(&response, "healthz");
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        assert!(body.is_empty(), "{body:?}");
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
