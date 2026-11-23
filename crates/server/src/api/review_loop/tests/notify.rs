//! Notification channels (RN1–RN3), agent push targets (RF5) and the delivery task against in-test HTTP targets
//! (wiremock): message shapes, retries and health, outbound limits (plan §5.5; threats T28, T29).

use std::sync::Arc;

use contracts::api::review_loop::UserEventKind;
use contracts::ids::UserEventId;
use db::repo::review_loop::NewUserEvent;
use wiremock::matchers::header;

use super::feed::{EXECUTE, agent, fix_here, with_token};
use super::*;
use crate::review_loop::feed;
use crate::review_loop::notify::outbound::{Outbound, Reach, SendError};
use crate::review_loop::notify::{self, MESSAGES_PER_HOUR};
use crate::test_support::logs::LogCapture;

const CHANNELS: &str = "/api/v1/settings/notifications/channels";
const SECRET: &str = "tk_live_secret_9f2a";
const COMMENTS_PATH: &str = "/pr/acme/node-client/482/comments";
const COMMENTS_URL: &str = "https://review.example.test/pr/acme/node-client/482/comments";

async fn put_channel(w: &World, kind: &str, body: Value) -> (StatusCode, Value) {
    w.call("PUT", &format!("{CHANNELS}/{kind}"), Some(body))
        .await
}

async fn questions_ready(w: &World) -> UserEventId {
    w.state
        .db
        .review_loop()
        .add_user_event(
            DEFAULT_TENANT,
            &NewUserEvent {
                user_id: "u1".into(),
                kind: UserEventKind::QuestionsReady,
                pull_request_id: Some(w.pr_id().await),
                count: 5,
                text: "acme/node-client #482: 5 questions ready".into(),
                url: Some(COMMENTS_PATH.into()),
            },
            Timestamp::now(),
        )
        .await
        .expect("event")
}

async fn deliver(w: &World, at: Timestamp) {
    let outbound = Outbound::new(Reach::of(&w.state.config)).expect("client");
    notify::deliver_due(&w.state, &outbound, at)
        .await
        .expect("deliver");
}

async fn target(status: u16) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(status).set_body_string("x".repeat(64 * 1024)))
        .mount(&server)
        .await;
    server
}

async fn webhook(w: &World, server: &MockServer, secret: Option<&str>) {
    let mut body = json!({"url": format!("{}/api/webhook/diffpilot", server.uri()), "topic": null,
        "events": ["questions_ready", "round_executed"], "enabled": true});
    if let Some(secret) = secret {
        body["secret"] = json!({"action": "replace", "value": secret});
    }
    let (status, channel) = put_channel(w, "webhook", body).await;
    assert_eq!(status, StatusCode::OK, "{channel}");
}

