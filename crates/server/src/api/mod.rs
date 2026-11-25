//! HTTP routes on the public, runner and webhook listeners.

use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{from_fn_with_state, map_response};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use contracts::api::error::{ApiError, ApiErrorCode};
use contracts::routes::{
    GET_GITHUB_WEBHOOK, GET_LLM_ACTIVITY, GET_RANKING_USAGE, GET_REPOSITORY_INSTALLATIONS,
    GITHUB_WEBHOOK, LIST_MY_PRS, REJUDGE_DASHBOARD_ITEM, RESET_RANKING_USAGE,
};

use crate::auth::{Session, SessionCsrf, TokenGate, access_tokens};
use crate::frontend::SECURITY_HEADERS;
use crate::state::AppState;

pub mod analyses;
pub mod assessment;
pub mod auth;
pub mod codex;
pub mod comments;
pub mod dashboard;
pub mod decision_feed;
pub mod deps;
pub mod diff;
pub mod error;
pub mod events;
pub mod feedback;
pub mod files;
pub mod flow_work;
pub mod health;
pub mod lines;
pub mod notifications;
pub mod pr;
pub mod questions;
mod ranking_usage;
pub mod review_loop;
pub mod review_state;
pub mod runner;
pub mod settings;
pub mod tour;
mod webhook_fallback;

/// Headers of API and auth responses besides [`SECURITY_HEADERS`]: JSON and event streams load nothing and
/// are never framed; responses carry session data, so no cache keeps them.
const API_HEADERS: [(&str, &str); 2] = [
    (
        "content-security-policy",
        "default-src 'none'; frame-ancestors 'none'",
    ),
    ("cache-control", "no-store"),
];

/// Builds the public router for the browser API, auth and web UI.
pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(analyses::routes())
        .merge(assessment::routes())
        .merge(flow_work::routes())
        .merge(auth::routes())
        .merge(comments::routes())
        .merge(dashboard::routes())
        .merge(decision_feed::routes())
        .merge(deps::routes())
        .merge(diff::routes())
        .merge(events::routes())
        .merge(feedback::routes())
        .merge(files::routes())
        .merge(health::routes())
        .merge(lines::routes())
        .merge(notifications::routes())
        .merge(pr::routes())
        .merge(questions::routes())
        .merge(ranking_usage::routes())
        .merge(review_loop::routes())
        .merge(review_state::routes())
        .merge(settings::routes())
        .merge(codex::routes())
        .merge(tour::routes())
        .merge(reserved_routes())
        .route_layer(from_fn_with_state(
            TokenGate::new(state.clone()),
            access_tokens::check,
        ))
        .layer(map_response(api_security_headers))
        .merge(webhook_fallback::routes())
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

/// Builds the router for the dedicated GitHub webhook listener.
pub fn webhook_router() -> Router {
    Router::new().route(
        GITHUB_WEBHOOK.path,
        post(|| async { not_implemented() }).fallback(|| async { StatusCode::NOT_FOUND }),
    )
}

fn reserved_routes() -> Router<AppState> {
    let read = get(|_: Session| async { not_implemented() });
    let write = post(|_: SessionCsrf| async { not_implemented() });
    Router::new()
        .route(GET_GITHUB_WEBHOOK.path, read.clone())
        .route(GET_REPOSITORY_INSTALLATIONS.path, read.clone())
        .route(GET_LLM_ACTIVITY.path, read.clone())
        .route(LIST_MY_PRS.path, read.clone())
        .route(GET_RANKING_USAGE.path, read)
        .route(REJUDGE_DASHBOARD_ITEM.path, write.clone())
        .route(RESET_RANKING_USAGE.path, write)
}

