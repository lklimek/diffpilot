//! Closing security review of stage v1.1 (plan WP11-SEC): personal access token cases the package tests above do not
//! name, exercised with fake tokens only.

use std::path::Path;

use contracts::ids::JobId;
use contracts::job::protocol::{ClaimRequest, ClaimResponse};
use contracts::job::spec::JobPriority;
use contracts::time::Timestamp;
use db::repo::analyses::testing::{judge_spec, seed_ranking_refresh};
use db::repo::jobs::JobStage;

use super::*;
use crate::jobs::executor::JobRequest;

/// Claims a queued judge job through the runner listener; returns the job token it hands out.
async fn claimed_job_token(state: &AppState) -> String {
    let refresh = seed_ranking_refresh(&state.db).await.expect("refresh");
    state
        .executor
        .submit(JobRequest {
            spec: judge_spec(refresh),
            priority: JobPriority::Interactive,
            stage: JobStage::Judge { refresh, batch: 1 },
            generation: 0,
            pr_repo: None,
        })
        .await
        .expect("submit");
    let runner_secret = {
        use secrecy::ExposeSecret;
        state.config.runner_secret.expose_secret().to_owned()
    };
    let body = serde_json::to_string(&ClaimRequest {
        accept_job: true,
        runner_id: "runner-sec".to_owned(),
        retired: Vec::new(),
    })
    .expect("json");
    let response = api::runner_router(state.clone())
        .oneshot(
            Request::post(RUNNER_CLAIM.path)
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {runner_secret}"))
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("infallible");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let claimed: ClaimResponse = serde_json::from_slice(&bytes).expect("claim response");
    claimed
        .claim
        .expect("a claimed job")
        .job_token
        .expose()
        .to_owned()
}

/// A job token (really handed out by the runner listener), the runner secret and bearers shaped like other
/// credentials (wrong prefix) are no public API credential: reads answer 401 without a bearer challenge, `/me`
/// stays signed out, a mutation is refused, and nothing reaches the token check's failure limit.
#[tokio::test]
async fn job_token_runner_secret_and_wrong_prefixes_are_no_public_api_credential() {
    let s = setup().await;
    let job_token = claimed_job_token(&s.state).await;
    let runner_secret = {
        use secrecy::ExposeSecret;
        s.state.config.runner_secret.expose_secret().to_owned()
    };
    let secret_part = &s.token[13..];
    let id_part = &s.token[4..12];
    let read = uri(&GET_REVIEW_STATE, s.analysis);
    for (case, bearer) in [
        ("job token", job_token.clone()),
        ("runner secret", runner_secret),
        ("upper-case prefix", format!("SRP_{id_part}_{secret_part}")),
        ("hyphen prefix", format!("srp-{id_part}_{secret_part}")),
        (
            "GitHub user token prefix",
            format!("ghu_{id_part}_{secret_part}"),
        ),
        ("bare secret", secret_part.to_owned()),
    ] {
        let response = send(&s.app, request("GET", &read, Some(&bearer), None)).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{case}");
        assert!(
            response.headers().get(WWW_AUTHENTICATE).is_none(),
            "{case}: the session check answers, not the token check"
        );
        let (status, me) = get_with_token(&s.app, GET_ME.path, &bearer).await;
        assert_eq!(status, StatusCode::OK, "{case}");
        assert_eq!(me["user"], Value::Null, "{case}: {me}");
        let mutation = Request::put(contracts::routes::SET_LANGUAGE.path)
            .header("content-type", "application/json")
            .header(AUTHORIZATION, format!("Bearer {bearer}"))
            .header(CSRF_HEADER, CSRF)
            .body(Body::from(json!({ "language": "pl" }).to_string()))
            .expect("request");
        assert_eq!(
            send(&s.app, mutation).await.status(),
            StatusCode::UNAUTHORIZED,
            "{case}"
        );
    }
    let user = s.state.db.users().get("u1").await.expect("user");
    assert_eq!(serde_json::to_value(user.language).expect("json"), "en");
}

/// A valid personal access token is refused on every runner route (claim, events, output, result).
#[tokio::test]
async fn personal_tokens_are_refused_on_every_runner_route() {
    let s = setup().await;
    let runner = api::runner_router(s.state.clone());
    let mut checked = 0;
    for route in ROUTES
        .iter()
        .filter(|route| route.listener == Listener::Runner)
    {
        let path = route.path.replace("{id}", &JobId::new_random().to_string());
        let response = runner
            .clone()
            .oneshot(
                Request::builder()
                    .method(route.method.as_str())
                    .uri(&path)
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {}", s.token))
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        checked += 1;
    }
    assert_eq!(checked, 4, "claim, events, output and result");
}

/// The failure limit behind a trusted proxy counts the client the proxy names; from any other peer
/// `X-Forwarded-For` is ignored, so rotating it never resets the limit. A valid token is still admitted.
#[tokio::test]
async fn failure_limit_counts_forwarded_clients_only_behind_a_trusted_proxy() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut config = crate::config::tests::test_config();
    config.data_dir = dir.path().to_owned();
    config.trusted_proxies = vec!["172.29.73.1/32".parse().expect("net")];
    let state = AppState::with_queue(config, db::repo::Db::in_memory().await.expect("db"));
    seed_analysis(&state.db).await.expect("analysis");
    session_cookie(&state, DEFAULT_TENANT).await;
    let (token, _) = read_token(&state, "u1").await;
    let app = api::router(state);
    let wrong = format!("{}{}", &token[..13], "E".repeat(43));
    let via = |peer: &str, forwarded: &str, token: &str| {
        let mut request = request("GET", GET_ME.path, Some(token), None);
        request
            .headers_mut()
            .insert("x-forwarded-for", forwarded.parse().expect("header"));
        request
            .extensions_mut()
            .insert(ConnectInfo(peer.parse::<SocketAddr>().expect("peer")));
        request
    };
    // Behind the proxy: one forwarded client is limited, another one is not.
    for n in 0..FAILED_ATTEMPTS_PER_CLIENT_PER_MINUTE {
        let status = send(&app, via("172.29.73.1:5000", "198.51.100.1", &wrong))
            .await
            .status();
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{n}");
    }
    let status = send(
        &app,
        via("172.29.73.1:5001", "203.0.113.200, 198.51.100.1", &wrong),
    )
    .await
    .status();
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "the proxy's (last) entry counts"
    );
    let status = send(&app, via("172.29.73.1:5002", "198.51.100.2", &wrong))
        .await
        .status();
    assert_eq!(status, StatusCode::UNAUTHORIZED, "another forwarded client");
    let status = send(&app, via("172.29.73.1:5003", "198.51.100.1", &token))
        .await
        .status();
    assert_eq!(
        status,
        StatusCode::OK,
        "a valid token of the limited client"
    );
    // Not from the proxy: a forged header is ignored and rotating it does not help.
    for n in 0..FAILED_ATTEMPTS_PER_CLIENT_PER_MINUTE {
        let forged = format!("198.51.100.{}", 10 + n);
        let status = send(&app, via("192.0.2.50:6000", &forged, &wrong))
            .await
            .status();
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{n}");
    }
    let status = send(&app, via("192.0.2.50:6001", "198.51.100.250", &wrong))
        .await
        .status();
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "counted on the peer");
}

