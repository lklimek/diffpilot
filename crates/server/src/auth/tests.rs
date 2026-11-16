//! Sign-in, session, CSRF and token tests through the public router against a wiremock GitHub.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::header::{COOKIE, LOCATION, SET_COOKIE};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{TimeDelta, Utc};
use db::repo::DEFAULT_TENANT;
use db::repo::Db;
use db::repo::sessions::NewSession;
use db::repo::users::GithubProfile;
use http_body_util::BodyExt;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use tower::ServiceExt;
use tracing_subscriber::fmt::MakeWriter;
use url::Url;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{CSRF_HEADER, UserTokenError, lock_user, store_sign_in_tokens, user_access_token};
use crate::api;
use crate::config::tests::test_config;
use crate::github::oauth::TokenGrant;
use crate::jobs::queue::QueueExecutor;
use crate::secrets::{Aad, TokenCipher, sha256};
use crate::state::AppState;

const ACCESS: &str = "ghu_test_access_canary";
const REFRESH: &str = "ghr_test_refresh_canary";
const CODE: &str = "test-code-canary";
const CALLBACK: &str = "https://review.example.test/auth/github/callback";

struct Harness {
    github: MockServer,
    state: AppState,
}

struct StartedLogin {
    state: String,
    cookie: String,
    location: Url,
}

struct SignedIn {
    cookie: String,
    csrf: String,
    user_id: String,
}

async fn harness_with(public_url: &str, trusted_proxies: &[&str]) -> Harness {
    let github = MockServer::start().await;
    let mut config = test_config();
    config.public_url = Url::parse(public_url).expect("public url");
    config.trusted_proxies = trusted_proxies
        .iter()
        .map(|net| net.parse().expect("network"))
        .collect();
    config.github.oauth_url = Url::parse(&github.uri()).expect("oauth url");
    config.github.api_url = Url::parse(&format!("{}/api", github.uri())).expect("api url");
    let db = Db::in_memory().await.expect("db");
    let executor = Arc::new(QueueExecutor::new(db.clone()));
    Harness {
        github,
        state: AppState::new(config, db, executor),
    }
}

async fn harness() -> Harness {
    harness_with("https://review.example.test", &[]).await
}

fn token_response(access: &str, refresh: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "access_token": access,
        "expires_in": 28800,
        "refresh_token": refresh,
        "refresh_token_expires_in": 15_638_400,
        "token_type": "bearer",
        "scope": ""
    }))
}

fn set_cookies(response: &Response) -> Vec<String> {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|value| value.to_str().expect("ascii").to_owned())
        .collect()
}

/// The `Set-Cookie` line for `name`.
fn set_cookie(response: &Response, name: &str) -> Option<String> {
    set_cookies(response)
        .into_iter()
        .find(|line| line.starts_with(&format!("{name}=")))
}

/// `name=value` of a cookie set with a non-empty value, ready for a `Cookie` header.
fn cookie_pair(response: &Response, name: &str) -> Option<String> {
    set_cookie(response, name)
        .and_then(|line| line.split(';').next().map(str::to_owned))
        .filter(|pair| pair.len() > name.len() + 1)
}

fn location(response: &Response) -> String {
    response
        .headers()
        .get(LOCATION)
        .expect("Location")
        .to_str()
        .expect("ascii")
        .to_owned()
}

async fn json_body(response: Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("JSON body")
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).expect("request")
}

fn get_with_cookie(uri: &str, cookie: &str) -> Request<Body> {
    Request::get(uri)
        .header(COOKIE, cookie)
        .body(Body::empty())
        .expect("request")
}

fn logout_request(cookie: Option<&str>, csrf: Option<&str>, everywhere: bool) -> Request<Body> {
    let mut builder = Request::post("/auth/logout").header("content-type", "application/json");
    if let Some(cookie) = cookie {
        builder = builder.header(COOKIE, cookie);
    }
    if let Some(csrf) = csrf {
        builder = builder.header(CSRF_HEADER, csrf);
    }
    builder
        .body(Body::from(json!({ "everywhere": everywhere }).to_string()))
        .expect("request")
}

impl Harness {
    async fn send(&self, request: Request<Body>) -> Response {
        api::router(self.state.clone())
            .oneshot(request)
            .await
            .expect("infallible")
    }

