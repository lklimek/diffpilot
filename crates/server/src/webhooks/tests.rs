use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::http::{Request, Response, StatusCode, header};
use contracts::api::dashboard::DashboardSettings;
use contracts::time::Timestamp;
use db::repo::{DEFAULT_TENANT, Db, users::GithubProfile};
use futures::{StreamExt, stream};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};
use tower::ServiceExt;
use uuid::Uuid;

use super::{Webhooks, drain::MAX_BYTES, receive, serve};
use crate::api::webhook_router;
use crate::config::tests::test_config;
use crate::test_support::logs::LogCapture;

const SECRET: &str = "synthetic-current-secret-long-enough";
const PREVIOUS: &str = "synthetic-previous-secret-long-enough";
const PATH: &str = "/webhooks/github";

async fn fixture(secret: Option<&str>) -> Webhooks {
    let mut config = test_config();
    config.github_webhook_secret = secret.map(Into::into);
    config.github_webhook_secret_previous = Some(PREVIOUS.into());
    Webhooks::new(Db::in_memory().await.unwrap(), Arc::new(config))
}

fn worker_state(hooks: &Webhooks) -> crate::state::AppState {
    let mut config = test_config();
    config.github_webhook_secret = hooks.config.github_webhook_secret.clone();
    crate::state::AppState::with_queue(config, hooks.db.clone())
}

fn request(hooks: &Webhooks, event: &str, id: Uuid, body: &str, secret: &str) -> Request<Body> {
    let (parts, body) = fake_github::signed_webhook(
        PATH,
        hooks.config.github.app_id,
        event,
        id,
        body.as_bytes(),
        secret.as_bytes(),
    )
    .unwrap()
    .into_parts();
    Request::from_parts(parts, Body::from(body))
}

fn ping() -> String {
    json!({"zen":"payload-text-not-for-logs","hook_id":123,"hook":{"events":["issues","pull_request"]}}).to_string()
}

fn signal(owner: &str, id: i64) -> String {
    json!({"action":"opened","repository":{"id":id,"full_name":format!("{owner}/code")},
           "number":482,"installation":{"id":10},"pull_request":{"head":{"sha":"1".repeat(40)}},
           "body":"payload-text-not-for-logs"})
    .to_string()
}

async fn send(hooks: &Webhooks, req: Request<Body>) -> Response<Body> {
    webhook_router(hooks.clone()).oneshot(req).await.unwrap()
}

