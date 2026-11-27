//! The decision feed (RF1–RF5) through its routes: claims, leases, reports, the stream with presence, push
//! wake-ups and `round_executed` (plan §5.2, §5.5; threats T22, T25, T28).

use std::time::Duration;

use axum::http::HeaderMap;
use axum::http::header::WWW_AUTHENTICATE;
use contracts::api::review_loop::{EffectKind, UserEventKind};
use contracts::api::review_loop::{FeedEvent, ReportOutcome};
use contracts::ids::DecisionId;
use db::repo::review_loop::{DeliveryTarget, ReportRequest};
use futures::StreamExt;

use super::*;
use crate::auth::TokenPrincipal;
use crate::review_loop::feed;

const FEED: &str = "/api/v1/decision-feed";
const ALL: &str = "fix,reply,resolve,pr_comment,follow_up_pr,upstream_issue,todo,dismiss_review";

/// A token of `u1` with `scopes`.
pub(super) async fn agent(w: &World, scopes: &[TokenScope]) -> (String, TokenPrincipal) {
    crate::auth::access_tokens::stored_token(
        &w.state.db,
        "u1",
        scopes.to_vec(),
        Timestamp::now() + SignedDuration::from_hours(24),
    )
    .await
}

pub(super) const EXECUTE: [TokenScope; 2] = [TokenScope::Read, TokenScope::ReviewLoopExecute];

