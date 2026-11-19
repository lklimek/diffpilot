//! Drives the fake GitHub over real TCP: OAuth web flow, `/user`, recorded REST endpoints, startup checks.
#![expect(clippy::unwrap_used, reason = "test helpers outside #[test] functions")]

use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fake_github::{Config, StartError};
use reqwest::header::{ACCEPT, AUTHORIZATION, LOCATION, USER_AGENT};
use reqwest::{Client, StatusCode, redirect};
use serde_json::Value;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use url::Url;

const APP_URL: &str = "http://app.test:3000";
const CALLBACK: &str = "http://app.test:3000/auth/github/callback";
const CLIENT_ID: &str = "fixture-app-client-id";
const CLIENT_SECRET: &str = "fixture-client-secret";
const VERIFIER: &str = "correct-verifier-0123456789-0123456789-0123456789";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture_dir() -> PathBuf {
    repo_root().join("fixtures/pr-482/github")
}

fn config() -> Config {
    Config {
        fixture_dir: fixture_dir(),
        app_url: Url::parse(APP_URL).unwrap(),
        user_login: None,
        user_id: None,
        app_id: None,
        app_public_key_file: None,
        dashboard_fixture_dir: None,
        dashboard_world_at: None,
    }
}

fn recording(file: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixture_dir().join(file)).unwrap()).unwrap()
}

/// The `response` of a recording exactly as written in the file.
fn recorded_response_text(file: &str) -> String {
    #[derive(serde::Deserialize)]
    struct Raw {
        response: Box<RawValue>,
    }
    let text = std::fs::read_to_string(fixture_dir().join(file)).unwrap();
    serde_json::from_str::<Raw>(&text)
        .unwrap()
        .response
        .get()
        .to_owned()
}

async fn spawn(config: Config) -> String {
    let router = fake_github::router(&config).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}

