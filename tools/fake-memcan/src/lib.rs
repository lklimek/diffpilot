//! Fake MemCan for development and the full-stack tests: a real MCP server (rmcp, streamable HTTP) with the tools
//! the smart-review server calls (`_import_records`, `delete_memory`, `search_memories`), following MemCan's
//! contract (memcan-server `serve.rs`, memcan-core `import.rs` at dd00d97): record validation and the
//! `imported`/`skipped`/`errors` counters, MemCan's JSON-RPC errors, `has_more` past the limit. Also MemCan's
//! bearer check and a control endpoint (`/_fake/*`, same bearer) to inspect what was written or deleted and to
//! switch failure modes. Memories are kept in memory only.

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
    /// MemCan's answers while its embedding provider fails: an import reports every valid record skipped with an
    /// `embedding batch failed` error; a search fails with an internal JSON-RPC error. Deletes still work.
    EmbeddingFailure,
    /// Synthetic, not a MemCan behaviour: every tool call answers with an `isError` result; nothing changes.
    SyntheticToolError,
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
        Mode::Normal | Mode::EmbeddingFailure | Mode::SyntheticToolError | Mode::Delay { .. } => {
            next.run(request).await
        }
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
            Mode::SyntheticToolError => {
                tracing::info!(tool = %call.tool, "fake-memcan answered a synthetic tool error");
                Ok(CallToolResult::error(vec![ContentBlock::text(
                    "fake-memcan synthetic tool error",
                )])
                .into())
            }
            Mode::Delay { ms } => {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                execute(&self.0, &call, false)
            }
            Mode::EmbeddingFailure => execute(&self.0, &call, true),
            Mode::Normal | Mode::Unavailable | Mode::Hang => execute(&self.0, &call, false),
        }
    }
}

fn text(value: Value) -> Result<CallToolResponse, ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(value.to_string())]).into())
}

/// MemCan's default scope when a call names neither `project` nor `user_id`.
const DEFAULT_USER_ID: &str = "global";
/// MemCan's collections an import may name.
const COLLECTIONS: [&str; 4] = ["memories", "standards", "code", "todos"];
/// Records MemCan embeds in one batch.
const EMBED_BATCH_SIZE: usize = 50;
/// What MemCan's embedder reports in [`Mode::EmbeddingFailure`].
const EMBEDDING_ERROR: &str = "embedding provider unavailable";

#[derive(Deserialize)]
struct ImportParams {
    records: String,
}