#[tokio::test]
async fn ntfy_message_has_title_body_and_click_link() {
    let w = world().await;
    let capture = LogCapture::start();
    w.get(&format!("{PR}/comments")).await;
    let server = target(200).await;
    let (status, channel) = put_channel(
        &w,
        "ntfy",
        json!({"url": server.uri(), "topic": "dp-alice", "secret": {"action": "replace", "value": SECRET},
            "events": ["questions_ready"], "enabled": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{channel}");
    questions_ready(&w).await;
    deliver(&w, Timestamp::now()).await;
    let received = server.received_requests().await.expect("recorded");
    assert_eq!(received.len(), 1);
    let message = &received[0];
    assert_eq!(message.url.path(), "/dp-alice");
    assert_eq!(message.headers["title"], "diffpilot");
    assert_eq!(message.headers["click"], COMMENTS_URL);
    assert_eq!(
        message.headers["authorization"],
        format!("Bearer {SECRET}").as_str()
    );
    assert_eq!(
        String::from_utf8_lossy(&message.body),
        "acme/node-client #482: 5 questions ready"
    );
    assert!(!capture.text().contains(SECRET), "a secret is never logged");
    let settings = w.get("/api/v1/settings/notifications").await;
    let delivery = &settings["channels"][0]["delivery"];
    assert_eq!(delivery["last"]["event"], "questions_ready");
    assert_eq!(delivery["last"]["outcome"], "delivered");
    assert_eq!(delivery["failing_since"], Value::Null);
}

#[tokio::test]
async fn webhook_payload_has_fixed_shape() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let server = target(204).await;
    webhook(&w, &server, Some("Bearer ha-long-lived-token")).await;
    questions_ready(&w).await;
    deliver(&w, Timestamp::now()).await;
    let received = server.received_requests().await.expect("recorded");
    assert_eq!(received.len(), 1);
    assert_eq!(
        received[0].headers["authorization"],
        "Bearer ha-long-lived-token"
    );
    assert_eq!(received[0].headers["content-type"], "application/json");
    let payload: Value = serde_json::from_slice(&received[0].body).expect("json");
    let keys: Vec<&str> = payload
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.len(), 6, "{payload}");
    for key in ["v", "event", "text", "url", "pr", "at"] {
        assert!(keys.contains(&key), "{key}: {payload}");
    }
    assert_eq!(payload["v"], 1);
    assert_eq!(payload["event"], "questions_ready");
    assert_eq!(payload["pr"], "acme/node-client#482");
    assert_eq!(payload["url"], COMMENTS_URL);
}

#[tokio::test]
async fn delivery_tries_three_times_then_fails() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let server = target(503).await;
    webhook(&w, &server, None).await;
    questions_ready(&w).await;
    let start = Timestamp::now();
    for (offset, attempts) in [(0, 1), (30, 1), (61, 2), (200, 2), (301, 3), (3600, 3)] {
        deliver(&w, start + SignedDuration::from_secs(offset)).await;
        let received = server.received_requests().await.expect("recorded").len();
        assert_eq!(received, attempts, "after {offset} s");
    }
    let settings = w.get("/api/v1/settings/notifications").await;
    let delivery = &settings["channels"][0]["delivery"];
    assert_eq!(delivery["last"]["outcome"], "failed");
    assert_eq!(delivery["last_error"], "HTTP 503 (3 attempts)");
    assert!(delivery["failing_since"].is_string(), "{delivery}");
    let body = settings.to_string();
    assert!(!body.contains("xxxxxxxx"), "the answer is never shown");
}

#[tokio::test]
async fn failure_logged_once_per_state_change() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let server = target(500).await;
    webhook(&w, &server, None).await;
    let capture = LogCapture::start();
    for _ in 0..2 {
        questions_ready(&w).await;
        // The event's delivery is due from when it was stored.
        let start = Timestamp::now();
        for offset in [0, 61, 301] {
            deliver(&w, start + SignedDuration::from_secs(offset)).await;
        }
    }
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    for _ in 0..2 {
        questions_ready(&w).await;
        deliver(&w, Timestamp::now()).await;
    }
    let text = capture.text();
    assert_eq!(
        text.matches("a notification target started failing")
            .count(),
        1,
        "{text}"
    );
    assert_eq!(
        text.matches("a failing notification target delivers again")
            .count(),
        1,
        "{text}"
    );
}

#[tokio::test]
async fn notify_does_not_follow_redirects() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "https://elsewhere.example/hook"),
        )
        .mount(&server)
        .await;
    webhook(&w, &server, None).await;
    let (status, result) = w
        .post(&format!("{CHANNELS}/webhook/test"), json!(null))
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["outcome"], "failed");
    assert_eq!(result["http_status"], 302);
    assert_eq!(
        result["error"],
        "redirect to elsewhere.example not followed"
    );
    assert_eq!(server.received_requests().await.expect("recorded").len(), 1);
}

/// A POST to `localhost` (the in-test target) through a client with `reach`.
async fn post_to_localhost(server: &MockServer, reach: Reach) -> Result<u16, SendError> {
    let port = server.address().port();
    Outbound::new(reach)
        .expect("client")
        .post(
            &url::Url::parse(&format!("http://localhost:{port}/hook")).expect("url"),
            reqwest::header::HeaderMap::new(),
            Vec::new(),
        )
        .await
}

#[tokio::test]
async fn notify_denies_private_when_configured() {
    let w = world_with(|state| {
        Arc::get_mut(&mut state.config)
            .expect("unique config")
            .notify_deny_private = true;
    })
    .await;
    let (status, error) = put_channel(
        &w,
        "webhook",
        json!({"url": "http://192.168.1.10:8123/api/webhook/x", "events": [], "enabled": true}),
    )
    .await;
    assert_eq!(
        (status, &error["code"]),
        (StatusCode::BAD_REQUEST, &json!("invalid_request")),
        "{error}"
    );
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|m| m.contains("private")),
        "{error}"
    );
}

