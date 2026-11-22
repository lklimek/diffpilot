//! Personal access tokens through the public router (ADR-019, threat model T20): the route table decides, the
//! token reads as its user, every refusal answers with the documented code.

use std::net::SocketAddr;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::header::{AUTHORIZATION, COOKIE, RETRY_AFTER, WWW_AUTHENTICATE};
use axum::http::{Method, Request, StatusCode};
use axum::response::Response;
use contracts::api::access_tokens::TokenScope;
use contracts::ids::{AnalysisId, QuestionId, StepId};
use contracts::routes::{
    CREATE_ACCESS_TOKEN, GET_ME, GET_NOTIFICATION_SETTINGS, GET_REVIEW_STATE, HttpMethod,
    LIST_ACCESS_TOKENS, LIST_QUESTIONS, Listener, REVOKE_ACCESS_TOKEN, ROUTES, RUNNER_CLAIM, Route,
    RouteAuth,
};
use contracts::time::Timestamp;
use db::repo::DEFAULT_TENANT;
use db::repo::analyses::testing::seed_analysis;
use db::repo::questions::{NewQuestion, QuestionLimits};
use db::repo::tenants::testing::remove_login;
use http_body_util::BodyExt;
use jiff::SignedDuration;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::*;
use crate::api;
use crate::auth::{CSRF_HEADER, Session};
use crate::test_support::logs::LogCapture;
use crate::test_support::{other_user_cookie, session_cookie};

const CSRF: &str = "csrf";

struct Setup {
    /// Data directory (mirrors) of the state; removed on drop.
    _dir: tempfile::TempDir,
    state: AppState,
    app: Router,
    analysis: AnalysisId,
    /// Session cookie of `u1`.
    cookie: String,
    /// A `read` token of `u1`.
    token: String,
}

async fn setup() -> Setup {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::tests::test_config();
    config.data_dir = dir.path().to_owned();
    let state = AppState::with_queue(config, db::repo::Db::in_memory().await.expect("db"));
    let analysis = seed_analysis(&state.db).await.expect("analysis");
    let cookie = session_cookie(&state, DEFAULT_TENANT).await;
    let (token, _) = read_token(&state, "u1").await;
    Setup {
        _dir: dir,
        app: api::router(state.clone()),
        state,
        analysis,
        cookie,
        token,
    }
}

async fn read_token(state: &AppState, user: &str) -> (String, TokenPrincipal) {
    stored_token(
        &state.db,
        user,
        vec![TokenScope::Read],
        Timestamp::now() + SignedDuration::from_hours(24 * 90),
    )
    .await
}

/// `route.path` with its parameters filled for the seeded analysis; lines and diff get a valid range.
fn uri(route: &Route, analysis: AnalysisId) -> String {
    let path = route
        .path
        .replace("{id}", &analysis.to_string())
        .replace("{owner}", "acme")
        .replace("{name}", "node-client")
        .replace("{step_id}", "f1.1")
        .replace("{flow}", "f1")
        .replace("{feedback_id}", "00000000-0000-4000-8000-000000000000")
        .replace("{comment_id}", "1")
        .replace("{number}", "482")
        .replace("{item}", "C1")
        .replace("{kind}", "ntfy");
    if path.ends_with("/lines") || path.ends_with("/diff") {
        path + "?path=src/lib.rs&side=head&from=1&to=5"
    } else {
        path
    }
}

fn request(method: &str, uri: &str, token: Option<&str>, cookie: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(cookie) = cookie {
        builder = builder.header(COOKIE, cookie).header(CSRF_HEADER, CSRF);
    }
    builder.body(Body::from("{}")).expect("request")
}

async fn send(app: &Router, request: Request<Body>) -> Response {
    app.clone().oneshot(request).await.expect("infallible")
}