/// `sqlite3 .dump` of `path`, or `None` when the `sqlite3` tool is not installed.
fn sqlite_dump(path: &Path) -> Option<String> {
    match std::process::Command::new("sqlite3")
        .arg(path)
        .arg(".dump")
        .output()
    {
        Ok(output) => {
            assert!(
                output.status.success(),
                "sqlite3 .dump failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            Some(String::from_utf8(output.stdout).expect("utf-8 dump"))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("sqlite3 could not run: {error}"),
    }
}

/// Every byte of every file in `dir` (database, WAL, shared memory).
fn raw_bytes(dir: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    for entry in std::fs::read_dir(dir).expect("dir") {
        let path = entry.expect("entry").path();
        if path.is_file() {
            bytes.extend(std::fs::read(&path).expect("read"));
        }
    }
    bytes
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// A file database after creating and using a token holds only its SHA-256: neither the token nor its secret is
/// anywhere in the database, WAL or shared-memory file, nor in `sqlite3 .dump`; the dump shows the hash.
#[tokio::test]
async fn database_holds_only_the_token_hash() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_dir = dir.path().join("db");
    std::fs::create_dir(&db_dir).expect("db dir");
    let db_path = db_dir.join("diffpilot.db");
    let mut config = crate::config::tests::test_config();
    config.data_dir = dir.path().join("data");
    let state = AppState::with_queue(config, db::repo::Db::open(&db_path).await.expect("db"));
    let analysis = seed_analysis(&state.db).await.expect("analysis");
    let cookie = session_cookie(&state, DEFAULT_TENANT).await;
    let app = api::router(state.clone());
    let (status, created) = status_and_body(
        &app,
        Request::post(CREATE_ACCESS_TOKEN.path)
            .header("content-type", "application/json")
            .header(COOKIE, &cookie)
            .header(CSRF_HEADER, CSRF)
            .body(Body::from(create_body("dump-check").to_string()))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let token = created["token"].as_str().expect("token").to_owned();
    let read = uri(&GET_REVIEW_STATE, analysis);
    assert_eq!(get_with_token(&app, &read, &token).await.0, StatusCode::OK);
    let wrong = format!("{}{}", &token[..13], "F".repeat(43));
    assert_eq!(
        get_with_token(&app, &read, &wrong).await.0,
        StatusCode::UNAUTHORIZED
    );
    let secret = &token[13..];
    let bytes = raw_bytes(&db_dir);
    assert!(!bytes.is_empty());
    for (what, value) in [
        ("token", token.as_str()),
        ("secret", secret),
        ("wrong secret", &wrong[13..]),
    ] {
        assert!(!contains(&bytes, value), "{what} in the database files");
    }
    let hash: String = sha256(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect();
    if let Some(dump) = sqlite_dump(&db_path) {
        assert!(!dump.contains(secret), "secret in the dump");
        let rows: Vec<&str> = dump
            .lines()
            .filter(|line| line.starts_with("INSERT INTO access_tokens"))
            .collect();
        assert_eq!(rows.len(), 1, "{dump}");
        assert!(
            rows[0].to_ascii_uppercase().contains(&format!("X'{hash}'")),
            "{}",
            rows[0]
        );
    }
}

/// Reads with a token change nothing in the database but the token's last use (written at most once a minute):
/// every `read` route and `/me`, including the GitHub-backed ones with a stored pair and the refresh status and
/// stream, leave a `sqlite3 .dump` unchanged; in particular `users.updated_at` (the schedule's 30-day keep bound,
/// ADR-020) does not move. GitHub sees only REST `GET`s and GraphQL queries.
#[tokio::test]
async fn token_reads_leave_the_database_unchanged() {
    use wiremock::MockServer;
    let github = MockServer::start().await;
    let dir = tempfile::tempdir().expect("temp dir");
    let db_dir = dir.path().join("db");
    std::fs::create_dir(&db_dir).expect("db dir");
    let db_path = db_dir.join("diffpilot.db");
    let mut config = crate::config::tests::test_config();
    config.data_dir = dir.path().join("data");
    config.github.api_url = url::Url::parse(&format!("{}/api", github.uri())).expect("url");
    let state = AppState::with_queue(config, db::repo::Db::open(&db_path).await.expect("db"));
    let analysis = seed_analysis(&state.db).await.expect("analysis");
    session_cookie(&state, DEFAULT_TENANT).await;
    let lock = crate::auth::lock_user("u1").await;
    crate::auth::store_sign_in_tokens(
        &state,
        &lock,
        DEFAULT_TENANT,
        &crate::github::oauth::TokenGrant {
            access_token: secrecy::SecretString::from("ghu_fake_sec_review"),
            expires_in: 28_800,
            refresh_token: secrecy::SecretString::from("ghr_fake_sec_review"),
            refresh_token_expires_in: 15_638_400,
        },
        Timestamp::now(),
    )
    .await
    .expect("pair");
    drop(lock);
    let refresh = seed_ranking_refresh(&state.db).await.expect("refresh");
    let (token, _) = read_token(&state, "u1").await;
    let app = api::router(state.clone());
    // The first use records `last_used_at`; later ones within the minute write nothing.
    assert_eq!(
        get_with_token(&app, GET_ME.path, &token).await.0,
        StatusCode::OK
    );
    let Some(before) = sqlite_dump(&db_path) else {
        return; // sqlite3 not installed
    };
    let mut paths = vec![GET_ME.path.to_owned()];
    for route in ROUTES
        .iter()
        .filter(|route| matches!(route.auth, RouteAuth::SessionOrToken(_)))
    {
        paths.push(if route.path.starts_with("/api/v1/dashboard/refreshes/") {
            route.path.replace("{id}", &refresh.to_string())
        } else {
            uri(route, analysis)
        });
    }
    for path in &paths {
        let (status, body) = get_with_token(&app, path, &token).await;
        assert_ne!(status, StatusCode::FORBIDDEN, "{path}: {body}");
        assert_ne!(status, StatusCode::UNAUTHORIZED, "{path}: {body}");
    }
    let after = sqlite_dump(&db_path).expect("dump");
    let differing: Vec<(&str, &str)> = before
        .lines()
        .zip(after.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert!(
        differing.is_empty() && before.lines().count() == after.lines().count(),
        "token reads changed the database: {differing:#?}"
    );
    // GitHub sees REST `GET`s and GraphQL queries only (pending review, App installations).
    for request in github.received_requests().await.unwrap_or_default() {
        if request.method.as_str() == "POST" && request.url.path() == "/api/graphql" {
            let body: Value = serde_json::from_slice(&request.body).expect("GraphQL body");
            let document = body["query"].as_str().expect("document").trim_start();
            assert!(document.starts_with("query"), "{document}");
        } else {
            assert_eq!(request.method.as_str(), "GET", "{}", request.url);
        }
    }
}