/// Loopback, link-local and reserved addresses are refused whatever `SR_NOTIFY_DENY_PRIVATE` says: in the URL of a
/// channel or push target, and when a host name resolves only to them.
#[tokio::test]
async fn notify_refuses_loopback_and_link_local_even_when_private_allowed() {
    let w = world_with(|state| {
        Arc::get_mut(&mut state.config)
            .expect("unique config")
            .notify_loopback = false;
    })
    .await;
    assert!(!w.state.config.notify_deny_private);
    let (token, _) = agent(&w, &EXECUTE).await;
    for url in [
        "http://127.0.0.1:8123/api/webhook/x",
        "http://[::1]/hook",
        "http://169.254.169.254/latest/meta-data",
        "http://[fe80::1]/hook",
        "http://0.0.0.0/hook",
        "http://198.18.0.1/hook",
        "http://[64:ff9b::7f00:1]/hook",
        "http://[2002:a9fe:a9fe::1]/hook",
    ] {
        let (status, error) = put_channel(
            &w,
            "webhook",
            json!({"url": url, "events": [], "enabled": true}),
        )
        .await;
        assert_eq!(
            (status, &error["code"]),
            (StatusCode::BAD_REQUEST, &json!("invalid_request")),
            "{url}: {error}"
        );
        let (status, _, error) = with_token(
            &w,
            &token,
            "PUT",
            "/api/v1/decision-feed/push-target",
            Some(json!({"kind": "webhook", "url": url})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{url}: {error}");
    }
    let (status, saved) = put_channel(
        &w,
        "webhook",
        json!({"url": "http://192.168.1.10:8123/api/webhook/x", "events": [], "enabled": true}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a LAN address stays allowed: {saved}"
    );
    let server = target(200).await;
    for reach in [
        Reach::of(&w.state.config),
        Reach {
            private: false,
            loopback: false,
        },
    ] {
        let sent = post_to_localhost(&server, reach).await;
        assert!(matches!(sent, Err(SendError::Unreachable)), "{sent:?}");
    }
    assert!(
        server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty()
    );
}

#[tokio::test]
async fn channel_with_non_http_scheme_is_422() {
    let w = world().await;
    for url in ["ftp://ntfy.example", "file:///etc/passwd"] {
        let (status, error) = put_channel(
            &w,
            "ntfy",
            json!({"url": url, "topic": "t", "events": [], "enabled": true}),
        )
        .await;
        assert_eq!(
            (status, &error["code"]),
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                &json!("scheme_not_allowed")
            ),
            "{url}"
        );
    }
    let (status, _) = put_channel(
        &w,
        "ntfy",
        json!({"url": "https://ntfy.sh", "topic": "bad topic", "events": [], "enabled": true}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// T28 and G8: the secret is masked in every response and never logged; an absent field keeps it and
/// `remove` deletes it.
#[tokio::test]
async fn channel_secret_is_masked_in_responses() {
    let w = world().await;
    let capture = LogCapture::start();
    let body = |secret: Value| {
        json!({"url": "https://ntfy.sh", "topic": "dp-alice", "secret": secret,
            "events": ["questions_ready"], "enabled": true})
    };
    let (status, saved) = put_channel(
        &w,
        "ntfy",
        body(json!({"action": "replace", "value": SECRET})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["secret_last4"], "9f2a");
    let settings = w.get("/api/v1/settings/notifications").await;
    assert_eq!(settings["channels"][0]["secret_last4"], "9f2a");
    let mut kept = body(Value::Null);
    kept.as_object_mut().expect("object").remove("secret");
    let (_, kept) = put_channel(&w, "ntfy", kept).await;
    assert_eq!(kept["secret_last4"], "9f2a", "an absent field keeps it");
    let (_, kept) = put_channel(&w, "ntfy", body(json!({"action": "keep"}))).await;
    assert_eq!(kept["secret_last4"], "9f2a");
    for leaked in [
        saved.to_string(),
        settings.to_string(),
        kept.to_string(),
        capture.text(),
    ] {
        assert!(!leaked.contains(SECRET), "{leaked}");
    }
    let (status, error) = put_channel(
        &w,
        "ntfy",
        body(json!({"action": "replace", "value": "short"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    let (_, removed) = put_channel(&w, "ntfy", body(json!({"action": "remove"}))).await;
    assert_eq!(removed["secret_last4"], Value::Null);
    let (status, _) = w.call("DELETE", &format!("{CHANNELS}/ntfy"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let settings = w.get("/api/v1/settings/notifications").await;
    assert_eq!(settings["channels"], json!([]));
}

#[tokio::test]
async fn test_button_sends_one_attempt_and_records_it() {
    let w = world().await;
    let server = target(200).await;
    let (status, error) = w
        .post(&format!("{CHANNELS}/webhook/test"), json!(null))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{error}");
    webhook(&w, &server, None).await;
    let (status, result) = w
        .post(&format!("{CHANNELS}/webhook/test"), json!(null))
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        result,
        json!({"outcome": "delivered", "http_status": 200, "error": null})
    );
    let received = server.received_requests().await.expect("recorded");
    let payload: Value = serde_json::from_slice(&received[0].body).expect("json");
    assert_eq!(payload["event"], "test");
    assert_eq!(payload["pr"], Value::Null);
    let settings = w.get("/api/v1/settings/notifications").await;
    assert_eq!(settings["channels"][0]["delivery"]["last"]["event"], "test");
    // Tokens never reach the channel settings.
    let (token, _) = agent(&w, &EXECUTE).await;
    let (status, _, _) =
        with_token(&w, &token, "GET", "/api/v1/settings/notifications", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// T28: an agent push carries ids and the fetch path only; its secret goes in the `Authorization` header.
#[tokio::test]
async fn push_payload_has_no_secret_or_text() {
    let w = world().await;
    feed::sync(&w.state).await.expect("initial sync");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("authorization", "Bearer agent-push-secret-1"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let (token, _) = agent(&w, &EXECUTE).await;
    let (status, _, target) = with_token(
        &w,
        &token,
        "PUT",
        "/api/v1/decision-feed/push-target",
        Some(
            json!({"kind": "webhook", "url": format!("{}/agent", server.uri()),
            "secret": {"action": "replace", "value": "Bearer agent-push-secret-1"}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{target}");
    assert_eq!(target["secret_last4"], "et-1");
    assert!(!target.to_string().contains("agent-push-secret"));
    with_token(
        &w,
        &token,
        "GET",
        "/api/v1/decision-feed?capabilities=fix,reply,resolve",
        None,
    )
    .await;
    let decision = fix_here(&w).await;
    feed::sync(&w.state).await.expect("sync");
    deliver(&w, Timestamp::now()).await;
    let received = server.received_requests().await.expect("recorded");
    assert_eq!(received.len(), 1);
    let payload: Value = serde_json::from_slice(&received[0].body).expect("json");
    let id = decision["id"].as_str().expect("id");
    assert_eq!(
        payload,
        json!({"v": 1, "event": "decision_available", "decision_id": id, "pr": "acme/node-client#482",
            "fetch": format!("/api/v1/decision-feed/decisions/{id}")})
    );
    let raw = String::from_utf8_lossy(&received[0].body);
    assert!(
        !raw.contains("keep the test small") && !raw.contains("secret"),
        "{raw}"
    );
    let log = w.get(&format!("{PR}/comments/log")).await;
    let push = &log["subscribers"][0]["push"];
    assert_eq!(push["delivery"]["last"]["outcome"], "delivered", "{log}");
    assert_eq!(push["secret_last4"], "et-1");
    let (status, _, _) = with_token(
        &w,
        &token,
        "DELETE",
        "/api/v1/decision-feed/push-target",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

/// A kept secret never follows its channel to another origin: a changed scheme, host or port asks for `replace` or
/// `remove` (422 `secret_required`); the same origin keeps it.
#[tokio::test]
async fn url_change_with_kept_secret_is_refused() {
    let w = world().await;
    let body = |url: &str, secret: Value| {
        json!({"url": url, "topic": "dp-alice", "secret": secret, "events": ["questions_ready"],
            "enabled": true})
    };
    let replace = json!({"action": "replace", "value": SECRET});
    let (status, _) = put_channel(&w, "ntfy", body("https://ntfy.sh", replace.clone())).await;
    assert_eq!(status, StatusCode::OK);
    for moved in [
        "https://ntfy.attacker.example",
        "http://ntfy.sh",
        "https://ntfy.sh:8443",
    ] {
        let (status, error) = put_channel(&w, "ntfy", body(moved, json!({"action": "keep"}))).await;
        assert_eq!(
            (status, &error["code"]),
            (StatusCode::UNPROCESSABLE_ENTITY, &json!("secret_required")),
            "{moved}: {error}"
        );
        let mut absent = body(moved, Value::Null);
        absent.as_object_mut().expect("object").remove("secret");
        let (status, _) = put_channel(&w, "ntfy", absent).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{moved}");
    }
    let (status, kept) = put_channel(
        &w,
        "ntfy",
        body("https://ntfy.sh/", json!({"action": "keep"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "same origin: {kept}");
    assert_eq!(kept["secret_last4"], "9f2a");
    let (status, moved) = put_channel(&w, "ntfy", body("https://ntfy.example", replace)).await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    let (status, _) = put_channel(
        &w,
        "ntfy",
        body("https://ntfy.other.example", json!({"action": "remove"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = put_channel(
        &w,
        "ntfy",
        body("https://ntfy.sh", json!({"action": "keep"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "nothing kept, nothing to protect");
}

/// RF5: a kept push secret follows neither a changed kind (a webhook value sent as an ntfy token) nor another
/// origin.
#[tokio::test]
async fn kind_change_with_kept_secret_is_refused() {
    let w = world().await;
    let (token, _) = agent(&w, &EXECUTE).await;
    let put = |body: Value| {
        let w = &w;
        let token = token.clone();
        async move {
            with_token(
                w,
                &token,
                "PUT",
                "/api/v1/decision-feed/push-target",
                Some(body),
            )
            .await
        }
    };
    let (status, _, _) = put(
        json!({"kind": "webhook", "url": "https://hooks.example/agent",
        "secret": {"action": "replace", "value": "Bearer agent-push-secret-1"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for moved in [
        json!({"kind": "ntfy", "url": "https://hooks.example", "topic": "agent"}),
        json!({"kind": "webhook", "url": "https://elsewhere.example/agent"}),
        json!({"kind": "webhook", "url": "https://elsewhere.example/agent", "secret": {"action": "keep"}}),
    ] {
        let (status, _, error) = put(moved.clone()).await;
        assert_eq!(
            (status, &error["code"]),
            (StatusCode::UNPROCESSABLE_ENTITY, &json!("secret_required")),
            "{moved}: {error}"
        );
    }
    let (status, _, target) =
        put(json!({"kind": "webhook", "url": "https://hooks.example/other"})).await;
    assert_eq!(status, StatusCode::OK, "same origin: {target}");
    assert_eq!(target["secret_last4"], "et-1");
    let (status, _, target) = put(
        json!({"kind": "ntfy", "url": "https://ntfy.sh", "topic": "agent",
        "secret": {"action": "remove"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{target}");
    assert_eq!(target["secret_last4"], Value::Null);
}

/// The hourly limit counts stored sends: removing and saving the channel again, or a restart, does not reset it; a
/// Test press over it answers 429 and leaves the channel's health alone.
#[tokio::test]
async fn channel_limit_survives_delete_and_recreate() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let server = target(200).await;
    webhook(&w, &server, None).await;
    let (status, _) = w
        .post(&format!("{CHANNELS}/webhook/test"), json!(null))
        .await;
    assert_eq!(status, StatusCode::OK);
    let rl = w.state.db.review_loop();
    for _ in 1..MESSAGES_PER_HOUR {
        rl.take_send_slot(
            DEFAULT_TENANT,
            "channel:u1/webhook",
            MESSAGES_PER_HOUR,
            SignedDuration::from_hours(1),
            Timestamp::now(),
        )
        .await
        .expect("slot")
        .expect("under the limit");
    }
    let (status, _) = w.call("DELETE", &format!("{CHANNELS}/webhook"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    webhook(&w, &server, None).await;
    let request = Request::post(format!("{CHANNELS}/webhook/test"))
        .header(COOKIE, &w.cookie)
        .header("x-csrf-token", "csrf")
        .body(Body::empty())
        .expect("request");
    let response = w.app.clone().oneshot(request).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let wait: u64 = response.headers()["retry-after"]
        .to_str()
        .expect("text")
        .parse()
        .expect("seconds");
    assert!((3000..=3600).contains(&wait), "{wait}");
    let settings = w.get("/api/v1/settings/notifications").await;
    let delivery = &settings["channels"][0]["delivery"];
    assert_eq!(delivery["failing_since"], Value::Null, "{settings}");
    assert_eq!(delivery["last"], Value::Null, "{settings}");

    // After a restart (state rebuilt over the same database) a queued event waits too.
    let restarted = AppState::with_queue(crate::config::tests::test_config(), w.state.db.clone());
    questions_ready(&w).await;
    let outbound = Outbound::new(Reach::of(&restarted.config)).expect("client");
    notify::deliver_due(&restarted, &outbound, Timestamp::now())
        .await
        .expect("deliver");
    assert_eq!(server.received_requests().await.expect("recorded").len(), 1);
    let due = rl
        .due_deliveries(Timestamp::now() + SignedDuration::from_hours(1), 10)
        .await
        .expect("due");
    assert_eq!(due.len(), 1, "postponed, not dropped");
    assert_eq!(due[0].attempts, 0);
}

/// A push waits for nobody connected: an agent that opened its stream after the push was queued is not pushed.
#[tokio::test]
async fn push_is_dropped_once_the_agent_holds_a_stream() {
    let w = world().await;
    feed::sync(&w.state).await.expect("initial sync");
    let server = target(200).await;
    let (token, principal) = agent(&w, &EXECUTE).await;
    let (status, _, _) = with_token(
        &w,
        &token,
        "PUT",
        "/api/v1/decision-feed/push-target",
        Some(json!({"kind": "webhook", "url": format!("{}/agent", server.uri())})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    with_token(
        &w,
        &token,
        "GET",
        "/api/v1/decision-feed?capabilities=fix,reply,resolve",
        None,
    )
    .await;
    fix_here(&w).await;
    feed::sync(&w.state).await.expect("sync");
    let presence = w.state.feed.connect(&principal.token_id);
    deliver(&w, Timestamp::now()).await;
    assert!(
        server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty()
    );
    drop(presence);
}

/// RC10 shows another user's agent push target only as its kind and delivery state: its URL, ntfy topic, secret
/// tail and error text can be credentials.
#[tokio::test]
async fn subscriber_push_targets_of_other_users_are_hidden() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let (own, _) = agent(&w, &EXECUTE).await;
    let (status, _, _) = with_token(
        &w,
        &own,
        "PUT",
        "/api/v1/decision-feed/push-target",
        Some(
            json!({"kind": "webhook", "url": "https://ha.example/api/webhook/own-hook-id",
            "secret": {"action": "replace", "value": "Bearer own-secret-1234"}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let other_cookie = crate::test_support::other_user_cookie(&w.state).await;
    let (other, _) = crate::auth::access_tokens::stored_token(
        &w.state.db,
        "u2",
        EXECUTE.to_vec(),
        Timestamp::now() + SignedDuration::from_hours(24),
    )
    .await;
    let (status, _, target) = with_token(
        &w,
        &other,
        "PUT",
        "/api/v1/decision-feed/push-target",
        Some(
            json!({"kind": "ntfy", "url": "https://ntfy.sh", "topic": "colleague-secret-topic",
            "secret": {"action": "replace", "value": "tk_colleague_qzxw"}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{target}");
    let log = w.get(&format!("{PR}/comments/log")).await;
    let subscribers = log["subscribers"].as_array().expect("subscribers");
    assert_eq!(subscribers.len(), 2, "{log}");
    let text = log.to_string();
    for hidden in ["colleague-secret-topic", "ntfy.sh", "qzxw"] {
        assert!(!text.contains(hidden), "{hidden}: {log}");
    }
    let theirs = subscribers
        .iter()
        .find(|s| s["push"]["kind"] == "ntfy")
        .expect("other user's agent");
    assert_eq!(
        (
            &theirs["push"]["url"],
            &theirs["push"]["topic"],
            &theirs["push"]["secret_last4"]
        ),
        (&Value::Null, &Value::Null, &Value::Null)
    );
    assert!(theirs["push"]["delivery"].is_object());
    let mine = subscribers
        .iter()
        .find(|s| s["push"]["kind"] == "webhook")
        .expect("own agent");
    assert_eq!(
        mine["push"]["url"],
        "https://ha.example/api/webhook/own-hook-id"
    );
    assert_eq!(mine["push"]["secret_last4"], "1234");

    // The colleague sees it the other way round.
    let request = Request::get(format!("{PR}/comments/log"))
        .header(COOKIE, &other_cookie)
        .body(Body::empty())
        .expect("request");
    let response = w.app.clone().oneshot(request).await.expect("infallible");
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let theirs = String::from_utf8_lossy(&bytes);
    assert!(theirs.contains("colleague-secret-topic"), "{theirs}");
    assert!(!theirs.contains("own-hook-id"), "{theirs}");
}