    /// GitHub answers the code exchange and `/user` for this login.
    async fn mock_github_user(&self, login: &str, id: i64) {
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .and(body_string_contains(format!("code={CODE}")))
            .respond_with(token_response(ACCESS, REFRESH))
            .mount(&self.github)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/user"))
            .and(header("authorization", format!("Bearer {ACCESS}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": id,
                "login": login,
                "name": "Test User",
                "avatar_url": "https://avatars.example.test/u"
            })))
            .mount(&self.github)
            .await;
    }

    async fn start_login(&self) -> StartedLogin {
        let response = self.send(get("/auth/github/login")).await;
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = Url::parse(&location(&response)).expect("authorize URL");
        let state = location
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .expect("state parameter");
        let cookie = cookie_pair(&response, "__Host-sr_oauth_state").expect("state cookie");
        StartedLogin {
            state,
            cookie,
            location,
        }
    }

    async fn callback(&self, started: &StartedLogin) -> Response {
        self.send(get_with_cookie(
            &format!("/auth/github/callback?code={CODE}&state={}", started.state),
            &started.cookie,
        ))
        .await
    }

    async fn sign_in(&self, login: &str, id: i64) -> SignedIn {
        self.github.reset().await;
        self.mock_github_user(login, id).await;
        let started = self.start_login().await;
        let response = self.callback(&started).await;
        assert_eq!(location(&response), "/", "sign-in must succeed");
        let cookie = cookie_pair(&response, "__Host-sr_session").expect("session cookie");
        let me = json_body(self.send(get_with_cookie("/api/v1/me", &cookie)).await).await;
        let user = self
            .state
            .db
            .users()
            .find_by_github_id(id)
            .await
            .expect("lookup")
            .expect("user stored");
        SignedIn {
            cookie,
            csrf: me["csrf_token"].as_str().expect("csrf token").to_owned(),
            user_id: user.id,
        }
    }
}

#[tokio::test]
async fn login_against_wiremock_github() {
    let h = harness().await;
    h.mock_github_user("lklimek", 1001).await;
    let started = h.start_login().await;
    let query: std::collections::HashMap<String, String> =
        started.location.query_pairs().into_owned().collect();
    assert_eq!(
        started.location.as_str().split('?').next(),
        Some(format!("{}/login/oauth/authorize", h.github.uri()).as_str())
    );
    assert_eq!(query["client_id"], "Iv1.client");
    assert_eq!(query["redirect_uri"], CALLBACK);
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["state"].len(), 43);

    let response = h.callback(&started).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(location(&response), "/");
    let session = set_cookie(&response, "__Host-sr_session").expect("session cookie");
    for attribute in ["Path=/", "HttpOnly", "SameSite=Lax", "Secure"] {
        assert!(
            session.contains(attribute),
            "{attribute} missing: {session}"
        );
    }
    let cleared = set_cookie(&response, "__Host-sr_oauth_state").expect("state cookie cleared");
    assert!(cleared.contains("Max-Age=0"), "{cleared}");

    let cookie = cookie_pair(&response, "__Host-sr_session").expect("session cookie");
    let me = h.send(get_with_cookie("/api/v1/me", &cookie)).await;
    assert_eq!(me.status(), StatusCode::OK);
    assert_eq!(
        me.headers().get("cache-control").map(|v| v.as_bytes()),
        Some(&b"no-store"[..])
    );
    let me = json_body(me).await;
    assert_eq!(me["user"]["login"], "lklimek");
    assert_eq!(me["user"]["name"], "Test User");
    assert_eq!(me["user"]["language"], "en");
    assert_eq!(me["csrf_token"].as_str().map(str::len), Some(43));
}

#[tokio::test]
async fn me_signed_out_returns_nulls() {
    let h = harness().await;
    for request in [
        get("/api/v1/me"),
        get_with_cookie("/api/v1/me", "__Host-sr_session=unknown-session-token"),
    ] {
        let response = h.send(request).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json_body(response).await,
            json!({"user": null, "csrf_token": null})
        );
    }
}

