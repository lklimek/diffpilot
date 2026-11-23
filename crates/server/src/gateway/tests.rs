use super::*;
use serde_json::json;

#[test]
fn body_policy_covers_responses_lite_and_nested_namespaces() {
    let valid = json!({"store":false,"stream":true,"input":[{"type":"additional_tools","tools":[
        {"type":"namespace","name":"functions","tools":[{"type":"custom","name":"exec"}]}
    ]}]});
    assert!(validate_body(&valid).is_ok());
    for kind in [
        "web_search",
        "image_generation",
        "mcp",
        "file_search",
        "unknown",
    ] {
        let mut bad = valid.clone();
        bad["input"][0]["tools"][0]["tools"][0]["type"] = json!(kind);
        assert!(validate_body(&bad).is_err(), "{kind}");
        assert!(
            validate_body(&json!({"store":false,"stream":true,"tools":[{"type":kind}]})).is_err()
        );
    }
    for bad in [
        json!({}),
        json!({"store":true,"stream":true}),
        json!({"store":false,"stream":false}),
        json!({"store":false,"stream":true,"previous_response_id":null}),
        json!({"store":false,"stream":true,"tools":{}}),
    ] {
        assert!(validate_body(&bad).is_err());
    }
}

use contracts::{
    gateway::{CredentialState, RelinkReason},
    ids::UserId,
    job::result::Usage,
    runtime::ContainerId,
};
use std::{sync::Arc, time::Duration};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

async fn fixture() -> (tempfile::TempDir, MockServer, Arc<Gateway>, UserId) {
    let dir = tempfile::tempdir().expect("temp");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private directory");
    let upstream = MockServer::start().await;
    let gateway = Gateway::for_test(dir.path(), &upstream.uri()).expect("gateway");
    let user = UserId("payer-1".into());
    (dir, upstream, Arc::new(gateway), user)
}

fn bundle(expiry: i64) -> auth::Bundle {
    auth::Bundle {
        access: "fake-access".into(),
        refresh: "fake-refresh-0".into(),
        id_token: "fake-id".into(),
        account: "fake-account".into(),
        label: "fake@example.invalid".into(),
        plan: "plus".into(),
        fedramp: false,
        expires_at: expiry,
        earliest_refresh_at: None,
        scope: "openid offline_access".into(),
    }
}

async fn seed(g: &Gateway, user: &UserId, value: auth::Bundle) {
    let mut tx = g.store.lock(user).await.expect("lock");
    tx.record.bundle = Some(value);
    tx.save().expect("seed");
}

fn token_json(refresh: &str) -> serde_json::Value {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let claims = json!({"iss":"https://auth.openai.com","aud":auth::CLIENT_ID,"exp":now()+3600,
        "email":"fake@example.invalid","https://api.openai.com/auth": {
            "chatgpt_account_id":"fake-account","chatgpt_plan_type":"plus","chatgpt_account_is_fedramp":false}});
    let jwt = format!("e30.{}.fake", URL_SAFE_NO_PAD.encode(claims.to_string()));
    json!({"access_token":"fake-access-1","refresh_token":refresh,"id_token":jwt,
        "expires_in":3600,"earliest_refresh_at":now()+3000,"scope":"openid offline_access","token_type":"Bearer"})
}

#[tokio::test]
async fn refresh_is_near_expiry_only_and_respects_earliest_timing() {
    let (_d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 864000)).await;
    assert_eq!(
        g.access(&u, None).await.expect("access").bundle.access,
        "fake-access"
    );
    let mut b = bundle(now() + 30);
    b.earliest_refresh_at = Some(now() + 20);
    seed(&g, &u, b).await;
    assert_eq!(
        g.access(&u, None)
            .await
            .expect("not before earliest")
            .bundle
            .access,
        "fake-access"
    );
    assert!(up.received_requests().await.expect("requests").is_empty());
    g.shutdown().await;
}

