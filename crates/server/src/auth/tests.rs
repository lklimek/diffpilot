//! Sign-in, session, CSRF and token tests through the public router against a wiremock GitHub.

use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::header::{COOKIE, LOCATION, SET_COOKIE};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use contracts::routes::{HttpMethod, Listener, ROUTES, RouteAuth};
use contracts::time::Timestamp;
use db::repo::DEFAULT_TENANT;
use db::repo::Db;
use db::repo::sessions::NewSession;
use db::repo::tenants::testing::{allow_login, remove_login};
use db::repo::testing::{file_backed_with_busy_timeout, hold_write_lock};
use db::repo::tokens::testing::{allow_token_writes, fail_token_writes};
use db::repo::users::GithubProfile;
use http_body_util::BodyExt;
use jiff::SignedDuration;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use tower::ServiceExt;
use url::Url;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{CSRF_HEADER, UserTokenError, lock_user, store_sign_in_tokens, user_access_token};
use crate::api;
use crate::config::tests::test_config;
use crate::github::oauth::TokenGrant;
use crate::secrets::{Aad, TokenCipher, sha256};
use crate::state::AppState;
use crate::test_support::logs::LogCapture;

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
    harness_on(
        Db::in_memory().await.expect("db"),
        public_url,
        trusted_proxies,
    )
    .await
}