fn client() -> Client {
    Client::builder()
        .redirect(redirect::Policy::none())
        .build()
        .unwrap()
}

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Runs the authorize step like the server's sign-in and returns the redirect `Location`.
async fn authorize(base: &str, redirect_uri: &str, state: &str) -> Url {
    let response = client()
        .get(format!("{base}/login/oauth/authorize"))
        .query(&[
            ("client_id", CLIENT_ID),
            ("redirect_uri", redirect_uri),
            ("state", state),
            ("code_challenge", &challenge(VERIFIER)),
            ("code_challenge_method", "S256"),
            ("allow_signup", "false"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    let location = response.headers()[LOCATION].to_str().unwrap();
    Url::parse(location).unwrap()
}

fn query_value(url: &Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

/// Posts the token request like `server::github::oauth::token_request`; returns status and JSON body.
async fn token_request(base: &str, params: &[(&str, &str)]) -> (StatusCode, Value) {
    let mut form = vec![("client_id", CLIENT_ID), ("client_secret", CLIENT_SECRET)];
    form.extend_from_slice(params);
    let response = client()
        .post(format!("{base}/login/oauth/access_token"))
        .header(ACCEPT, "application/json")
        .header(USER_AGENT, "smart-review")
        .form(&form)
        .send()
        .await
        .unwrap();
    let status = response.status();
    (status, response.json().await.unwrap())
}

async fn exchange(base: &str, code: &str, verifier: &str) -> (StatusCode, Value) {
    token_request(
        base,
        &[
            ("code", code),
            ("redirect_uri", CALLBACK),
            ("code_verifier", verifier),
        ],
    )
    .await
}

async fn refresh(base: &str, refresh_token: &str) -> (StatusCode, Value) {
    token_request(
        base,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )
    .await
}

/// `GET /user` like `server::github::oauth::fetch_user`.
async fn get_user(base: &str, token: &str) -> reqwest::Response {
    client()
        .get(format!("{base}/user"))
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header(ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header(USER_AGENT, "smart-review")
        .send()
        .await
        .unwrap()
}

/// Signs in and returns the token response.
async fn sign_in(base: &str) -> Value {
    let location = authorize(base, CALLBACK, "s1").await;
    let code = query_value(&location, "code").unwrap();
    let (status, body) = exchange(base, &code, VERIFIER).await;
    assert_eq!(status, StatusCode::OK);
    body
}

fn token(body: &Value, field: &str) -> String {
    body[field].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn oauth_round_trip_with_pkce_and_user() {
    let base = spawn(config()).await;

    let location = authorize(&base, CALLBACK, "state-123").await;
    assert_eq!(
        format!(
            "{}://{}{}",
            location.scheme(),
            location.authority(),
            location.path()
        ),
        CALLBACK
    );
    assert_eq!(
        query_value(&location, "state").as_deref(),
        Some("state-123")
    );
    let code = query_value(&location, "code").unwrap();
    assert_eq!(
        code,
        recording("oauth/access-token.json")["request"]["body"]["code"]
    );

    let (status, body) = exchange(&base, &code, "wrong-verifier").await;
    assert_eq!(status, StatusCode::OK, "OAuth errors arrive as HTTP 200");
    assert_eq!(body["error"], "bad_verification_code");
    assert!(body.get("access_token").is_none());

    let (status, body) = exchange(&base, &code, VERIFIER).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, recording("oauth/access-token.json")["response"]);

    let user = get_user(&base, &token(&body, "access_token")).await;
    assert_eq!(user.status(), StatusCode::OK);
    assert_eq!(
        user.text().await.unwrap(),
        recorded_response_text("rest/user.json")
    );
}

#[tokio::test]
async fn authorization_code_is_single_use() {
    let base = spawn(config()).await;
    let location = authorize(&base, CALLBACK, "s").await;
    let code = query_value(&location, "code").unwrap();
    let (_, first) = exchange(&base, &code, VERIFIER).await;
    assert!(first.get("access_token").is_some());

    let (status, second) = exchange(&base, &code, VERIFIER).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["error"], "bad_verification_code");
}

#[tokio::test]
async fn second_sign_in_gets_a_new_code_and_token_pair() {
    let base = spawn(config()).await;
    let first = sign_in(&base).await;
    let second = sign_in(&base).await;
    assert_ne!(
        token(&first, "access_token"),
        token(&second, "access_token")
    );
    assert_ne!(
        token(&first, "refresh_token"),
        token(&second, "refresh_token")
    );
    for grant in [&first, &second] {
        let user = get_user(&base, &token(grant, "access_token")).await;
        assert_eq!(user.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn refresh_rotates_the_pair_and_rejects_replay() {
    let base = spawn(config()).await;
    let old = sign_in(&base).await;

    let (status, new) = refresh(&base, &token(&old, "refresh_token")).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(token(&new, "access_token"), token(&old, "access_token"));
    assert_ne!(token(&new, "refresh_token"), token(&old, "refresh_token"));
    let recorded = &recording("oauth/access-token.json")["response"];
    for field in [
        "expires_in",
        "refresh_token_expires_in",
        "scope",
        "token_type",
    ] {
        assert_eq!(new[field], recorded[field], "{field}");
    }

    let stale = get_user(&base, &token(&old, "access_token")).await;
    assert_eq!(stale.status(), StatusCode::UNAUTHORIZED);
    let fresh = get_user(&base, &token(&new, "access_token")).await;
    assert_eq!(fresh.status(), StatusCode::OK);

    let (status, replay) = refresh(&base, &token(&old, "refresh_token")).await;
    assert_eq!(status, StatusCode::OK, "OAuth errors arrive as HTTP 200");
    assert_eq!(replay["error"], "bad_refresh_token");
    assert!(replay.get("access_token").is_none());

    let still_valid = get_user(&base, &token(&new, "access_token")).await;
    assert_eq!(still_valid.status(), StatusCode::OK);
}

#[tokio::test]
async fn tokens_are_deterministic_across_instances() {
    let first = sign_in(&spawn(config()).await).await;
    let second = sign_in(&spawn(config()).await).await;
    assert_eq!(first, second);
}

#[tokio::test]
async fn wrong_client_secret_is_rejected_as_200_with_error() {
    let base = spawn(config()).await;
    let location = authorize(&base, CALLBACK, "s").await;
    let code = query_value(&location, "code").unwrap();
    let response = client()
        .post(format!("{base}/login/oauth/access_token"))
        .header(ACCEPT, "application/json")
        .form(&[
            ("client_id", CLIENT_ID),
            ("client_secret", "wrong"),
            ("code", &code),
            ("code_verifier", VERIFIER),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"], "incorrect_client_credentials");
}

#[tokio::test]
async fn token_request_without_json_accept_is_refused() {
    let base = spawn(config()).await;
    let response = client()
        .post(format!("{base}/login/oauth/access_token"))
        .form(&[("client_id", CLIENT_ID), ("client_secret", CLIENT_SECRET)])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);
}

#[tokio::test]
async fn unregistered_redirect_uri_redirects_to_callback_with_error() {
    let base = spawn(config()).await;
    let location = authorize(&base, "http://evil.test/callback", "st").await;
    assert!(location.as_str().starts_with(CALLBACK));
    assert_eq!(
        query_value(&location, "error").as_deref(),
        Some("redirect_uri_mismatch")
    );
    assert_eq!(query_value(&location, "state").as_deref(), Some("st"));
    assert_eq!(query_value(&location, "code"), None);
}

#[tokio::test]
async fn incomplete_pkce_parameters_are_rejected() {
    let base = spawn(config()).await;
    for params in [
        [("code_challenge_method", "S256")],
        [("code_challenge", "abc")],
    ] {
        let response = client()
            .get(format!("{base}/login/oauth/authorize"))
            .query(&[("client_id", CLIENT_ID), ("redirect_uri", CALLBACK)])
            .query(&params)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{params:?}");
    }
}

#[tokio::test]
async fn authorize_without_pkce_still_grants() {
    let base = spawn(config()).await;
    let response = client()
        .get(format!("{base}/login/oauth/authorize"))
        .query(&[("client_id", CLIENT_ID), ("redirect_uri", CALLBACK)])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    let location = Url::parse(response.headers()[LOCATION].to_str().unwrap()).unwrap();
    let code = query_value(&location, "code").unwrap();
    let (status, body) = token_request(&base, &[("code", &code)]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("access_token").is_some(), "{body}");
}

#[tokio::test]
async fn unknown_client_id_is_not_found() {
    let base = spawn(config()).await;
    let response = client()
        .get(format!("{base}/login/oauth/authorize"))
        .query(&[("client_id", "other"), ("redirect_uri", CALLBACK)])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn user_without_valid_token_is_401() {
    let base = spawn(config()).await;
    let response = get_user(&base, "not-a-token").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["message"], "Bad credentials");
}

#[tokio::test]
async fn configured_user_overrides_login_and_id() {
    let base = spawn(Config {
        user_login: Some("bob".to_owned()),
        user_id: Some(77),
        ..config()
    })
    .await;
    let grant = sign_in(&base).await;
    let user: Value = get_user(&base, &token(&grant, "access_token"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(user["login"], "bob");
    assert_eq!(user["id"], 77);
    assert_eq!(
        user["name"],
        recording("rest/user.json")["response"]["name"]
    );
}

#[tokio::test]
async fn fake_user_control_switches_the_user_of_later_sign_ins() {
    let base = spawn(config()).await;
    let first = sign_in(&base).await;
    let answer = client()
        .post(format!("{base}/_fake/user"))
        .json(&serde_json::json!({"login": "bob", "id": 77}))
        .send()
        .await
        .unwrap();
    assert_eq!(answer.status(), StatusCode::NO_CONTENT);
    let second = sign_in(&base).await;
    let user: Value = get_user(&base, &token(&second, "access_token"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        (user["login"].clone(), user["id"].clone()),
        ("bob".into(), 77.into())
    );
    assert_eq!(
        user["name"],
        recording("rest/user.json")["response"]["name"]
    );
    // The earlier sign-in stays the recorded user; a refreshed pair keeps its user.
    let alice = get_user(&base, &token(&first, "access_token")).await;
    assert_eq!(
        alice.text().await.unwrap(),
        recorded_response_text("rest/user.json")
    );
    let (status, rotated) = refresh(&base, &token(&second, "refresh_token")).await;
    assert_eq!(status, StatusCode::OK);
    let user: Value = get_user(&base, &token(&rotated, "access_token"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(user["login"], "bob");
}

const RECORDED_REST: [&str; 5] = [
    "rest/pull.json",
    "rest/pull-commits.json",
    "rest/pull-files.json",
    "rest/check-runs.json",
    "rest/commit-status.json",
];

/// `GET` of a recorded REST endpoint (path and query as recorded), anonymous or with a bearer token.
async fn get_recorded(base: &str, file: &str, token: Option<&str>) -> reqwest::Response {
    let recorded = recording(file);
    let url = Url::parse(recorded["request"]["url"].as_str().unwrap()).unwrap();
    let path_and_query = &url[url::Position::BeforePath..];
    let mut request = client()
        .get(format!("{base}{path_and_query}"))
        .header(ACCEPT, "application/vnd.github+json")
        .header(USER_AGENT, "smart-review");
    if let Some(token) = token {
        request = request.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    request.send().await.unwrap()
}

async fn assert_recorded(response: reqwest::Response, file: &str) {
    assert_eq!(
        response.status().as_u16(),
        recording(file)["status"].as_u64().unwrap() as u16,
        "{file}"
    );
    assert_eq!(
        response.text().await.unwrap(),
        recorded_response_text(file),
        "{file}"
    );
}

#[tokio::test]
async fn pull_request_endpoints_serve_recordings_byte_equal() {
    let base = spawn(config()).await;
    for file in RECORDED_REST {
        assert_recorded(get_recorded(&base, file, None).await, file).await;
    }
}

#[tokio::test]
async fn recorded_endpoints_validate_a_present_token() {
    let base = spawn(config()).await;
    let revoked = sign_in(&base).await;
    let (_, current) = refresh(&base, &token(&revoked, "refresh_token")).await;
    for file in RECORDED_REST {
        assert_recorded(get_recorded(&base, file, None).await, file).await;
        let current = get_recorded(&base, file, Some(&token(&current, "access_token"))).await;
        assert_recorded(current, file).await;
        for bad in [token(&revoked, "access_token"), "unknown-token".to_owned()] {
            let response = get_recorded(&base, file, Some(&bad)).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{file} {bad}");
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["message"], "Bad credentials", "{file}");
        }
    }
}

#[tokio::test]
async fn rest_request_without_user_agent_is_forbidden() {
    let base = spawn(config()).await;
    let response = client()
        .get(format!("{base}/repos/acme/node-client/pulls/482"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// Current-thread runtime: the server task runs on this thread, under the thread-local subscriber.
#[tokio::test(flavor = "current_thread")]
async fn unknown_route_is_404_and_logged() {
    let logs = Captured::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let base = spawn(config()).await;
    let response = client()
        .get(format!("{base}/repos/acme/node-client/pulls/999"))
        .header(USER_AGENT, "smart-review")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["message"], "Not Found");

    let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    let line = logs
        .lines()
        .find(|line| line.contains("fake-github has no recording"))
        .unwrap_or_else(|| panic!("no log line for the unknown route in:\n{logs}"));
    assert!(line.contains("WARN"), "{line}");
    assert!(line.contains("GET"), "{line}");
    assert!(line.contains("/repos/acme/node-client/pulls/999"), "{line}");
}

#[test]
fn router_fails_without_fixture_dir() {
    let missing = tempfile::tempdir().unwrap().path().join("missing");
    let error = fake_github::router(&Config {
        fixture_dir: missing.clone(),
        ..config()
    })
    .unwrap_err();
    assert!(
        matches!(&error, StartError::FixtureDirMissing { path } if *path == missing),
        "{error:?}"
    );
}

#[test]
fn router_fails_when_a_recording_is_missing() {
    let empty = tempfile::tempdir().unwrap();
    let error = fake_github::router(&Config {
        fixture_dir: empty.path().to_owned(),
        ..config()
    })
    .unwrap_err();
    assert!(matches!(error, StartError::ReadFixture { .. }), "{error:?}");
}

#[test]
fn binary_refuses_to_start_without_fixture_dir() {
    let missing = tempfile::tempdir().unwrap().path().join("missing");
    let output = Command::new(env!("CARGO_BIN_EXE_fake-github"))
        .args([
            "--listen",
            "127.0.0.1:0",
            "--app-url",
            APP_URL,
            "--fixture-dir",
        ])
        .arg(&missing)
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fixture directory"), "{stderr}");
    assert!(stderr.contains(missing.to_str().unwrap()), "{stderr}");
}

#[test]
fn clone_base_script_builds_the_fixture_remote() {
    let dest = tempfile::tempdir().unwrap();
    let status = Command::new("bash")
        .arg(repo_root().join("tools/fake-github/prepare-clone-base.sh"))
        .arg(dest.path())
        .status()
        .unwrap();
    assert!(status.success());

    let commits: Value = serde_json::from_str(
        &std::fs::read_to_string(repo_root().join("fixtures/pr-482/repo/commits.json")).unwrap(),
    )
    .unwrap();
    let remote = format!("file://{}/acme/node-client.git", dest.path().display());
    let output = Command::new("git")
        .args(["ls-remote", &remote])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(output.status.success());
    let refs = String::from_utf8(output.stdout).unwrap();
    for (sha, name) in [
        (&commits["base"], "refs/heads/main"),
        (&commits["head"], "refs/heads/feat/retry-timeouts"),
        (&commits["head"], "refs/pull/482/head"),
    ] {
        let line = format!("{}\t{name}", sha.as_str().unwrap());
        assert!(refs.lines().any(|l| l == line), "{line} not in:\n{refs}");
    }
}

/// Posts a GraphQL document like `server::github::graphql::execute`.
async fn graphql(base: &str, token: &str, query: &str, variables: Value) -> (StatusCode, Value) {
    let response = client()
        .post(format!("{base}/graphql"))
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header(USER_AGENT, "smart-review")
        .json(&serde_json::json!({"query": query, "variables": variables}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or(Value::Null))
}

const ADD_THREAD: &str = "mutation($in:AddPullRequestReviewThreadInput!){addPullRequestReviewThread(input:$in){thread{id}}}";
const PENDING: &str = "query($o:String!,$n:String!,$pr:Int!){repository(owner:$o,name:$n){pullRequest(number:$pr){id headRefOid reviews(states:[PENDING],first:10){nodes{id}} reviewThreads(first:100){nodes{id}}}}}";

fn pull_variables() -> Value {
    serde_json::json!({"o": "acme", "n": "node-client", "pr": 482})
}

async fn add(base: &str, token: &str, input: Value) -> Value {
    let (status, body) = graphql(base, token, ADD_THREAD, serde_json::json!({"in": input})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"]["addPullRequestReviewThread"]["thread"].clone()
}

async fn pending(base: &str, token: &str) -> Value {
    let (status, body) = graphql(base, token, PENDING, pull_variables()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"]["repository"]["pullRequest"].clone()
}

#[tokio::test]
async fn graphql_pending_review_is_created_reused_edited_and_deleted() {
    let base = spawn(config()).await;
    let token = token(&sign_in(&base).await, "access_token");
    let pull = pending(&base, &token).await;
    assert_eq!(pull["id"], "PR_fixture482");
    assert_eq!(
        pull["headRefOid"],
        recording("rest/pull.json")["response"]["head"]["sha"]
    );
    assert_eq!(pull["reviews"]["nodes"], serde_json::json!([]));

    let first = add(
        &base,
        &token,
        serde_json::json!({"pullRequestId": "PR_fixture482",
        "path": "src/rpc/client.rs", "line": 96, "side": "RIGHT", "body": "one"}),
    )
    .await;
    let second = add(
        &base,
        &token,
        serde_json::json!({"pullRequestId": "PR_fixture482",
        "path": "src/rpc/client.rs", "subjectType": "FILE", "body": "two"}),
    )
    .await;
    assert!(first["id"].is_string() && second["id"].is_string());
    assert_eq!(
        first["comments"]["nodes"][0]["commit"]["oid"], pull["headRefOid"],
        "a new comment is placed on the current head"
    );
    let reviews = &pending(&base, &token).await["reviews"]["nodes"];
    assert_eq!(
        reviews.as_array().unwrap().len(),
        1,
        "one pending review reused: {reviews}"
    );
    assert_eq!(reviews[0]["comments"]["totalCount"], 2);
    let comment = reviews[0]["comments"]["nodes"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let update = "mutation($in:UpdatePullRequestReviewCommentInput!){updatePullRequestReviewComment(input:$in){pullRequestReviewComment{id body}}}";
    let (_, body) = graphql(
        &base,
        &token,
        update,
        serde_json::json!({"in": {"pullRequestReviewCommentId": comment, "body": "edited"}}),
    )
    .await;
    let edited = &body["data"]["updatePullRequestReviewComment"]["pullRequestReviewComment"];
    assert_eq!(edited["body"], "edited");
    assert_eq!(edited["pullRequestReview"]["state"], "PENDING");

    let delete = "mutation($in:DeletePullRequestReviewCommentInput!){deletePullRequestReviewComment(input:$in){pullRequestReviewComment{id}}}";
    let (_, body) = graphql(
        &base,
        &token,
        delete,
        serde_json::json!({"in": {"id": comment}}),
    )
    .await;
    assert_eq!(
        body["data"]["deletePullRequestReviewComment"]["pullRequestReviewComment"]["id"],
        comment.as_str()
    );
    assert_eq!(
        body["data"]["deletePullRequestReviewComment"]["pullRequestReview"]["state"],
        "PENDING"
    );
    let (_, body) = graphql(
        &base,
        &token,
        delete,
        serde_json::json!({"in": {"id": comment}}),
    )
    .await;
    assert_eq!(body["errors"][0]["type"], "NOT_FOUND", "{body}");
    let other = reviews[0]["comments"]["nodes"][1]["id"].clone();
    graphql(
        &base,
        &token,
        delete,
        serde_json::json!({"in": {"id": other}}),
    )
    .await;
    assert_eq!(
        pending(&base, &token).await["reviews"]["nodes"],
        serde_json::json!([]),
        "the review disappears with its last comment"
    );
}

#[tokio::test]
async fn graphql_rejects_positions_silently_and_never_submits() {
    let base = spawn(config()).await;
    let token = token(&sign_in(&base).await, "access_token");
    for input in [
        serde_json::json!({"pullRequestId": "PR_fixture482", "path": "src/rpc/client.rs", "line": 60,
            "side": "RIGHT", "body": "outside the hunks"}),
        serde_json::json!({"pullRequestId": "PR_fixture482", "path": "src/rpc/transport.rs",
            "subjectType": "FILE", "body": "file not in the PR"}),
    ] {
        assert_eq!(add(&base, &token, input).await, Value::Null);
    }
    let left = add(
        &base,
        &token,
        serde_json::json!({"pullRequestId": "PR_fixture482",
        "path": "src/rpc/client.rs", "line": 53, "side": "LEFT", "body": "removed"}),
    )
    .await;
    assert!(left["id"].is_string(), "{left}");
    let submit = concat!(
        "mutation{submit",
        "PullRequestReview(input:{event:COMMENT}){clientMutationId}}"
    );
    let (_, body) = graphql(&base, &token, submit, Value::Null).await;
    assert_eq!(body["errors"][0]["type"], "FORBIDDEN");
    let (status, _) = graphql(&base, "not-a-token", PENDING, pull_variables()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn moved_head_is_served_by_rest_and_graphql() {
    let base = spawn(config()).await;
    let token = token(&sign_in(&base).await, "access_token");
    let moved = "d41b7a0d41b7a0d41b7a0d41b7a0d41b7a0d41b7";
    let refused = client()
        .post(format!("{base}/_fake/head"))
        .json(&serde_json::json!({"sha": "not-a-commit"}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    let accepted = client()
        .post(format!("{base}/_fake/head"))
        .json(&serde_json::json!({"sha": moved}))
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::NO_CONTENT);

    let pull: Value = get_recorded(&base, "rest/pull.json", Some(&token))
        .await
        .json()
        .await
        .unwrap();
    let mut expected = recording("rest/pull.json")["response"].clone();
    expected["head"]["sha"] = serde_json::json!(moved);
    assert_eq!(pull, expected, "only the head moved");
    assert_eq!(pending(&base, &token).await["headRefOid"], moved);
    let thread = add(
        &base,
        &token,
        serde_json::json!({"pullRequestId": "PR_fixture482",
        "path": "src/rpc/client.rs", "line": 96, "side": "RIGHT", "body": "on the new head"}),
    )
    .await;
    assert_eq!(thread["comments"]["nodes"][0]["commit"]["oid"], moved);
}

#[tokio::test]
async fn ci_serves_skipped_neutral_and_pending_statuses() {
    let base = spawn(config()).await;
    let runs: Value = get_recorded(&base, "rest/check-runs.json", None)
        .await
        .json()
        .await
        .unwrap();
    for conclusion in ["skipped", "neutral"] {
        assert!(
            runs["check_runs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["conclusion"] == conclusion)
        );
    }
    let response = get_recorded(&base, "rest/commit-status.json", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let statuses: Value = response.json().await.unwrap();
    assert_eq!(statuses["statuses"][0]["state"], "pending");
}