#[tokio::test]
async fn allowlist_rejects_unlisted_login() {
    let h = harness().await;
    h.mock_github_user("mallory", 666).await;
    let started = h.start_login().await;
    let response = h.callback(&started).await;
    assert_eq!(location(&response), "/?login_error=login_not_allowed");
    assert_eq!(cookie_pair(&response, "__Host-sr_session"), None);
    assert_eq!(
        h.state
            .db
            .users()
            .find_by_github_id(666)
            .await
            .expect("lookup"),
        None,
        "no user, token or session is stored for a refused login"
    );
}

#[tokio::test]
async fn login_bound_to_github_id() {
    let h = harness().await;
    let first = h.sign_in("lklimek", 1001).await;
    assert!(!first.user_id.is_empty());

    h.github.reset().await;
    h.mock_github_user("lklimek", 2002).await;
    let response = h.callback(&h.start_login().await).await;
    assert_eq!(
        location(&response),
        "/?login_error=login_not_allowed",
        "same login, other GitHub id"
    );
    assert_eq!(
        h.state
            .db
            .users()
            .find_by_github_id(2002)
            .await
            .expect("lookup"),
        None
    );

    let again = h.sign_in("LKLIMEK", 1001).await;
    assert_eq!(again.user_id, first.user_id, "bound id signs in again");
}

#[tokio::test]
async fn state_mismatch_rejected() {
    let h = harness().await;
    h.mock_github_user("lklimek", 1001).await;
    let callback = |query: String, cookie: Option<String>| {
        let mut builder = Request::get(format!("/auth/github/callback?{query}"));
        if let Some(cookie) = cookie {
            builder = builder.header(COOKIE, cookie);
        }
        builder.body(Body::empty()).expect("request")
    };

    let started = h.start_login().await;
    let other = h.start_login().await;
    let expired_state = "expired-state-value";
    let now = Utc::now();
    h.state
        .db
        .sessions()
        .create_oauth_login(
            expired_state,
            DEFAULT_TENANT,
            b"ct",
            now - TimeDelta::minutes(20),
            now - TimeDelta::minutes(10),
        )
        .await
        .expect("expired login");
    let cases = [
        (
            "no state cookie",
            format!("code={CODE}&state={}", started.state),
            None,
        ),
        (
            "cookie of another login",
            format!("code={CODE}&state={}", started.state),
            Some(other.cookie.clone()),
        ),
        (
            "unknown state",
            format!("code={CODE}&state=unknown"),
            Some("__Host-sr_oauth_state=unknown".to_owned()),
        ),
        (
            "expired state",
            format!("code={CODE}&state={expired_state}"),
            Some(format!("__Host-sr_oauth_state={expired_state}")),
        ),
        (
            "no state parameter",
            format!("code={CODE}"),
            Some(started.cookie.clone()),
        ),
    ];
    for (case, query, cookie) in cases {
        let response = h.send(callback(query, cookie)).await;
        assert_eq!(
            location(&response),
            "/?login_error=state_mismatch",
            "{case}"
        );
        assert_eq!(cookie_pair(&response, "__Host-sr_session"), None, "{case}");
    }

    let response = h.callback(&started).await;
    assert_eq!(location(&response), "/", "the login itself is still usable");
    let replay = h.callback(&started).await;
    assert_eq!(
        location(&replay),
        "/?login_error=state_mismatch",
        "state is single use"
    );
    let exchanges = h
        .github
        .received_requests()
        .await
        .expect("recording")
        .into_iter()
        .filter(|request| request.url.path() == "/login/oauth/access_token")
        .count();
    assert_eq!(exchanges, 1, "a rejected state never reaches GitHub");
}

