//! Fake MemCan for the tests: a real MCP server (rmcp, streamable HTTP) with the tools the server calls, MemCan's
//! answer shapes, a bearer check and switchable misbehaviour.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ServerCapabilities,
    ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use secrecy::SecretString;
use serde_json::{Value, json};
use url::Url;

use super::client::MAX_ANSWER_BYTES;
use crate::config::MemcanConfig;

/// The bearer token the fake accepts (a canary for leak checks).
pub(crate) const TOKEN: &str = "memcan-token-CANARY-4711";

/// How the fake answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Like MemCan.
    Normal,
    /// Every HTTP request hangs (no handshake).
    HangAll,
    /// Tool calls hang.
    SlowCalls,
    /// Tool calls answer with an error result that quotes the token.
    ToolError,
    /// Tool calls answer with text that is not JSON.
    Garbage,
    /// Tool calls answer with more text than the client accepts.
    Oversized,
    /// Every HTTP request fails with a 500 whose body echoes the `Authorization` header.
    EchoToken,
}

#[derive(Debug)]
struct Inner {
    mode: Mode,
    calls: Vec<(String, Value)>,
    authorization: Vec<Option<String>>,
    memories: BTreeMap<String, Value>,
}

/// A running fake MemCan on a free local port.
#[derive(Clone, Debug)]
pub(crate) struct FakeMemcan {
    inner: Arc<Mutex<Inner>>,
    url: Url,
}

impl FakeMemcan {
    /// Starts the fake on its own thread, so its logs never mix with the test's [`LogCapture`].
    ///
    /// [`LogCapture`]: crate::test_support::logs::LogCapture
    pub(crate) fn start() -> Self {
        let inner = Arc::new(Mutex::new(Inner {
            mode: Mode::Normal,
            calls: Vec::new(),
            authorization: Vec::new(),
            memories: BTreeMap::new(),
        }));
        let handler = Handler(Arc::clone(&inner));
        let service: StreamableHttpService<Handler, LocalSessionManager> =
            StreamableHttpService::new(
                move || Ok(handler.clone()),
                Arc::default(),
                StreamableHttpServerConfig::default(),
            );
        let router = axum::Router::new()
            .nest_service("/mcp", service)
            .layer(from_fn_with_state(Arc::clone(&inner), gate));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake MemCan");
        listener.set_nonblocking(true).expect("non-blocking");
        let addr = listener.local_addr().expect("address");
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
                axum::serve(listener, router).await
            })
        });
        Self {
            inner,
            url: Url::parse(&format!("http://{addr}/mcp")).expect("url"),
        }
    }

    /// Configuration pointing at the fake with the right token.
    pub(crate) fn config(&self) -> MemcanConfig {
        self.config_with_token(TOKEN)
    }

    /// Configuration pointing at the fake with `token`.
    pub(crate) fn config_with_token(&self, token: &str) -> MemcanConfig {
        MemcanConfig {
            url: self.url.clone(),
            token: SecretString::from(token),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Switches the behaviour.
    pub(crate) fn set_mode(&self, mode: Mode) {
        self.lock().mode = mode;
    }

    /// Tool calls received, as `(tool, arguments)`.
    pub(crate) fn calls(&self) -> Vec<(String, Value)> {
        self.lock().calls.clone()
    }

    /// `Authorization` header of every HTTP request received.
    pub(crate) fn authorization(&self) -> Vec<Option<String>> {
        self.lock().authorization.clone()
    }

    /// Stored memories by id (payload as imported, without `id` and `_collection`).
    pub(crate) fn memories(&self) -> BTreeMap<String, Value> {
        self.lock().memories.clone()
    }

    /// Stores a memory directly (as any other MemCan writer could).
    pub(crate) fn insert_memory(&self, id: &str, payload: Value) {
        self.lock().memories.insert(id.to_owned(), payload);
    }
}

/// Records the `Authorization` header, applies the HTTP-level modes and MemCan's bearer check.
async fn gate(State(inner): State<Arc<Mutex<Inner>>>, request: Request, next: Next) -> Response {
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let mode = {
        let mut inner = inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.authorization.push(authorization.clone());
        inner.mode
    };
    match mode {
        Mode::HangAll => {
            tokio::time::sleep(Duration::from_secs(60)).await;
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Mode::EchoToken => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Body::from(format!("failed for {}", authorization.unwrap_or_default())),
        )
            .into_response(),
        _ if authorization.as_deref() != Some(&format!("Bearer {TOKEN}")) => {
            (StatusCode::UNAUTHORIZED, "invalid API key").into_response()
        }
        _ => next.run(request).await,
    }
}

#[derive(Clone, Debug)]
struct Handler(Arc<Mutex<Inner>>);

fn text(value: Value) -> Result<CallToolResponse, ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(value.to_string())]).into())
}

impl ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let arguments = Value::Object(request.arguments.unwrap_or_default().into_iter().collect());
        let mode = {
            let mut inner = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            inner
                .calls
                .push((request.name.to_string(), arguments.clone()));
            inner.mode
        };
        match mode {
            Mode::SlowCalls => tokio::time::sleep(Duration::from_secs(60)).await,
            Mode::ToolError => {
                return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                    "rejected the request with Bearer {TOKEN}"
                ))])
                .into());
            }
            Mode::Garbage => {
                return Ok(CallToolResult::success(vec![ContentBlock::text("<html>busy")]).into());
            }
            Mode::Oversized => {
                return Ok(CallToolResult::success(vec![ContentBlock::text(
                    "x".repeat(MAX_ANSWER_BYTES + 1),
                )])
                .into());
            }
            Mode::Normal | Mode::HangAll | Mode::EchoToken => {}
        }
        let mut inner = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match request.name.as_ref() {
            "_import_records" => {
                let records = arguments["records"].as_str().unwrap_or_default();
                let mut imported = 0;
                for line in records.lines() {
                    let Ok(Value::Object(mut record)) = serde_json::from_str::<Value>(line) else {
                        continue;
                    };
                    record.remove("_collection");
                    if let Some(Value::String(id)) = record.remove("id") {
                        inner.memories.insert(id, Value::Object(record));
                        imported += 1;
                    }
                }
                text(json!({"imported": imported, "skipped": 0, "errors": []}))
            }
            "delete_memory" => {
                let id = arguments["memory_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                match inner.memories.remove(&id) {
                    Some(_) => text(json!({"status": "deleted", "memory_id": id})),
                    None => Err(ErrorData::invalid_params(
                        format!("delete_memory failed: memory not found: {id}"),
                        None,
                    )),
                }
            }
            "search_memories" => {
                let scope = format!(
                    "project:{}",
                    arguments["project"].as_str().unwrap_or_default()
                );
                let limit = arguments["limit"].as_u64().unwrap_or(10);
                let memories: Vec<Value> = inner
                    .memories
                    .iter()
                    .filter(|(_, payload)| payload["user_id"] == scope.as_str())
                    .take(usize::try_from(limit).unwrap_or(usize::MAX))
                    .map(|(id, payload)| {
                        let mut entry = payload.clone();
                        if let Value::Object(entry) = &mut entry {
                            entry.insert("id".to_owned(), json!(id));
                            entry.insert("score".to_owned(), json!(0.9));
                            if let Some(data) = entry.remove("data") {
                                entry.insert("memory".to_owned(), data);
                            }
                        }
                        entry
                    })
                    .collect();
                text(json!({"memories": memories, "has_more": false}))
            }
            other => Err(ErrorData::invalid_params(
                format!("unknown tool {other}"),
                None,
            )),
        }
    }
}