async fn harness_on(db: Db, public_url: &str, trusted_proxies: &[&str]) -> Harness {
    let github = MockServer::start().await;
    let mut config = test_config();
    config.public_url = Url::parse(public_url).expect("public url");
    config.trusted_proxies = trusted_proxies
        .iter()
        .map(|net| net.parse().expect("network"))
        .collect();
    config.github.oauth_url = Url::parse(&github.uri()).expect("oauth url");
    config.github.api_url = Url::parse(&format!("{}/api", github.uri())).expect("api url");
    Harness {
        github,
        state: AppState::with_queue(config, db),
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
async fn allowlist_binds_github_user_id() {
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
async fn oauth_state_single_use_and_bound_to_browser() {
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
    let now = Timestamp::now();
    h.state
        .db
        .sessions()
        .create_oauth_login(
            expired_state,
            DEFAULT_TENANT,
            b"ct",
            now - SignedDuration::from_mins(20),
            now - SignedDuration::from_mins(10),
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
        .take_oauth_login(&started.state, Timestamp::now())
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
            Timestamp::now(),
            Timestamp::now() + SignedDuration::from_mins(5),
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

/// Redirects and the OAuth callback come from `SR_PUBLIC_URL`, never from request headers.
#[tokio::test]
async fn callback_url_ignores_host_headers() {
    let h = harness().await;
    h.mock_github_user("lklimek", 1001).await;
    let mut request = get("/auth/github/login");
    for (name, value) in [
        ("host", "evil.example"),
        ("x-forwarded-host", "evil.example"),
        ("x-forwarded-proto", "http"),
        ("forwarded", "host=evil.example;proto=http"),
    ] {
        request
            .headers_mut()
            .insert(name, value.parse().expect("header"));
    }
    let response = h.send(request).await;
    let authorize = Url::parse(&location(&response)).expect("authorize URL");
    let redirect_uri = authorize
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .map(|(_, value)| value.into_owned());
    assert_eq!(redirect_uri.as_deref(), Some(CALLBACK));
    let cookie = cookie_pair(&response, "__Host-sr_oauth_state").expect("state cookie");
    let state = authorize
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .expect("state");
    let mut callback = get_with_cookie(
        &format!("/auth/github/callback?code={CODE}&state={state}"),
        &cookie,
    );
    callback
        .headers_mut()
        .insert("host", "evil.example".parse().expect("header"));
    assert_eq!(
        location(&h.send(callback).await),
        "/",
        "relative to the page's own origin"
    );
}

/// Local development over plain HTTP works only by configuring an `http://` public URL; cookies then lack Secure.
#[tokio::test]
async fn localhost_development_by_explicit_public_url() {
    let h = harness_with("http://localhost:3000", &[]).await;
    h.mock_github_user("lklimek", 1001).await;
    let response = h.send(get("/auth/github/login")).await;
    let authorize = Url::parse(&location(&response)).expect("authorize URL");
    let query: std::collections::HashMap<String, String> =
        authorize.query_pairs().into_owned().collect();
    assert_eq!(
        query["redirect_uri"],
        "http://localhost:3000/auth/github/callback"
    );
    let cookie = cookie_pair(&response, "sr_oauth_state").expect("plain state cookie");
    let response = h
        .send(get_with_cookie(
            &format!("/auth/github/callback?code={CODE}&state={}", query["state"]),
            &cookie,
        ))
        .await;
    assert_eq!(location(&response), "/");
    let session = set_cookie(&response, "sr_session").expect("session cookie");
    assert!(
        !session.contains("Secure") && session.contains("HttpOnly"),
        "{session}"
    );
    let cookie = cookie_pair(&response, "sr_session").expect("session");
    assert_eq!(me_login(&h, &cookie).await.as_deref(), Some("lklimek"));
}

#[test]
fn plain_http_public_url_warns_unless_local() {
    let capture = LogCapture::start();
    for (url, warns) in [
        ("https://review.example.test", false),
        ("http://localhost:3000", false),
        ("http://127.0.0.1:3000", false),
        ("http://[::1]:3000", false),
        ("http://review.lan:3000", true),
        ("http://192.168.1.5:3000", true),
    ] {
        let mut config = test_config();
        config.public_url = Url::parse(url).expect("url");
        assert_eq!(super::warn_if_public_url_insecure(&config), warns, "{url}");
    }
    assert!(
        capture.text().contains("SR_PUBLIC_URL is plain http"),
        "{}",
        capture.text()
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

/// `path` with every `{param}` replaced by a well-formed id.
fn with_placeholder_ids(path: &str) -> String {
    let mut uri = String::new();
    let mut rest = path;
    while let Some((head, tail)) = rest.split_once('{') {
        uri.push_str(head);
        uri.push_str("00000000-0000-4000-8000-000000000000");
        rest = tail.split_once('}').map_or("", |(_, after)| after);
    }
    uri + rest
}

/// Every state-changing public route of the generated route table rejects a missing or wrong CSRF token before it
/// acts, so a route added later cannot skip the check (threat model T7a). A route the router does not serve yet
/// (bare 404, no API error body) cannot change state and is skipped.
#[tokio::test]
async fn csrf_required_on_mutations() {
    let h = harness().await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    let mut checked = 0;
    for route in ROUTES
        .iter()
        .filter(|route| route.listener == Listener::Public && route.method != HttpMethod::Get)
    {
        assert_eq!(route.auth, RouteAuth::SessionCsrf, "{}", route.path);
        let uri = with_placeholder_ids(route.path);
        for csrf in [None, Some("wrong-token"), Some("")] {
            let mut request = Request::builder()
                .method(route.method.as_str())
                .uri(&uri)
                .header(COOKIE, &signed_in.cookie)
                .header("content-type", "application/json");
            if let Some(csrf) = csrf {
                request = request.header(CSRF_HEADER, csrf);
            }
            let response = h
                .send(request.body(Body::from("{}")).expect("request"))
                .await;
            let status = response.status();
            let body = response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes();
            if status == StatusCode::NOT_FOUND && body.is_empty() {
                continue;
            }
            let case = format!("{} {uri} with {csrf:?}: {body:?}", route.method.as_str());
            assert_eq!(status, StatusCode::FORBIDDEN, "{case}");
            let error: Value = serde_json::from_slice(&body).expect("API error");
            assert_eq!(error["code"], "csrf_rejected", "{case}");
            checked += 1;
        }
    }
    assert!(
        checked >= 3 * 10,
        "too few served mutations checked: {checked}"
    );
    assert_eq!(
        me_login(&h, &signed_in.cookie).await.as_deref(),
        Some("lklimek"),
        "rejected requests changed nothing"
    );
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
            .live(token.as_bytes(), super::liveness(Timestamp::now()))
            .await
            .expect("lookup"),
        None,
        "the raw token is not stored"
    );
    let stored = sessions
        .live(&sha256(token.as_bytes()), super::liveness(Timestamp::now()))
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

/// `/api/v1/me` login for this `Cookie` header, or `None` when the request is signed out.
async fn me_login(h: &Harness, cookie: &str) -> Option<String> {
    json_body(h.send(get_with_cookie("/api/v1/me", cookie)).await).await["user"]["login"]
        .as_str()
        .map(str::to_owned)
}

/// A session of `user_id` created `created_ago`, last seen then, with a 7-day expiry; returns its cookie.
async fn backdated_session(
    h: &Harness,
    user_id: &str,
    name: &str,
    created_ago: SignedDuration,
) -> String {
    let created = Timestamp::now() - created_ago;
    let token = format!("{name}-session-token");
    h.state
        .db
        .sessions()
        .create(
            &NewSession {
                token_hash: &sha256(token.as_bytes()),
                tenant_id: DEFAULT_TENANT,
                user_id,
                csrf_token: "csrf",
                expires_at: created + super::SESSION_TTL,
            },
            created,
        )
        .await
        .expect("session");
    format!("__Host-sr_session={token}")
}

#[tokio::test]
async fn idle_session_ends() {
    let h = harness().await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    let recent = backdated_session(
        &h,
        &signed_in.user_id,
        "recent",
        SignedDuration::from_hours(23),
    )
    .await;
    let idle = backdated_session(
        &h,
        &signed_in.user_id,
        "idle",
        SignedDuration::from_hours(25),
    )
    .await;
    assert_eq!(me_login(&h, &recent).await.as_deref(), Some("lklimek"));
    assert_eq!(
        me_login(&h, &recent).await.as_deref(),
        Some("lklimek"),
        "each use restarts the idle window"
    );
    assert_eq!(me_login(&h, &idle).await, None, "idle past the timeout");
}

/// Another connection holds the write lock past the busy timeout: a session validated by its read still serves
/// the request, and the failed record of its use is logged without the session token.
#[tokio::test]
async fn busy_database_keeps_a_validated_session_signed_in() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = file_backed_with_busy_timeout(dir.path(), Duration::from_millis(100))
        .await
        .expect("db");
    let h = harness_on(db, "https://review.example.test", &[]).await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    let stale = backdated_session(
        &h,
        &signed_in.user_id,
        "stale",
        SignedDuration::from_mins(5),
    )
    .await;
    let capture = LogCapture::start();
    let lock = hold_write_lock(&h.state.db).await.expect("write lock");
    assert_eq!(
        me_login(&h, &signed_in.cookie).await.as_deref(),
        Some("lklimek"),
        "a session seen moments ago is not written again"
    );
    assert_eq!(
        me_login(&h, &stale).await.as_deref(),
        Some("lklimek"),
        "a failed record of the use does not fail the request"
    );
    drop(lock);
    let logs = capture.text();
    assert!(
        logs.contains("WARN") && logs.contains("sqlite_code=Some(\"5\")"),
        "{logs}"
    );
    assert!(!logs.contains("stale-session-token"), "{logs}");
    let before = last_seen(&h, "stale").await;
    assert!(Timestamp::now() - before > super::SESSION_TOUCH_INTERVAL);
    assert!(me_login(&h, &stale).await.is_some());
    let recorded = last_seen(&h, "stale").await;
    assert!(
        Timestamp::now() - recorded < super::SESSION_TOUCH_INTERVAL,
        "the next use records it"
    );
    assert!(me_login(&h, &stale).await.is_some());
    assert_eq!(
        last_seen(&h, "stale").await,
        recorded,
        "a use within the interval is not written"
    );
}

/// The recorded last use of the session `backdated_session` created under `name`.
async fn last_seen(h: &Harness, name: &str) -> Timestamp {
    h.state
        .db
        .sessions()
        .live(
            &sha256(format!("{name}-session-token").as_bytes()),
            super::liveness(Timestamp::now()),
        )
        .await
        .expect("read")
        .expect("live session")
        .last_seen_at
}

#[tokio::test]
async fn removed_login_loses_access_on_next_request() {
    let h = harness().await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    let settings = || Request::get("/api/v1/settings").header(COOKIE, &signed_in.cookie);
    assert_eq!(
        me_login(&h, &signed_in.cookie).await.as_deref(),
        Some("lklimek")
    );

    remove_login(&h.state.db, DEFAULT_TENANT, "lklimek")
        .await
        .expect("remove");
    assert_eq!(me_login(&h, &signed_in.cookie).await, None);
    let response = h
        .send(settings().body(Body::empty()).expect("request"))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = h
        .send(logout_request(
            Some(&signed_in.cookie),
            Some(&signed_in.csrf),
            false,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    allow_login(&h.state.db, DEFAULT_TENANT, "lklimek", 2002)
        .await
        .expect("entry bound to another account");
    assert_eq!(me_login(&h, &signed_in.cookie).await, None);
}

#[tokio::test]
async fn session_is_bound_to_its_tenant() {
    let h = harness().await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    db::repo::analyses::testing::seed_tenant(&h.state.db, "other")
        .await
        .expect("tenant");
    let now = Timestamp::now();
    h.state
        .db
        .sessions()
        .create(
            &NewSession {
                token_hash: &sha256(b"other-tenant-token"),
                tenant_id: "other",
                user_id: &signed_in.user_id,
                csrf_token: "csrf",
                expires_at: now + SignedDuration::from_hours(1),
            },
            now,
        )
        .await
        .expect("session");
    assert_eq!(
        me_login(&h, "__Host-sr_session=other-tenant-token").await,
        None,
        "a session in a tenant that does not allow its user is not accepted"
    );
}

#[tokio::test]
async fn logout_keeps_tokens_until_the_last_session_ends() {
    let h = harness().await;
    let first = h.sign_in("lklimek", 1001).await;
    let second = h.sign_in("lklimek", 1001).await;
    let stored = || async { h.state.db.user_tokens().get(&first.user_id).await };
    let response = h
        .send(logout_request(
            Some(&first.cookie),
            Some(&first.csrf),
            false,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        stored().await.expect("lookup").is_some(),
        "the other session still uses the GitHub tokens"
    );
    let response = h
        .send(logout_request(
            Some(&second.cookie),
            Some(&second.csrf),
            false,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        stored().await.expect("lookup"),
        None,
        "the last session takes the GitHub tokens with it"
    );
}

/// ADR-020: with the dashboard schedule on, signing out of the last session keeps the GitHub tokens for the
/// scheduled refresh; signing out everywhere deletes them.
#[tokio::test]
async fn last_sign_out_keeps_tokens_while_the_schedule_runs() {
    let h = harness().await;
    let first = h.sign_in("lklimek", 1001).await;
    let settings = contracts::api::dashboard::DashboardSettings {
        time_zone: Some("Europe/Warsaw".to_owned()),
        ..Default::default()
    };
    h.state
        .db
        .dashboard()
        .save_settings(DEFAULT_TENANT, &first.user_id, &settings, Timestamp::now())
        .await
        .expect("schedule on");
    let stored = || async { h.state.db.user_tokens().get(&first.user_id).await };
    let response = h
        .send(logout_request(
            Some(&first.cookie),
            Some(&first.csrf),
            false,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(me_login(&h, &first.cookie).await, None, "signed out");
    assert!(
        stored().await.expect("lookup").is_some(),
        "the schedule keeps the GitHub tokens"
    );
    let again = h.sign_in("lklimek", 1001).await;
    let response = h
        .send(logout_request(Some(&again.cookie), Some(&again.csrf), true))
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        stored().await.expect("lookup"),
        None,
        "signing out everywhere deletes them"
    );
}

/// A sign-in whose token write fails leaves no session behind, so logging out the only delivered session still
/// deletes the pair.
#[tokio::test]
async fn failed_token_write_leaves_no_session() {
    let h = harness().await;
    let delivered = h.sign_in("lklimek", 1001).await;
    fail_token_writes(&h.state.db)
        .await
        .expect("inject failure");
    let response = h.callback(&h.start_login().await).await;
    assert_eq!(location(&response), "/?login_error=oauth_failed");
    assert_eq!(cookie_pair(&response, "__Host-sr_session"), None);
    allow_token_writes(&h.state.db)
        .await
        .expect("remove failure");

    let response = h
        .send(logout_request(
            Some(&delivered.cookie),
            Some(&delivered.csrf),
            false,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        h.state
            .db
            .user_tokens()
            .get(&delivered.user_id)
            .await
            .expect("lookup"),
        None,
        "the pair ends with the last delivered session"
    );
    assert_eq!(
        h.state
            .db
            .sessions()
            .delete_for_user(&delivered.user_id)
            .await
            .expect("count"),
        0,
        "no undelivered session left"
    );
}

#[tokio::test]
async fn token_ciphertext_bound_to_row() {
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
    store_sign_in_tokens(&h.state, &lock, DEFAULT_TENANT, &grant, Timestamp::now())
        .await
        .expect("store pair");
}

/// GitHub's grant lifetimes are numbers from the network: one past the representable range is handled, not a panic.
#[tokio::test]
async fn a_grant_lifetime_out_of_range_does_not_panic() {
    let h = harness().await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    let state = h.state.clone();
    let outcome = tokio::spawn(async move {
        let lock = lock_user(&signed_in.user_id).await;
        // About 12,700 years: the chrono-based stack stored it; it lies past jiff's year 9999.
        let far = grant("ghu_far", 400_000_000_000, "ghr_far");
        store_sign_in_tokens(&state, &lock, DEFAULT_TENANT, &far, Timestamp::now())
            .await
            .is_ok()
    })
    .await;
    assert!(
        matches!(outcome, Ok(false)),
        "an out-of-range grant lifetime must be refused, not panic: {outcome:?}"
    );
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
    assert!(after.tokens.access_expires_at > Timestamp::now() + SignedDuration::from_hours(7));
}

/// GitHub rotates the pair as soon as it answers; a request dropped meanwhile (browser gone) must still store it.
#[tokio::test]
async fn refresh_survives_a_cancelled_request() {
    let h = harness().await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    store_pair(
        &h,
        &signed_in.user_id,
        grant("ghu_expiring", 60, "ghr_current"),
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .and(body_string_contains("refresh_token=ghr_current"))
        .respond_with(
            token_response("ghu_rotated", "ghr_rotated").set_delay(Duration::from_millis(300)),
        )
        .expect(1)
        .mount(&h.github)
        .await;
    let cancelled = tokio::time::timeout(
        Duration::from_millis(100),
        user_access_token(&h.state, &signed_in.user_id),
    )
    .await;
    assert!(
        cancelled.is_err(),
        "the caller gave up before GitHub answered"
    );
    tokio::time::sleep(Duration::from_millis(700)).await;
    let stored = h
        .state
        .db
        .user_tokens()
        .get(&signed_in.user_id)
        .await
        .expect("lookup")
        .expect("pair kept");
    assert_eq!(
        stored_access(&h, &stored, &signed_in.user_id),
        "ghu_rotated"
    );
    assert_eq!(
        user_access_token(&h.state, &signed_in.user_id)
            .await
            .expect("token")
            .expose_secret(),
        "ghu_rotated",
        "no second refresh"
    );
}

/// GitHub rotates the pair while another connection holds the write lock past the busy timeout: the new pair is
/// used at once, never rotated again, and stored once the lock is released.
#[tokio::test]
async fn refresh_keeps_the_rotated_pair_while_the_database_is_busy() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = file_backed_with_busy_timeout(dir.path(), Duration::from_millis(100))
        .await
        .expect("db");
    let h = harness_on(db, "https://review.example.test", &[]).await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    store_pair(
        &h,
        &signed_in.user_id,
        grant("ghu_expiring", 60, "ghr_current"),
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .and(body_string_contains("refresh_token=ghr_current"))
        .respond_with(token_response("ghu_rotated", "ghr_rotated"))
        .expect(1)
        .mount(&h.github)
        .await;
    let capture = LogCapture::start();
    let lock = hold_write_lock(&h.state.db).await.expect("write lock");
    for attempt in ["rotated", "kept in memory"] {
        let access = user_access_token(&h.state, &signed_in.user_id)
            .await
            .unwrap_or_else(|error| panic!("{attempt}: {error:?}"));
        assert_eq!(access.expose_secret(), "ghu_rotated", "{attempt}");
    }
    drop(lock);
    let stored = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let stored = h
                .state
                .db
                .user_tokens()
                .get(&signed_in.user_id)
                .await
                .expect("lookup")
                .expect("pair");
            if stored_access(&h, &stored, &signed_in.user_id) == "ghu_rotated" {
                break stored;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the rotated pair is stored after the lock is released");
    let cipher = TokenCipher::new(&h.state.config.secret_key);
    let refresh = cipher
        .decrypt(
            Aad {
                table: "user_tokens",
                column: "refresh_ct",
                row_id: &signed_in.user_id,
            },
            &stored.tokens.refresh_ct,
        )
        .expect("refresh");
    assert_eq!(refresh.expose_secret(), "ghr_rotated");
    assert_eq!(
        user_access_token(&h.state, &signed_in.user_id)
            .await
            .expect("token")
            .expose_secret(),
        "ghu_rotated"
    );
    let logs = capture.text();
    for secret in ["ghu_rotated", "ghr_rotated", "ghr_current"] {
        assert!(!logs.contains(secret), "{logs}");
    }
}

/// A pair that no longer decrypts (`SR_SECRET_KEY` replaced, ADR-001) ends the sign-in instead of failing forever.
#[tokio::test]
async fn unreadable_tokens_ask_to_sign_in_again() {
    let h = harness().await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    let mut config = test_config();
    config.github.oauth_url = h.state.config.github.oauth_url.clone();
    config.github.api_url = h.state.config.github.api_url.clone();
    config.secret_key = secrecy::SecretBox::new(Box::new([9u8; 32]));
    let rekeyed = AppState::with_queue(config, h.state.db.clone());
    let result = user_access_token(&rekeyed, &signed_in.user_id).await;
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
    assert_eq!(me_login(&h, &signed_in.cookie).await, None, "session ended");
}

/// GitHub answers `/user/installations` with 401 for `rejected` and with no installations for `accepted`.
async fn mock_installations(h: &Harness, rejected: &str, accepted: &str) {
    for (token, response) in [
        (rejected, ResponseTemplate::new(401)),
        (
            accepted,
            ResponseTemplate::new(200)
                .set_body_json(json!({"total_count": 0, "installations": []})),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path("/api/user/installations"))
            .and(header("authorization", format!("Bearer {token}").as_str()))
            .respond_with(response)
            .mount(&h.github)
            .await;
    }
}

async fn settings_installations(h: &Harness, cookie: &str) -> (StatusCode, Value) {
    let response = h.send(get_with_cookie("/api/v1/settings", cookie)).await;
    let status = response.status();
    (
        status,
        json_body(response).await["app_installations"].clone(),
    )
}

/// A GitHub 401 for a token the server still holds as fresh marks it expired: the next use refreshes the pair.
#[tokio::test]
async fn github_401_refreshes_the_pair_on_next_use() {
    let h = harness().await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    h.github.reset().await;
    mock_installations(&h, ACCESS, "ghu_after_401").await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .and(body_string_contains(format!("refresh_token={REFRESH}")))
        .respond_with(token_response("ghu_after_401", "ghr_after_401"))
        .expect(1)
        .mount(&h.github)
        .await;
    assert_eq!(
        settings_installations(&h, &signed_in.cookie).await,
        (StatusCode::OK, Value::Null),
        "not checked"
    );
    assert_eq!(
        settings_installations(&h, &signed_in.cookie).await,
        (StatusCode::OK, json!([])),
        "read with the refreshed token"
    );
}

/// A GitHub 401 while reading a pull request for an analysis marks the held token expired.
#[tokio::test]
async fn github_401_on_the_pull_request_marks_the_token_expired() {
    let h = harness().await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    h.github.reset().await;
    Mock::given(method("GET"))
        .and(path("/api/repos/acme/node-client/pulls/482"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&h.github)
        .await;
    let request = Request::post("/api/v1/pr/resolve")
        .header(COOKIE, signed_in.cookie.as_str())
        .header(CSRF_HEADER, signed_in.csrf.as_str())
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "url": "https://github.com/acme/node-client/pull/482" }).to_string(),
        ))
        .expect("request");
    let response = h.send(request).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let stored = h
        .state
        .db
        .user_tokens()
        .get(&signed_in.user_id)
        .await
        .expect("lookup")
        .expect("pair kept for the refresh");
    assert!(
        stored.tokens.access_expires_at <= Timestamp::now(),
        "the rejected access token is refreshed on next use"
    );
}

/// After a GitHub 401, a refresh GitHub rejects ends the sign-in: the user is asked to sign in again.
#[tokio::test]
async fn github_401_then_rejected_refresh_asks_to_sign_in_again() {
    let h = harness().await;
    let signed_in = h.sign_in("lklimek", 1001).await;
    h.github.reset().await;
    mock_installations(&h, ACCESS, "unused").await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"error": "bad_refresh_token"})),
        )
        .expect(1)
        .mount(&h.github)
        .await;
    assert_eq!(
        settings_installations(&h, &signed_in.cookie).await,
        (StatusCode::OK, Value::Null)
    );
    assert_eq!(
        settings_installations(&h, &signed_in.cookie).await,
        (StatusCode::OK, Value::Null)
    );
    let response = h
        .send(get_with_cookie("/api/v1/settings", &signed_in.cookie))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json_body(response).await["message"], "Sign in to continue.");
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
    let now = Timestamp::now();
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
    allow_login(&h.state.db, DEFAULT_TENANT, "Claudius-Maginificent", 77)
        .await
        .expect("allowlist");
    h.state
        .db
        .sessions()
        .create(
            &NewSession {
                token_hash: &sha256(b"orphan-session-token"),
                tenant_id: DEFAULT_TENANT,
                user_id: "u-orphan",
                csrf_token: "csrf",
                expires_at: now + SignedDuration::from_hours(1),
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

/// The PKCE verifiers GitHub received in code exchanges.
async fn exchanged_verifiers(h: &Harness) -> Vec<String> {
    h.github
        .received_requests()
        .await
        .expect("recording")
        .into_iter()
        .filter_map(|request| {
            url::form_urlencoded::parse(&request.body)
                .find(|(key, _)| key == "code_verifier")
                .map(|(_, value)| value.into_owned())
        })
        .collect()
}

/// Sign-in → refresh → 401 → rejected refresh → logout, captured at TRACE ([`LogCapture`]): no secret in the logs,
/// error bodies or redirects (threat model T7a, T7b `no_secret_in_logs`).
#[tokio::test]
async fn no_secret_in_logs_over_sign_in_refresh_logout() {
    let capture = LogCapture::start();
    let h = harness().await;
    let key = h.state.config.secret_key.expose_secret().to_vec();
    let mut exposed = vec![
        h.state
            .config
            .github
            .client_secret
            .expose_secret()
            .to_owned(),
        base64::engine::general_purpose::STANDARD.encode(&key),
        URL_SAFE_NO_PAD.encode(&key),
        key.iter().map(|byte| format!("{byte:02x}")).collect(),
        format!("{key:?}"),
        ACCESS.to_owned(),
        REFRESH.to_owned(),
        CODE.to_owned(),
    ];
    let mut bodies = Vec::new();

    // Sign-in (twice, so one session can end alone); the OAuth state, PKCE verifier and session ids are secret.
    h.github.reset().await;
    h.mock_github_user("lklimek", 1001).await;
    let mut sessions = Vec::new();
    for _ in 0..2 {
        let started = h.start_login().await;
        exposed.push(started.state.clone());
        let response = h.callback(&started).await;
        bodies.push(location(&response));
        let cookie = cookie_pair(&response, "__Host-sr_session").expect("session cookie");
        let me = json_body(h.send(get_with_cookie("/api/v1/me", &cookie)).await).await;
        let csrf = me["csrf_token"].as_str().expect("csrf").to_owned();
        exposed.push(cookie.split_once('=').expect("cookie").1.to_owned());
        exposed.push(csrf.clone());
        sessions.push((cookie, csrf));
    }
    exposed.extend(exchanged_verifiers(&h).await);
    let user_id = h
        .state
        .db
        .users()
        .find_by_github_id(1001)
        .await
        .expect("lookup")
        .expect("user")
        .id;

    // Refused login and failed exchange.
    h.github.reset().await;
    h.mock_github_user("mallory", 666).await;
    bodies.push(location(&h.callback(&h.start_login().await).await));
    h.github.reset().await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"error": "bad_verification_code"})),
        )
        .mount(&h.github)
        .await;
    bodies.push(location(&h.callback(&h.start_login().await).await));

    // Refresh, then a GitHub 401 for the rotated token and a rejected refresh.
    h.github.reset().await;
    store_pair(
        &h,
        &user_id,
        grant("ghu_expiring_canary", 0, "ghr_current_canary"),
    )
    .await;
    let canaries = [
        "ghu_expiring_canary",
        "ghr_current_canary",
        "ghu_rotated_canary",
        "ghr_rotated_canary",
    ];
    exposed.extend(canaries.map(str::to_owned));
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .and(body_string_contains("refresh_token=ghr_current_canary"))
        .respond_with(token_response("ghu_rotated_canary", "ghr_rotated_canary"))
        .mount(&h.github)
        .await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .and(body_string_contains("refresh_token=ghr_rotated_canary"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"error": "bad_refresh_token"})),
        )
        .mount(&h.github)
        .await;
    mock_installations(&h, "ghu_rotated_canary", "unused").await;
    let (first_cookie, first_csrf) = &sessions[0];
    for _ in 0..2 {
        let response = h
            .send(get_with_cookie("/api/v1/settings", first_cookie))
            .await;
        bodies.push(json_body(response).await.to_string());
    }
    let signed_out = h
        .send(get_with_cookie("/api/v1/settings", first_cookie))
        .await;
    assert_eq!(
        signed_out.status(),
        StatusCode::UNAUTHORIZED,
        "the rejected refresh ended the sign-in"
    );
    bodies.push(json_body(signed_out).await.to_string());

    // Sign in again, rejected and accepted logouts.
    h.github.reset().await;
    h.mock_github_user("lklimek", 1001).await;
    let again = h.sign_in("lklimek", 1001).await;
    let other = h.sign_in("lklimek", 1001).await;
    exposed.extend(exchanged_verifiers(&h).await);
    for signed_in in [&again, &other] {
        exposed.push(
            signed_in
                .cookie
                .split_once('=')
                .expect("cookie")
                .1
                .to_owned(),
        );
        exposed.push(signed_in.csrf.clone());
    }
    for (cookie, csrf) in [
        (first_cookie.as_str(), first_csrf.as_str()),
        (&again.cookie, "wrong"),
    ] {
        let response = h
            .send(logout_request(Some(cookie), Some(csrf), false))
            .await;
        bodies.push(json_body(response).await.to_string());
    }
    for (signed_in, everywhere) in [(&again, false), (&other, true)] {
        let response = h
            .send(logout_request(
                Some(&signed_in.cookie),
                Some(&signed_in.csrf),
                everywhere,
            ))
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    let logs = capture.text();
    for expected in [
        "user signed in",
        "GitHub user token refreshed",
        "ending the sign-in",
        "user signed out",
    ] {
        assert!(logs.contains(expected), "{expected} not logged: {logs}");
    }
    for secret in &exposed {
        assert!(
            secret.len() >= 8,
            "canary too short to be meaningful: {secret}"
        );
        assert!(!logs.contains(secret.as_str()), "secret in logs: {secret}");
        for body in &bodies {
            assert!(!body.contains(secret.as_str()), "secret in {body}");
        }
    }
}
