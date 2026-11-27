//! The fake's GitHub App endpoints over real TCP: JWT verification like GitHub's and the scope of minted tokens.
#![expect(clippy::unwrap_used, reason = "test helpers outside #[test] functions")]

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der};
use aws_lc_rs::rsa::{KeyPair, KeySize};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use contracts::time::Timestamp;
use fake_github::Config;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::{Value, json};
use url::Url;

const APP_ID: u64 = 1;
const INSTALLATION: &str = "4820001";

/// A throwaway RSA key pair: (PKCS#8 private key PEM, `RSA PUBLIC KEY` PEM).
fn key_pair() -> (String, String) {
    let pair = KeyPair::generate(KeySize::Rsa2048).unwrap();
    let der: Pkcs8V1Der<'static> = pair.as_der().unwrap();
    let public = aws_lc_rs::signature::KeyPair::public_key(&pair)
        .as_ref()
        .to_vec();
    (
        pem("PRIVATE KEY", der.as_ref()),
        pem("RSA PUBLIC KEY", &public),
    )
}

fn pem(label: &str, der: &[u8]) -> String {
    let body = STANDARD.encode(der);
    let lines: Vec<&str> = body
        .as_bytes()
        .chunks(64)
        .map(|chunk| std::str::from_utf8(chunk).unwrap())
        .collect();
    format!(
        "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
        lines.join("\n")
    )
}

/// The App's key (private PEM, public key file) and a second key GitHub does not know.
static KEYS: LazyLock<(String, PathBuf, String, tempfile::TempDir)> = LazyLock::new(|| {
    let dir = tempfile::tempdir().unwrap();
    let (private, public) = key_pair();
    let public_file = dir.path().join("app.pub.pem");
    std::fs::write(&public_file, public).unwrap();
    let (stranger, _) = key_pair();
    (private, public_file, stranger, dir)
});

fn config(verified: bool) -> Config {
    Config {
        fixture_dir: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482/github"),
        app_url: Url::parse("http://app.test:3000").unwrap(),
        user_login: None,
        user_id: None,
        app_id: verified.then_some(APP_ID),
        app_public_key_file: verified.then(|| KEYS.1.clone()),
        dashboard_fixture_dir: None,
        dashboard_world_at: None,
        review_loop_fixture_dir: None,
    }
}

async fn spawn(config: Config) -> String {
    let router = fake_github::router(&config).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}

/// An App JWT signed with `key` (PKCS#8 PEM) carrying `claims`.
fn jwt(key: &str, claims: &Value) -> String {
    jsonwebtoken::encode(
        &Header::new(Algorithm::RS256),
        claims,
        &EncodingKey::from_rsa_pem(key.as_bytes()).unwrap(),
    )
    .unwrap()
}

/// The JWT the server sends: issued 60 s back, valid 9 minutes from then.
fn good_jwt() -> String {
    let iat = Timestamp::now().as_second() - 60;
    jwt(
        &KEYS.0,
        &json!({"iss": APP_ID.to_string(), "iat": iat, "exp": iat + 540}),
    )
}

/// The server's job token request: the pull request's repository only, read-only.
fn read_only() -> Value {
    json!({
        "repositories": ["node-client"],
        "permissions": {"contents": "read", "issues": "read", "metadata": "read", "pull_requests": "read"}
    })
}

async fn send(
    method: reqwest::Method,
    url: String,
    bearer: &str,
    body: Option<&Value>,
) -> reqwest::Response {
    let mut request = reqwest::Client::new()
        .request(method, url)
        .header(ACCEPT, "application/vnd.github+json")
        .header(USER_AGENT, "diffpilot")
        .header(AUTHORIZATION, format!("Bearer {bearer}"));
    if let Some(body) = body {
        request = request.json(body);
    }
    request.send().await.unwrap()
}

async fn mint(base: &str, bearer: &str, body: &Value) -> reqwest::Response {
    send(
        reqwest::Method::POST,
        format!("{base}/app/installations/{INSTALLATION}/access_tokens"),
        bearer,
        Some(body),
    )
    .await
}

async fn lookup(base: &str, bearer: &str) -> reqwest::Response {
    send(
        reqwest::Method::GET,
        format!("{base}/repos/acme/node-client/installation"),
        bearer,
        None,
    )
    .await
}