/// Status and JSON body (`Null` for an empty or streamed body that is not JSON).
async fn status_and_body(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = send(app, request).await;
    let status = response.status();
    if response
        .headers()
        .get("content-type")
        .is_some_and(|value| value == "text/event-stream")
    {
        return (status, Value::Null);
    }
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn get_with_token(app: &Router, uri: &str, token: &str) -> (StatusCode, Value) {
    status_and_body(app, request("GET", uri, Some(token), None)).await
}

#[test]
fn issued_tokens_have_the_documented_form() {
    let first = issue().expect("token");
    let second = issue().expect("token");
    for issued in [&first, &second] {
        let token = &issued.token.0;
        assert_eq!(token.len(), 4 + 8 + 1 + 43, "{}", issued.id);
        assert!(token.starts_with(&format!("srp_{}_", issued.id)));
        assert!(is_token_id(&issued.id.0));
        assert_eq!(token_id(token), Some(issued.id.clone()));
        assert_eq!(issued.hash, sha256(token.as_bytes()));
        assert!(!format!("{:?}", issued.token).contains(&token[13..]));
    }
    assert_ne!(first.id, second.id);
    assert_ne!(first.token, second.token);
}

#[test]
fn malformed_tokens_have_no_id() {
    let secret = "a".repeat(43);
    for bad in [
        String::new(),
        "srp_".to_owned(),
        format!("srp_Ab3dE5g_{secret}"),
        format!("srp_Ab3dE5gH{secret}"),
        format!("srp_Ab3dE5gH_{}", "a".repeat(42)),
        format!("srp_Ab3dE5gH_{}", "a".repeat(44)),
        format!("srp_Ab3d-5gH_{secret}"),
        format!("srp_Ab3dE5gH_{}+", "a".repeat(42)),
        format!("SRP_Ab3dE5gH_{secret}"),
        format!("ghu_Ab3dE5gH_{secret}"),
        format!("srp_Ab3dÉ5g_{secret}"),
    ] {
        assert_eq!(token_id(&bad), None, "{bad}");
    }
    assert_eq!(
        token_id(&format!("srp_Ab3dE5gH_{}-_", "a".repeat(41))),
        Some(AccessTokenId("Ab3dE5gH".to_owned()))
    );
}

/// No route is left unclassified: every route of the table either accepts a token as a read or a `TokenOnly`
/// route with its scope, identifies the user (`/api/v1/me`) or refuses it; a new `RouteAuth` variant fails to compile here.
#[test]
fn every_route_has_a_token_classification() {
    for route in ROUTES {
        let method = Method::from_bytes(route.method.as_str().as_bytes()).expect("method");
        let classified = token_use(&method, Some(route.path));
        let expected = match (route.listener, route.auth) {
            (Listener::Runner, _) => TokenUse::Refused,
            (Listener::Public, RouteAuth::SessionOrToken(scope)) => {
                assert_eq!(route.method, HttpMethod::Get, "{}", route.path);
                TokenUse::Read(scope)
            }
            (Listener::Public, RouteAuth::Public) if *route == GET_ME => TokenUse::Identify,
            (Listener::Public, RouteAuth::TokenOnly(scope)) => TokenUse::Execute(scope),
            (
                Listener::Public,
                RouteAuth::Public
                | RouteAuth::Session
                | RouteAuth::SessionCsrf
                | RouteAuth::RunnerSecret
                | RouteAuth::JobToken,
            ) => TokenUse::Refused,
        };
        assert_eq!(
            classified,
            expected,
            "{} {}",
            route.method.as_str(),
            route.path
        );
        if let TokenUse::Read(_) = expected {
            assert_eq!(token_use(&Method::HEAD, Some(route.path)), expected);
            assert_eq!(
                token_use(&Method::OPTIONS, Some(route.path)),
                TokenUse::Refused
            );
        }
    }
    assert_eq!(token_use(&Method::GET, None), TokenUse::Refused);
    assert_eq!(
        token_use(&Method::GET, Some("/api/v1/unknown")),
        TokenUse::Refused
    );
}

/// ADR-019 §5: every `GET` route with session auth is `read`, except the token list (R8).
#[tokio::test]
async fn every_session_get_route_accepts_read_tokens_except_listed() {
    const SESSION_ONLY: [Route; 2] = [LIST_ACCESS_TOKENS, GET_NOTIFICATION_SETTINGS];
    for route in ROUTES.iter().filter(|route| {
        route.listener == Listener::Public
            && route.method == HttpMethod::Get
            && route.auth == RouteAuth::Session
    }) {
        assert!(
            SESSION_ONLY.contains(route),
            "{} is session only",
            route.path
        );
    }
    // A token reads every served `read` route exactly as its user's session does.
    let s = setup().await;
    let mut served = 0;
    for route in ROUTES
        .iter()
        .filter(|route| matches!(route.auth, RouteAuth::SessionOrToken(_)))
    {
        let uri = uri(route, s.analysis);
        // A GitHub-backed read without a stored GitHub token pair ends the user's sessions (v1), so each route
        // starts with a fresh one.
        let _ = s
            .state
            .db
            .sessions()
            .delete(&sha256(b"session-token-default"))
            .await;
        let cookie = session_cookie(&s.state, DEFAULT_TENANT).await;
        let (cookie_status, cookie_body) =
            status_and_body(&s.app, request("GET", &uri, None, Some(&cookie))).await;
        if cookie_status == StatusCode::NOT_FOUND && cookie_body.is_null() {
            continue; // not served yet (bare 404 of the router)
        }
        // Same answer as the session, including a v1 answer such as "sign in to GitHub again" (no pair stored).
        let response = send(&s.app, request("GET", &uri, Some(&s.token), None)).await;
        assert!(response.headers().get(WWW_AUTHENTICATE).is_none(), "{uri}");
        let (status, body) = get_with_token(&s.app, &uri, &s.token).await;
        assert_eq!(status, cookie_status, "{uri}: {body}");
        assert_eq!(body, cookie_body, "{uri}");
        served += 1;
    }
    assert!(served >= 11, "too few read routes served: {served}");
}

/// Every mutating route answers a `read` token with 403 before its handler runs: `token_not_accepted`, or on the
/// feed's `TokenOnly` routes (ADR-019 amendment) `insufficient_scope`.
#[tokio::test]
async fn no_mutating_route_accepts_a_token() {
    let s = setup().await;
    let mut checked = 0;
    for route in ROUTES
        .iter()
        .filter(|route| route.listener == Listener::Public && route.method != HttpMethod::Get)
    {
        let uri = uri(route, s.analysis);
        let (status, body) = status_and_body(
            &s.app,
            request(route.method.as_str(), &uri, Some(&s.token), None),
        )
        .await;
        if status == StatusCode::NOT_FOUND && body.is_null() {
            continue;
        }
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
        let expected = match route.auth {
            RouteAuth::TokenOnly(_) => "insufficient_scope",
            _ => "token_not_accepted",
        };
        assert_eq!(body["code"], expected, "{uri}");
        checked += 1;
    }
    assert!(checked >= 20, "too few mutations checked: {checked}");
}

/// T20: a token with a CSRF header is still refused on a mutation, and nothing changes.
#[tokio::test]
async fn token_never_accepted_on_mutating_routes() {
    let s = setup().await;
    let body = json!({ "language": "pl" }).to_string();
    for route in [
        contracts::routes::SET_LANGUAGE,
        contracts::routes::AUTH_LOGOUT,
    ] {
        let request = Request::builder()
            .method(route.method.as_str())
            .uri(route.path)
            .header("content-type", "application/json")
            .header(AUTHORIZATION, format!("Bearer {}", s.token))
            .header(CSRF_HEADER, CSRF)
            .body(Body::from(body.clone()))
            .expect("request");
        let (status, error) = status_and_body(&s.app, request).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{}", route.path);
        assert_eq!(error["code"], "token_not_accepted");
    }
    let user = s.state.db.users().get("u1").await.expect("user");
    assert_eq!(serde_json::to_value(user.language).expect("json"), "en");
    let (status, _) = get_with_token(&s.app, GET_ME.path, &s.token).await;
    assert_eq!(status, StatusCode::OK, "the token still works");
}

#[tokio::test]
async fn token_cannot_list_or_mint_tokens() {
    let s = setup().await;
    for (route, body) in [
        (LIST_ACCESS_TOKENS, None),
        (
            CREATE_ACCESS_TOKEN,
            Some(json!({"name": "minted", "scopes": ["read"], "expires_in_days": 30})),
        ),
        (REVOKE_ACCESS_TOKEN, None),
    ] {
        let uri = route.path.replace("{id}", "Ab3dE5gH");
        let mut request = Request::builder()
            .method(route.method.as_str())
            .uri(&uri)
            .header("content-type", "application/json")
            .header(AUTHORIZATION, format!("Bearer {}", s.token));
        if route.method != HttpMethod::Get {
            request = request.header(CSRF_HEADER, CSRF);
        }
        let body = body.map_or_else(String::new, |body| body.to_string());
        let (status, error) =
            status_and_body(&s.app, request.body(Body::from(body)).expect("request")).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(error["code"], "token_not_accepted", "{uri}");
    }
    let listed = s
        .state
        .db
        .access_tokens()
        .list(DEFAULT_TENANT, "u1")
        .await
        .expect("list");
    assert_eq!(listed.len(), 1, "nothing minted or revoked");
}

/// `GET /api/v1/me` names the token's user and never hands out a CSRF token; other public routes refuse it.
#[tokio::test]
async fn me_names_the_token_user_without_csrf() {
    let s = setup().await;
    let (status, me) = get_with_token(&s.app, GET_ME.path, &s.token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["user"]["login"], "lklimek");
    assert_eq!(me["csrf_token"], Value::Null);
    for path in ["/healthz", "/auth/github/login"] {
        let (status, error) = get_with_token(&s.app, path, &s.token).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(error["code"], "token_not_accepted", "{path}");
    }
}

async fn create_token(s: &Setup, body: Value) -> (StatusCode, Value) {
    status_and_body(
        &s.app,
        Request::post(CREATE_ACCESS_TOKEN.path)
            .header("content-type", "application/json")
            .header(COOKIE, &s.cookie)
            .header(CSRF_HEADER, CSRF)
            .body(Body::from(body.to_string()))
            .expect("request"),
    )
    .await
}

fn create_body(name: &str) -> Value {
    json!({"name": name, "scopes": ["read"], "expires_in_days": 90})
}

#[tokio::test]
async fn token_shown_once_and_stored_as_hash() {
    let s = setup().await;
    let (status, created) = create_token(&s, create_body("dispatcher")).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let token = created["token"].as_str().expect("token").to_owned();
    let listed = &created["access_token"];
    let id = listed["id"].as_str().expect("id");
    assert!(token.starts_with(&format!("srp_{id}_")));
    assert_eq!(listed["prefix"], format!("srp_{id}"));
    assert_eq!(listed["scopes"], json!(["read"]));
    assert_eq!(listed["last_used_at"], Value::Null);
    let expires: Timestamp = listed["expires_at"]
        .as_str()
        .expect("at")
        .parse()
        .expect("time");
    let created_at: Timestamp = listed["created_at"]
        .as_str()
        .expect("at")
        .parse()
        .expect("time");
    assert_eq!(expires - created_at, SignedDuration::from_hours(24 * 90));

    let credential = s
        .state
        .db
        .access_tokens()
        .credential(&AccessTokenId(id.to_owned()))
        .await
        .expect("read")
        .expect("stored");
    assert_eq!(credential.token_hash, sha256(token.as_bytes()));

    let (status, list) = status_and_body(
        &s.app,
        request("GET", LIST_ACCESS_TOKENS.path, None, Some(&s.cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let secret = &token[13..];
    assert!(!list.to_string().contains(secret), "{list}");
    assert!(
        list.as_array()
            .expect("list")
            .iter()
            .all(|t| t.get("token").is_none())
    );
    assert_eq!(list[0]["id"], id, "newest first");

    let (status, me) = get_with_token(&s.app, GET_ME.path, &token).await;
    assert_eq!(
        (status, &me["user"]["login"]),
        (StatusCode::OK, &json!("lklimek"))
    );
}

/// Bad requests answer 400, a name in use 409 `conflict`, the eleventh live token 409 `token_limit_reached`.
#[tokio::test]
async fn token_limit_per_user() {
    let s = setup().await;
    for bad in [
        json!({"name": "", "scopes": ["read"], "expires_in_days": 90}),
        json!({"name": "x".repeat(61), "scopes": ["read"], "expires_in_days": 90}),
        json!({"name": "a", "scopes": [], "expires_in_days": 90}),
        json!({"name": "a", "scopes": ["read", "read"], "expires_in_days": 90}),
        json!({"name": "a", "scopes": ["write"], "expires_in_days": 90}),
        json!({"name": "a", "scopes": ["review_loop_execute"], "expires_in_days": 90}),
        json!({"name": "a", "scopes": ["read", "review_loop_execute", "read"], "expires_in_days": 90}),
        json!({"name": "a", "scopes": ["read"], "expires_in_days": 7}),
        json!({"name": "a", "scopes": ["read"]}),
    ] {
        let (status, error) = create_token(&s, bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        assert_eq!(error["code"], "invalid_request", "{bad}");
    }
    let (status, _) = create_token(&s, create_body(&"é".repeat(60))).await;
    assert_eq!(status, StatusCode::CREATED, "60 characters");
    let agent =
        json!({"name": "agent", "scopes": ["read", "review_loop_execute"], "expires_in_days": 90});
    let (status, created) = create_token(&s, agent).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(
        created["access_token"]["scopes"],
        json!(["read", "review_loop_execute"])
    );
    for days in [30, 365] {
        let body = json!({"name": format!("d{days}"), "scopes": ["read"], "expires_in_days": days});
        assert_eq!(create_token(&s, body).await.0, StatusCode::CREATED);
    }
    let (status, error) = create_token(&s, create_body("d30")).await;
    assert_eq!(
        (status, &error["code"]),
        (StatusCode::CONFLICT, &json!("conflict"))
    );
    // setup's token + 4 = 5 live; five more reach the limit.
    for n in 0..5 {
        assert_eq!(
            create_token(&s, create_body(&format!("n{n}"))).await.0,
            StatusCode::CREATED
        );
    }
    let (status, error) = create_token(&s, create_body("eleventh")).await;
    assert_eq!(
        (status, &error["code"]),
        (StatusCode::CONFLICT, &json!("token_limit_reached"))
    );
}

#[tokio::test]
async fn revoke_ends_the_token_and_is_scoped_to_its_owner() {
    let s = setup().await;
    let other_cookie = other_user_cookie(&s.state).await;
    let (status, created) = create_token(&s, create_body("short-lived")).await;
    assert_eq!(status, StatusCode::CREATED);
    let token = created["token"].as_str().expect("token");
    let path = REVOKE_ACCESS_TOKEN
        .path
        .replace("{id}", created["access_token"]["id"].as_str().expect("id"));
    for (uri, cookie) in [
        (path.as_str(), other_cookie.as_str()),
        ("/api/v1/settings/access-tokens/unknown1", s.cookie.as_str()),
        (
            "/api/v1/settings/access-tokens/not-an-id",
            s.cookie.as_str(),
        ),
    ] {
        let (status, error) =
            status_and_body(&s.app, request("DELETE", uri, None, Some(cookie))).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(error["code"], "not_found", "{uri}");
    }
    assert_eq!(
        get_with_token(&s.app, GET_ME.path, token).await.0,
        StatusCode::OK
    );
    let (status, _) =
        status_and_body(&s.app, request("DELETE", &path, None, Some(&s.cookie))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = get_with_token(&s.app, GET_ME.path, token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) =
        status_and_body(&s.app, request("DELETE", &path, None, Some(&s.cookie))).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "already revoked");
}

#[tokio::test]
async fn insufficient_scope_is_403() {
    let s = setup().await;
    let (scopeless, _) = stored_token(
        &s.state.db,
        "u1",
        Vec::new(),
        Timestamp::now() + SignedDuration::from_hours(24),
    )
    .await;
    let uri = uri(&GET_REVIEW_STATE, s.analysis);
    let (status, error) = get_with_token(&s.app, &uri, &scopeless).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error["code"], "insufficient_scope");
    let (status, _) = get_with_token(&s.app, GET_ME.path, &scopeless).await;
    assert_eq!(status, StatusCode::OK, "naming the user needs no scope");
}

/// A token reads its own user's private data only (questions, review state, feedback).
#[tokio::test]
async fn token_reads_as_its_user_only() {
    let s = setup().await;
    other_user_cookie(&s.state).await;
    let (token_u2, _) = read_token(&s.state, "u2").await;
    for user in ["u1", "u2"] {
        s.state
            .db
            .questions()
            .create(
                NewQuestion {
                    id: QuestionId::new_random(),
                    analysis_id: s.analysis,
                    anchor: contracts::api::questions::AskAnchor::Step {
                        step_id: StepId("f1.1".to_owned()),
                        lines: None,
                    },
                    parent_id: None,
                    user_id: user.to_owned(),
                    text: format!("PRIVATE_{user}"),
                },
                QuestionLimits {
                    per_analysis: 10,
                    per_user: 10,
                    user_window_start: Timestamp::now(),
                },
                Timestamp::now(),
            )
            .await
            .expect("question");
    }
    s.state
        .db
        .review_states()
        .put(
            "u1",
            s.analysis,
            &contracts::api::review::ReviewState {
                ticked_steps: vec![StepId("f1.1".to_owned())],
                ..Default::default()
            },
            Timestamp::now(),
        )
        .await
        .expect("state");
    for (token, own, other) in [(&s.token, "u1", "u2"), (&token_u2, "u2", "u1")] {
        let (status, questions) =
            get_with_token(&s.app, &uri(&LIST_QUESTIONS, s.analysis), token).await;
        assert_eq!(status, StatusCode::OK, "{questions}");
        let text = questions.to_string();
        assert!(text.contains(&format!("PRIVATE_{own}")), "{text}");
        assert!(!text.contains(&format!("PRIVATE_{other}")), "{text}");
        let (_, state) = get_with_token(&s.app, &uri(&GET_REVIEW_STATE, s.analysis), token).await;
        let ticked = state["ticked_steps"].as_array().expect("ticked").len();
        assert_eq!(ticked, usize::from(own == "u1"), "{own}: {state}");
    }
    let (_, me) = get_with_token(&s.app, GET_ME.path, &token_u2).await;
    assert_eq!(me["user"]["login"], "colleague");
}

fn assert_unauthenticated(response: &Response, case: &str) {
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{case}");
    assert_eq!(
        response
            .headers()
            .get(WWW_AUTHENTICATE)
            .map(|v| v.as_bytes()),
        Some(&b"Bearer"[..]),
        "{case}"
    );
}

#[tokio::test]
async fn revoked_expired_or_unlisted_user_token_is_rejected() {
    let s = setup().await;
    let (expired, _) = stored_token(
        &s.state.db,
        "u1",
        vec![TokenScope::Read],
        Timestamp::now() - SignedDuration::from_secs(1),
    )
    .await;
    let (revoked, revoked_principal) = read_token(&s.state, "u1").await;
    s.state
        .db
        .access_tokens()
        .revoke(
            DEFAULT_TENANT,
            "u1",
            &revoked_principal.token_id,
            Timestamp::now(),
        )
        .await
        .expect("revoke");
    let wrong_secret = format!("{}{}", &s.token[..13], "A".repeat(43));
    let unknown_id = format!("srp_Zz9yX8wV{}", &s.token[12..]);
    let runner_secret = {
        use secrecy::ExposeSecret;
        s.state.config.runner_secret.expose_secret().to_owned()
    };
    let uri = uri(&GET_REVIEW_STATE, s.analysis);
    for (case, header) in [
        ("expired", format!("Bearer {expired}")),
        ("revoked", format!("Bearer {revoked}")),
        ("wrong secret", format!("Bearer {wrong_secret}")),
        ("unknown id", format!("Bearer {unknown_id}")),
        ("malformed", "Bearer srp_short".to_owned()),
        ("lower-case scheme", format!("bearer {wrong_secret}")),
    ] {
        let request = Request::get(&uri)
            .header(AUTHORIZATION, header)
            .body(Body::empty())
            .expect("request");
        let response = send(&s.app, request).await;
        assert_unauthenticated(&response, case);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let error: Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(
            error,
            json!({"code": "unauthenticated", "message": "The access token is not valid."}),
            "{case}: no reason given"
        );
    }
    // Not a personal access token: the session check answers, without a bearer challenge.
    for (case, header) in [
        ("runner secret", format!("Bearer {runner_secret}")),
        ("empty", "Bearer ".to_owned()),
        ("other scheme", format!("Basic {}", s.token)),
    ] {
        let request = Request::get(&uri)
            .header(AUTHORIZATION, header)
            .body(Body::empty())
            .expect("request");
        let response = send(&s.app, request).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{case}");
        assert!(response.headers().get(WWW_AUTHENTICATE).is_none(), "{case}");
    }
    let twice = Request::get(&uri)
        .header(AUTHORIZATION, format!("Bearer {}", s.token))
        .header(AUTHORIZATION, format!("Bearer {}", s.token))
        .body(Body::empty())
        .expect("request");
    assert_unauthenticated(&send(&s.app, twice).await, "two Authorization headers");
    // A user removed from the allow-list loses its tokens on the next request.
    assert_eq!(
        get_with_token(&s.app, &uri, &s.token).await.0,
        StatusCode::OK
    );
    remove_login(&s.state.db, DEFAULT_TENANT, "lklimek")
        .await
        .expect("remove");
    let response = send(&s.app, request("GET", &uri, Some(&s.token), None)).await;
    assert_unauthenticated(&response, "unlisted user");
}

#[tokio::test]
async fn token_and_cookie_together_are_rejected() {
    let s = setup().await;
    let uri = uri(&GET_REVIEW_STATE, s.analysis);
    for cookie in [s.cookie.as_str(), "sr_session=whatever"] {
        let request = Request::get(&uri)
            .header(AUTHORIZATION, format!("Bearer {}", s.token))
            .header(COOKIE, cookie)
            .body(Body::empty())
            .expect("request");
        let (status, error) = status_and_body(&s.app, request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{cookie}");
        assert_eq!(error["code"], "invalid_request");
    }
}

/// RFC 6750 §2.3 query tokens are refused, so a token never ends up in a URL; the header is the only way.
#[tokio::test]
async fn token_in_the_query_string_is_rejected() {
    let s = setup().await;
    let uri = format!(
        "{}?access_token={}",
        uri(&GET_REVIEW_STATE, s.analysis),
        s.token
    );
    for cookie in [None, Some(s.cookie.as_str())] {
        let mut builder = Request::get(&uri);
        if let Some(cookie) = cookie {
            builder = builder.header(COOKIE, cookie);
        }
        let (status, error) =
            status_and_body(&s.app, builder.body(Body::empty()).expect("request")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["code"], "invalid_request");
    }
}

/// One audit line per token request with the route template; never the token, its hash or the query string.
#[tokio::test]
async fn token_not_logged() {
    let s = setup().await;
    let logs = LogCapture::start();
    let (status, created) = create_token(&s, create_body("logged")).await;
    assert_eq!(status, StatusCode::CREATED);
    let minted = created["token"].as_str().expect("token").to_owned();
    let lines = format!(
        "/api/v1/analyses/{}/lines?path=src/query-canary.rs&side=head&from=1&to=5",
        s.analysis
    );
    for token in [&s.token, &minted] {
        send(&s.app, request("GET", &lines, Some(token), None)).await;
        send(
            &s.app,
            request(
                "POST",
                contracts::routes::RESOLVE_PR.path,
                Some(token),
                None,
            ),
        )
        .await;
    }
    let wrong = format!("{}{}", &s.token[..13], "B".repeat(43));
    send(&s.app, request("GET", &lines, Some(&wrong), None)).await;
    let text = logs.text();
    for secret in [&s.token, &minted, &wrong] {
        assert!(!text.contains(&secret[13..]), "secret logged: {text}");
        let hash = sha256(secret.as_bytes());
        let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
        assert!(!text.contains(&hex), "hash logged: {text}");
    }
    assert!(
        !text.contains("query-canary"),
        "query string logged: {text}"
    );
    assert!(
        !text.to_ascii_lowercase().contains("authorization"),
        "{text}"
    );
    let audit: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("access token request"))
        .collect();
    assert_eq!(audit.len(), 5, "{text}");
    assert!(audit.iter().all(|line| line.contains("audit")), "{text}");
    assert!(
        audit[0].contains("route=/api/v1/analyses/{id}/lines")
            && audit[0].contains("status=")
            && audit[0].contains("user_id=")
            && audit[0].contains("u1"),
        "{}",
        audit[0]
    );
    assert!(text.contains("access token created"), "{text}");
}

#[tokio::test]
async fn runner_listener_rejects_personal_tokens() {
    let s = setup().await;
    let response = api::runner_router(s.state.clone())
        .oneshot(
            Request::post(RUNNER_CLAIM.path)
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {}", s.token))
                .body(Body::from("{}"))
                .expect("request"),
        )
        .await
        .expect("infallible");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn per_token_limit_answers_429_with_retry_after() {
    let s = setup().await;
    for n in 0..REQUESTS_PER_TOKEN_PER_MINUTE {
        let (status, _) = get_with_token(&s.app, GET_ME.path, &s.token).await;
        assert_eq!(status, StatusCode::OK, "{n}");
    }
    let response = send(&s.app, request("GET", GET_ME.path, Some(&s.token), None)).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry: u64 = response.headers()[RETRY_AFTER]
        .to_str()
        .expect("ascii")
        .parse()
        .expect("seconds");
    assert!((1..=60).contains(&retry), "{retry}");
    let (other, _) = read_token(&s.state, "u1").await;
    let (status, _) = get_with_token(&s.app, GET_ME.path, &other).await;
    assert_eq!(status, StatusCode::OK, "per token");
}

fn from(address: &str, token: &str) -> Request<Body> {
    let mut request = request("GET", GET_ME.path, Some(token), None);
    request
        .extensions_mut()
        .insert(ConnectInfo(address.parse::<SocketAddr>().expect("address")));
    request
}

/// Only failures are counted and throttled: past 30 failures a minute an address gets 429 for further failures,
/// while a valid token from the same address (or with no known address at all) is still admitted.
#[tokio::test]
async fn failed_attempts_are_throttled_but_never_block_a_valid_token() {
    let s = setup().await;
    let wrong = format!("{}{}", &s.token[..13], "C".repeat(43));
    for address in ["192.0.2.7:4000", "192.0.2.7:4001"] {
        for n in 0..FAILED_ATTEMPTS_PER_CLIENT_PER_MINUTE / 2 {
            let status = send(&s.app, from(address, &wrong)).await.status();
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{address} {n}");
        }
    }
    let (status, error) = status_and_body(&s.app, from("192.0.2.7:4002", &wrong)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error["code"], "rate_limited");
    let status = send(&s.app, from("192.0.2.7:4003", &s.token))
        .await
        .status();
    assert_eq!(
        status,
        StatusCode::OK,
        "a valid token from the throttled address"
    );
    let status = send(&s.app, from("192.0.2.8:4000", &wrong)).await.status();
    assert_eq!(status, StatusCode::UNAUTHORIZED, "another address");
    // No address known (every client shares one key): failures there never lock out a valid token either.
    for _ in 0..=FAILED_ATTEMPTS_PER_CLIENT_PER_MINUTE {
        send(&s.app, request("GET", GET_ME.path, Some(&wrong), None)).await;
    }
    let (status, _) = get_with_token(&s.app, GET_ME.path, &s.token).await;
    assert_eq!(status, StatusCode::OK);
}

/// An IPv6 client counts as its /64: rotating the interface id does not reset the failure limit.
#[tokio::test]
async fn ipv6_failures_count_per_64_prefix() {
    let s = setup().await;
    let wrong = format!("{}{}", &s.token[..13], "D".repeat(43));
    for n in 0..FAILED_ATTEMPTS_PER_CLIENT_PER_MINUTE {
        let address = format!("[2001:db8:1:2::{:x}]:443", n + 1);
        let status = send(&s.app, from(&address, &wrong)).await.status();
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{address}");
    }
    let status = send(&s.app, from("[2001:db8:1:2:ffff::1]:443", &wrong))
        .await
        .status();
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let status = send(&s.app, from("[2001:db8:1:3::1]:443", &wrong))
        .await
        .status();
    assert_eq!(status, StatusCode::UNAUTHORIZED, "another /64");
}

/// Only `Authorization: Bearer srp_…` is a token attempt: an authenticating proxy's own credential passes to the
/// session check untouched, with or without a session cookie.
#[tokio::test]
async fn other_authorization_headers_pass_to_the_session() {
    let s = setup().await;
    let uri = uri(&GET_REVIEW_STATE, s.analysis);
    for header in [
        "Basic dXNlcjpwYXNz",
        "Bearer eyJhbGciOiJSUzI1NiJ9.proxy-id-token",
    ] {
        let with_cookie = Request::get(&uri)
            .header(AUTHORIZATION, header)
            .header(COOKIE, &s.cookie)
            .body(Body::empty())
            .expect("request");
        assert_eq!(
            send(&s.app, with_cookie).await.status(),
            StatusCode::OK,
            "{header}"
        );
        let mutation = Request::put(contracts::routes::SET_LANGUAGE.path)
            .header("content-type", "application/json")
            .header(AUTHORIZATION, header)
            .header(COOKIE, &s.cookie)
            .header(CSRF_HEADER, CSRF)
            .body(Body::from(json!({ "language": "pl" }).to_string()))
            .expect("request");
        assert_eq!(
            send(&s.app, mutation).await.status(),
            StatusCode::NO_CONTENT,
            "{header}"
        );
        let alone = Request::get(&uri)
            .header(AUTHORIZATION, header)
            .body(Body::empty())
            .expect("request");
        let (status, error) = status_and_body(&s.app, alone).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{header}");
        assert_eq!(error["code"], "unauthenticated", "{header}");
    }
}

/// GitHub-backed reads (here App installations in Settings) are read from GitHub at most once a minute per user
/// for access tokens; a browser session still reads every time.
#[tokio::test]
async fn token_reads_backed_by_github_are_cached_per_user() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let github = MockServer::start().await;
    let dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::tests::test_config();
    config.data_dir = dir.path().to_owned();
    config.github.api_url = url::Url::parse(&format!("{}/api", github.uri())).expect("url");
    let state = AppState::with_queue(config, db::repo::Db::in_memory().await.expect("db"));
    seed_analysis(&state.db).await.expect("analysis");
    let cookie = session_cookie(&state, DEFAULT_TENANT).await;
    let lock = crate::auth::lock_user("u1").await;
    crate::auth::store_sign_in_tokens(
        &state,
        &lock,
        DEFAULT_TENANT,
        &crate::github::oauth::TokenGrant {
            access_token: secrecy::SecretString::from("ghu_cache_test"),
            expires_in: 28_800,
            refresh_token: secrecy::SecretString::from("ghr_cache_test"),
            refresh_token_expires_in: 15_638_400,
        },
        Timestamp::now(),
    )
    .await
    .expect("pair");
    drop(lock);
    Mock::given(method("GET"))
        .and(path("/api/user/installations"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"total_count": 0, "installations": []})),
        )
        .expect(4)
        .mount(&github)
        .await;
    let app = api::router(state.clone());
    let (token, _) = read_token(&state, "u1").await;
    for _ in 0..3 {
        let (status, settings) =
            get_with_token(&app, contracts::routes::GET_SETTINGS.path, &token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(settings["app_installations"], json!([]));
    }
    for _ in 0..3 {
        let request = request(
            "GET",
            contracts::routes::GET_SETTINGS.path,
            None,
            Some(&cookie),
        );
        assert_eq!(send(&app, request).await.status(), StatusCode::OK);
    }
    github.verify().await;
}

#[test]
fn limiter_counts_per_key_and_window() {
    let limiter = Limiter::new(2);
    let start = Instant::now();
    assert_eq!(limiter.hit("a", "x", start), Ok(()));
    assert_eq!(limiter.hit("a", "x", start), Ok(()));
    assert_eq!(limiter.hit("a", "x", start), Err(60));
    assert_eq!(
        limiter.hit("a", "x", start + Duration::from_secs(45)),
        Err(15)
    );
    assert_eq!(limiter.hit("b", "x", start), Ok(()));
    assert_eq!(limiter.hit("a", "x", start + WINDOW), Ok(()));
}

#[test]
fn limiter_memory_is_bounded() {
    let limiter = Limiter::new(1);
    let start = Instant::now();
    for n in 0..MAX_TRACKED {
        assert_eq!(limiter.hit(Some(n), None, start), Ok(()));
    }
    // New keys share the overflow counter while every window is open, so they are still limited.
    assert_eq!(limiter.hit(Some(MAX_TRACKED), None, start), Ok(()));
    assert!(limiter.hit(Some(MAX_TRACKED + 1), None, start).is_err());
    assert!(
        limiter.hit(Some(MAX_TRACKED + 2), None, start).is_err(),
        "every further new key is limited"
    );
    let len = limiter.table.lock().expect("lock").windows.len();
    assert_eq!(len, MAX_TRACKED + 1);
    // Ended windows make room again.
    assert_eq!(limiter.hit(Some(usize::MAX), None, start + WINDOW), Ok(()));
    assert_eq!(limiter.table.lock().expect("lock").windows.len(), 1);
}

#[test]
fn client_address_trusts_forwarded_for_only_from_a_trusted_proxy() {
    let mut config = crate::config::tests::test_config();
    config.trusted_proxies = vec!["172.29.73.1/32".parse().expect("net")];
    let parts = |peer: Option<&str>, forwarded: Option<&str>| {
        let mut builder = Request::get("/");
        if let Some(forwarded) = forwarded {
            builder = builder.header("x-forwarded-for", forwarded);
        }
        let (mut parts, ()) = builder.body(()).expect("request").into_parts();
        if let Some(peer) = peer {
            parts
                .extensions
                .insert(ConnectInfo(peer.parse::<SocketAddr>().expect("peer")));
        }
        parts
    };
    let ip = |text: &str| Some(text.parse::<IpAddr>().expect("ip"));
    let cases = [
        (Some("192.0.2.1:1"), Some("198.51.100.9"), ip("192.0.2.1")),
        (
            Some("172.29.73.1:1"),
            Some("203.0.113.5, 198.51.100.9"),
            ip("198.51.100.9"),
        ),
        (Some("172.29.73.1:1"), Some("not-an-ip"), ip("172.29.73.1")),
        (Some("172.29.73.1:1"), None, ip("172.29.73.1")),
        (Some("[::ffff:192.0.2.1]:1"), None, ip("192.0.2.1")),
        (Some("[2001:db8:1:2:3:4:5:6]:1"), None, ip("2001:db8:1:2::")),
        (
            Some("172.29.73.1:1"),
            Some("2001:db8:9:9::7"),
            ip("2001:db8:9:9::"),
        ),
        (None, Some("198.51.100.9"), None),
    ];
    for (peer, forwarded, expected) in cases {
        assert_eq!(
            client_address(&parts(peer, forwarded), &config),
            expected,
            "{peer:?} {forwarded:?}"
        );
    }
}

/// The use is recorded once a minute at most, and R8 shows it.
#[tokio::test]
async fn last_use_is_recorded_and_listed() {
    let s = setup().await;
    let (status, created) = create_token(&s, create_body("used")).await;
    assert_eq!(status, StatusCode::CREATED);
    let token = created["token"].as_str().expect("token");
    let id = AccessTokenId(
        created["access_token"]["id"]
            .as_str()
            .expect("id")
            .to_owned(),
    );
    let last_used = || async {
        s.state
            .db
            .access_tokens()
            .credential(&id)
            .await
            .expect("read")
            .expect("stored")
            .record
            .last_used_at
    };
    assert_eq!(last_used().await, None);
    get_with_token(&s.app, GET_ME.path, token).await;
    let first = last_used().await.expect("recorded");
    get_with_token(&s.app, GET_ME.path, token).await;
    assert_eq!(last_used().await, Some(first), "not again within a minute");
    let (_, list) = status_and_body(
        &s.app,
        request("GET", LIST_ACCESS_TOKENS.path, None, Some(&s.cookie)),
    )
    .await;
    let listed = list
        .as_array()
        .expect("list")
        .iter()
        .find(|token| token["id"] == json!(id.0))
        .expect("listed");
    assert!(listed["last_used_at"].is_string(), "{listed}");
}

/// "Sign out everywhere" revokes every token of the user; an ordinary sign-out keeps them.
#[tokio::test]
async fn sign_out_everywhere_revokes_the_users_tokens() {
    let s = setup().await;
    let (other, _) = read_token(&s.state, "u1").await;
    let logout = |everywhere: bool| {
        Request::post(contracts::routes::AUTH_LOGOUT.path)
            .header("content-type", "application/json")
            .header(COOKIE, &s.cookie)
            .header(CSRF_HEADER, CSRF)
            .body(Body::from(json!({ "everywhere": everywhere }).to_string()))
            .expect("request")
    };
    assert_eq!(
        send(&s.app, logout(false)).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        get_with_token(&s.app, GET_ME.path, &s.token).await.0,
        StatusCode::OK
    );
    let cookie = session_cookie(&s.state, DEFAULT_TENANT).await;
    assert_eq!(cookie, s.cookie);
    assert_eq!(
        send(&s.app, logout(true)).await.status(),
        StatusCode::NO_CONTENT
    );
    for token in [&s.token, &other] {
        let response = send(&s.app, request("GET", GET_ME.path, Some(token), None)).await;
        assert_unauthenticated(&response, "after sign-out everywhere");
    }
}

/// The SSE heartbeat re-check: a token session stays live until the token is revoked or expires.
#[tokio::test]
async fn token_session_liveness_follows_revocation() {
    let s = setup().await;
    let (_, principal) = read_token(&s.state, "u1").await;
    let session = Session::of_token(&principal, crate::auth::BrowserTransport { https: true });
    assert_eq!(session.csrf_token(), None);
    assert!(session.is_live(&s.state).await.expect("check"));
    s.state
        .db
        .access_tokens()
        .revoke(DEFAULT_TENANT, "u1", &principal.token_id, Timestamp::now())
        .await
        .expect("revoke");
    assert!(!session.is_live(&s.state).await.expect("check"));
    let (_, expired) = stored_token(
        &s.state.db,
        "u1",
        vec![TokenScope::Read],
        Timestamp::now() - SignedDuration::from_secs(1),
    )
    .await;
    let session = Session::of_token(&expired, crate::auth::BrowserTransport { https: true });
    assert!(!session.is_live(&s.state).await.expect("check"));
}

mod sec_review;