#[derive(Deserialize)]
struct ImportRecord {
    _collection: String,
    id: String,
    #[serde(flatten)]
    payload: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
struct DeleteParams {
    memory_id: String,
}

#[derive(Deserialize)]
struct SearchParams {
    #[expect(dead_code, reason = "required like MemCan's; the fake does not rank")]
    query: String,
    project: Option<String>,
    user_id: Option<String>,
    limit: Option<u32>,
}

/// The tool's parameters, or rmcp's invalid-params error MemCan answers for unusable arguments.
fn params<T: serde::de::DeserializeOwned>(call: &Call) -> Result<T, ErrorData> {
    serde_json::from_value(call.arguments.clone()).map_err(|error| {
        ErrorData::invalid_params(format!("failed to deserialize parameters: {error}"), None)
    })
}

/// Carries out a tool call like MemCan; `embedding_fails` as in [`Mode::EmbeddingFailure`].
fn execute(
    shared: &Shared,
    call: &Call,
    embedding_fails: bool,
) -> Result<CallToolResponse, ErrorData> {
    match call.tool.as_str() {
        "_import_records" => {
            let records = params::<ImportParams>(call)?.records;
            text(import(&mut shared.lock().state, &records, embedding_fails))
        }
        "delete_memory" => {
            let id = params::<DeleteParams>(call)?.memory_id;
            let state = &mut shared.lock().state;
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
            let search = params::<SearchParams>(call)?;
            if embedding_fails {
                return Err(ErrorData::internal_error(
                    format!("embedding failed: {EMBEDDING_ERROR}"),
                    None,
                ));
            }
            let scope = match (search.user_id, search.project) {
                (Some(user_id), _) => user_id,
                (None, Some(project)) => format!("project:{project}"),
                (None, None) => DEFAULT_USER_ID.to_owned(),
            };
            let limit = usize::try_from(search.limit.unwrap_or(10).clamp(1, 1000)).unwrap_or(10);
            let state = &shared.lock().state;
            let mut memories: Vec<Value> = state
                .memories
                .iter()
                .filter(|(_, payload)| payload["user_id"] == scope.as_str())
                .take(limit + 1)
                .map(|(id, payload)| search_entry(id, payload))
                .collect();
            let has_more = memories.len() > limit;
            memories.truncate(limit);
            text(json!({"memories": memories, "has_more": has_more}))
        }
        _ => Err(ErrorData::invalid_params("tool not found", None)),
    }
}

/// MemCan's `_import_records` (memcan-server `_import_records`, memcan-core `import_records`): a line that does
/// not parse is an error only; records are grouped by collection (first-seen order here) and embedded in batches
/// of 50, where an unknown collection, a record without text `data` and every record of a failed embedding batch
/// count as skipped with an error. Parse errors come first in `errors`.
fn import(state: &mut Snapshot, records: &str, embedding_fails: bool) -> Value {
    let mut errors = Vec::new();
    let mut groups: Vec<(String, Vec<ImportRecord>)> = Vec::new();
    for (index, line) in records.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<ImportRecord>(line) {
            Ok(mut record) => {
                record.payload.remove("id");
                record.payload.remove("_collection");
                match groups
                    .iter_mut()
                    .find(|(name, _)| *name == record._collection)
                {
                    Some((_, group)) => group.push(record),
                    None => groups.push((record._collection.clone(), vec![record])),
                }
            }
            Err(error) => errors.push(format!("line {}: parsing JSONL line: {error}", index + 1)),
        }
    }
    let (mut imported, mut skipped) = (0_usize, 0_usize);
    for (collection, group) in groups {
        if !COLLECTIONS.contains(&collection.as_str()) {
            errors.push(format!(
                "unknown collection '{collection}': unknown collection: {collection}"
            ));
            skipped += group.len();
            continue;
        }
        let mut group = group.into_iter().peekable();
        while group.peek().is_some() {
            let mut valid = Vec::new();
            for record in group.by_ref().take(EMBED_BATCH_SIZE) {
                if record.payload.get("data").is_some_and(Value::is_string) {
                    valid.push(record);
                } else {
                    errors.push(format!("record {} missing 'data' field", record.id));
                    skipped += 1;
                }
            }
            if valid.is_empty() {
                continue;
            }
            if embedding_fails {
                errors.push(format!(
                    "embedding batch failed for {collection}: {EMBEDDING_ERROR}"
                ));
                skipped += valid.len();
                continue;
            }
            imported += valid.len();
            if collection == "memories" {
                for record in valid {
                    tracing::info!(id = %record.id, "fake-memcan stored a memory");
                    state
                        .memories
                        .insert(record.id, Value::Object(record.payload));
                }
            }
        }
    }
    json!({"imported": imported, "skipped": skipped, "errors": errors})
}

/// One `search_memories` entry in MemCan's shape (`format_memory_results`).
fn search_entry(id: &str, payload: &Value) -> Value {
    let mut entry = json!({
        "id": id,
        "memory": payload["data"].as_str().unwrap_or_default(),
        "hash": payload["hash"].as_str().unwrap_or_default(),
        "score": 0.9,
        "created_at": payload.get("created_at"),
        "updated_at": payload.get("updated_at"),
        "user_id": payload["user_id"].as_str().unwrap_or_default(),
    });
    if let (Some(payload), Some(entry)) = (payload.as_object(), entry.as_object_mut()) {
        for (key, value) in payload {
            if !matches!(
                key.as_str(),
                "data" | "hash" | "created_at" | "updated_at" | "user_id"
            ) {
                entry.insert(key.clone(), value.clone());
            }
        }
    }
    entry
}