/// A request with `token` as its only credential.
pub(super) async fn with_token(
    w: &World,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .expect("request");
    let response = w.app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The "Fix here" decision on L2 (effects fix, reply, resolve, all for an agent).
pub(super) async fn fix_here(w: &World) -> Value {
    w.get(&format!("{PR}/comments")).await;
    let mut body = decision("fix_here");
    body["comment"] = json!("keep the test small");
    w.decide_ok(L2, body).await
}

fn claim_uri(decision: &Value) -> String {
    format!(
        "{FEED}/decisions/{}/claim",
        decision["id"].as_str().expect("id")
    )
}

fn all_kinds() -> Value {
    json!({"capabilities": ["fix", "reply", "resolve"]})
}

fn report(outcome: &str, reference: Option<&str>, message: Option<&str>) -> Value {
    json!({"outcome": outcome, "reference": reference, "message": message,
        "idempotency_key": uuid::Uuid::new_v4()})
}

#[tokio::test]
async fn claim_conflict_is_409() {
    let w = world().await;
    let decision = fix_here(&w).await;
    let (a, _) = agent(&w, &EXECUTE).await;
    let (b, _) = agent(&w, &EXECUTE).await;
    let (status, _, claim) =
        with_token(&w, &a, "POST", &claim_uri(&decision), Some(all_kinds())).await;
    assert_eq!(status, StatusCode::CREATED, "{claim}");
    assert_eq!(claim["decision"]["decision"]["id"], decision["id"]);
    assert_eq!(
        claim["decision"]["decision"]["comment"], "keep the test small",
        "the owner's comment goes to the executor"
    );
    let lease: Timestamp = claim["lease_until"]
        .as_str()
        .expect("lease")
        .parse()
        .expect("time");
    let minutes = (lease - Timestamp::now()).as_secs() / 60;
    assert!((29..=30).contains(&minutes), "{minutes}");
    let (status, _, error) =
        with_token(&w, &b, "POST", &claim_uri(&decision), Some(all_kinds())).await;
    assert_eq!(
        (status, &error["code"]),
        (StatusCode::CONFLICT, &json!("claimed"))
    );
    let (status, _, again) =
        with_token(&w, &a, "POST", &claim_uri(&decision), Some(all_kinds())).await;
    assert_eq!(status, StatusCode::CREATED, "the claimer renews: {again}");
    let narrow = json!({"capabilities": ["reply", "resolve"]});
    let (status, _, error) = with_token(&w, &b, "POST", &claim_uri(&decision), Some(narrow)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    let unknown = format!("{FEED}/decisions/{}/claim", DecisionId::new_random());
    let (status, _, _) = with_token(&w, &a, "POST", &unknown, Some(all_kinds())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn capability_outside_the_decision_is_422() {
    let w = world().await;
    let decision = fix_here(&w).await;
    let (a, _) = agent(&w, &EXECUTE).await;
    let narrow = json!({"capabilities": ["reply", "resolve"]});
    let (status, _, error) = with_token(&w, &a, "POST", &claim_uri(&decision), Some(narrow)).await;
    assert_eq!(
        (status, &error["code"]),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            &json!("capability_missing")
        )
    );
    let (status, _, feed) = with_token(
        &w,
        &a,
        "GET",
        &format!("{FEED}?capabilities=reply,resolve"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        feed["decisions"],
        json!([]),
        "not claimable for these capabilities"
    );
}

#[tokio::test]
async fn expired_lease_returns_decision() {
    let w = world().await;
    let decision = fix_here(&w).await;
    let id: DecisionId = serde_json::from_value(decision["id"].clone()).expect("id");
    let (a, principal_a) = agent(&w, &EXECUTE).await;
    let (b, _) = agent(&w, &EXECUTE).await;
    let all = [EffectKind::Fix, EffectKind::Reply, EffectKind::Resolve];
    // A claim made 31 minutes ago: its lease ended a minute ago.
    w.state
        .db
        .review_loop()
        .claim(
            id,
            &principal_a.token_id,
            &all,
            feed::LEASE,
            Timestamp::now() - SignedDuration::from_mins(31),
        )
        .await
        .expect("claim");
    let fix = decision["effects"][0]["id"].as_str().expect("effect");
    let (status, _, error) = with_token(
        &w,
        &a,
        "POST",
        &format!("{FEED}/effects/{fix}/reports"),
        Some(report("done", Some("abc123"), None)),
    )
    .await;
    assert_eq!(
        (status, &error["code"]),
        (StatusCode::CONFLICT, &json!("lease_expired"))
    );
    feed::sync(&w.state).await.expect("sync");
    let (status, headers, page) =
        with_token(&w, &b, "GET", &format!("{FEED}?capabilities={ALL}"), None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["decisions"][0]["decision"]["id"], decision["id"]);
    assert!(headers.get("etag").is_some());
    let (status, _, claim) =
        with_token(&w, &b, "POST", &claim_uri(&decision), Some(all_kinds())).await;
    assert_eq!(status, StatusCode::CREATED, "{claim}");
}

#[tokio::test]
async fn feed_page_answers_304_while_unchanged() {
    let w = world().await;
    fix_here(&w).await;
    let (a, _) = agent(&w, &EXECUTE).await;
    let uri = format!("{FEED}?capabilities={ALL}&repo=acme/node-client");
    let (status, headers, page) = with_token(&w, &a, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["decisions"].as_array().map(Vec::len), Some(1));
    let tag = headers["etag"].to_str().expect("tag").to_owned();
    let request = Request::get(&uri)
        .header(AUTHORIZATION, format!("Bearer {a}"))
        .header("if-none-match", &tag)
        .body(Body::empty())
        .expect("request");
    let response = w.app.clone().oneshot(request).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    let (_, _, other) = with_token(
        &w,
        &a,
        "GET",
        &format!("{FEED}?capabilities={ALL}&repo=acme/other"),
        None,
    )
    .await;
    assert_eq!(other["decisions"], json!([]));
    let (status, _, error) =
        with_token(&w, &a, "GET", &format!("{FEED}?capabilities=merge"), None).await;
    assert_eq!(
        (status, &error["code"]),
        (StatusCode::BAD_REQUEST, &json!("invalid_request"))
    );
    // The subscription and its capabilities are recorded for the execute token.
    let subscriptions = w
        .state
        .db
        .review_loop()
        .subscriptions(DEFAULT_TENANT, Timestamp::now())
        .await
        .expect("subscriptions");
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(subscriptions[0].capabilities.len(), 8);
}

/// T22, T25: an execute token claims and reports only; it cannot decide, and the feed's write routes take no
/// session and no read-only token.
#[tokio::test]
async fn execute_token_cannot_decide() {
    let w = world().await;
    let fixed = fix_here(&w).await;
    let (token, _) = agent(&w, &EXECUTE).await;
    let item = w.item(L6).await;
    let mut body = decision("reject");
    body["reason"] = json!("no");
    body["item_revision"] = item["revision"].clone();
    for (method, uri, body) in [
        (
            "POST",
            format!("{PR}/comments/items/C{}/decisions", item["no"]),
            Some(body),
        ),
        (
            "POST",
            format!("{PR}/comments/decision-batches"),
            Some(json!({"via": "table", "rows": []})),
        ),
        (
            "POST",
            format!("{PR}/comments/items/C{}/undo", item["no"]),
            Some(json!({"item_revision": 0})),
        ),
        (
            "PUT",
            "/api/v1/settings/notifications/channels/ntfy".to_owned(),
            Some(json!({})),
        ),
    ] {
        let (status, _, error) = with_token(&w, &token, method, &uri, body).await;
        assert_eq!(
            (status, &error["code"]),
            (StatusCode::FORBIDDEN, &json!("token_not_accepted")),
            "{method} {uri}"
        );
    }
    // A session never reaches a token-only route.
    let request = Request::post(claim_uri(&fixed))
        .header(COOKIE, &w.cookie)
        .header("x-csrf-token", "csrf")
        .header("content-type", "application/json")
        .body(Body::from(all_kinds().to_string()))
        .expect("request");
    let response = w.app.clone().oneshot(request).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()[WWW_AUTHENTICATE], "Bearer");
    // A read token lacks the scope.
    let (read, _) = agent(&w, &[TokenScope::Read]).await;
    let (status, _, error) =
        with_token(&w, &read, "POST", &claim_uri(&fixed), Some(all_kinds())).await;
    assert_eq!(
        (status, &error["code"]),
        (StatusCode::FORBIDDEN, &json!("insufficient_scope"))
    );
    assert_eq!(
        w.state
            .db
            .review_loop()
            .pr_decisions(w.pr_id().await)
            .await
            .expect("decisions")
            .len(),
        1,
        "only the owner's decision"
    );
}

#[tokio::test]
async fn report_on_foreign_claim_is_rejected() {
    let w = world().await;
    let decision = fix_here(&w).await;
    let (a, _) = agent(&w, &EXECUTE).await;
    let (b, _) = agent(&w, &EXECUTE).await;
    with_token(&w, &a, "POST", &claim_uri(&decision), Some(all_kinds())).await;
    let fix = decision["effects"][0]["id"].as_str().expect("effect");
    let reports = format!("{FEED}/effects/{fix}/reports");
    let (status, _, error) = with_token(
        &w,
        &b,
        "POST",
        &reports,
        Some(report("done", Some("abc"), None)),
    )
    .await;
    assert_eq!(
        (status, &error["code"]),
        (StatusCode::CONFLICT, &json!("claimed"))
    );
    let (status, _, error) =
        with_token(&w, &a, "POST", &reports, Some(report("done", None, None))).await;
    assert_eq!(
        (status, &error["code"]),
        (StatusCode::BAD_REQUEST, &json!("invalid_request")),
        "done needs a reference"
    );
    let done = report("done", Some("9f3c2e1"), None);
    let (status, _, _) = with_token(&w, &a, "POST", &reports, Some(done.clone())).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _, _) = with_token(&w, &a, "POST", &reports, Some(done)).await;
    assert_eq!(status, StatusCode::CREATED, "a repeated key is a no-op");
    let log = w.get(&format!("{PR}/comments/log")).await;
    let fix_effect = &log["decisions"][0]["effects"][0];
    assert_eq!(fix_effect["state"], "done");
    assert_eq!(fix_effect["reports"].as_array().map(Vec::len), Some(1));
    assert!(
        fix_effect["reports"][0]["token_name"]
            .as_str()
            .is_some_and(|name| name.starts_with("agent ")),
        "{fix_effect}"
    );
}

#[tokio::test]
async fn round_executed_once_every_decision_is_done() {
    let w = world().await;
    let decision = fix_here(&w).await;
    let (a, _) = agent(&w, &EXECUTE).await;
    with_token(&w, &a, "POST", &claim_uri(&decision), Some(all_kinds())).await;
    let rl = w.state.db.review_loop();
    for (index, reference) in [
        "9f3c2e1",
        "https://github.com/acme/node-client/pull/482#r1",
        "resolved",
    ]
    .into_iter()
    .enumerate()
    {
        let effect = decision["effects"][index]["id"].as_str().expect("effect");
        let (status, _, body) = with_token(
            &w,
            &a,
            "POST",
            &format!("{FEED}/effects/{effect}/reports"),
            Some(report("done", Some(reference), None)),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let executed: Vec<_> = rl
            .user_events("u1", None, 10)
            .await
            .expect("events")
            .into_iter()
            .filter(|event| event.kind == UserEventKind::RoundExecuted)
            .collect();
        if index < 2 {
            assert!(
                executed.is_empty(),
                "effects still open after report {index}"
            );
        } else {
            assert_eq!(executed.len(), 1);
            assert_eq!(executed[0].count, 1);
            assert_eq!(
                executed[0].text,
                "acme/node-client #482: 1 decision executed"
            );
            assert_eq!(
                executed[0].url.as_deref(),
                Some("/pr/acme/node-client/482/comments")
            );
        }
    }
}

/// Sets a webhook push target for `token` and records its capabilities through RF1.
async fn push_target(w: &World, token: &str, capabilities: &str, url: &str) {
    let (status, _, target) = with_token(
        w,
        token,
        "PUT",
        &format!("{FEED}/push-target"),
        Some(json!({"kind": "webhook", "url": url, "topic": null})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{target}");
    let (status, _, _) = with_token(
        w,
        token,
        "GET",
        &format!("{FEED}?capabilities={capabilities}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

async fn pushed_tokens(w: &World) -> Vec<String> {
    w.state
        .db
        .review_loop()
        .due_deliveries(Timestamp::now(), 50)
        .await
        .expect("due")
        .into_iter()
        .filter_map(|delivery| match delivery.target {
            DeliveryTarget::Push { token, .. } => Some(token.0),
            DeliveryTarget::Channel { .. } => None,
        })
        .collect()
}

#[tokio::test]
async fn push_only_for_claimable_capabilities() {
    let w = world().await;
    feed::sync(&w.state).await.expect("initial sync");
    let (wide, wide_principal) = agent(&w, &EXECUTE).await;
    let (narrow, _) = agent(&w, &EXECUTE).await;
    let (reader, _) = agent(&w, &[TokenScope::Read]).await;
    push_target(&w, &wide, "fix,reply,resolve", "http://hooks.example/wide").await;
    push_target(&w, &narrow, "reply,resolve", "http://hooks.example/narrow").await;
    let (status, _, _) = with_token(
        &w,
        &reader,
        "PUT",
        &format!("{FEED}/push-target"),
        Some(json!({"kind": "webhook", "url": "http://hooks.example/r"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a read token has no push target"
    );
    fix_here(&w).await;
    feed::sync(&w.state).await.expect("sync");
    assert_eq!(pushed_tokens(&w).await, [wide_principal.token_id.0]);
    feed::sync(&w.state).await.expect("again");
    assert_eq!(pushed_tokens(&w).await.len(), 1, "one push per decision");
}

#[tokio::test]
async fn agent_with_live_stream_is_not_pushed() {
    let w = world().await;
    feed::sync(&w.state).await.expect("initial sync");
    let (live, live_principal) = agent(&w, &EXECUTE).await;
    let (idle, idle_principal) = agent(&w, &EXECUTE).await;
    push_target(&w, &live, ALL, "http://hooks.example/live").await;
    push_target(&w, &idle, ALL, "http://hooks.example/idle").await;
    let presence = w.state.feed.connect(&live_principal.token_id);
    fix_here(&w).await;
    feed::sync(&w.state).await.expect("sync");
    assert_eq!(pushed_tokens(&w).await, [idle_principal.token_id.0]);
    drop(presence);
}

/// RF2: the snapshot of claimable decisions, presence in the decision log while open, and `decision_cancelled`
/// once another agent claims.
#[tokio::test]
async fn feed_stream_lists_claimable_and_marks_the_agent_connected() {
    let w = world().await;
    let decision = fix_here(&w).await;
    feed::sync(&w.state).await.expect("sync");
    let (a, _) = agent(&w, &EXECUTE).await;
    let (b, _) = agent(&w, &EXECUTE).await;
    let request = Request::get(format!("{FEED}/events?capabilities={ALL}"))
        .header(AUTHORIZATION, format!("Bearer {a}"))
        .body(Body::empty())
        .expect("request");
    let response = w.app.clone().oneshot(request).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::OK);
    let mut frames = response.into_body().into_data_stream();
    let mut next = async || {
        let frame = tokio::time::timeout(Duration::from_secs(10), frames.next())
            .await
            .expect("frame in time")
            .expect("open")
            .expect("frame");
        String::from_utf8(frame.to_vec()).expect("utf-8")
    };
    let first = next().await;
    assert!(first.starts_with("event: decision_available"), "{first}");
    assert!(
        first.contains(decision["id"].as_str().expect("id")),
        "{first}"
    );
    assert!(
        !first.contains("keep the test small"),
        "no text on the stream: {first}"
    );
    let log = w.get(&format!("{PR}/comments/log")).await;
    let subscriber = log["subscribers"]
        .as_array()
        .expect("subscribers")
        .iter()
        .find(|s| s["connected"] == true)
        .cloned()
        .expect("a connected agent");
    assert_eq!(subscriber["capabilities"].as_array().map(Vec::len), Some(8));
    with_token(&w, &b, "POST", &claim_uri(&decision), Some(all_kinds())).await;
    feed::sync(&w.state).await.expect("sync");
    let cancelled = next().await;
    assert!(
        cancelled.starts_with("event: decision_cancelled"),
        "{cancelled}"
    );
}

/// RF4 after the feed task returned a lapsed claim to the feed: the claimer's late report still answers 409
/// `lease_expired` (agent-api step 6: "the lease ended and the decision went back to the feed (claim again)"), never
/// `claimed` ("another agent holds it"), which would tell the agent to give up a decision nobody holds.
#[tokio::test]
async fn late_report_after_the_feed_returned_the_claim_is_lease_expired() {
    let w = world().await;
    let decision = fix_here(&w).await;
    let id: DecisionId = serde_json::from_value(decision["id"].clone()).expect("id");
    let (a, principal_a) = agent(&w, &EXECUTE).await;
    let all = [EffectKind::Fix, EffectKind::Reply, EffectKind::Resolve];
    w.state
        .db
        .review_loop()
        .claim(
            id,
            &principal_a.token_id,
            &all,
            feed::LEASE,
            Timestamp::now() - SignedDuration::from_mins(31),
        )
        .await
        .expect("claim");
    feed::sync(&w.state)
        .await
        .expect("the feed task returns the lapsed claim");
    let fix = decision["effects"][0]["id"].as_str().expect("effect");
    let (status, _, error) = with_token(
        &w,
        &a,
        "POST",
        &format!("{FEED}/effects/{fix}/reports"),
        Some(report("done", Some("abc123"), None)),
    )
    .await;
    assert_eq!(
        (status, &error["code"]),
        (StatusCode::CONFLICT, &json!("lease_expired")),
        "{error}"
    );
}

/// Every effect of the decision reported `done` straight in the database (as if the server stopped before its round
/// check); returns the last report's idempotency key.
async fn executed_without_round_check(
    w: &World,
    decision: &Value,
    principal: &TokenPrincipal,
) -> String {
    let id: DecisionId = serde_json::from_value(decision["id"].clone()).expect("id");
    let rl = w.state.db.review_loop();
    let all = [EffectKind::Fix, EffectKind::Reply, EffectKind::Resolve];
    rl.claim(id, &principal.token_id, &all, feed::LEASE, Timestamp::now())
        .await
        .expect("claim");
    let mut key = String::new();
    for effect in decision["effects"].as_array().expect("effects") {
        key = uuid::Uuid::new_v4().to_string();
        let effect = serde_json::from_value(effect["id"].clone()).expect("effect id");
        rl.report(
            DEFAULT_TENANT,
            effect,
            &principal.token_id,
            &ReportRequest {
                outcome: ReportOutcome::Done,
                reference: Some("9f3c2e1".into()),
                message: None,
                idempotency_key: key.clone(),
            },
            feed::LEASE,
            Timestamp::now(),
        )
        .await
        .expect("report");
    }
    key
}

async fn rounds_executed(w: &World) -> usize {
    w.state
        .db
        .review_loop()
        .user_events("u1", None, 10)
        .await
        .expect("events")
        .into_iter()
        .filter(|event| event.kind == UserEventKind::RoundExecuted)
        .count()
}

/// A retried report (same idempotency key) announces a round whose first check did not run, once.
#[tokio::test]
async fn a_retried_report_announces_a_round_its_first_check_missed() {
    let w = world().await;
    let decision = fix_here(&w).await;
    let (a, principal) = agent(&w, &EXECUTE).await;
    let key = executed_without_round_check(&w, &decision, &principal).await;
    assert_eq!(rounds_executed(&w).await, 0);
    let last = decision["effects"][2]["id"].as_str().expect("effect");
    let retry =
        json!({"outcome": "done", "reference": "9f3c2e1", "message": null, "idempotency_key": key});
    for _ in 0..2 {
        let (status, _, body) = with_token(
            &w,
            &a,
            "POST",
            &format!("{FEED}/effects/{last}/reports"),
            Some(retry.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    assert_eq!(rounds_executed(&w).await, 1);
    let (status, _, error) = with_token(
        &w,
        &a,
        "POST",
        &format!("{FEED}/effects/{last}/reports"),
        Some(report("done", Some("9f3c2e1"), None)),
    )
    .await;
    assert_eq!(
        (status, &error["code"]),
        (StatusCode::CONFLICT, &json!("conflict")),
        "a new report on an effect that already ended: {error}"
    );
}

/// The feed task announces at its start the rounds completed while the server was down, once.
#[tokio::test]
async fn rounds_completed_while_down_are_announced_at_start() {
    let w = world().await;
    let decision = fix_here(&w).await;
    let (_, principal) = agent(&w, &EXECUTE).await;
    executed_without_round_check(&w, &decision, &principal).await;
    feed::recover_rounds(&w.state).await;
    feed::recover_rounds(&w.state).await;
    assert_eq!(rounds_executed(&w).await, 1);
}

/// A decision that stays claimable while its open effects shrink is announced to the agents it newly fits (stream
/// and push), not again to those it fitted before.
#[tokio::test]
async fn a_decision_newly_within_capabilities_is_announced() {
    let w = world().await;
    feed::sync(&w.state).await.expect("initial sync");
    let (narrow, narrow_principal) = agent(&w, &EXECUTE).await;
    push_target(&w, &narrow, "reply,resolve", "http://hooks.example/narrow").await;
    let decision = fix_here(&w).await;
    feed::sync(&w.state).await.expect("sync");
    assert!(
        pushed_tokens(&w).await.is_empty(),
        "fix is outside its capabilities"
    );

    // Between two syncs another agent claims, executes the fix and gives the rest back.
    let (_, wide) = agent(&w, &EXECUTE).await;
    let id: DecisionId = serde_json::from_value(decision["id"].clone()).expect("id");
    let all = [EffectKind::Fix, EffectKind::Reply, EffectKind::Resolve];
    let rl = w.state.db.review_loop();
    rl.claim(id, &wide.token_id, &all, feed::LEASE, Timestamp::now())
        .await
        .expect("claim");
    for (index, outcome, reference) in [
        (0, ReportOutcome::Done, Some("9f3c2e1")),
        (1, ReportOutcome::Released, None),
    ] {
        let effect = serde_json::from_value(decision["effects"][index]["id"].clone()).expect("id");
        rl.report(
            DEFAULT_TENANT,
            effect,
            &wide.token_id,
            &ReportRequest {
                outcome,
                reference: reference.map(str::to_owned),
                message: None,
                idempotency_key: uuid::Uuid::new_v4().to_string(),
            },
            feed::LEASE,
            Timestamp::now(),
        )
        .await
        .expect("report");
    }

    let mut signals = w.state.feed.subscribe();
    feed::sync(&w.state).await.expect("sync");
    assert_eq!(pushed_tokens(&w).await, [narrow_principal.token_id.0]);
    let signal = signals.try_recv().expect("announced");
    assert!(matches!(signal.event, FeedEvent::DecisionAvailable { .. }));
    assert!(signal.concerns(&[EffectKind::Reply, EffectKind::Resolve]));
    assert!(
        !signal.concerns(&all),
        "an agent the decision fitted before was told already"
    );
}

#[tokio::test]
async fn follow_up_issue_attempt_is_fenced_across_claimants_and_reports() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let mut body = decision("follow_up_issue");
    body["target"] = json!({"type": "follow_up_issue", "title": "Bound retries"});
    body["resolve_ticked"] = json!(true);
    let decision = w.decide_ok(L2, body).await;
    let effects = decision["effects"].as_array().expect("effects");
    assert_eq!(effects.len(), 3);
    let effect = |index: usize| effects[index]["id"].as_str().expect("effect id");
    let attempt_uri = format!("{FEED}/effects/{}/issue-attempt", effect(0));
    let claim_uri = claim_uri(&decision);
    let kinds = json!({"capabilities": ["upstream_issue", "reply", "resolve"]});
    let (a, _) = agent(&w, &EXECUTE).await;
    let (b, _) = agent(&w, &EXECUTE).await;
    let reserve = |retry_of: Option<uuid::Uuid>| json!({"retry_of": retry_of});
    let (status, _, value) = with_token(&w, &a, "POST", &claim_uri, Some(kinds.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    let (left, right) = tokio::join!(
        with_token(&w, &a, "POST", &attempt_uri, Some(reserve(None))),
        with_token(&w, &a, "POST", &attempt_uri, Some(reserve(None))),
    );
    assert_eq!(
        (left.0, right.0),
        (StatusCode::OK, StatusCode::OK),
        "{left:?} {right:?}"
    );
    assert_ne!(left.2["reserved"], right.2["reserved"]);
    assert_eq!(left.2["attempt_id"], right.2["attempt_id"]);
    let previous: uuid::Uuid =
        serde_json::from_value(left.2["attempt_id"].clone()).expect("attempt");
    let (status, _, duplicate) =
        with_token(&w, &a, "POST", &attempt_uri, Some(reserve(None))).await;
    assert_eq!(status, StatusCode::OK, "{duplicate}");
    assert_eq!(
        duplicate,
        json!({"attempt_id": previous, "reserved": false})
    );

    let report_uri = format!("{FEED}/effects/{}/reports", effect(0));
    let (status, _, value) = with_token(
        &w,
        &a,
        "POST",
        &report_uri,
        Some(report("failed", None, Some("Issue creation failed."))),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    let fd = format!("{FEED}/decisions/{}", decision["id"].as_str().unwrap());
    let (status, _, value) = with_token(&w, &b, "GET", &fd, None).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let after = value["decision"]["effects"].as_array().expect("effects");
    assert_eq!(after[0]["state"], "open");
    assert_eq!(after[0]["reports"][0]["message"], "Issue creation failed.");
    assert_eq!(after[1]["state"], "waiting");
    assert_eq!(after[2]["state"], "waiting");
    let item = w.item(L2).await;
    for action in ["undo", "reopen"] {
        let (status, value) = w
            .post(
                &format!("{PR}/comments/items/C{}/{action}", item["no"]),
                json!({"item_revision": item["revision"]}),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{action}: {value}");
    }

    let (status, _, value) = with_token(&w, &b, "POST", &claim_uri, Some(kinds.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    let (status, _, blocked) = with_token(&w, &b, "POST", &attempt_uri, Some(reserve(None))).await;
    assert_eq!(status, StatusCode::OK, "{blocked}");
    assert_eq!(blocked, json!({"attempt_id": previous, "reserved": false}));
    let (status, _, wrong) = with_token(
        &w,
        &b,
        "POST",
        &attempt_uri,
        Some(reserve(Some(uuid::Uuid::new_v4()))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{wrong}");
    assert_eq!(wrong, blocked);
    let (status, _, now) =
        with_token(&w, &b, "POST", &attempt_uri, Some(reserve(Some(previous)))).await;
    assert_eq!(status, StatusCode::OK, "{now}");
    assert_eq!(now["reserved"], true);
    let replacement: uuid::Uuid =
        serde_json::from_value(now["attempt_id"].clone()).expect("replacement");
    assert_ne!(replacement, previous);
    let (status, _, outdated) =
        with_token(&w, &b, "POST", &attempt_uri, Some(reserve(Some(previous)))).await;
    assert_eq!(status, StatusCode::OK, "{outdated}");
    assert_eq!(
        outdated,
        json!({"attempt_id": replacement, "reserved": false})
    );
    let (status, _, forbidden) = with_token(
        &w,
        &a,
        "POST",
        &attempt_uri,
        Some(reserve(Some(replacement))),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{forbidden}");

    w.state
        .db
        .review_loop()
        .expire_leases(Timestamp::now() + SignedDuration::from_mins(31))
        .await
        .expect("expire lease");
    let (status, _, expired) = with_token(&w, &b, "POST", &attempt_uri, Some(reserve(None))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{expired}");
    assert_eq!(expired["code"], "lease_expired");
    let (status, _, value) = with_token(&w, &b, "POST", &claim_uri, Some(kinds)).await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    let (status, _, recovered) =
        with_token(&w, &b, "POST", &attempt_uri, Some(reserve(None))).await;
    assert_eq!(status, StatusCode::OK, "{recovered}");
    assert_eq!(recovered, outdated);

    let (status, _, wrong_repo) = with_token(
        &w,
        &b,
        "POST",
        &report_uri,
        Some(report(
            "done",
            Some("https://github.com/other/repo/issues/2"),
            None,
        )),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{wrong_repo}");
    let url = "https://github.com/acme/node-client/issues/2";
    let (status, _, body) = with_token(
        &w,
        &b,
        "POST",
        &report_uri,
        Some(report("done", Some(url), None)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, _, value) = with_token(&w, &b, "GET", &fd, None).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let open = value["decision"]["effects"].as_array().expect("effects");
    assert_eq!(open[0]["reports"][1]["reference"], url);
}

#[tokio::test]
async fn failed_follow_up_issue_leaves_descendants_waiting() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let mut body = decision("follow_up_issue");
    body["target"] = json!({"type": "follow_up_issue", "title": "Bound retries"});
    body["resolve_ticked"] = json!(true);
    let decision = w.decide_ok(L2, body).await;
    let (token, _) = agent(&w, &EXECUTE).await;
    let (status, _, value) = with_token(
        &w,
        &token,
        "POST",
        &claim_uri(&decision),
        Some(json!({"capabilities": ["upstream_issue", "reply", "resolve"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    let issue = decision["effects"][0]["id"].as_str().expect("id");
    let (status, _, value) = with_token(
        &w,
        &token,
        "POST",
        &format!("{FEED}/effects/{issue}/reports"),
        Some(report("failed", None, Some("Permission denied."))),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    let (_, _, value) = with_token(
        &w,
        &token,
        "GET",
        &format!("{FEED}/decisions/{}", decision["id"].as_str().expect("id")),
        None,
    )
    .await;
    let effects = value["decision"]["effects"].as_array().expect("effects");
    assert_eq!(effects[0]["state"], "open");
    assert_eq!(effects[0]["reports"][0]["message"], "Permission denied.");
    assert_eq!(effects[1]["state"], "waiting");
    assert_eq!(effects[2]["state"], "waiting");
}

async fn carry_follow_up_issue(reference: Option<&str>) -> (Value, Value) {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let mut body = decision("follow_up_issue");
    body["target"] = json!({"type": "follow_up_issue", "title": "Bound retries"});
    let first = w.decide_ok(L2, body).await;
    if let Some(reference) = reference {
        let (token, _) = agent(&w, &EXECUTE).await;
        let (status, _, value) = with_token(
            &w,
            &token,
            "POST",
            &claim_uri(&first),
            Some(json!({"capabilities": ["upstream_issue", "reply", "resolve"]})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{value}");
        let issue = first["effects"][0]["id"].as_str().expect("id");
        let (status, _, value) = with_token(
            &w,
            &token,
            "POST",
            &format!("{FEED}/effects/{issue}/reports"),
            Some(report("done", Some(reference), None)),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{value}");
    }

    let new_thread: &dyn Fn(&mut Value) = &|answer| {
        let thread = thread_node(answer, "PRRT_fixture482_l2");
        thread["id"] = json!("PRRT_fixture482_l2b");
        thread["comments"]["nodes"][0]["url"] =
            json!("https://github.com/acme/node-client/pull/482#discussion_r3000000999");
    };
    w.reread(&[("review-threads.json", new_thread)]).await;
    (first, w.item(L2).await)
}

#[tokio::test]
async fn carried_follow_up_issue_reuses_the_recorded_issue() {
    let url = "https://github.com/acme/node-client/issues/97";
    let (first, item) = carry_follow_up_issue(Some(url)).await;
    let decisions = item["decisions"].as_array().expect("decisions");
    assert_eq!(decisions.len(), 2, "{item}");
    assert_eq!(decisions[1]["carried_from"], first["id"]);
    assert_eq!(decisions[1]["kind"], "upstream_issue");
    assert_eq!(
        decisions[1]["target"],
        json!({"type": "upstream_issue", "url": url})
    );
    assert_eq!(decisions[1]["effects"][0]["params"]["url"], url);
}

#[tokio::test]
async fn carried_follow_up_issue_without_a_reference_is_asked_again() {
    let (_, item) = carry_follow_up_issue(None).await;
    assert_eq!(item["state"], "reopened", "{item}");
    assert_eq!(item["decisions"].as_array().map(Vec::len), Some(1));
    assert_eq!(item["decisions"][0]["effects"][0]["state"], "cancelled");
}
