//! `GET /api/v1/analyses`: the reader's own analyses only.

use axum::body::Body;
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::{Request, StatusCode};
use contracts::api::access_tokens::TokenScope;
use contracts::routes::LIST_MY_ANALYSES;
use contracts::time::Timestamp;
use db::repo::DEFAULT_TENANT;
use db::repo::analyses::testing::seed_analysis;
use http_body_util::BodyExt;
use jiff::SignedDuration;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::api::router;
use crate::auth::access_tokens::stored_token;
use crate::state::AppState;
use crate::state::tests::test_state;
use crate::test_support::{other_user_cookie, session_cookie};

async fn list(state: &AppState, query: &str, auth: (&str, &str)) -> (StatusCode, Value) {
    let request = Request::get(format!("{}{query}", LIST_MY_ANALYSES.path))
        .header(auth.0, auth.1)
        .body(Body::empty())
        .expect("request");
    let response = router(state.clone())
        .oneshot(request)
        .await
        .expect("infallible");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn lists_the_readers_own_analyses_and_never_another_users() {
    let state = test_state().await;
    // u1 requested this analysis (repository and title stand in for a private one).
    let analysis = seed_analysis(&state.db).await.expect("seed");
    let mine = session_cookie(&state, DEFAULT_TENANT).await;
    let theirs = other_user_cookie(&state).await;

    let (status, body) = list(&state, "", (COOKIE.as_str(), &mine)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"analyses": [{
            "id": analysis.to_string(),
            "pr": {"repo": {"owner": "acme", "name": "node-client"}, "number": 482},
            "title": "Retry RPC calls",
            "status": "planning",
            "created_at": "2026-11-16T00:00:00Z",
        }]})
    );

    let (status, body) = list(&state, "", (COOKIE.as_str(), &theirs)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"analyses": []}));
}

#[tokio::test]
async fn a_token_lists_what_its_user_sees() {
    let state = test_state().await;
    seed_analysis(&state.db).await.expect("seed");
    let cookie = session_cookie(&state, DEFAULT_TENANT).await;
    other_user_cookie(&state).await;
    let expires = Timestamp::now() + SignedDuration::from_hours(1);
    let (own, _) = stored_token(&state.db, "u1", vec![TokenScope::Read], expires).await;
    let (other, _) = stored_token(&state.db, "u2", vec![TokenScope::Read], expires).await;

    let by_cookie = list(&state, "", (COOKIE.as_str(), &cookie)).await;
    let by_token = list(
        &state,
        "",
        (AUTHORIZATION.as_str(), &format!("Bearer {own}")),
    )
    .await;
    assert_eq!(by_token, by_cookie);
    let (status, body) = list(
        &state,
        "",
        (AUTHORIZATION.as_str(), &format!("Bearer {other}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"analyses": []}));
}

#[tokio::test]
async fn limit_caps_the_rows_and_is_validated() {
    let state = test_state().await;
    seed_analysis(&state.db).await.expect("seed");
    let cookie = session_cookie(&state, DEFAULT_TENANT).await;
    let (status, body) = list(&state, "?limit=1", (COOKIE.as_str(), &cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["analyses"].as_array().map(Vec::len), Some(1));
    for query in ["?limit=0", "?limit=101", "?limit=x"] {
        let (status, body) = list(&state, query, (COOKIE.as_str(), &cookie)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
        assert_eq!(body["code"], "invalid_request", "{query}");
    }
    let (status, _) = list(&state, "", ("x-none", "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