async fn track(hooks: &Webhooks, owner: &str) {
    hooks
        .db
        .users()
        .upsert_member(
            DEFAULT_TENANT,
            "u1",
            &GithubProfile {
                github_id: 1,
                login: "fixture".to_owned(),
                name: None,
                avatar_url: None,
            },
            Timestamp::now(),
        )
        .await
        .unwrap();
    hooks
        .db
        .dashboard()
        .save_settings(
            DEFAULT_TENANT,
            "u1",
            &DashboardSettings {
                enumerate_owners: vec![owner.to_owned()],
                ..DashboardSettings::default()
            },
            Timestamp::now(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn unsigned_or_bad_signature_is_401_before_parse() {
    let hooks = fixture(Some(SECRET)).await;
    let id = Uuid::new_v4();
    let mut unsigned = request(&hooks, "ping", id, "not json", SECRET);
    unsigned.headers_mut().remove("x-hub-signature-256");
    for req in [
        unsigned,
        request(
            &hooks,
            "ping",
            id,
            "not json",
            PREVIOUS.trim_end_matches("enough"),
        ),
    ] {
        assert_eq!(send(&hooks, req).await.status(), 401);
    }
    assert_eq!(hooks.db.github().last_delivery_at().await.unwrap(), None);
    assert_eq!(
        send(&hooks, request(&hooks, "ping", id, "not json", SECRET))
            .await
            .status(),
        400
    );
    assert!(
        hooks
            .db
            .github()
            .last_delivery_at()
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn two_signature_headers_401() {
    let hooks = fixture(Some(SECRET)).await;
    let mut req = request(&hooks, "ping", Uuid::new_v4(), &ping(), SECRET);
    let value = req.headers()["x-hub-signature-256"].clone();
    req.headers_mut().append("x-hub-signature-256", value);
    assert_eq!(send(&hooks, req).await.status(), 401);
}

#[tokio::test]
async fn malformed_signature_format_401() {
    let hooks = fixture(Some(SECRET)).await;
    for value in [
        "sha256=12".to_owned(),
        format!("sha256={}", "A".repeat(64)),
        format!("sha1={}", "a".repeat(64)),
    ] {
        let mut req = request(&hooks, "ping", Uuid::new_v4(), &ping(), SECRET);
        req.headers_mut()
            .insert("x-hub-signature-256", value.parse().unwrap());
        assert_eq!(receive::signature(req.headers()), None, "{value}");
        assert_eq!(send(&hooks, req).await.status(), 401);
    }
}

#[tokio::test]
async fn wrong_hook_target_401() {
    let hooks = fixture(Some(SECRET)).await;
    for (key, value) in [
        ("x-github-hook-installation-target-type", "repository"),
        ("x-github-hook-installation-target-id", "456"),
    ] {
        let mut req = request(&hooks, "ping", Uuid::new_v4(), "not json", SECRET);
        req.headers_mut().insert(key, value.parse().unwrap());
        assert_eq!(send(&hooks, req).await.status(), 401);
    }
}

#[tokio::test]
async fn non_json_content_type_415() {
    let hooks = fixture(Some(SECRET)).await;
    let mut req = request(&hooks, "ping", Uuid::new_v4(), &ping(), SECRET);
    req.headers_mut().insert(
        header::CONTENT_TYPE,
        "application/x-www-form-urlencoded".parse().unwrap(),
    );
    assert_eq!(send(&hooks, req).await.status(), 415);
}

#[tokio::test]
async fn oversized_content_length_413_before_body_read() {
    let hooks = fixture(Some(SECRET)).await;
    let mut req = request(&hooks, "ping", Uuid::new_v4(), &ping(), SECRET);
    req.headers_mut()
        .insert(header::CONTENT_LENGTH, (MAX_BYTES + 1).into());
    *req.body_mut() = Body::from_stream(stream::poll_fn(
        |_| -> std::task::Poll<Option<Result<Bytes, std::io::Error>>> {
            panic!("oversized request body polled");
        },
    ));
    assert_eq!(send(&hooks, req).await.status(), 413);
}

#[tokio::test]
async fn body_limit_4mib_route_level() {
    let hooks = fixture(Some(SECRET)).await;
    let mut body = ping();
    body.extend(std::iter::repeat_n(' ', MAX_BYTES - body.len()));
    assert_eq!(
        send(
            &hooks,
            request(&hooks, "ping", Uuid::new_v4(), &body, SECRET)
        )
        .await
        .status(),
        200
    );
    let mut req = request(&hooks, "ping", Uuid::new_v4(), &body, SECRET);
    req.headers_mut().remove(header::CONTENT_LENGTH);
    *req.body_mut() = Body::from(body + " ");
    let response = send(&hooks, req).await;
    assert_eq!(response.status(), 413);
    assert_eq!(response.headers()[header::CONNECTION], "close");
}

#[tokio::test]
async fn slow_body_times_out() {
    let hooks = fixture(Some(SECRET)).await;
    tokio::time::pause();
    let mut req = request(&hooks, "ping", Uuid::new_v4(), &ping(), SECRET);
    *req.body_mut() = Body::from_stream(stream::pending::<Result<Bytes, std::io::Error>>());
    let started = Instant::now();
    let response = send(&hooks, req).await;
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()[header::CONNECTION], "close");
    let limit = Duration::from_secs(8);
    assert!((limit..=limit + Duration::from_millis(1)).contains(&started.elapsed()));
}

#[tokio::test]
async fn absent_secret_server_starts_route_503_logged_once() {
    let hooks = fixture(None).await;
    let (capture, _subscriber) = LogCapture::start_scoped();
    let (_stop, stopped) = watch::channel(false);
    assert!(hooks.spawn(worker_state(&hooks), stopped).is_none());
    for _ in 0..2 {
        let response = send(
            &hooks,
            request(&hooks, "ping", Uuid::new_v4(), "bad json", PREVIOUS),
        )
        .await;
        assert_eq!(response.status(), 503);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap()).unwrap();
        assert_eq!(body["code"], "webhook_not_configured");
    }
    assert_eq!(
        capture
            .text()
            .matches("GitHub webhook secret not set")
            .count(),
        1
    );
}

#[tokio::test]
async fn previous_secret_accepted_during_rotation() {
    let hooks = fixture(Some(SECRET)).await;
    let (capture, _subscriber) = LogCapture::start_scoped();
    for (secret, padding) in [(PREVIOUS, " "), (SECRET, "")] {
        assert_eq!(
            send(
                &hooks,
                request(&hooks, "ping", Uuid::new_v4(), &(ping() + padding), secret)
            )
            .await
            .status(),
            200
        );
    }
    let logs = capture.text();
    assert!(logs.contains("secret=previous"), "{logs}");
    assert!(logs.contains("secret=current"), "{logs}");
}

#[tokio::test]
async fn ping_ok() {
    let hooks = fixture(Some(SECRET)).await;
    let (capture, _subscriber) = LogCapture::start_scoped();
    let id = Uuid::new_v4();
    assert_eq!(
        send(&hooks, request(&hooks, "ping", id, &ping(), SECRET))
            .await
            .status(),
        200
    );
    assert!(
        hooks
            .db
            .github()
            .last_delivery_at()
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        hooks
            .db
            .github()
            .next_pending(Timestamp::now())
            .await
            .unwrap()
            .is_none()
    );
    let logs = capture.text();
    for expected in ["hook_id=123", "issues", "pull_request", "outcome=ignored"] {
        assert!(logs.contains(expected), "{logs}");
    }
}

#[tokio::test]
async fn duplicate_delivery_200_noop() {
    let hooks = fixture(Some(SECRET)).await;
    track(&hooks, "acme").await;
    let id = Uuid::new_v4();
    let body = signal("acme", 7);
    let send_it = || send(&hooks, request(&hooks, "pull_request", id, &body, SECRET));
    assert_eq!(send_it().await.status(), 202);
    let pending = hooks
        .db
        .github()
        .next_pending(Timestamp::now())
        .await
        .unwrap()
        .unwrap();
    hooks
        .db
        .github()
        .finish(&pending, Timestamp::now())
        .await
        .unwrap();
    assert_eq!(send_it().await.status(), 200);
    assert!(
        hooks
            .db
            .github()
            .next_pending(Timestamp::now())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn same_body_new_guid_is_duplicate() {
    let hooks = fixture(Some(SECRET)).await;
    track(&hooks, "acme").await;
    let body = signal("acme", 7);
    for status in [202, 200] {
        assert_eq!(
            send(
                &hooks,
                request(&hooks, "pull_request", Uuid::new_v4(), &body, SECRET)
            )
            .await
            .status(),
            status
        );
    }
}

#[tokio::test]
async fn delivery_for_untracked_repo_dropped_and_counted() {
    let hooks = fixture(Some(SECRET)).await;
    let (capture, _subscriber) = LogCapture::start_scoped();
    for id in [1, 2] {
        assert_eq!(
            send(
                &hooks,
                request(
                    &hooks,
                    "pull_request",
                    Uuid::new_v4(),
                    &(signal("untracked", 7) + &" ".repeat(id)),
                    SECRET
                )
            )
            .await
            .status(),
            202
        );
    }
    let (last, dropped) = hooks
        .metrics
        .status(&hooks.db, Timestamp::now())
        .await
        .unwrap();
    assert_eq!(hooks.db.github().last_delivery_at().await.unwrap(), None);
    let future = Timestamp::now() + super::DROP_RETENTION;
    assert_eq!(
        hooks.metrics.status(&hooks.db, future).await.unwrap(),
        (None, 0)
    );
    assert!(last.is_some());
    assert_eq!(dropped, 2);
    assert!(
        hooks
            .db
            .github()
            .next_pending(Timestamp::now())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(capture.text().matches("outcome=not_allowed").count(), 1);
}

#[tokio::test]
async fn repo_tracked_via_owners_to_list_processed() {
    let hooks = fixture(Some(SECRET)).await;
    track(&hooks, "ACME").await;
    assert_eq!(
        send(
            &hooks,
            request(
                &hooks,
                "pull_request",
                Uuid::new_v4(),
                &signal("acme", 7),
                SECRET
            )
        )
        .await
        .status(),
        202
    );
    timeout(Duration::from_millis(100), hooks.wake.notified())
        .await
        .unwrap();
    let pending = hooks
        .db
        .github()
        .next_pending(Timestamp::now())
        .await
        .unwrap()
        .unwrap();
    let change = &pending.changes[0];
    assert_eq!(change.number, Some(482));
    assert_eq!(change.repository.id, Some(7));
    assert_eq!(change.installation_id, Some(10));
}

#[tokio::test]
async fn check_run_success_ignored_at_receipt() {
    let hooks = fixture(Some(SECRET)).await;
    track(&hooks, "acme").await;
    for (event, action, conclusion) in [
        ("check_run", "completed", "success"),
        ("check_run", "created", "failure"),
        ("pull_request", "locked", ""),
        ("github_app_authorization", "revoked", ""),
        ("projects_v2", "created", ""),
    ] {
        let id = Uuid::new_v4();
        let mut event_body: Value = serde_json::from_str(&signal("acme", 7)).unwrap();
        event_body["action"] = json!(action);
        event_body["check_run"] = json!({"conclusion": conclusion});
        let body = event_body.to_string();
        assert_eq!(
            send(&hooks, request(&hooks, event, id, &body, SECRET))
                .await
                .status(),
            202
        );
        assert_eq!(
            send(&hooks, request(&hooks, event, id, &body, SECRET))
                .await
                .status(),
            200
        );
    }
    assert!(
        hooks
            .db
            .github()
            .next_pending(Timestamp::now())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        hooks
            .metrics
            .status(&hooks.db, Timestamp::now())
            .await
            .unwrap()
            .1,
        0
    );
}

#[tokio::test]
async fn logs_never_contain_body_or_signature() {
    let hooks = fixture(Some(SECRET)).await;
    let (capture, _subscriber) = LogCapture::start_scoped();
    let body = ping();
    let accepted = request(&hooks, "ping", Uuid::new_v4(), &body, SECRET);
    let signature = accepted.headers()["x-hub-signature-256"]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(send(&hooks, accepted).await.status(), 200);
    for _ in 0..2 {
        assert_eq!(
            send(
                &hooks,
                request(&hooks, "ping", Uuid::new_v4(), &body, "wrong")
            )
            .await
            .status(),
            401
        );
    }
    let logs = capture.text();
    assert_eq!(logs.matches("GitHub webhook signature rejected").count(), 1);
    assert!(!logs.contains("payload-text-not-for-logs"), "{logs}");
    assert!(!logs.contains(&signature), "{logs}");
    assert!(!logs.contains(SECRET), "{logs}");
}

#[tokio::test]
async fn cf_connecting_ip_never_becomes_client_address() {
    let hooks = fixture(Some(SECRET)).await;
    let (capture, _subscriber) = LogCapture::start_scoped();
    let mut invalid = request(&hooks, "ping", Uuid::new_v4(), &ping(), "wrong");
    invalid
        .headers_mut()
        .insert("cf-connecting-ip", "203.0.113.19".parse().unwrap());
    assert_eq!(send(&hooks, invalid).await.status(), 401);
    let mut valid = request(&hooks, "ping", Uuid::new_v4(), &ping(), SECRET);
    valid
        .headers_mut()
        .insert("cf-connecting-ip", "arbitrary-header-text".parse().unwrap());
    assert_eq!(send(&hooks, valid).await.status(), 200);
    let logs = capture.text();
    assert!(logs.contains("cf_ip=203.0.113.19"), "{logs}");
    assert!(!logs.contains("client_address"), "{logs}");
    assert!(!logs.contains("arbitrary-header-text"), "{logs}");
}

async fn occupy(app: &Router, hooks: &Webhooks) -> Vec<JoinHandle<()>> {
    let started = Arc::new(Semaphore::new(0));
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let mut req = request(hooks, "ping", Uuid::new_v4(), &ping(), SECRET);
        let started = started.clone();
        *req.body_mut() = Body::from_stream(stream::once(async move {
            started.add_permits(1);
            std::future::pending::<Result<Bytes, std::io::Error>>().await
        }));
        let app = app.clone();
        tasks.push(tokio::spawn(async move {
            let _ = app.oneshot(req).await;
        }));
    }
    timeout(Duration::from_secs(1), started.acquire_many(8))
        .await
        .unwrap()
        .unwrap()
        .forget();
    tasks
}

#[tokio::test]
async fn full_handler_sheds_503() {
    let hooks = fixture(Some(SECRET)).await;
    let app = webhook_router(hooks.clone());
    let tasks = occupy(&app, &hooks).await;
    let response = timeout(
        Duration::from_millis(200),
        app.oneshot(request(&hooks, "ping", Uuid::new_v4(), &ping(), SECRET)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), 503);
    for task in tasks {
        task.abort();
    }
}

async fn listen(app: Router) -> (SocketAddr, watch::Sender<bool>, JoinHandle<()>) {
    listen_with_metrics(app, super::WebhookMetrics::default()).await
}

async fn listen_with_metrics(
    app: Router,
    metrics: super::WebhookMetrics,
) -> (SocketAddr, watch::Sender<bool>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = watch::channel(false);
    (
        addr,
        stop,
        tokio::spawn(serve(listener, app, metrics, stopped)),
    )
}

async fn assert_drained(addr: SocketAddr, method: &str, path: &str, headers: &str, status: u16) {
    let mut client = TcpStream::connect(addr).await.unwrap();
    client.write_all(format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: 61440\r\nConnection: close\r\n{headers}\r\n").as_bytes()).await.unwrap();
    client.write_all(&[0; 1024]).await.unwrap();
    let mut probe = [0];
    assert!(
        timeout(Duration::from_millis(60), client.read(&mut probe))
            .await
            .is_err(),
        "server answered before the entire request body was sent ({status})"
    );
    for _ in 1..60 {
        client.write_all(&[0; 1024]).await.unwrap();
    }
    let mut response = String::new();
    timeout(Duration::from_secs(2), client.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(
        response.starts_with(&format!("HTTP/1.1 {status} ")),
        "{response}"
    );
}

#[tokio::test]
async fn early_answer_drains_slow_streamed_body() {
    let hooks = fixture(Some(SECRET)).await;
    let app = webhook_router(hooks.clone());
    let (addr, _stop, _server) = listen(app.clone()).await;
    for (method, path, headers, status) in [
        ("POST", "/unknown", "", 404),
        ("PUT", PATH, "", 405),
        ("POST", PATH, "", 415),
        (
            "POST",
            PATH,
            "Content-Type: application/json\r\nX-GitHub-Event: ping\r\nX-GitHub-Delivery: 12345678-1234-1234-1234-123456789abc\r\n",
            401,
        ),
    ] {
        assert_drained(addr, method, path, headers, status).await;
    }
    let (unconfigured, _stop_absent, _absent) = listen(webhook_router(fixture(None).await)).await;
    assert_drained(unconfigured, "POST", PATH, "", 503).await;
    let occupied = occupy(&app, &hooks).await;
    assert_drained(addr, "PUT", PATH, "", 405).await;
    assert_drained(addr, "POST", PATH, "", 503).await;
    for task in occupied {
        task.abort();
    }
}

#[tokio::test]
async fn oversized_content_length_413_connection_close() {
    let (addr, _stop, _server) = listen(webhook_router(fixture(None).await)).await;
    let mut client = TcpStream::connect(addr).await.unwrap();
    client
        .write_all(
            format!(
                "POST {PATH} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
                MAX_BYTES + 1
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = String::new();
    timeout(Duration::from_secs(1), client.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.starts_with("HTTP/1.1 413 "), "{response}");
    assert!(response.contains("connection: close"), "{response}");
}

#[tokio::test]
async fn drain_stops_at_limit_and_timeout_with_connection_close() {
    let hooks = fixture(Some(SECRET)).await;
    tokio::time::pause();
    let oversized = Body::from_stream(
        stream::iter([Ok::<_, std::io::Error>(Bytes::from(vec![0; MAX_BYTES + 1]))])
            .chain(stream::pending()),
    );
    let pending = Body::from_stream(stream::pending::<Result<Bytes, std::io::Error>>());
    for (body, elapsed) in [
        (oversized, Duration::ZERO),
        (pending, Duration::from_secs(8)),
    ] {
        let started = Instant::now();
        let response = send(&hooks, Request::post("/unknown").body(body).unwrap()).await;
        assert_eq!(response.status(), 404);
        assert_eq!(response.headers()[header::CONNECTION], "close");
        assert!((elapsed..=elapsed + Duration::from_millis(1)).contains(&started.elapsed()));
    }
}

#[tokio::test]
async fn drain_does_not_take_a_handler_slot() {
    let hooks = fixture(Some(SECRET)).await;
    let app = webhook_router(hooks.clone());
    let started = Arc::new(Semaphore::new(0));
    let mut drains = Vec::new();
    for _ in 0..8 {
        let started = started.clone();
        let body = Body::from_stream(stream::once(async move {
            started.add_permits(1);
            std::future::pending::<Result<Bytes, std::io::Error>>().await
        }));
        let invalid = Request::post(PATH).body(body).unwrap();
        drains.push(tokio::spawn(app.clone().oneshot(invalid)));
    }
    timeout(Duration::from_secs(1), started.acquire_many(8))
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert_eq!(
        timeout(
            Duration::from_secs(1),
            app.oneshot(request(&hooks, "ping", Uuid::new_v4(), &ping(), SECRET))
        )
        .await
        .unwrap()
        .unwrap()
        .status(),
        200
    );
    for task in drains {
        task.abort();
    }
}

#[tokio::test]
async fn slow_http_headers_are_closed_after_five_seconds() {
    let (addr, _stop, _server) = listen(webhook_router(fixture(None).await)).await;
    let mut client = TcpStream::connect(addr).await.unwrap();
    client
        .write_all(b"POST /webhooks/github HTTP/1.1\r\nHost: ")
        .await
        .unwrap();
    let mut response = Vec::new();
    timeout(Duration::from_secs(6), client.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.is_empty(), "{response:?}");
}

#[tokio::test]
async fn webhook_status_reports_audit_to_signed_in_users() {
    use contracts::routes::GET_GITHUB_WEBHOOK;
    use db::repo::analyses::testing::seed_analysis;

    let mut config = test_config();
    config.github_webhook_secret = Some(SECRET.into());
    config.github_webhook_url = Some(
        "https://webhooks.example.test/diffpilot/github"
            .parse()
            .unwrap(),
    );
    let db = Db::in_memory().await.unwrap();
    seed_analysis(&db).await.unwrap();
    let state = crate::state::AppState::with_queue(config, db);
    let cookie = crate::test_support::session_cookie(&state, DEFAULT_TENANT).await;
    let hooks = Webhooks::from_app(&state);
    assert_eq!(
        send(
            &hooks,
            request(
                &hooks,
                "pull_request",
                Uuid::new_v4(),
                &signal("foreign", 7),
                SECRET
            )
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    let app = crate::api::router(state);
    let req = || Request::get(GET_GITHUB_WEBHOOK.path);
    assert_eq!(
        app.clone()
            .oneshot(req().body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status(),
        401
    );
    let response = app
        .oneshot(
            req()
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let status: contracts::api::settings::GithubWebhook =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap()).unwrap();
    assert!(status.configured);
    assert_eq!(
        status.webhook_url.as_deref(),
        Some("https://webhooks.example.test/diffpilot/github")
    );
    assert!(status.last_delivery_at.is_some());
    assert_eq!(status.dropped_deliveries, 1);
    assert_eq!(status.refused_connections, 0);
}

#[tokio::test]
async fn worker_drains_a_queued_installation_delivery() {
    let hooks = fixture(Some(SECRET)).await;
    track(&hooks, "acme").await;
    let body = json!({"action":"created","installation":{"id":10},"repositories":[
        {"id":7,"full_name":"acme/code"},{"id":8,"full_name":"foreign/code"}
    ]})
    .to_string();
    let id = Uuid::new_v4();
    let delivery = || request(&hooks, "installation", id, &body, SECRET);
    let (stop, stopped) = watch::channel(false);
    let worker = hooks.spawn(worker_state(&hooks), stopped).unwrap();
    assert_eq!(send(&hooks, delivery()).await.status(), 202);
    timeout(Duration::from_secs(1), async {
        while hooks
            .db
            .github()
            .next_pending(Timestamp::now())
            .await
            .unwrap()
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(send(&hooks, delivery()).await.status(), 200);
    stop.send(true).unwrap();
    worker.await.unwrap();
}

#[tokio::test]
async fn repository_name_fallback_requires_an_absent_repository_object() {
    let hooks = fixture(Some(SECRET)).await;
    track(&hooks, "acme").await;
    let mut payload = json!({
        "action":"opened",
        "issue":{"number":17,"repository_url":"https://api.github.com/repos/acme/code"},
        "repository":{"full_name":"acme/code"}
    });
    let send_payload = |payload: &Value| {
        send(
            &hooks,
            request(
                &hooks,
                "issues",
                Uuid::new_v4(),
                &payload.to_string(),
                SECRET,
            ),
        )
    };
    assert_eq!(send_payload(&payload).await.status(), 400);
    payload.as_object_mut().unwrap().remove("repository");
    assert_eq!(send_payload(&payload).await.status(), 202);
    let pending = hooks
        .db
        .github()
        .next_pending(Timestamp::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.changes[0].number, Some(17));
    assert_eq!(pending.changes[0].repository.id, None);
}

#[tokio::test]
async fn webhook_chunked_delivery_is_accepted_without_content_length() {
    let hooks = fixture(Some(SECRET)).await;
    let body = ping();
    let req = request(&hooks, "ping", Uuid::new_v4(), &body, SECRET);
    let mut headers = String::new();
    for (name, value) in req.headers() {
        if name != header::CONTENT_LENGTH {
            headers.push_str(&format!("{name}: {}\r\n", value.to_str().unwrap()));
        }
    }
    let (addr, _stop, _server) = listen(webhook_router(hooks)).await;
    let mut client = TcpStream::connect(addr).await.unwrap();
    let (first, second) = body.split_at(body.len() / 2);
    let message = format!(
        "POST {PATH} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n{headers}\r\n{:x}\r\n{first}\r\n{:x}\r\n{second}\r\n0\r\n\r\n",
        first.len(),
        second.len(),
    );
    client.write_all(message.as_bytes()).await.unwrap();
    let mut response = String::new();
    timeout(Duration::from_secs(2), client.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.starts_with("HTTP/1.1 200 "), "{response}");
}

#[tokio::test]
async fn webhook_rejects_missing_event_and_invalid_delivery_before_signature() {
    let hooks = fixture(Some(SECRET)).await;
    for (name, value) in [
        ("x-github-event", None),
        ("x-github-delivery", Some("not-a-uuid")),
    ] {
        let mut req = request(&hooks, "ping", Uuid::new_v4(), &ping(), SECRET);
        req.headers_mut().remove("x-hub-signature-256");
        req.headers_mut().remove(name);
        if let Some(value) = value {
            req.headers_mut().insert(name, value.parse().unwrap());
        }
        assert_eq!(send(&hooks, req).await.status(), 400);
    }
}

#[tokio::test]
async fn verified_webhook_payload_errors_are_audited_and_warn_rate_limited() {
    let hooks = fixture(Some(SECRET)).await;
    let (logs, _subscriber) = LogCapture::start_scoped();
    for body in [
        "raw-payload-must-stay-private",
        "false-payload-must-stay-private",
        "null",
        "[]",
        r#"{"action":"opened","number":1}"#,
        "{}",
    ] {
        let response = send(
            &hooks,
            request(&hooks, "pull_request", Uuid::new_v4(), body, SECRET),
        )
        .await;
        // An object with no action is ignored. A syntactically invalid body and a non-object are bad requests.
        assert_eq!(response.status(), if body == "{}" { 202 } else { 400 });
        if body != "{}" {
            assert!(
                hooks
                    .db
                    .github()
                    .last_delivery_at()
                    .await
                    .unwrap()
                    .is_some()
            );
        }
    }
    let logs = logs.text();
    assert_eq!(logs.matches("code=invalid_json").count(), 1, "{logs}");
    assert_eq!(logs.matches("code=invalid_payload").count(), 1, "{logs}");
    assert!(!logs.contains("raw-payload-must-stay-private"));
}

#[tokio::test]
async fn webhook_connection_limit_refuses_excess_tcp_connections() {
    use contracts::routes::GET_GITHUB_WEBHOOK;
    use db::repo::analyses::testing::seed_analysis;

    let mut config = test_config();
    config.github_webhook_secret = Some(SECRET.into());
    let db = Db::in_memory().await.unwrap();
    seed_analysis(&db).await.unwrap();
    let state = crate::state::AppState::with_queue(config, db);
    let cookie = crate::test_support::session_cookie(&state, DEFAULT_TENANT).await;
    let hooks = Webhooks::from_app(&state);
    let (logs, _subscriber) = LogCapture::start_scoped();
    let (addr, _stop, _server) =
        listen_with_metrics(webhook_router(hooks), state.webhook_metrics.clone()).await;
    let mut occupied = Vec::new();
    for _ in 0..64 {
        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(b"GET / HTTP/1.1\r\nHost: ").await.unwrap();
        occupied.push(client);
    }
    for _ in 0..2 {
        let mut excess = TcpStream::connect(addr).await.unwrap();
        let result = timeout(Duration::from_secs(1), excess.read(&mut [0u8]))
            .await
            .unwrap();
        assert!(matches!(result, Ok(0)) || result.is_err(), "{result:?}");
    }
    drop(occupied);
    let response = crate::api::router(state)
        .oneshot(
            Request::get(GET_GITHUB_WEBHOOK.path)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let status: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(status["refused_connections"], 2);
    let logs = logs.text();
    assert_eq!(
        logs.matches("GitHub webhook connection limit reached")
            .count(),
        1,
        "{logs}"
    );
    assert!(logs.contains("count=1"), "{logs}");
}

#[tokio::test]
async fn verified_wrong_hook_target_warns_once_and_remains_401() {
    let hooks = fixture(Some(SECRET)).await;
    let (logs, _subscriber) = LogCapture::start_scoped();
    for _ in 0..2 {
        let mut req = request(&hooks, "ping", Uuid::new_v4(), &ping(), SECRET);
        req.headers_mut().insert(
            "x-github-hook-installation-target-type",
            "repository".parse().unwrap(),
        );
        req.headers_mut().insert(
            "x-github-hook-installation-target-id",
            "456".parse().unwrap(),
        );
        assert_eq!(send(&hooks, req).await.status(), 401);
    }
    let logs = logs.text();
    assert_eq!(
        logs.matches("Verified GitHub webhook targets a different hook")
            .count(),
        1,
        "{logs}"
    );
    assert!(
        logs.contains("target_type=Some(\"repository\")") && logs.contains("target_id=Some(456)"),
        "{logs}"
    );
    assert_eq!(hooks.db.github().last_delivery_at().await.unwrap(), None);
}

#[tokio::test]
async fn webhook_duplicate_is_logged_at_info() {
    let hooks = fixture(Some(SECRET)).await;
    let (logs, _subscriber) = LogCapture::start_scoped();
    let delivery = Uuid::new_v4();
    for _ in 0..2 {
        assert_eq!(
            send(&hooks, request(&hooks, "ping", delivery, &ping(), SECRET))
                .await
                .status(),
            200
        );
    }
    assert!(
        logs.text()
            .lines()
            .any(|line| line.contains("INFO") && line.contains("outcome=duplicate"))
    );
}

#[tokio::test]
async fn webhook_installation_cap_warns_once() {
    let hooks = fixture(Some(SECRET)).await;
    track(&hooks, "acme").await;
    let (logs, _subscriber) = LogCapture::start_scoped();
    let repositories: Vec<_> = (1..=501)
        .map(|id| json!({"id":id, "full_name":format!("acme/repo-{id}")}))
        .collect();
    for id in [10, 20] {
        let body = json!({"action":"created","installation":{"id":id},"repositories":repositories})
            .to_string();
        assert_eq!(
            send(
                &hooks,
                request(&hooks, "installation", Uuid::new_v4(), &body, SECRET)
            )
            .await
            .status(),
            202
        );
    }
    let logs = logs.text();
    assert_eq!(logs.matches("code=installation_cap").count(), 1, "{logs}");
    assert!(
        hooks
            .db
            .github()
            .next_pending(Timestamp::now())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn webhook_worker_dispatch_failure_uses_backoff_and_warns_once() {
    use db::repo::github::DeliveryError;
    use jiff::SignedDuration;

    let hooks = fixture(Some(SECRET)).await;
    track(&hooks, "acme").await;
    let id = Uuid::new_v4();
    let req = request(&hooks, "pull_request", id, &signal("acme", 7), SECRET);
    assert_eq!(send(&hooks, req).await.status(), 202);
    let (logs, _subscriber) = LogCapture::start_scoped();
    let mut now =
        Timestamp::from(jiff::Timestamp::from_second(Timestamp::now().as_second() + 1).unwrap());
    let github = hooks.db.github();
    for (attempt, delay) in [10, 60, 600, 0].into_iter().enumerate() {
        let current = github.next_pending(now).await.unwrap().unwrap();
        assert_eq!((current.id, current.attempts), (id, attempt as i64));
        super::worker::complete(&hooks.db, &current, Err(DeliveryError::Dispatch), now)
            .await
            .unwrap();
        assert!(github.next_pending(now).await.unwrap().is_none());
        if delay != 0 {
            let retry = now + SignedDuration::from_secs(delay);
            assert_eq!(github.next_retry(now).await.unwrap(), Some(retry));
            assert!(
                github
                    .next_pending(retry - SignedDuration::from_secs(1))
                    .await
                    .unwrap()
                    .is_none()
            );
            assert_eq!(logs.text().matches("exhausted its retries").count(), 0);
            now = retry;
        }
    }
    assert!(github.next_retry(now).await.unwrap().is_none());
    assert_eq!(
        logs.text()
            .matches("GitHub webhook delivery exhausted its retries")
            .count(),
        1
    );
}