#[tokio::test]
async fn replica_race_exchanges_once_and_commits_before_use() {
    let (d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 5)).await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(token_json("fake-refresh-1")))
        .expect(1)
        .mount(&up)
        .await;
    let other = Gateway::for_test(d.path(), &up.uri()).expect("replica");
    let (a, b) = tokio::join!(g.access(&u, None), other.access(&u, None));
    assert_eq!(a.expect("first").bundle.refresh, "fake-refresh-1");
    assert_eq!(b.expect("second").bundle.refresh, "fake-refresh-1");
    let tx = g.store.lock(&u).await.expect("read");
    assert_eq!(tx.record.generation, 1);
    assert!(!tx.record.inflight);
    assert_eq!(
        tx.record.bundle.as_ref().expect("bundle").refresh,
        "fake-refresh-1"
    );
    drop(tx);
    let reqs = up.received_requests().await.expect("requests");
    assert_eq!(
        reqs[0].body_json::<serde_json::Value>().expect("body")["refresh_reason"],
        "proactive_expiry"
    );
    g.shutdown().await;
    other.shutdown().await;
}

#[tokio::test]
async fn unknown_exchange_never_replays_and_terminal_errors_stay_distinct() {
    for (status, body, reason) in [
        (500, json!({}), RelinkReason::Quarantined),
        (
            200,
            json!({"access_token":"incomplete"}),
            RelinkReason::Quarantined,
        ),
        (401, json!({}), RelinkReason::Terminal),
        (
            400,
            json!({"error":"invalid_grant"}),
            RelinkReason::Terminal,
        ),
        (
            400,
            json!({"error":{"code":"refresh_token_reused"}}),
            RelinkReason::Terminal,
        ),
    ] {
        let (_d, up, g, u) = fixture().await;
        seed(&g, &u, bundle(now() + 1)).await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .expect(1)
            .mount(&up)
            .await;
        assert!(g.access(&u, None).await.is_err());
        assert!(g.access(&u, None).await.is_err());
        assert_eq!(
            g.status(&u).await.expect("status").relink_reason,
            Some(reason)
        );
        g.shutdown().await;
    }
}

#[tokio::test]
async fn cancelled_refresh_restart_and_stale_snapshot_fail_closed() {
    let (d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 1)).await;
    let stale = std::fs::read(g.store.record_path(&u)).expect("backup");
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(token_json("fake-refresh-1"))
                .set_delay(Duration::from_secs(30)),
        )
        .expect(1)
        .mount(&up)
        .await;
    let task = {
        let g = g.clone();
        let u = u.clone();
        tokio::spawn(async move { g.access(&u, None).await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while up.received_requests().await.expect("requests").is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("request sent");
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    let other = Gateway::for_test(d.path(), &up.uri()).expect("restart");
    assert!(other.access(&u, None).await.is_err());
    assert_eq!(
        other.status(&u).await.expect("status").relink_reason,
        Some(RelinkReason::Quarantined)
    );
    std::fs::write(g.store.record_path(&u), stale).expect("simulate stale snapshot");
    assert!(other.access(&u, None).await.is_err());
    g.shutdown().await;
    other.shutdown().await;
}

#[tokio::test]
async fn failed_persistence_after_exchange_never_publishes_replacement() {
    let (_d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 1)).await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(token_json("fake-refresh-1")))
        .expect(1)
        .mount(&up)
        .await;
    g.store
        .fail_after_marker
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(g.access(&u, None).await.is_err());
    g.store
        .fail_after_marker
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert!(g.access(&u, None).await.is_err());
    assert_eq!(
        g.status(&u).await.expect("status").relink_reason,
        Some(RelinkReason::Quarantined)
    );
    g.shutdown().await;
}

#[tokio::test]
async fn key_rotation_is_local_and_explicit_unlink_revokes_only_its_payer() {
    let (_d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 3600)).await;
    let other = UserId("payer-2".into());
    seed(&g, &other, bundle(now() + 3600)).await;
    let container = ContainerId::Judge { payer: u.clone() };
    let a = g.mint(&container).await.expect("key");
    let b = g.mint(&container).await.expect("rotate");
    assert!(g.authenticate(a.key.expose()).await.is_err());
    assert!(g.authenticate(b.key.expose()).await.is_ok());
    g.revoke(&container).await.expect("local revoke");
    assert!(up.received_requests().await.expect("requests").is_empty());
    Mock::given(method("POST"))
        .and(path("/oauth/revoke"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&up)
        .await;
    g.unlink(&u).await.expect("best effort unlink");
    assert!(g.access(&other, None).await.is_ok());
    assert!(g.access(&u, None).await.is_err());
    g.shutdown().await;
}