#[tokio::test]
async fn pkce_verifier_required() {
    let h = harness().await;
    h.mock_github_user("lklimek", 1001).await;
    let started = h.start_login().await;
    let challenge = started
        .location
        .query_pairs()
        .find(|(key, _)| key == "code_challenge")
        .map(|(_, value)| value.into_owned())
        .expect("challenge");
    assert_eq!(h.callback(&started).await.status(), StatusCode::FOUND);

    let exchange = h
        .github
        .received_requests()
        .await
        .expect("recording")
        .into_iter()
        .find(|request| request.url.path() == "/login/oauth/access_token")
        .expect("code exchange");
    let form: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(&exchange.body)
            .into_owned()
            .collect();
    let verifier = &form["code_verifier"];
    assert_eq!(
        URL_SAFE_NO_PAD.encode(sha256(verifier.as_bytes())),
        challenge
    );
    assert_eq!(form["code"], CODE);
    assert_eq!(form["redirect_uri"], CALLBACK);
    assert_eq!(form["client_id"], "Iv1.client");

    // The verifier is stored encrypted and bound to its state.
    let started = h.start_login().await;
    let pending = h
        .state
        .db
        .sessions()
        .take_oauth_login(&started.state, Utc::now())
        .await
        .expect("take")
        .expect("pending login");
    let cipher = TokenCipher::new(&h.state.config.secret_key);
    let aad = |row_id| Aad {
        table: "oauth_logins",
        column: "code_verifier_ct",
        row_id,
    };
    let stored = cipher
        .decrypt(aad(&started.state), &pending.code_verifier_ct)
        .expect("verifier decrypts under its state");
    assert!(
        !pending
            .code_verifier_ct
            .windows(stored.expose_secret().len())
            .any(|window| window == stored.expose_secret().as_bytes())
    );

    // Without a readable verifier the flow stops before GitHub.
    h.github.reset().await;
    h.state
        .db
        .sessions()
        .create_oauth_login(
            &started.state,
            DEFAULT_TENANT,
            &pending.code_verifier_ct[1..],
            Utc::now(),
            Utc::now() + TimeDelta::minutes(5),
        )
        .await
        .expect("damaged login");
    let response = h.callback(&started).await;
    assert_eq!(location(&response), "/?login_error=oauth_failed");
    assert_eq!(
        h.github.received_requests().await.expect("recording").len(),
        0
    );
}

#[tokio::test]
async fn browser_transport_from_peer_and_forwarded_proto() {
    let h = harness_with("http://review.lan:3000", &["10.0.0.0/8"]).await;
    let login_from = |peer: &str, forwarded_https: bool| {
        let mut request = get("/auth/github/login");
        request
            .extensions_mut()
            .insert(ConnectInfo(peer.parse::<SocketAddr>().expect("peer")));
        if forwarded_https {
            request
                .headers_mut()
                .insert("x-forwarded-proto", "https".parse().expect("header"));
        }
        request
    };
    for (peer, forwarded, secure) in [
        ("10.1.2.3:4000", true, true),
        ("[::ffff:10.1.2.3]:4000", true, true),
        ("10.1.2.3:4000", false, false),
        ("192.168.1.5:4000", true, false),
    ] {
        let response = h.send(login_from(peer, forwarded)).await;
        let cookies = set_cookies(&response);
        let [cookie] = cookies.as_slice() else {
            panic!("one cookie expected: {cookies:?}");
        };
        assert_eq!(
            cookie.contains("; Secure"),
            secure,
            "{peer} {forwarded}: {cookie}"
        );
        assert_eq!(
            cookie.starts_with("__Host-sr_oauth_state="),
            secure,
            "{peer} {forwarded}: {cookie}"
        );
        assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"));
    }

    let h = harness_with("https://review.example.test", &[]).await;
    let response = h.send(get("/auth/github/login")).await;
    assert!(
        set_cookie(&response, "__Host-sr_oauth_state")
            .is_some_and(|cookie| cookie.contains("; Secure")),
        "an https public URL always sets Secure"
    );
}

