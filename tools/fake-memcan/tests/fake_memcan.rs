//! Drives the fake MemCan over real TCP with the MCP client the server uses (rmcp, streamable HTTP).
#![expect(clippy::unwrap_used, reason = "test helpers outside #[test] functions")]

use std::time::{Duration, Instant};

use fake_memcan::{Call, Config, Mode, Snapshot};
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult};
use rmcp::service::{RoleClient, RunningService, ServiceError};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};

const TOKEN: &str = "fake-memcan-test-token";

struct Fake {
    base: String,
    http: reqwest::Client,
}

impl Fake {
    async fn start() -> Self {
        let router = fake_memcan::router(Config {
            token: TOKEN.to_owned(),
            path: "/mcp".to_owned(),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await });
        Self {
            base: format!("http://{addr}"),
            http: reqwest::Client::new(),
        }
    }

    async fn connect(&self) -> Result<RunningService<RoleClient, ()>, String> {
        let transport = StreamableHttpClientTransport::with_client(
            reqwest::Client::new(),
            StreamableHttpClientTransportConfig::with_uri(format!("{}/mcp", self.base))
                .auth_header(TOKEN),
        );
        tokio::time::timeout(Duration::from_secs(5), ().serve(transport))
            .await
            .map_err(|_| "handshake timed out".to_owned())?
            .map_err(|error| error.to_string())
    }

    async fn state(&self) -> Snapshot {
        self.http
            .get(format!("{}/_fake/state", self.base))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn set_mode(&self, mode: Mode) {
        let status = self
            .http
            .post(format!("{}/_fake/mode", self.base))
            .bearer_auth(TOKEN)
            .json(&mode)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, reqwest::StatusCode::NO_CONTENT);
    }
}

async fn call(
    client: &RunningService<RoleClient, ()>,
    tool: &'static str,
    arguments: Value,
) -> Result<CallToolResult, ServiceError> {
    let Value::Object(arguments) = arguments else {
        panic!("arguments must be an object")
    };
    match client
        .call_tool_once(CallToolRequestParams::new(tool).with_arguments(arguments))
        .await?
    {
        CallToolResponse::Complete(result) => Ok(result),
        other => panic!("unexpected response {other:?}"),
    }
}

fn answer(result: &CallToolResult) -> Value {
    let text = result.content[0].as_text().unwrap().text.clone();
    serde_json::from_str(&text).unwrap()
}

fn record(id: &str, project: &str) -> String {
    json!({
        "_collection": "memories",
        "id": id,
        "data": "severity calibration example: overrated",
        "user_id": format!("project:{project}"),
        "project": project,
        "type": "calibration",
        "value": "overrated",
    })
    .to_string()
}

