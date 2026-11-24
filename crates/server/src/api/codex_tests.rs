//! Codex settings routes with the authenticated HTTP harness.
use super::tests::*;
use super::*;
use axum::http::StatusCode;
use contracts::job::result::JobOutcome;
use db::repo::DEFAULT_TENANT;
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn codex_linking_syncs_only_the_signed_in_payers_safe_status_and_unlinks() {
    let gateway = MockServer::start().await;
    let mut h = harness().await;
    std::sync::Arc::get_mut(&mut h.state.config)
        .unwrap()
        .codex_gateway = Some(
        crate::gateway::client::Admin::new(
            gateway.uri().parse().unwrap(),
            "fake-credentials-scope".into(),
        )
        .unwrap(),
    );
    crate::test_support::link_payer(&h.state.db, DEFAULT_TENANT, "codex-1").await;
    let route = "/api/v1/settings/codex";
    let linking = format!("{route}/link");
    let common = |state, reason| {
        json!({"mode":"chatgpt_plan", "state":state, "relink_reason":reason,
        "label":"Fake plan", "plan":"fake", "paused_until":null, "last_call":null,
        "access_token":"gateway-only-access-canary", "refresh_token":"gateway-only-refresh-canary"})
    };
    for method in ["POST", "DELETE"] {
        let path = if method == "POST" { &linking } else { route };
        let (status, _) = h
            .send(
                method,
                path,
                Some(&h.cookie),
                false,
                Some(json!({"mode":"chatgpt_plan"})),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    for body in [
        json!({"mode":"api_key"}),
        json!({"mode":"chatgpt_plan", "access_token":"forbidden"}),
        json!({"mode":"chatgpt_plan", "base_url":"http://127.0.0.1"}),
    ] {
        let (status, _) = h
            .send("POST", &linking, Some(&h.cookie), true, Some(body))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    assert!(gateway.received_requests().await.unwrap().is_empty());
    Mock::given(method("POST")).and(path("/admin/credentials/u1/link"))
        .and(header("authorization", "Bearer fake-credentials-scope"))
        .and(wiremock::matchers::body_json(json!({"mode":"chatgpt_plan"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"user_code":"FAKE-CODE", "verification_url":"http://127.0.0.1/device", "expires_at":"2026-11-24T12:00:00Z", "access_token":"must-not-cross"})))
        .expect(1).mount(&gateway).await;
    Mock::given(method("GET"))
        .and(path("/admin/credentials/u1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(common("linking", Value::Null)))
        .mount(&gateway)
        .await;
    let (status, challenge) = h
        .send(
            "POST",
            &linking,
            Some(&h.cookie),
            true,
            Some(json!({"mode":"chatgpt_plan"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{challenge}");
    assert_eq!(challenge.as_object().unwrap().len(), 3);
    assert_eq!(
        h.state
            .db
            .llm_credentials()
            .status("u1")
            .await
            .unwrap()
            .unwrap()
            .state,
        contracts::gateway::CredentialState::Linking
    );
    gateway.reset().await;
    for (state, reason) in [
        ("linked", Value::Null),
        ("relink_needed", json!("quarantined")),
        ("relink_needed", json!("terminal")),
    ] {
        Mock::given(method("GET"))
            .and(path("/admin/credentials/u1"))
            .and(header("authorization", "Bearer fake-credentials-scope"))
            .respond_with(ResponseTemplate::new(200).set_body_json(common(state, reason.clone())))
            .mount(&gateway)
            .await;
        Mock::given(method("GET"))
            .and(path("/admin/credentials/u1/link"))
            .respond_with(ResponseTemplate::new(200).set_body_json(common(state, reason.clone())))
            .mount(&gateway)
            .await;
        for endpoint in [route, &linking] {
            let (status, response) = h.get(endpoint).await;
            assert_eq!(status, StatusCode::OK, "{response}");
            assert_eq!(response["state"], state);
            assert_eq!(response["relink_reason"], reason);
            assert!(!response.to_string().contains("canary"));
            let saved = h
                .state
                .db
                .llm_credentials()
                .status("u1")
                .await
                .unwrap()
                .unwrap();
            let mut public: contracts::gateway::CredentialStatus =
                serde_json::from_value(response).unwrap();
            assert_eq!(public.telemetry.live_containers, Some(vec![]));
            public.telemetry.live_containers = None;
            assert_eq!(
                saved, public,
                "credential persistence excludes the live server view"
            );
        }
        gateway.reset().await;
    }
    Mock::given(method("DELETE"))
        .and(path("/admin/credentials/u1"))
        .and(wiremock::matchers::body_json(json!({"user":"u1"})))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&gateway)
        .await;
    let (status, _) = h.send("DELETE", route, Some(&h.cookie), true, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        h.state
            .db
            .llm_credentials()
            .status("u1")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        h.state
            .db
            .llm_credentials()
            .status("codex-1")
            .await
            .unwrap()
            .is_some(),
        "another payer is untouched"
    );
}

#[tokio::test]
async fn codex_gateway_failure_preserves_status_and_redacts_its_body() {
    let gateway = MockServer::start().await;
    let mut h = harness().await;
    std::sync::Arc::get_mut(&mut h.state.config)
        .unwrap()
        .codex_gateway = Some(
        crate::gateway::client::Admin::new(gateway.uri().parse().unwrap(), "fake-scope".into())
            .unwrap(),
    );
    let before = h.state.db.llm_credentials().status("u1").await.unwrap();
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503).set_body_string("upstream-secret-canary"))
        .mount(&gateway)
        .await;
    let (status, body) = h.get("/api/v1/settings/codex").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!body.to_string().contains("canary"));
    assert_eq!(
        h.state.db.llm_credentials().status("u1").await.unwrap(),
        before
    );
}

#[tokio::test]
async fn codex_status_rejects_redirects_and_oversized_or_malformed_replies() {
    let gateway = MockServer::start().await;
    let destination = MockServer::start().await;
    let mut h = harness().await;
    std::sync::Arc::get_mut(&mut h.state.config)
        .unwrap()
        .codex_gateway = Some(
        crate::gateway::client::Admin::new(gateway.uri().parse().unwrap(), "fake-scope".into())
            .unwrap(),
    );
    let before = h.state.db.llm_credentials().status("u1").await.unwrap();
    for reply in [
        ResponseTemplate::new(302).insert_header("location", destination.uri()),
        ResponseTemplate::new(200).set_body_string("x".repeat(16_385)),
        ResponseTemplate::new(200).set_body_json(json!({"access_token":"invalid-status-canary"})),
    ] {
        Mock::given(method("GET"))
            .respond_with(reply)
            .mount(&gateway)
            .await;
        let (status, _) = h.get("/api/v1/settings/codex").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            h.state.db.llm_credentials().status("u1").await.unwrap(),
            before
        );
        gateway.reset().await;
    }
    assert!(destination.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn codex_unlink_requires_gateway_acknowledgement() {
    let gateway = MockServer::start().await;
    let mut h = harness().await;
    std::sync::Arc::get_mut(&mut h.state.config)
        .unwrap()
        .codex_gateway = Some(
        crate::gateway::client::Admin::new(gateway.uri().parse().unwrap(), "fake-scope".into())
            .unwrap(),
    );
    crate::test_support::link_payer(&h.state.db, DEFAULT_TENANT, "u1").await;
    for status in [302, 202, 503] {
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&gateway)
            .await;
        let (response, _) = h
            .send(
                "DELETE",
                "/api/v1/settings/codex",
                Some(&h.cookie),
                true,
                None,
            )
            .await;
        assert_eq!(response, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            h.state
                .db
                .llm_credentials()
                .status("u1")
                .await
                .unwrap()
                .unwrap()
                .state,
            contracts::gateway::CredentialState::Linked
        );
        gateway.reset().await;
    }
}

#[tokio::test]
async fn codex_status_refresh_preserves_server_pause_and_typed_failure() {
    let gateway = MockServer::start().await;
    let mut h = harness().await;
    std::sync::Arc::get_mut(&mut h.state.config)
        .unwrap()
        .codex_gateway = Some(
        crate::gateway::client::Admin::new(gateway.uri().parse().unwrap(), "fake-scope".into())
            .unwrap(),
    );
    fail_one_job(
        &h,
        "u1",
        JobOutcome::UsageLimited { resets_at: None },
        "server-call-canary",
    )
    .await;
    let before = h
        .state
        .db
        .llm_credentials()
        .status("u1")
        .await
        .unwrap()
        .unwrap();
    let until = before.paused_until.unwrap();
    assert!(until > Timestamp::now() + jiff::SignedDuration::from_mins(29));
    let last = before.last_call.as_ref().unwrap();
    for gateway_call in [
        None,
        Some(last.clone()),
        Some(LlmCallOutcome {
            at: last.at - jiff::SignedDuration::from_secs(1),
            status: LlmCallStatus::Working,
        }),
    ] {
        let mut remote = before.clone();
        remote.paused_until = gateway_call
            .as_ref()
            .map(|_| until - jiff::SignedDuration::from_secs(1));
        remote.last_call = gateway_call;
        if let Some(LlmCallOutcome {
            status: LlmCallStatus::NotWorking { message },
            ..
        }) = &mut remote.last_call
        {
            *message = "Model request returned HTTP 429".into();
        }
        Mock::given(method("GET"))
            .and(path("/admin/credentials/u1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&remote))
            .mount(&gateway)
            .await;
        for endpoint in ["/api/v1/settings/codex", "/api/v1/settings/codex/link"] {
            Mock::given(method("GET"))
                .and(path("/admin/credentials/u1/link"))
                .respond_with(ResponseTemplate::new(200).set_body_json(&remote))
                .mount(&gateway)
                .await;
            let (status, _) = h.get(endpoint).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                h.state.db.llm_credentials().status("u1").await.unwrap(),
                Some(before.clone())
            );
            assert_eq!(h.llm_status().await["status"]["message"], USAGE_LIMIT);
            assert_eq!(
                h.state
                    .db
                    .settings()
                    .last_call(DEFAULT_TENANT, "u1")
                    .await
                    .unwrap()
                    .unwrap()
                    .failure,
                Some(CallFailure::UsageLimited)
            );
        }
        gateway.reset().await;
    }
    let mut newer = before.clone();
    newer.last_call = Some(LlmCallOutcome {
        at: last.at + jiff::SignedDuration::from_secs(1),
        status: LlmCallStatus::Working,
    });
    newer.paused_until = Some(until + jiff::SignedDuration::from_secs(1));
    newer.label = Some("Updated fake label".into());
    Mock::given(method("GET"))
        .and(path("/admin/credentials/u1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&newer))
        .mount(&gateway)
        .await;
    let (status, _) = h.get("/api/v1/settings/codex").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        h.state.db.llm_credentials().status("u1").await.unwrap(),
        Some(newer)
    );
    let call = h
        .state
        .db
        .settings()
        .last_call(DEFAULT_TENANT, "u1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.failure, None);
    assert_eq!(call.outcome.status, LlmCallStatus::Working);
}