#[tokio::test]
async fn csrf_required_for_cookie_mutations() {
    let h = harness().await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    let unauthenticated = h
        .send(logout_request(None, Some(&signed_in.csrf), false))
        .await;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json_body(unauthenticated).await["code"], "unauthenticated");

    for csrf in [None, Some("wrong-token"), Some("")] {
        let response = h
            .send(logout_request(Some(&signed_in.cookie), csrf, false))
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{csrf:?}");
        assert_eq!(json_body(response).await["code"], "csrf_rejected");
    }
    let me = json_body(
        h.send(get_with_cookie("/api/v1/me", &signed_in.cookie))
            .await,
    )
    .await;
    assert_eq!(
        me["user"]["login"], "lklimek",
        "rejected requests change nothing"
    );

    let response = h
        .send(logout_request(
            Some(&signed_in.cookie),
            Some(&signed_in.csrf),
            false,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn session_id_stored_hashed() {
    let h = harness().await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    let token = signed_in
        .cookie
        .split_once('=')
        .map(|(_, token)| token.to_owned())
        .expect("token");
    let sessions = h.state.db.sessions();
    assert_eq!(
        sessions
            .touch(token.as_bytes(), Utc::now())
            .await
            .expect("lookup"),
        None,
        "the raw token is not stored"
    );
    let stored = sessions
        .touch(&sha256(token.as_bytes()), Utc::now())
        .await
        .expect("lookup")
        .expect("session found by hash");
    assert_eq!(stored.user_id, signed_in.user_id);
}

#[tokio::test]
async fn logout_clears_session() {
    let h = harness().await;
    let first = h.sign_in("lklimek", 1001).await;
    let second = h.sign_in("lklimek", 1001).await;
    assert_ne!(first.cookie, second.cookie);

    let response = h
        .send(logout_request(
            Some(&first.cookie),
            Some(&first.csrf),
            false,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cleared = set_cookie(&response, "__Host-sr_session").expect("cookie cleared");
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
    let harness = &h;
    let me = |cookie: String| async move {
        json_body(harness.send(get_with_cookie("/api/v1/me", &cookie)).await).await
    };
    assert_eq!(me(first.cookie.clone()).await["user"], Value::Null);
    assert_eq!(me(second.cookie.clone()).await["user"]["login"], "lklimek");

    let third = h.sign_in("lklimek", 1001).await;
    let response = h
        .send(logout_request(Some(&third.cookie), Some(&third.csrf), true))
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        me(second.cookie.clone()).await["user"],
        Value::Null,
        "everywhere"
    );
    assert_eq!(me(third.cookie.clone()).await["user"], Value::Null);
    assert_eq!(
        h.state
            .db
            .user_tokens()
            .get(&third.user_id)
            .await
            .expect("lookup"),
        None,
        "signing out everywhere drops the GitHub tokens"
    );
}

#[tokio::test]
async fn token_row_is_ciphertext() {
    let h = harness().await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    let stored = h
        .state
        .db
        .user_tokens()
        .get(&signed_in.user_id)
        .await
        .expect("lookup")
        .expect("tokens stored");
    for (ciphertext, plain) in [
        (&stored.tokens.access_ct, ACCESS),
        (&stored.tokens.refresh_ct, REFRESH),
    ] {
        assert!(
            !ciphertext
                .windows(plain.len())
                .any(|window| window == plain.as_bytes()),
            "plaintext token in the row"
        );
    }
    let cipher = TokenCipher::new(&h.state.config.secret_key);
    let aad = |column| Aad {
        table: "user_tokens",
        column,
        row_id: &signed_in.user_id,
    };
    assert_eq!(
        cipher
            .decrypt(aad("refresh_ct"), &stored.tokens.refresh_ct)
            .expect("refresh")
            .expose_secret(),
        REFRESH
    );
    assert!(
        cipher
            .decrypt(aad("access_ct"), &stored.tokens.refresh_ct)
            .is_err(),
        "a value moved to another column does not decrypt"
    );
    let access = user_access_token(&h.state, &signed_in.user_id)
        .await
        .expect("fresh token");
    assert_eq!(access.expose_secret(), ACCESS);
}

async fn store_pair(h: &Harness, user_id: &str, grant: TokenGrant) {
    let lock = lock_user(user_id).await;
    store_sign_in_tokens(&h.state, &lock, DEFAULT_TENANT, &grant, Utc::now())
        .await
        .expect("store pair");
}

fn grant(access: &str, expires_in: i64, refresh: &str) -> TokenGrant {
    TokenGrant {
        access_token: SecretString::from(access),
        expires_in,
        refresh_token: SecretString::from(refresh),
        refresh_token_expires_in: 15_638_400,
    }
}

#[tokio::test]
async fn refresh_is_serialised_and_atomic() {
    let h = harness().await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    store_pair(
        &h,
        &signed_in.user_id,
        grant("ghu_expiring", 60, "ghr_current"),
    )
    .await;
    let before = h
        .state
        .db
        .user_tokens()
        .get(&signed_in.user_id)
        .await
        .expect("lookup")
        .expect("stored");
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=ghr_current"))
        .respond_with(
            token_response("ghu_rotated", "ghr_rotated").set_delay(Duration::from_millis(200)),
        )
        .expect(1)
        .mount(&h.github)
        .await;

    let results =
        futures::future::join_all((0..5).map(|_| user_access_token(&h.state, &signed_in.user_id)))
            .await;
    for result in results {
        assert_eq!(result.expect("token").expose_secret(), "ghu_rotated");
    }
    let after = h
        .state
        .db
        .user_tokens()
        .get(&signed_in.user_id)
        .await
        .expect("lookup")
        .expect("stored");
    assert_eq!(
        after.version,
        before.version + 1,
        "exactly one rotation stored"
    );
    let cipher = TokenCipher::new(&h.state.config.secret_key);
    let aad = |column| Aad {
        table: "user_tokens",
        column,
        row_id: &signed_in.user_id,
    };
    assert_eq!(
        cipher
            .decrypt(aad("refresh_ct"), &after.tokens.refresh_ct)
            .expect("refresh")
            .expose_secret(),
        "ghr_rotated",
        "the rotated refresh token replaced the old one together with the access token"
    );
    assert!(after.tokens.access_expires_at > Utc::now() + TimeDelta::hours(7));
}

#[tokio::test]
async fn bad_refresh_token_ends_sign_in() {
    let h = harness().await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    store_pair(
        &h,
        &signed_in.user_id,
        grant("ghu_expired", -10, "ghr_replayed"),
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": "bad_refresh_token",
            "error_description": "The refresh token passed is incorrect or expired."
        })))
        .mount(&h.github)
        .await;
    let result = user_access_token(&h.state, &signed_in.user_id).await;
    assert!(
        matches!(result, Err(UserTokenError::SignInRequired)),
        "{result:?}"
    );
    assert_eq!(
        h.state
            .db
            .user_tokens()
            .get(&signed_in.user_id)
            .await
            .expect("lookup"),
        None
    );
    let me = json_body(
        h.send(get_with_cookie("/api/v1/me", &signed_in.cookie))
            .await,
    )
    .await;
    assert_eq!(
        me["user"],
        Value::Null,
        "the session ended with the sign-in"
    );
}

