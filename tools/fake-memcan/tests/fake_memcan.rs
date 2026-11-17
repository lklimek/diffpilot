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

fn mcp_error(result: Result<CallToolResult, ServiceError>) -> (i32, String) {
    match result {
        Err(ServiceError::McpError(error)) => (error.code.0, error.message.to_string()),
        other => panic!("expected a JSON-RPC error, got {other:?}"),
    }
}

#[tokio::test]
async fn import_validates_records_and_counts_like_memcan() {
    let fake = Fake::start().await;
    let client = fake.connect().await.unwrap();
    let records = [
        record("ok", "p"),
        String::new(),
        json!({"id": "no-collection", "data": "text", "user_id": "project:p"}).to_string(),
        json!({"_collection": "memories", "id": "no-data", "user_id": "project:p"}).to_string(),
        json!({"_collection": "memories", "id": "number-data", "data": 7}).to_string(),
        json!({"_collection": "notes", "id": "foreign", "data": "text"}).to_string(),
        "not json".to_owned(),
    ]
    .join("\n");
    let result = call(&client, "_import_records", json!({ "records": records }))
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true));
    let answer = answer(&result);
    // Lines that do not parse are errors only; invalid records are skipped as well.
    assert_eq!(answer["imported"], 1, "{answer}");
    assert_eq!(answer["skipped"], 3, "{answer}");
    let errors: Vec<&str> = answer["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| error.as_str().unwrap())
        .collect();
    assert_eq!(errors.len(), 5, "{answer}");
    assert!(
        errors[0].starts_with("line 3: parsing JSONL line: missing field `_collection`"),
        "{answer}"
    );
    assert!(
        errors[1].starts_with("line 7: parsing JSONL line:"),
        "{answer}"
    );
    assert_eq!(errors[2], "record no-data missing 'data' field");
    assert_eq!(errors[3], "record number-data missing 'data' field");
    assert_eq!(
        errors[4],
        "unknown collection 'notes': unknown collection: notes"
    );
    let stored: Vec<String> = fake.state().await.memories.into_keys().collect();
    assert_eq!(stored, ["ok"]);
}

#[tokio::test]
async fn unusable_arguments_and_unknown_tools_are_json_rpc_errors() {
    let fake = Fake::start().await;
    let client = fake.connect().await.unwrap();
    for (tool, arguments, field) in [
        ("_import_records", json!({}), "records"),
        ("_import_records", json!({"records": 3}), ""),
        ("delete_memory", json!({}), "memory_id"),
        ("search_memories", json!({"project": "p"}), "query"),
    ] {
        let (code, message) = mcp_error(call(&client, tool, arguments).await);
        assert_eq!(code, -32602, "{tool}: invalid params");
        assert!(
            message.starts_with("failed to deserialize parameters: ") && message.contains(field),
            "{tool}: {message}"
        );
    }
    let (code, message) = mcp_error(call(&client, "add_memory", json!({"memory": "x"})).await);
    assert_eq!((code, message.as_str()), (-32602, "tool not found"));
}

#[tokio::test]
async fn search_scopes_like_memcan_and_reports_has_more_past_the_limit() {
    let fake = Fake::start().await;
    let client = fake.connect().await.unwrap();
    let records = ["a", "b", "c"].map(|id| record(id, "p")).join("\n");
    call(&client, "_import_records", json!({ "records": records }))
        .await
        .unwrap();
    call(
        &client,
        "_import_records",
        json!({"records": json!({"_collection": "memories", "id": "g", "data": "global", "user_id": "global"}).to_string()}),
    )
    .await
    .unwrap();
    let search = |arguments: Value| {
        let client = &client;
        async move { answer(&call(client, "search_memories", arguments).await.unwrap()) }
    };
    let page = search(json!({"query": "q", "project": "p", "limit": 2})).await;
    assert_eq!(page["memories"].as_array().unwrap().len(), 2);
    assert_eq!(page["has_more"], true);
    let all = search(json!({"query": "q", "project": "p", "limit": 3})).await;
    assert_eq!(all["memories"].as_array().unwrap().len(), 3);
    assert_eq!(all["has_more"], false);
    let entry = &all["memories"][0];
    for key in [
        "id",
        "memory",
        "hash",
        "score",
        "created_at",
        "updated_at",
        "user_id",
        "type",
    ] {
        assert!(entry.get(key).is_some(), "{key} missing in {entry}");
    }
    assert!(entry.get("data").is_none(), "{entry}");
    let by_user = search(json!({"query": "q", "project": "p", "user_id": "global"})).await;
    assert_eq!(
        by_user["memories"][0]["id"], "g",
        "user_id wins over project"
    );
    let default = search(json!({"query": "q"})).await;
    assert_eq!(default["memories"][0]["id"], "g", "MemCan's default scope");
    assert_eq!(default["has_more"], false);
}

#[tokio::test]
async fn embedding_failure_skips_imports_and_fails_searches_like_memcan() {
    let fake = Fake::start().await;
    let client = fake.connect().await.unwrap();
    call(
        &client,
        "_import_records",
        json!({"records": record("old", "p")}),
    )
    .await
    .unwrap();
    fake.set_mode(Mode::EmbeddingFailure).await;
    let result = call(
        &client,
        "_import_records",
        json!({"records": record("m1", "p")}),
    )
    .await
    .unwrap();
    assert_ne!(
        result.is_error,
        Some(true),
        "a successful result with counters, not an error"
    );
    assert_eq!(
        answer(&result),
        json!({"imported": 0, "skipped": 1,
            "errors": ["embedding batch failed for memories: embedding provider unavailable"]})
    );
    let batch = (0..51)
        .map(|n| record(&format!("b{n}"), "p"))
        .collect::<Vec<_>>()
        .join("\n");
    let batch = answer(
        &call(&client, "_import_records", json!({ "records": batch }))
            .await
            .unwrap(),
    );
    assert_eq!(batch["skipped"], 51, "{batch}");
    assert_eq!(
        batch["errors"].as_array().unwrap().len(),
        2,
        "one error per batch of 50: {batch}"
    );
    let (code, message) = mcp_error(
        call(
            &client,
            "search_memories",
            json!({"query": "q", "project": "p"}),
        )
        .await,
    );
    assert_eq!(code, -32603, "internal error");
    assert_eq!(message, "embedding failed: embedding provider unavailable");
    let deleted = call(&client, "delete_memory", json!({"memory_id": "old"}))
        .await
        .unwrap();
    assert_eq!(
        answer(&deleted),
        json!({"status": "deleted", "memory_id": "old"})
    );
    let state = fake.state().await;
    assert!(state.memories.is_empty());
    assert_eq!(state.mode, Mode::EmbeddingFailure);
}

#[tokio::test]
async fn synthetic_tool_error_answers_is_error_and_changes_nothing() {
    let fake = Fake::start().await;
    let client = fake.connect().await.unwrap();
    fake.set_mode(Mode::SyntheticToolError).await;
    let result = call(
        &client,
        "_import_records",
        json!({"records": record("m1", "p")}),
    )
    .await
    .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.content[0].as_text().unwrap().text,
        "fake-memcan synthetic tool error"
    );
    let state = fake.state().await;
    assert!(state.memories.is_empty());
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
    fake.set_mode(Mode::SyntheticToolError).await;
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