#[tokio::test]
async fn usage_deduplicates_response_ids_across_keys_and_restart_without_time_windows() {
    let (d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 3600)).await;
    let key = g
        .mint(&ContainerId::Judge { payer: u.clone() })
        .await
        .expect("key");
    let usage = Usage {
        input_tokens: 9,
        cached_input_tokens: 3,
        output_tokens: 2,
        requests: Some(1),
    };
    g.store
        .meter(&u, &key.id, "response-a", usage)
        .await
        .expect("meter");
    g.store
        .meter(&u, &key.id, "response-a", usage)
        .await
        .expect("duplicate");
    let next = g
        .mint(&ContainerId::Judge { payer: u.clone() })
        .await
        .expect("rotate");
    g.store
        .meter(&u, &next.id, "response-a", usage)
        .await
        .expect("cross-key duplicate");
    g.store
        .meter(&u, &key.id, "response-final", usage)
        .await
        .expect("late final");
    let other = Gateway::for_test(d.path(), &up.uri()).expect("restart");
    assert_eq!(
        other
            .usage(&u, None)
            .await
            .expect("usage")
            .usage
            .input_tokens,
        18
    );
    assert_eq!(
        other
            .usage(&u, Some(&key.id))
            .await
            .expect("key usage")
            .usage
            .requests,
        Some(2)
    );
    assert_eq!(
        other
            .usage(&u, Some(&next.id))
            .await
            .expect("next usage")
            .usage
            .input_tokens,
        0
    );
    g.shutdown().await;
    other.shutdown().await;
}

#[tokio::test]
async fn failed_link_preserves_existing_connection_and_does_not_revoke() {
    let (_d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 3600)).await;
    Mock::given(method("POST"))
        .and(path("/api/accounts/deviceauth/usercode"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&up)
        .await;
    assert!(g.begin_link(&u).await.is_err());
    assert_eq!(
        g.status(&u).await.expect("status").state,
        CredentialState::Linked
    );
    assert_eq!(
        g.access(&u, None).await.expect("old access").bundle.access,
        "fake-access"
    );
    g.shutdown().await;
}

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use tower::ServiceExt;
use wiremock::matchers::header;

async fn call(
    router: axum::Router,
    method: &str,
    path: &str,
    bearer: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    router
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {bearer}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response")
}
async fn collect(response: axum::response::Response) -> axum::body::Bytes {
    response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
}