fn stored_access(h: &Harness, stored: &db::repo::tokens::StoredTokens, user_id: &str) -> String {
    TokenCipher::new(&h.state.config.secret_key)
        .decrypt(
            Aad {
                table: "user_tokens",
                column: "access_ct",
                row_id: user_id,
            },
            &stored.tokens.access_ct,
        )
        .expect("access token")
        .expose_secret()
        .to_owned()
}

#[tokio::test]
async fn late_refresh_error_keeps_newer_sign_in() {
    let h = harness().await;
    let old = h.sign_in("lklimek", 1001).await;
    store_pair(&h, &old.user_id, grant("ghu_old", 60, "ghr_old")).await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .and(body_string_contains("refresh_token=ghr_old"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"error": "bad_refresh_token"}))
                .set_delay(Duration::from_millis(400)),
        )
        .mount(&h.github)
        .await;
    let started = h.start_login().await;

    // The callback of a fresh sign-in arrives while the refresh of the old pair waits for GitHub.
    let (refresh, callback) = tokio::join!(user_access_token(&h.state, &old.user_id), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        h.callback(&started).await
    });
    assert!(
        matches!(refresh, Err(UserTokenError::SignInRequired)),
        "{refresh:?}"
    );
    assert_eq!(location(&callback), "/");
    let cookie = cookie_pair(&callback, "__Host-sr_session").expect("new session");
    let me = json_body(h.send(get_with_cookie("/api/v1/me", &cookie)).await).await;
    assert_eq!(me["user"]["login"], "lklimek", "the newer sign-in survives");
    let stored = h
        .state
        .db
        .user_tokens()
        .get(&old.user_id)
        .await
        .expect("lookup")
        .expect("newer pair kept");
    assert_eq!(stored_access(&h, &stored, &old.user_id), ACCESS);
}