#[tokio::test]
async fn requests_without_the_bearer_token_are_refused() {
    let fake = Fake::start().await;
    for (path, token) in [
        ("/mcp", None),
        ("/mcp", Some("wrong")),
        ("/_fake/state", None),
    ] {
        let mut request = fake
            .http
            .post(format!("{}{path}", fake.base))
            .json(&json!({}));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.unwrap();
        assert_eq!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
    assert_eq!(fake.state().await.unauthorized, 3);
}

#[tokio::test]
async fn import_search_delete_round_trip_in_memcan_shapes() {
    let fake = Fake::start().await;
    let client = fake.connect().await.unwrap();

    let imported = call(
        &client,
        "_import_records",
        json!({"records": record("m1", "node-client")}),
    )
    .await
    .unwrap();
    assert_eq!(
        answer(&imported),
        json!({"imported": 1, "skipped": 0, "errors": []})
    );
    call(
        &client,
        "_import_records",
        json!({"records": record("m2", "other")}),
    )
    .await
    .unwrap();
    let state = fake.state().await;
    let stored = &state.memories["m1"];
    assert_eq!(stored["project"], "node-client");
    assert!(
        stored.get("id").is_none() && stored.get("_collection").is_none(),
        "{stored}"
    );

    let found = call(
        &client,
        "search_memories",
        json!({"query": "retry", "project": "node-client", "limit": 5}),
    )
    .await
    .unwrap();
    let found = answer(&found);
    let memories = found["memories"].as_array().unwrap();
    assert_eq!(memories.len(), 1, "only the project's memories: {found}");
    assert_eq!(memories[0]["id"], "m1");
    assert_eq!(
        memories[0]["memory"],
        "severity calibration example: overrated"
    );

    let deleted = call(&client, "delete_memory", json!({"memory_id": "m1"}))
        .await
        .unwrap();
    assert_eq!(
        answer(&deleted),
        json!({"status": "deleted", "memory_id": "m1"})
    );
    let again = call(&client, "delete_memory", json!({"memory_id": "m1"})).await;
    match again {
        Err(ServiceError::McpError(error)) => {
            assert_eq!(error.message, "delete_memory failed: memory not found: m1");
        }
        other => panic!("expected MemCan's not-found error, got {other:?}"),
    }
    let state = fake.state().await;
    assert_eq!(state.deleted, vec!["m1".to_owned()]);
    assert!(!state.memories.contains_key("m1"));
    assert_eq!(
        state
            .calls
            .iter()
            .map(|call| call.tool.as_str())
            .collect::<Vec<_>>(),
        [
            "_import_records",
            "_import_records",
            "search_memories",
            "delete_memory",
            "delete_memory"
        ]
    );
}

#[tokio::test]
async fn reject_mode_answers_with_an_error_result_and_stores_nothing() {
    let fake = Fake::start().await;
    let client = fake.connect().await.unwrap();
    fake.set_mode(Mode::Reject).await;
    let result = call(
        &client,
        "_import_records",
        json!({"records": record("m1", "p")}),
    )
    .await
    .unwrap();
    assert_eq!(result.is_error, Some(true));
    let state = fake.state().await;
    assert!(state.memories.is_empty());
    assert_eq!(state.mode, Mode::Reject);
    assert_eq!(
        state.calls,
        vec![Call {
            tool: "_import_records".to_owned(),
            arguments: json!({"records": record("m1", "p")}),
        }]
    );
}

#[tokio::test]
async fn unavailable_mode_fails_the_handshake_until_switched_back() {
    let fake = Fake::start().await;
    fake.set_mode(Mode::Unavailable).await;
    assert!(
        fake.connect().await.is_err(),
        "no MCP session while unavailable"
    );
    fake.set_mode(Mode::Normal).await;
    let client = fake.connect().await.unwrap();
    call(
        &client,
        "_import_records",
        json!({"records": record("m1", "p")}),
    )
    .await
    .unwrap();
    assert!(fake.state().await.memories.contains_key("m1"));
}

#[tokio::test]
async fn hang_mode_holds_requests_until_the_mode_changes() {
    let fake = Fake::start().await;
    fake.set_mode(Mode::Hang).await;
    let held = fake
        .http
        .post(format!("{}/mcp", fake.base))
        .bearer_auth(TOKEN)
        .json(&json!({}))
        .timeout(Duration::from_millis(500))
        .send()
        .await;
    assert!(
        held.is_err_and(|error| error.is_timeout()),
        "request answered while hanging"
    );
    let pending = tokio::spawn({
        let request = fake
            .http
            .post(format!("{}/mcp", fake.base))
            .bearer_auth(TOKEN)
            .json(&json!({}));
        async move { request.send().await.unwrap().status() }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    fake.set_mode(Mode::Normal).await;
    assert_eq!(
        pending.await.unwrap(),
        reqwest::StatusCode::SERVICE_UNAVAILABLE
    );
    fake.connect().await.unwrap();
}

#[tokio::test]
async fn delay_mode_carries_out_tool_calls_late() {
    let fake = Fake::start().await;
    let client = fake.connect().await.unwrap();
    fake.set_mode(Mode::Delay { ms: 400 }).await;
    let started = Instant::now();
    call(
        &client,
        "_import_records",
        json!({"records": record("m1", "p")}),
    )
    .await
    .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(400));
    assert!(fake.state().await.memories.contains_key("m1"));
}

#[tokio::test]
async fn reset_clears_memories_calls_and_mode() {
    let fake = Fake::start().await;
    let client = fake.connect().await.unwrap();
    call(
        &client,
        "_import_records",
        json!({"records": record("m1", "p")}),
    )
    .await
    .unwrap();
    fake.set_mode(Mode::Reject).await;
    let status = fake
        .http
        .post(format!("{}/_fake/reset", fake.base))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, reqwest::StatusCode::NO_CONTENT);
    assert_eq!(fake.state().await, Snapshot::default());
}