#[tokio::test]
async fn listeners_scopes_and_plan_only_link_contract_do_not_cross() {
    let (_d, up, g, _u) = fixture().await;
    for path in [
        "/admin/keys/mint",
        "/admin/credentials/payer-1/link",
        "/v1/responses/compact",
        "/oauth/token",
    ] {
        assert_eq!(
            call(g.model_router(), "POST", path, &"c".repeat(32), json!({}))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        call(
            g.admin_router(),
            "POST",
            "/v1/responses",
            &"k".repeat(32),
            json!({})
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            g.admin_router(),
            "GET",
            "/admin/credentials/payer-1",
            &"k".repeat(32),
            json!({})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            g.admin_router(),
            "POST",
            "/admin/keys/mint",
            &"c".repeat(32),
            json!({})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    for body in [
        json!({"mode":"api_key"}),
        json!({"mode":"chatgpt_plan","token":"x"}),
        json!({"mode":"chatgpt_plan","base_url":"http://evil.invalid"}),
    ] {
        assert_eq!(
            call(
                g.admin_router(),
                "POST",
                "/admin/credentials/payer-1/link",
                &"c".repeat(32),
                body
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        call(
            g.model_router(),
            "POST",
            "/v1/responses",
            "bad-key",
            json!({"store":false,"stream":true})
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(up.received_requests().await.expect("requests").is_empty());
    g.shutdown().await;
}

#[tokio::test]
async fn upstream_401_far_from_expiry_becomes_403_without_refresh_or_revoke() {
    let (_d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 864000)).await;
    let key = g
        .mint(&ContainerId::Judge { payer: u.clone() })
        .await
        .expect("key");
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error":"revoked"})))
        .expect(1)
        .mount(&up)
        .await;
    let response = call(
        g.model_router(),
        "POST",
        "/v1/responses",
        key.key.expose(),
        json!({"store":false,"stream":true}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&collect(response).await).expect("json")["error"]
            ["code"],
        "credential_unlinked"
    );
    assert_eq!(
        g.status(&u).await.expect("status").relink_reason,
        Some(RelinkReason::Terminal)
    );
    assert_eq!(up.received_requests().await.expect("requests").len(), 1);
    g.shutdown().await;
}

#[tokio::test]
async fn rejected_old_generation_uses_committed_successor_without_refresh() {
    let (_d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 864000)).await;
    let mut tx = g.store.lock(&u).await.expect("lock");
    tx.record.generation = 1;
    tx.record.bundle.as_mut().expect("bundle").access = "replacement".into();
    tx.save().expect("commit");
    drop(tx);
    assert_eq!(
        g.access(&u, Some(0)).await.expect("reload").bundle.access,
        "replacement"
    );
    assert!(up.received_requests().await.expect("requests").is_empty());
    g.shutdown().await;
}

#[tokio::test]
async fn body_rejection_never_reaches_backend_and_key_expiry_is_401() {
    let (_d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 3600)).await;
    let key = g
        .mint(&ContainerId::Judge { payer: u.clone() })
        .await
        .expect("key");
    let response = call(g.model_router(), "POST", "/v1/responses", key.key.expose(), json!({"store":false,"stream":true,"input":[{"type":"additional_tools","tools":[{"type":"image_generation"}]}]})).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let mut tx = g.store.lock(&u).await.expect("lock");
    tx.record.keys.get_mut(&key.id).expect("key").expires_at = now() - 1;
    tx.save().expect("save");
    drop(tx);
    assert_eq!(
        call(
            g.model_router(),
            "GET",
            "/v1/models",
            key.key.expose(),
            json!({})
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(up.received_requests().await.expect("requests").is_empty());
    g.shutdown().await;
}

#[tokio::test]
async fn models_revalidate_per_payer_cache_and_strip_sensitive_headers() {
    let (_d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 3600)).await;
    let key = g
        .mint(&ContainerId::Judge { payer: u.clone() })
        .await
        .expect("key");
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"models":[{"slug":"fake-model"}]}))
                .insert_header("x-models-etag", "test-etag")
                .insert_header("set-cookie", "secret")
                .insert_header("cf-ray", "private")
                .insert_header("x-codex-turn-state", "next"),
        )
        .up_to_n_times(1)
        .expect(1)
        .mount(&up)
        .await;
    let response = call(
        g.model_router(),
        "GET",
        "/v1/models?client_version=0.171.0",
        key.key.expose(),
        json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("set-cookie"));
    assert!(!response.headers().contains_key("cf-ray"));
    assert_eq!(response.headers()["x-codex-turn-state"], "next");
    let expected = collect(response).await;
    up.reset().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("if-none-match", "test-etag"))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&up)
        .await;
    let response = call(
        g.model_router(),
        "GET",
        "/v1/models",
        key.key.expose(),
        json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(collect(response).await, expected);
    up.reset().await;
    Mock::given(path("/models"))
        .respond_with(ResponseTemplate::new(503).set_body_string("model catalogue unavailable"))
        .expect(1)
        .mount(&up)
        .await;
    assert_eq!(
        call(
            g.model_router(),
            "GET",
            "/v1/models",
            key.key.expose(),
            json!({})
        )
        .await
        .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    g.shutdown().await;
}

fn completed(id: &str) -> String {
    format!(
        "data: {}\r\n\r\n",
        json!({"type":"response.completed","response":{"id":id,"usage":{
        "input_tokens":10,"input_tokens_details":{"cached_tokens":4},"output_tokens":2}}})
    )
}

#[tokio::test]
async fn sse_without_content_type_is_forwarded_and_final_id_metered_after_disconnect() {
    let (_d, up, g, u) = fixture().await;
    let mut b = bundle(now() + 3600);
    b.fedramp = true;
    seed(&g, &u, b).await;
    let key = g
        .mint(&ContainerId::Judge { payer: u.clone() })
        .await
        .expect("key");
    let body = format!(
        "{}{}{}",
        completed("one"),
        completed("two"),
        completed("two")
    );
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer fake-access"))
        .and(header("chatgpt-account-id", "fake-account"))
        .and(header("accept-encoding", "identity"))
        .and(header("x-openai-fedramp", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
        .expect(2)
        .mount(&up)
        .await;
    let response = call(
        g.model_router(),
        "POST",
        "/v1/responses",
        key.key.expose(),
        json!({"store":false,"stream":true}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(collect(response).await.as_ref(), body.as_bytes());
    assert_eq!(
        g.usage(&u, Some(&key.id))
            .await
            .expect("usage")
            .usage
            .input_tokens,
        20
    );
    drop(
        call(
            g.model_router(),
            "POST",
            "/v1/responses",
            key.key.expose(),
            json!({"store":false,"stream":true}),
        )
        .await,
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if g.tasks.lock().await.is_empty() {
                break;
            }
            {
                let mut tasks = g.tasks.lock().await;
                while tasks.try_join_next().is_some() {}
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("drained");
    assert_eq!(
        g.usage(&u, None).await.expect("usage").usage.input_tokens,
        20
    );
    g.shutdown().await;
}

#[tokio::test]
async fn usage_limit_body_and_headers_are_preserved_and_pause_is_stored() {
    let (_d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 3600)).await;
    let key = g
        .mint(&ContainerId::Judge { payer: u.clone() })
        .await
        .expect("key");
    let reset = now() + 100;
    let body = json!({"error":{"type":"usage_limit_reached","resets_at":reset}});
    Mock::given(path("/responses"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_json(&body)
                .insert_header("x-codex-primary-used-percent", "100")
                .insert_header("retry-after", "100"),
        )
        .expect(1)
        .mount(&up)
        .await;
    let response = call(
        g.model_router(),
        "POST",
        "/v1/responses",
        key.key.expose(),
        json!({"store":false,"stream":true}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()["retry-after"], "100");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&collect(response).await).expect("json"),
        body
    );
    assert_eq!(
        g.usage(&u, None)
            .await
            .expect("usage")
            .resets_at
            .expect("reset")
            .as_second(),
        reset
    );
    g.shutdown().await;
}

#[tokio::test]
async fn staged_device_link_commits_without_refresh_and_keeps_secrets_gateway_only() {
    let (d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 3600)).await;
    Mock::given(path("/api/accounts/deviceauth/usercode"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"device_auth_id":"fake-device","user_code":"ABC-FAKE","interval":"1"}),
        ))
        .expect(1)
        .mount(&up)
        .await;
    Mock::given(path("/api/accounts/deviceauth/token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"authorization_code":"fake-code","code_verifier":"fake-pkce"}),
            ),
        )
        .expect(1)
        .mount(&up)
        .await;
    Mock::given(path("/oauth/token"))
        .and(wiremock::matchers::body_string_contains(
            "grant_type=authorization_code",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(token_json("fake-refresh-1")))
        .expect(1)
        .mount(&up)
        .await;
    let response = call(
        g.admin_router(),
        "POST",
        "/admin/credentials/payer-1/link",
        &"c".repeat(32),
        json!({"mode":"chatgpt_plan"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let code: serde_json::Value = serde_json::from_slice(&collect(response).await).expect("code");
    assert_eq!(code["user_code"], "ABC-FAKE");
    assert_eq!(code.as_object().expect("object").len(), 3);
    assert_eq!(
        g.access(&u, None)
            .await
            .expect("old during link")
            .bundle
            .refresh,
        "fake-refresh-0"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while g.store.lock(&u).await.expect("read").record.generation == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("linked");
    assert_eq!(
        g.access(&u, None).await.expect("linked").bundle.refresh,
        "fake-refresh-1"
    );
    let sealed = std::fs::read(g.store.record_path(&u)).expect("sealed");
    assert!(!sealed.windows(12).any(|w| w == b"fake-refresh"));
    assert!(
        std::fs::read_dir(d.path().join("users"))
            .expect("users")
            .all(|p| !p
                .expect("path")
                .file_name()
                .to_string_lossy()
                .contains("payer"))
    );
    g.shutdown().await;
}

#[tokio::test]
async fn unlink_during_device_start_cannot_resurrect_connection() {
    let (_d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 3600)).await;
    Mock::given(path("/api/accounts/deviceauth/usercode"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(100))
                .set_body_json(json!({
            "device_auth_id":"fake-device","user_code":"FAKE","interval":1})),
        )
        .expect(1)
        .mount(&up)
        .await;
    Mock::given(path("/oauth/revoke"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&up)
        .await;
    let task = {
        let g = g.clone();
        let u = u.clone();
        tokio::spawn(async move { g.begin_link(&u).await.is_ok() })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while up.received_requests().await.expect("requests").is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("request");
    g.unlink(&u).await.expect("unlink");
    assert!(!task.await.expect("link task"));
    assert!(g.access(&u, None).await.is_err());
    g.shutdown().await;
}

#[test]
fn claim_validation_rejects_wrong_issuer_audience_scope_and_workspace_shape() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let valid = token_json("fake-refresh-1");
    assert!(auth::parse_bundle(valid.clone()).is_some());
    for field in ["iss", "aud", "exp", "https://api.openai.com/auth"] {
        let mut token = valid.clone();
        let jwt = token["id_token"].as_str().expect("jwt");
        let mut claims: serde_json::Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(jwt.split('.').nth(1).expect("claims"))
                .expect("base64"),
        )
        .expect("json");
        claims[field] = json!("wrong");
        token["id_token"] = json!(format!(
            "e30.{}.fake",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        ));
        assert!(auth::parse_bundle(token).is_none(), "{field}");
    }
    let mut token = valid;
    token["scope"] = json!("openid");
    assert!(auth::parse_bundle(token).is_none());
}

#[tokio::test]
async fn replica_unlink_fences_a_pending_device_link() {
    let (d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 3600)).await;
    Mock::given(path("/api/accounts/deviceauth/usercode"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"device_auth_id":"fake-device","user_code":"FAKE","interval":1}),
            ),
        )
        .expect(1)
        .mount(&up)
        .await;
    Mock::given(path("/api/accounts/deviceauth/token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"authorization_code":"fake-code","code_verifier":"fake-pkce"}),
            ),
        )
        .mount(&up)
        .await;
    Mock::given(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(token_json("fake-refresh-1")))
        .mount(&up)
        .await;
    Mock::given(path("/oauth/revoke"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&up)
        .await;
    g.begin_link(&u).await.expect("start");
    let other = Gateway::for_test(d.path(), &up.uri()).expect("replica");
    other.unlink(&u).await.expect("unlink");
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert!(g.access(&u, None).await.is_err());
    g.shutdown().await;
    other.shutdown().await;
}

#[tokio::test]
async fn refresh_401_without_json_is_terminal() {
    let (_d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 1)).await;
    Mock::given(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .expect(1)
        .mount(&up)
        .await;
    assert!(g.access(&u, None).await.is_err());
    assert_eq!(
        g.status(&u).await.expect("status").relink_reason,
        Some(RelinkReason::Terminal)
    );
    g.shutdown().await;
}

#[test]
fn sse_tap_handles_all_chunk_boundaries() {
    let event = completed("final");
    for split in 0..=event.len() {
        let mut tap = proxy::Tap::default();
        let mut events = tap.push(&event.as_bytes()[..split]).expect("part 1");
        events.extend(tap.push(&event.as_bytes()[split..]).expect("part 2"));
        assert_eq!(events.len(), 1, "split {split}");
    }
}

#[test]
fn hop_headers_and_malformed_usage_cannot_cross_the_boundary() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "connection",
        "x-codex-turn-state, x-models-etag".parse().expect("header"),
    );
    headers.insert("x-codex-turn-state", "hop".parse().expect("header"));
    headers.insert("x-models-etag", "hop".parse().expect("header"));
    headers.insert("x-oai-request-id", "request".parse().expect("header"));
    let forwarded = proxy::response_headers(&headers);
    assert_eq!(forwarded.len(), 1);
    assert!(forwarded.contains_key("x-oai-request-id"));
    assert!(proxy::completion(&json!({"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":1}}})).is_err());
    assert!(proxy::completion(&json!({"type":"response.completed","response":{"id":"id","usage":{"input_tokens":10,"output_tokens":1,"input_tokens_details":{"cached_tokens":11}}}})).is_err());
}

#[tokio::test]
async fn first_sse_chunk_arrives_before_upstream_finishes() {
    let d = tempfile::tempdir().expect("temp");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).expect("private");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let fake = axum::Router::new().route(
        "/responses",
        axum::routing::post(|| async {
            let stream = futures::stream::unfold(0, |step| async move {
                match step {
                    0 => Some((
                        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(
                            b"data: {\"type\":\"response.in_progress\"}\n\n",
                        )),
                        1,
                    )),
                    1 => {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        Some((Ok(axum::body::Bytes::from(completed("late-final"))), 2))
                    }
                    _ => None,
                }
            });
            Body::from_stream(stream)
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, fake).await });
    let g = Arc::new(Gateway::for_test(d.path(), &format!("http://{address}")).expect("gateway"));
    let u = UserId("stream-payer".into());
    seed(&g, &u, bundle(now() + 3600)).await;
    let key = g
        .mint(&ContainerId::Judge { payer: u.clone() })
        .await
        .expect("key");
    let response = call(
        g.model_router(),
        "POST",
        "/v1/responses",
        key.key.expose(),
        json!({"store":false,"stream":true}),
    )
    .await;
    let mut body = response.into_body();
    let first = tokio::time::timeout(Duration::from_millis(150), body.frame())
        .await
        .expect("streaming, not buffering")
        .expect("frame")
        .expect("chunk");
    assert!(first.into_data().expect("data").starts_with(b"data:"));
    body.collect().await.expect("rest");
    assert_eq!(
        g.usage(&u, Some(&key.id))
            .await
            .expect("usage")
            .usage
            .input_tokens,
        10
    );
    g.shutdown().await;
    server.abort();
    assert!(matches!(server.await, Err(error) if error.is_cancelled()));
}