#[tokio::test]
async fn refresh_logout_login_interleaving_keeps_latest_sign_in() {
    let h = harness().await;
    let old = h.sign_in("lklimek", 1001).await;
    store_pair(&h, &old.user_id, grant("ghu_old", 60, "ghr_old")).await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .and(body_string_contains("refresh_token=ghr_old"))
        .respond_with(
            token_response("ghu_rotated", "ghr_rotated").set_delay(Duration::from_millis(500)),
        )
        .mount(&h.github)
        .await;
    let started = h.start_login().await;

    // Refresh waits for GitHub; meanwhile sign-out everywhere, then a new sign-in.
    let (refresh, logout, callback) = tokio::join!(
        user_access_token(&h.state, &old.user_id),
        async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            h.send(logout_request(Some(&old.cookie), Some(&old.csrf), true))
                .await
        },
        async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            h.callback(&started).await
        },
    );
    assert_eq!(refresh.expect("refresh").expose_secret(), "ghu_rotated");
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert_eq!(location(&callback), "/");
    let stored = h
        .state
        .db
        .user_tokens()
        .get(&old.user_id)
        .await
        .expect("lookup")
        .expect("pair of the latest sign-in");
    assert_eq!(
        stored_access(&h, &stored, &old.user_id),
        ACCESS,
        "the stale refresh must not overwrite the pair of the later sign-in"
    );
    let cookie = cookie_pair(&callback, "__Host-sr_session").expect("new session");
    let me = json_body(h.send(get_with_cookie("/api/v1/me", &cookie)).await).await;
    assert_eq!(me["user"]["login"], "lklimek");
    let me = json_body(h.send(get_with_cookie("/api/v1/me", &old.cookie)).await).await;
    assert_eq!(me["user"], Value::Null, "signed out everywhere");
}

#[tokio::test]
async fn missing_token_row_ends_sessions() {
    let h = harness().await;
    let now = Utc::now();
    let profile = GithubProfile {
        github_id: 77,
        login: "Claudius-Maginificent".to_owned(),
        name: None,
        avatar_url: None,
    };
    // Sessions without a token pair, as an interrupted sign-out could leave them.
    h.state
        .db
        .users()
        .upsert_member(DEFAULT_TENANT, "u-orphan", &profile, now)
        .await
        .expect("user");
    h.state
        .db
        .sessions()
        .create(
            &NewSession {
                token_hash: &sha256(b"orphan-session-token"),
                tenant_id: DEFAULT_TENANT,
                user_id: "u-orphan",
                csrf_token: "csrf",
                expires_at: now + TimeDelta::hours(1),
            },
            now,
        )
        .await
        .expect("session");
    let cookie = "__Host-sr_session=orphan-session-token";
    let me = json_body(h.send(get_with_cookie("/api/v1/me", cookie)).await).await;
    assert_eq!(me["user"]["login"], "Claudius-Maginificent", "precondition");

    let result = user_access_token(&h.state, "u-orphan").await;
    assert!(
        matches!(result, Err(UserTokenError::SignInRequired)),
        "{result:?}"
    );
    let me = json_body(h.send(get_with_cookie("/api/v1/me", cookie)).await).await;
    assert_eq!(me["user"], Value::Null, "sessions without tokens end");
}

/// Serves the public router on a real TCP listener the way `main` does (with the peer address).
async fn serve_over_tcp(state: AppState) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("address");
    tokio::spawn(async move {
        axum::serve(
            listener,
            api::router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    });
    addr
}

async fn login_cookie_over_tcp(addr: SocketAddr, forwarded_https: bool) -> String {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client");
    let mut request = client.get(format!("http://{addr}/auth/github/login"));
    if forwarded_https {
        request = request.header("x-forwarded-proto", "https");
    }
    let response = request.send().await.expect("response");
    assert_eq!(response.status(), reqwest::StatusCode::FOUND);
    response
        .headers()
        .get(SET_COOKIE)
        .expect("Set-Cookie")
        .to_str()
        .expect("ascii")
        .to_owned()
}