async fn credentials(base: &str) -> Value {
    reqwest::get(format!("{base}/_fake/app-credentials"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn recording(file: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/pr-482/github")
        .join(file);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[tokio::test]
async fn a_verified_jwt_reads_the_installation_and_mints_unique_read_only_tokens() {
    let base = spawn(config(true)).await;
    let jwt = good_jwt();
    let installation = lookup(&base, &jwt).await;
    assert_eq!(installation.status(), StatusCode::OK);
    let installation: Value = installation.json().await.unwrap();
    assert_eq!(
        installation,
        recording("rest/repo-installation.json")["response"]
    );
    let missing = send(
        reqwest::Method::GET,
        format!("{base}/repos/acme/node-sdk/installation"),
        &jwt,
        None,
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let first = mint(&base, &jwt, &read_only()).await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let first: Value = first.json().await.unwrap();
    let recorded = recording("rest/installation-token.json")["response"].clone();
    assert_eq!(first["permissions"], read_only()["permissions"]);
    assert_eq!(first["repositories"], recorded["repositories"]);
    assert_eq!(first["repository_selection"], "selected");
    let expires: Timestamp = first["expires_at"].as_str().unwrap().parse().unwrap();
    let lifetime = (expires - Timestamp::now()).as_secs();
    assert!(
        (3500..=3600).contains(&lifetime),
        "valid one hour from now: {lifetime} s"
    );
    let second: Value = mint(&base, &jwt, &read_only()).await.json().await.unwrap();
    assert_ne!(first["token"], second["token"], "a token per request");

    let other = send(
        reqwest::Method::POST,
        format!("{base}/app/installations/1/access_tokens"),
        &jwt,
        Some(&read_only()),
    )
    .await;
    assert_eq!(other.status(), StatusCode::NOT_FOUND);

    let token = first["token"].as_str().unwrap();
    let revoke = |bearer: String| {
        let base = base.clone();
        async move {
            send(
                reqwest::Method::DELETE,
                format!("{base}/installation/token"),
                &bearer,
                None,
            )
            .await
            .status()
        }
    };
    assert_eq!(revoke(token.to_owned()).await, StatusCode::NO_CONTENT);
    assert_eq!(
        revoke(token.to_owned()).await,
        StatusCode::UNAUTHORIZED,
        "already revoked"
    );
    assert_eq!(revoke("x".to_owned()).await, StatusCode::UNAUTHORIZED);

    let issued = credentials(&base).await;
    assert_eq!(issued["jwts"].as_array().unwrap().len(), 5);
    assert!(
        issued["jwts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|seen| seen == &json!(jwt))
    );
    assert_eq!(
        issued["installation_tokens"],
        json!([first["token"], second["token"]])
    );
    assert_eq!(issued["revoked"], json!([token]));
}

#[tokio::test]
async fn jwts_github_would_refuse_are_refused() {
    let base = spawn(config(true)).await;
    let now = Timestamp::now().as_second();
    let claims = |iss: Value, iat: i64, exp: i64| json!({"iss": iss, "iat": iat, "exp": exp});
    let unsigned = format!(
        "{}.{}.c2lnbmF0dXJl",
        URL_SAFE_NO_PAD.encode(json!({"alg": "RS256", "typ": "JWT"}).to_string()),
        URL_SAFE_NO_PAD.encode(claims(json!("1"), now - 60, now + 480).to_string()),
    );
    let header_only = format!(
        "{}..",
        URL_SAFE_NO_PAD.encode(json!({"alg": "RS256"}).to_string())
    );
    let cases = [
        (
            "signed by another key",
            jwt(&KEYS.2, &claims(json!("1"), now - 60, now + 480)),
            "could not be decoded",
        ),
        ("fake signature", unsigned, "could not be decoded"),
        ("empty segments", header_only, "could not be decoded"),
        (
            "expired",
            jwt(&KEYS.0, &claims(json!("1"), now - 700, now - 100)),
            "'exp'",
        ),
        (
            "lives longer than 10 minutes",
            jwt(&KEYS.0, &claims(json!("1"), now - 60, now + 660)),
            "too far in the future",
        ),
        (
            "issued in the future",
            jwt(&KEYS.0, &claims(json!("1"), now + 120, now + 500)),
            "'iat'",
        ),
        (
            "another App",
            jwt(&KEYS.0, &claims(json!("2"), now - 60, now + 480)),
            "'iss'",
        ),
        (
            "no issuer",
            jwt(&KEYS.0, &json!({"iat": now - 60, "exp": now + 480})),
            "could not be decoded",
        ),
        (
            "a user token",
            "gho_user_token".to_owned(),
            "could not be decoded",
        ),
    ];
    for (case, bearer, message) in cases {
        let response = lookup(&base, &bearer).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{case}");
        let body: Value = response.json().await.unwrap();
        assert!(
            body["message"].as_str().unwrap().contains(message),
            "{case}: {body}"
        );
        assert_eq!(
            mint(&base, &bearer, &read_only()).await.status(),
            StatusCode::UNAUTHORIZED,
            "{case}"
        );
    }
    let issued = credentials(&base).await;
    assert_eq!(issued["jwts"], json!([]), "nothing accepted");
    assert_eq!(issued["installation_tokens"], json!([]), "nothing minted");

    let unconfigured = spawn(config(false)).await;
    assert_eq!(
        lookup(&unconfigured, &good_jwt()).await.status(),
        StatusCode::UNAUTHORIZED,
        "no App key, no App"
    );
}

#[tokio::test]
async fn installation_token_body_errors_follow_authentication_and_lookup() {
    let base = spawn(config(true)).await;
    let jwt = good_jwt();
    let valid_body = read_only().to_string();
    for (id, token, content_type, body, status) in [
        (
            INSTALLATION,
            "invalid",
            "application/json",
            "{",
            StatusCode::UNAUTHORIZED,
        ),
        ("1", &jwt, "application/json", "{", StatusCode::NOT_FOUND),
        (
            INSTALLATION,
            &jwt,
            "application/json",
            "{",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            INSTALLATION,
            &jwt,
            "text/plain",
            &valid_body,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let response = reqwest::Client::new()
            .post(format!("{base}/app/installations/{id}/access_tokens"))
            .bearer_auth(token)
            .header(USER_AGENT, "diffpilot")
            .header(CONTENT_TYPE, content_type)
            .body(body.to_owned())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    assert_eq!(credentials(&base).await["installation_tokens"], json!([]));
}

#[tokio::test]
async fn token_requests_beyond_read_only_for_the_pr_repository_are_refused() {
    let base = spawn(config(true)).await;
    let jwt = good_jwt();
    let read = read_only()["permissions"].clone();
    let cases = [
        ("no scope at all", json!({})),
        ("no permissions", json!({"repositories": ["node-client"]})),
        (
            "empty permissions",
            json!({"repositories": ["node-client"], "permissions": {}}),
        ),
        ("no repositories", json!({"permissions": read})),
        (
            "a foreign repository",
            json!({"repositories": ["node-sdk"], "permissions": read}),
        ),
        (
            "two repositories",
            json!({"repositories": ["node-client", "node-sdk"], "permissions": read}),
        ),
        (
            "by repository id",
            json!({"repository_ids": [4823200], "permissions": read}),
        ),
        (
            "write access",
            json!({"repositories": ["node-client"],
            "permissions": {"contents": "read", "pull_requests": "write"}}),
        ),
        (
            "a permission the installation lacks",
            json!({"repositories": ["node-client"],
            "permissions": {"contents": "read", "administration": "read"}}),
        ),
        (
            "an unknown field",
            json!({"repositories": ["node-client"], "permissions": read, "scope": "all"}),
        ),
    ];
    for (case, body) in cases {
        let response = mint(&base, &jwt, &body).await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{case}"
        );
    }
    assert_eq!(
        credentials(&base).await["installation_tokens"],
        json!([]),
        "nothing minted"
    );
    let narrower = mint(
        &base,
        &jwt,
        &json!({"repositories": ["node-client"], "permissions": {"contents": "read"}}),
    )
    .await;
    assert_eq!(
        narrower.status(),
        StatusCode::CREATED,
        "a narrower read-only scope is granted"
    );
    let narrower: Value = narrower.json().await.unwrap();
    assert_eq!(
        narrower["permissions"],
        json!({"contents": "read"}),
        "the token carries what was asked"
    );
}

#[tokio::test]
async fn an_app_jwt_lists_the_recorded_installation() {
    let base = spawn(config(true)).await;
    let endpoint = format!("{base}/app/installations");
    let denied = send(reqwest::Method::GET, endpoint.clone(), "wrong", None).await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let response = send(reqwest::Method::GET, endpoint, &good_jwt(), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let rows: Vec<Value> = response.json().await.unwrap();
    assert_eq!(
        rows,
        [recording("rest/repo-installation.json")["response"].clone()]
    );
}