#[tokio::test]
async fn crash_after_replacement_write_before_marker_resolution_requires_relink() {
    let (d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 1)).await;
    let mut tx = g.store.lock(&u).await.expect("lock");
    tx.begin_refresh().expect("marker");
    tx.record.bundle = Some(bundle(now() + 3600));
    tx.record.generation = 1;
    tx.record.inflight = false;
    tx.save().expect("replacement committed, marker unresolved");
    drop(tx);
    let other = Gateway::for_test(d.path(), &up.uri()).expect("restart");
    assert!(other.access(&u, None).await.is_err());
    assert_eq!(
        other.status(&u).await.expect("status").relink_reason,
        Some(RelinkReason::Quarantined)
    );
    assert!(up.received_requests().await.expect("requests").is_empty());
    g.shutdown().await;
    other.shutdown().await;
}

#[tokio::test]
async fn retryable_midstream_slowdown_does_not_pause_the_payer() {
    let (_d, up, g, u) = fixture().await;
    seed(&g, &u, bundle(now() + 3600)).await;
    let key = g
        .mint(&ContainerId::Judge { payer: u.clone() })
        .await
        .expect("key");
    let failed = format!(
        "data: {}\n\n",
        json!({"type":"response.failed","response":{"error":{"code":"slow_down"}}})
    );
    Mock::given(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(failed.clone()))
        .expect(1)
        .mount(&up)
        .await;
    let response = call(
        g.model_router(),
        "POST",
        "/v1/responses",
        key.key.expose(),
        json!({"store":false,"stream":true}),
    )
    .await;
    assert_eq!(collect(response).await.as_ref(), failed.as_bytes());
    assert_eq!(g.usage(&u, None).await.expect("usage").resets_at, None);
    g.shutdown().await;
}