#[tokio::test]
async fn secure_cookie_behind_trusted_proxy() {
    let h = harness_with("http://review.lan:3000", &["127.0.0.0/8"]).await;
    let addr = serve_over_tcp(h.state.clone()).await;
    let cookie = login_cookie_over_tcp(addr, true).await;
    assert!(
        cookie.starts_with("__Host-sr_oauth_state=") && cookie.contains("; Secure"),
        "{cookie}"
    );
    let cookie = login_cookie_over_tcp(addr, false).await;
    assert!(
        cookie.starts_with("sr_oauth_state=") && !cookie.contains("Secure"),
        "plain HTTP through the proxy: {cookie}"
    );
}

#[tokio::test]
async fn forwarded_headers_ignored_from_untrusted_peer() {
    let h = harness_with("http://review.lan:3000", &["10.0.0.0/8"]).await;
    let addr = serve_over_tcp(h.state.clone()).await;
    let cookie = login_cookie_over_tcp(addr, true).await;
    assert!(
        cookie.starts_with("sr_oauth_state=") && !cookie.contains("Secure"),
        "{cookie}"
    );
}

#[derive(Clone, Copy)]
struct LogBuffer;

/// Every event of this test process at all levels: thread-scoped subscribers miss events in parallel tests
/// because callsite interest is cached process-wide.
static LOGS: Mutex<Vec<u8>> = Mutex::new(Vec::new());

impl std::io::Write for LogBuffer {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        LOGS.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        *self
    }
}

fn captured_logs() -> String {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(LogBuffer)
            .finish();
        tracing::subscriber::set_global_default(subscriber).expect("first global subscriber");
    });
    String::from_utf8_lossy(
        &LOGS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
    .into_owned()
}

#[tokio::test]
async fn no_token_or_secret_in_logs_or_error_bodies() {
    captured_logs();
    let h = harness().await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    let mut exposed = vec![
        h.state
            .config
            .github
            .client_secret
            .expose_secret()
            .to_owned(),
        ACCESS.to_owned(),
        REFRESH.to_owned(),
        signed_in
            .cookie
            .split_once('=')
            .expect("cookie")
            .1
            .to_owned(),
        signed_in.csrf.clone(),
    ];

    // Refused login, failed exchange, refresh, rejected refresh and rejected requests.
    h.github.reset().await;
    h.mock_github_user("mallory", 666).await;
    let mut bodies = vec![location(&h.callback(&h.start_login().await).await)];
    h.github.reset().await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"error": "bad_verification_code"})),
        )
        .mount(&h.github)
        .await;
    bodies.push(location(&h.callback(&h.start_login().await).await));
    h.github.reset().await;
    store_pair(
        &h,
        &signed_in.user_id,
        grant("ghu_expiring_canary", 0, "ghr_current_canary"),
    )
    .await;
    exposed.extend(
        [
            "ghu_expiring_canary",
            "ghr_current_canary",
            "ghu_rotated_canary",
            "ghr_rotated_canary",
        ]
        .map(str::to_owned),
    );
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(token_response("ghu_rotated_canary", "ghr_rotated_canary"))
        .mount(&h.github)
        .await;
    user_access_token(&h.state, &signed_in.user_id)
        .await
        .expect("refreshed");
    let wrong_csrf = h
        .send(logout_request(
            Some(&signed_in.cookie),
            Some("wrong"),
            false,
        ))
        .await;
    bodies.push(json_body(wrong_csrf).await.to_string());
    h.github.reset().await;
    store_pair(
        &h,
        &signed_in.user_id,
        grant("ghu_dead_canary", 0, "ghr_dead_canary"),
    )
    .await;
    exposed.extend(["ghu_dead_canary", "ghr_dead_canary"].map(str::to_owned));
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"error": "bad_refresh_token"})),
        )
        .mount(&h.github)
        .await;
    let error = user_access_token(&h.state, &signed_in.user_id)
        .await
        .expect_err("rejected refresh");
    bodies.push(format!("{error} {error:?}"));

    let logs = captured_logs();
    assert!(
        logs.contains("user signed in"),
        "logs were captured: {logs}"
    );
    for secret in &exposed {
        assert!(!logs.contains(secret.as_str()), "secret in logs: {secret}");
        for body in &bodies {
            assert!(!body.contains(secret.as_str()), "secret in {body}");
        }
    }
}
