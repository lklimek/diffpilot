//! Fake MemCan for development and the full-stack tests: a real MCP server (rmcp, streamable HTTP) with the tools
//! the smart-review server calls (`_import_records`, `delete_memory`, `search_memories`) in MemCan's answer
//! shapes, MemCan's bearer check, and a control endpoint (`/_fake/*`, same bearer) to inspect what was written or
//! deleted and to switch failure modes. Memories are kept in memory only.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ServerCapabilities,
    ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Longest time a request waits in [`Mode::Hang`] before it gets a 503.
const HANG_LIMIT: Duration = Duration::from_secs(120);
/// How often a hanging request checks whether the mode changed.
const HANG_POLL: Duration = Duration::from_millis(100);

/// Configuration of the fake.
#[derive(Clone, Debug)]
pub struct Config {
    /// Bearer token MCP and control requests must present.
    pub token: String,
    /// Path of the MCP endpoint, e.g. `/mcp`.
    pub path: String,
}

/// How the fake answers; switched with `POST /_fake/mode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Mode {
    /// Like MemCan.
    #[default]
    Normal,
    /// Tool calls answer with an error result; nothing is stored or deleted.
    Reject,
    /// Every MCP request gets HTTP 503 (no MCP session: the client sees MemCan as unreachable).
    Unavailable,
    /// Every MCP request waits without an answer until the mode changes (at most two minutes).
    Hang,
    /// Tool calls are carried out after `ms` milliseconds.
    Delay {
        /// The delay.
        ms: u64,
    },
}

/// One tool call received.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Call {
    /// Tool name.
    pub tool: String,
    /// Arguments as sent.
    pub arguments: Value,
}

/// What the fake holds and saw (`GET /_fake/state`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Current mode.
    pub mode: Mode,
    /// Stored memories by id (the imported record without `id` and `_collection`).
    pub memories: BTreeMap<String, Value>,
    /// Ids of memories deleted, in order.
    pub deleted: Vec<String>,
    /// Every tool call, in order.
    pub calls: Vec<Call>,
    /// HTTP requests refused for a missing or wrong bearer token.
    pub unauthorized: u64,
}

#[derive(Debug)]
struct Inner {
    token: String,
    state: Snapshot,
}

#[derive(Clone, Debug)]
struct Shared(Arc<Mutex<Inner>>);

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn mode(&self) -> Mode {
        self.lock().state.mode
    }
}

/// The fake's router: MCP at `config.path`, control at `/_fake/state`, `/_fake/mode` and `/_fake/reset`.
pub fn router(config: Config) -> Router {
    let shared = Shared(Arc::new(Mutex::new(Inner {
        token: config.token,
        state: Snapshot::default(),
    })));
    let handler = Handler(shared.clone());
    let mcp: StreamableHttpService<Handler, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::default(),
        StreamableHttpServerConfig::default(),
    );
    let control = Router::new()
        .route("/_fake/state", get(state))
        .route("/_fake/mode", post(set_mode))
        .route("/_fake/reset", post(reset))
        .with_state(shared.clone());
    Router::new()
        .nest_service(&config.path, mcp)
        .layer(from_fn_with_state(shared.clone(), mcp_modes))
        .merge(control)
        .layer(from_fn_with_state(shared, bearer))
}

/// MemCan's bearer check for every request.
async fn bearer(State(shared): State<Shared>, request: Request, next: Next) -> Response {
    let presented = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let accepted = {
        let mut inner = shared.lock();
        let accepted = presented == Some(inner.token.as_str());
        if !accepted {
            inner.state.unauthorized += 1;
        }
        accepted
    };
    if accepted {
        next.run(request).await
    } else {
        tracing::warn!(
            path = request.uri().path(),
            "fake-memcan refused a request without the configured bearer token"
        );
        (StatusCode::UNAUTHORIZED, "invalid API key").into_response()
    }
}

/// The HTTP-level modes of the MCP endpoint.
async fn mcp_modes(State(shared): State<Shared>, request: Request, next: Next) -> Response {
    match shared.mode() {
        Mode::Unavailable => {
            tracing::info!("fake-memcan answered 503 (mode unavailable)");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Mode::Hang => {
            let mut waited = Duration::ZERO;
            while shared.mode() == Mode::Hang && waited < HANG_LIMIT {
                tokio::time::sleep(HANG_POLL).await;
                waited += HANG_POLL;
            }
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Mode::Normal | Mode::Reject | Mode::Delay { .. } => next.run(request).await,
    }
}

async fn state(State(shared): State<Shared>) -> Json<Snapshot> {
    Json(shared.lock().state.clone())
}

async fn set_mode(State(shared): State<Shared>, Json(mode): Json<Mode>) -> StatusCode {
    tracing::info!(?mode, "fake-memcan mode changed");
    shared.lock().state.mode = mode;
    StatusCode::NO_CONTENT
}

async fn reset(State(shared): State<Shared>) -> StatusCode {
    tracing::info!("fake-memcan reset");
    shared.lock().state = Snapshot::default();
    StatusCode::NO_CONTENT
}

#[derive(Clone, Debug)]
struct Handler(Shared);

impl ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let call = Call {
            tool: request.name.to_string(),
            arguments: Value::Object(request.arguments.unwrap_or_default().into_iter().collect()),
        };
        let mode = {
            let mut inner = self.0.lock();
            inner.state.calls.push(call.clone());
            inner.state.mode
        };
        match mode {
            Mode::Reject => {
                tracing::info!(tool = %call.tool, "fake-memcan rejected a tool call (mode reject)");
                Ok(CallToolResult::error(vec![ContentBlock::text("embedding failed")]).into())
            }
            Mode::Delay { ms } => {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                execute(&self.0, &call)
            }
            Mode::Normal | Mode::Unavailable | Mode::Hang => execute(&self.0, &call),
        }
    }
}

fn text(value: Value) -> Result<CallToolResponse, ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(value.to_string())]).into())
}

/// Carries out a tool call like MemCan.
fn execute(shared: &Shared, call: &Call) -> Result<CallToolResponse, ErrorData> {
    let mut inner = shared.lock();
    let state = &mut inner.state;
    match call.tool.as_str() {
        "_import_records" => {
            let records = call.arguments["records"].as_str().unwrap_or_default();
            let mut imported = 0;
            for line in records.lines() {
                let Ok(Value::Object(mut record)) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                record.remove("_collection");
                if let Some(Value::String(id)) = record.remove("id") {
                    tracing::info!(%id, "fake-memcan stored a memory");
                    state.memories.insert(id, Value::Object(record));
                    imported += 1;
                }
            }
            text(json!({"imported": imported, "skipped": 0, "errors": []}))
        }
        "delete_memory" => {
            let id = call.arguments["memory_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            match state.memories.remove(&id) {
                Some(_) => {
                    tracing::info!(%id, "fake-memcan deleted a memory");
                    state.deleted.push(id.clone());
                    text(json!({"status": "deleted", "memory_id": id}))
                }
                None => Err(ErrorData::invalid_params(
                    format!("delete_memory failed: memory not found: {id}"),
                    None,
                )),
            }
        }
        "search_memories" => {
            let scope = format!(
                "project:{}",
                call.arguments["project"].as_str().unwrap_or_default()
            );
            let limit = call.arguments["limit"].as_u64().unwrap_or(10);
            let memories: Vec<Value> = state
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