fn not_implemented() -> Response {
    let code = ApiErrorCode::NotImplemented;
    (
        error::status(code),
        Json(ApiError {
            code,
            message: "This feature is not available yet.".to_owned(),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::Request;
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

    #[tokio::test]
    async fn webhooks_catch_all_404_drains_slow_body() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::{TcpListener, TcpStream};
        use tokio::time::timeout;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(test_state().await);
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        timeout(Duration::from_secs(3), async {
            let mut client = TcpStream::connect(addr).await.unwrap();
            client
                .write_all(b"POST /webhooks/github HTTP/1.1\r\nHost: localhost\r\nContent-Length: 61440\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let chunk = [b'x'; 4096];
            client.write_all(&chunk).await.unwrap();
            let mut early = [0];
            assert!(
                timeout(Duration::from_millis(150), client.read(&mut early)).await.is_err(),
                "the response or connection close arrived before the remaining body"
            );
            for _ in 1..15 {
                client.write_all(&chunk).await.unwrap();
            }
            let mut response = Vec::new();
            client.read_to_end(&mut response).await.unwrap();
            assert!(response.starts_with(b"HTTP/1.1 404 "), "{response:?}");
        })
        .await
        .unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn webhooks_catch_all_bounds_the_drain_and_closes_the_connection() {
        use axum::body::Bytes;
        use axum::http::header::{CONNECTION, CONTENT_LENGTH};

        let app = router(test_state().await);
        tokio::time::pause();
        let never =
            || Body::from_stream(futures::stream::pending::<Result<Bytes, std::io::Error>>());
        let request = |body| Request::post("/webhooks/unknown").body(body).unwrap();
        let mut oversized = request(never());
        oversized
            .headers_mut()
            .insert(CONTENT_LENGTH, HeaderValue::from_static("4194305"));
        let start = tokio::time::Instant::now();
        let response = app.clone().oneshot(oversized).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(response.headers()[CONNECTION], "close");
        assert_eq!(start.elapsed(), Duration::ZERO);

        let beyond_limit = Bytes::from(vec![0; 4 * 1024 * 1024 + 1]);
        let chunked = futures::stream::iter([Ok::<_, std::io::Error>(beyond_limit)])
            .chain(futures::stream::pending());
        let response = app
            .clone()
            .oneshot(request(Body::from_stream(chunked)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[CONNECTION], "close");
        assert_eq!(start.elapsed(), Duration::ZERO);

        let response = app.oneshot(request(never())).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[CONNECTION], "close");
        let limit = Duration::from_secs(8);
        // Tokio's timer rounds a deadline up to a millisecond tick.
        assert!((limit..=limit + Duration::from_millis(1)).contains(&start.elapsed()));
    }

    #[tokio::test]
    async fn route_listener_matrix() {
        use contracts::routes::{GITHUB_WEBHOOK, Listener, ROUTES, RouteAuth};

        assert_eq!(GITHUB_WEBHOOK.auth, RouteAuth::GithubSignature);
        assert_eq!(
            ROUTES
                .iter()
                .filter(|route| route.listener == Listener::Webhook)
                .copied()
                .collect::<Vec<_>>(),
            [GITHUB_WEBHOOK]
        );

        let state = test_state().await;
        let public = router(state.clone());
        let runner = runner_router(state);
        let webhook = webhook_router();
        for app in [public, runner] {
            for method in ["GET", "POST", "HEAD"] {
                for path in ["/webhooks", "/webhooks/github", "/webhooks/no-such-route"] {
                    let request = Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .expect("request");
                    assert_eq!(
                        status_of(app.clone(), request).await,
                        StatusCode::NOT_FOUND,
                        "{method} {path}"
                    );
                }
            }
        }
        for route in ROUTES
            .iter()
            .filter(|route| route.listener != Listener::Webhook)
        {
            let path = route.path.replace(['{', '}'], "");
            let request = Request::builder()
                .method(route.method.as_str())
                .uri(&path)
                .body(Body::empty())
                .expect("request");
            assert_eq!(
                status_of(webhook.clone(), request).await,
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }
        for (method, path) in [
            ("GET", "/webhooks/github"),
            ("POST", "/webhooks/github/"),
            ("GET", "/"),
        ] {
            let request = Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .expect("request");
            assert_eq!(
                status_of(webhook.clone(), request).await,
                StatusCode::NOT_FOUND,
                "{method} {path}"
            );
        }
        let response = webhook
            .oneshot(
                Request::post("/webhooks/github")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_not_implemented(response).await;
    }

    async fn assert_not_implemented(response: Response) {
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).expect("JSON error");
        assert_eq!(body["code"], "not_implemented");
    }

    #[tokio::test]
    async fn phase_b_api_placeholders_require_their_route_auth() {
        let state = test_state().await;
        seed_analysis(&state.db).await.expect("seed");
        let cookie = session_cookie(&state, DEFAULT_TENANT).await;
        let app = router(state);
        for (method, path) in [
            ("GET", "/api/v1/llm/activity"),
            ("GET", "/api/v1/settings/repository-installations"),
            ("GET", "/api/v1/settings/github-webhook"),
            ("GET", "/api/v1/me/prs"),
            ("POST", "/api/v1/settings/ranking-usage/reset"),
            ("POST", "/api/v1/dashboard/items/acme~repo~pr~1/rejudge"),
        ] {
            let request = |cookie: Option<&str>, csrf: Option<&str>| {
                let mut request = Request::builder().method(method).uri(path);
                if let Some(cookie) = cookie {
                    request = request.header("cookie", cookie);
                }
                if let Some(csrf) = csrf {
                    request = request.header("x-csrf-token", csrf);
                }
                request.body(Body::empty()).expect("request")
            };
            assert_eq!(
                status_of(app.clone(), request(None, None)).await,
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
            if method == "POST" {
                assert_eq!(
                    status_of(app.clone(), request(Some(&cookie), None)).await,
                    StatusCode::FORBIDDEN,
                    "{path}"
                );
            }
            let response = app
                .clone()
                .oneshot(request(Some(&cookie), Some("csrf")))
                .await
                .expect("infallible");
            assert_not_implemented(response).await;
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
