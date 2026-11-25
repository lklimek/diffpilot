use super::*;
use contracts::job::event::{ActivityKind, JobEvent};
use contracts::time::Timestamp;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::HashMap;
use tokio::net::UnixStream;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::WebSocketConfig},
};

pub(super) const MAX_MESSAGE: usize = 8 * 1024 * 1024;

#[derive(Default)]
pub(super) struct TurnLog {
    pub thread: String,
    pub completed: Option<Value>,
    pub output: Option<String>,
    pub tool_used: bool,
    pub updated: bool,
}

pub(super) struct Rpc {
    socket: WebSocketStream<UnixStream>,
    next_id: u64,
    pub totals: HashMap<String, Usage>,
    pub last: HashMap<String, u64>,
    pub turn: TurnLog,
    pub kind: ThreadKind,
    pub scrubber: crate::scrub::Scrubber,
    events: mpsc::Sender<ProviderEvent>,
}

impl Rpc {
    pub async fn connect(
        socket: UnixStream,
        events: mpsc::Sender<ProviderEvent>,
        scrubber: crate::scrub::Scrubber,
    ) -> Result<Self, ProviderError> {
        let limits = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE));
        let (socket, _) =
            tokio_tungstenite::client_async_with_config("ws://localhost/", socket, Some(limits))
                .await
                .map_err(|_| {
                    fault(
                        JobFailureKind::WorkerLost,
                        "initialize",
                        "WebSocket handshake",
                    )
                })?;
        let mut rpc = Self {
            socket,
            next_id: 0,
            totals: HashMap::new(),
            last: HashMap::new(),
            turn: TurnLog::default(),
            kind: ThreadKind::Analytic,
            scrubber,
            events,
        };
        rpc.request("initialize", json!({"clientInfo":{"name":"diffpilot","title":"diffpilot","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})).await?;
        rpc.send(json!({"method":"initialized","params":{}}))
            .await?;
        Ok(rpc)
    }

    async fn send(&mut self, value: Value) -> Result<(), ProviderError> {
        let text = value.to_string();
        if text.len() > MAX_MESSAGE {
            return Err(fault(
                JobFailureKind::WorkerLost,
                "websocket/send",
                "outbound message exceeds cap",
            ));
        }
        self.socket
            .send(Message::Text(text.into()))
            .await
            .map_err(|_| fault(JobFailureKind::WorkerLost, "websocket/send", "frame write"))
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value, ProviderError> {
        tokio::time::timeout(
            std::time::Duration::from_secs(65),
            self.request_inner(method, params),
        )
        .await
        .map_err(|_| {
            fault(
                JobFailureKind::WorkerLost,
                method,
                "65 second request timeout",
            )
        })?
        .inspect_err(|_| tracing::warn!(method, step = "request", "app-server RPC request failed"))
    }

    async fn request_inner(&mut self, method: &str, params: Value) -> Result<Value, ProviderError> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"id":id,"method":method,"params":params}))
            .await?;
        loop {
            let value = self.receive().await?;
            if value.get("method").is_some() {
                self.observe(value).await?;
                continue;
            }
            if value["id"] != id {
                return Err(fault(
                    JobFailureKind::WorkerLost,
                    method,
                    "response id mismatch",
                ));
            }
            if let Some(error) = value.get("error") {
                if method == "thread/resume"
                    && error["message"].as_str().is_some_and(|m| {
                        m.contains("thread not found") || m.contains("no rollout found")
                    })
                {
                    return Err(ProviderError::SessionUnavailable);
                }
                return Err(failure(error));
            }
            return value.get("result").cloned().ok_or_else(|| {
                fault(JobFailureKind::WorkerLost, method, "response has no result")
            });
        }
    }

    async fn receive(&mut self) -> Result<Value, ProviderError> {
        loop {
            match self.socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    return serde_json::from_str(&text).map_err(|_| {
                        fault(
                            JobFailureKind::WorkerLost,
                            "websocket/receive",
                            "invalid JSON frame",
                        )
                    });
                }
                Some(Ok(Message::Ping(_))) => self.socket.flush().await.map_err(|_| {
                    fault(
                        JobFailureKind::WorkerLost,
                        "websocket/receive",
                        "ping flush",
                    )
                })?,
                Some(Ok(Message::Pong(_))) => {}
                _ => {
                    return Err(fault(
                        JobFailureKind::WorkerLost,
                        "websocket/receive",
                        "closed or invalid frame",
                    ));
                }
            }
        }
    }

    pub async fn next_notification(&mut self) -> Result<(), ProviderError> {
        let value = self.receive().await?;
        if value.get("method").is_none() {
            return Err(fault(
                JobFailureKind::WorkerLost,
                "websocket/receive",
                "unexpected response while awaiting notification",
            ));
        }
        self.observe(value).await
    }

    async fn observe(&mut self, value: Value) -> Result<(), ProviderError> {
        if value.get("id").is_some() {
            let method: String = self
                .scrubber
                .text(value["method"].as_str().unwrap_or(""))
                .chars()
                .take(128)
                .collect();
            tracing::warn!(
                method,
                step = "server request",
                "app-server requested an unsupported runner operation"
            );
            // Approval, user-input and credential-refresh requests are never granted.
            self.send(json!({"id":value["id"],"error":{"code":-32601,"message":"runner does not provide this operation"}})).await?;
            return Err(runtime(JobFailureKind::UpstreamRejected));
        }
        let method = value["method"].as_str().unwrap_or("");
        let p = &value["params"];
        let thread = p["threadId"].as_str().unwrap_or("");
        if method == "thread/tokenUsage/updated" {
            let total = usage(&p["tokenUsage"]["total"])?;
            let last = &p["tokenUsage"]["last"];
            let context = number(last, "inputTokens")?
                .checked_add(number(last, "outputTokens")?)
                .ok_or_else(|| {
                    fault(
                        JobFailureKind::ProviderFailed,
                        "thread/tokenUsage/updated",
                        "context usage overflow",
                    )
                })?;
            self.totals.insert(thread.to_owned(), total);
            self.last.insert(thread.to_owned(), context);
            if thread == self.turn.thread {
                self.turn.updated = true;
            }
        }
        if thread != self.turn.thread || thread.is_empty() {
            return Ok(());
        }
        match method {
            "turn/completed" => {
                for item in p["turn"]["items"].as_array().into_iter().flatten() {
                    match item["type"].as_str() {
                        Some("agentMessage") => {
                            if item["phase"].as_str() != Some("commentary") {
                                self.turn.output = item["text"].as_str().map(str::to_owned);
                            }
                        }
                        Some("userMessage" | "reasoning" | "contextCompaction") => {}
                        _ => self.turn.tool_used = true,
                    }
                }
                self.turn.completed = Some(p["turn"].clone());
            }
            "item/started" | "item/completed" => {
                let item = &p["item"];
                let item_type = item["type"].as_str().unwrap_or("");
                if !matches!(
                    item_type,
                    "userMessage" | "agentMessage" | "reasoning" | "contextCompaction"
                ) {
                    self.turn.tool_used = true;
                }
                if method == "item/completed"
                    && item_type == "agentMessage"
                    && item["phase"].as_str() != Some("commentary")
                {
                    self.turn.output = item["text"].as_str().map(str::to_owned);
                }
                if method == "item/started" {
                    let (activity, summary) = match item_type {
                        "commandExecution" => (ActivityKind::Command, "Agent ran a command"),
                        "mcpToolCall" => {
                            (ActivityKind::McpToolCall, "Agent called a configured tool")
                        }
                        "reasoning" => (ActivityKind::Reasoning, "Agent is reasoning"),
                        _ => (ActivityKind::AgentMessage, "Agent is working"),
                    };
                    // Liveness never blocks the protocol reader behind an absent consumer.
                    let _ = self.events.try_send(ProviderEvent::Activity(JobEvent {
                        seq: 0,
                        at: Timestamp::now(),
                        activity,
                        summary: summary.into(),
                    }));
                }
            }
            _ => {}
        }
        if !self.kind.allows_model_tools() && self.turn.tool_used {
            return Err(fault(
                JobFailureKind::UpstreamRejected,
                method,
                "judge model tool item",
            ));
        }
        Ok(())
    }

    pub async fn restored_usage(
        &mut self,
        thread: &str,
        has_history: bool,
        warm_resume: Option<Value>,
    ) -> Result<(), ProviderError> {
        if !has_history {
            self.totals.entry(thread.into()).or_default();
            return Ok(());
        }
        // Codex 0.171 skips usage replay on cold resume of a paginated fork.
        // A second resume on the same connection replays the loaded thread's actual totals.
        if let Some(params) = warm_resume
            && !self
                .wait_for_usage(thread, Duration::from_millis(250))
                .await?
        {
            tracing::debug!("requesting restored usage from the loaded fork");
            let result =
                self.request("thread/resume", params)
                    .await
                    .map_err(|error| match error {
                        ProviderError::SessionUnavailable => fault(
                            JobFailureKind::WorkerLost,
                            "thread/resume",
                            "loaded fork unavailable",
                        ),
                        error => error,
                    })?;
            if result["thread"]["id"].as_str() != Some(thread) {
                return Err(fault(
                    JobFailureKind::WorkerLost,
                    "thread/resume",
                    "loaded fork id mismatch",
                ));
            }
        }
        if !self.wait_for_usage(thread, Duration::from_secs(15)).await? {
            return Err(fault(
                JobFailureKind::WorkerLost,
                "thread/tokenUsage/updated",
                "15 second restored usage timeout",
            ));
        }
        Ok(())
    }

    async fn wait_for_usage(
        &mut self,
        thread: &str,
        wait: Duration,
    ) -> Result<bool, ProviderError> {
        tokio::time::timeout(wait, async {
            while !self.totals.contains_key(thread) {
                self.next_notification().await?;
            }
            Ok(true)
        })
        .await
        .unwrap_or(Ok(false))
    }

    pub async fn secret(&mut self, path: &str, bytes: &[u8]) -> Result<(), ProviderError> {
        self.remove(path).await?;
        self.write(path, bytes).await
    }

    pub async fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), ProviderError> {
        use base64::Engine;
        self.request("fs/writeFile", json!({"path":path,"dataBase64":base64::engine::general_purpose::STANDARD.encode(bytes)})).await?;
        Ok(())
    }

    pub async fn mkdir(&mut self, path: &str) -> Result<(), ProviderError> {
        self.request("fs/createDirectory", json!({"path":path,"recursive":true}))
            .await?;
        Ok(())
    }

    pub async fn remove(&mut self, path: &str) -> Result<(), ProviderError> {
        self.request(
            "fs/remove",
            json!({"path":path,"force":true,"recursive":true}),
        )
        .await?;
        Ok(())
    }

    pub async fn command(&mut self, command: Value) -> Result<(), ProviderError> {
        let result = self.request("command/exec", command).await?;
        if result["exitCode"] != 0 {
            return Err(fault(
                JobFailureKind::CheckoutFailed,
                "command/exec",
                "nonzero exit code",
            ));
        }
        Ok(())
    }

    pub async fn read_output(&mut self, path: &str, limit: usize) -> Result<String, ProviderError> {
        use base64::Engine;
        let parent = path.rsplit_once('/').map(|x| x.0).ok_or_else(|| {
            fault(
                JobFailureKind::Configuration,
                "fs/readFile",
                "output path has no parent",
            )
        })?;
        for (path, file) in [(parent, false), (path, true)] {
            let metadata = self.request("fs/getMetadata", json!({"path":path})).await?;
            let type_key = if file { "isFile" } else { "isDirectory" };
            if metadata[type_key] != true || metadata["isSymlink"] != false {
                return Err(fault(
                    JobFailureKind::ProviderFailed,
                    "fs/getMetadata",
                    "output path type or symlink",
                ));
            }
        }
        let result = self.request("fs/readFile", json!({"path":path})).await?;
        let encoded = result["dataBase64"].as_str().ok_or_else(|| {
            fault(
                JobFailureKind::WorkerLost,
                "fs/readFile",
                "missing base64 data",
            )
        })?;
        if encoded.len() > limit.div_ceil(3) * 4 {
            return Err(fault(
                JobFailureKind::ProviderFailed,
                "fs/readFile",
                "encoded output exceeds cap",
            ));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| {
                fault(
                    JobFailureKind::WorkerLost,
                    "fs/readFile",
                    "invalid base64 output",
                )
            })?;
        if bytes.len() > limit {
            return Err(fault(
                JobFailureKind::ProviderFailed,
                "fs/readFile",
                "decoded output exceeds cap",
            ));
        }
        String::from_utf8(bytes).map_err(|_| {
            tracing::warn!(
                method = "fs/readFile",
                step = "decode text",
                "output is not UTF-8"
            );
            ProviderError::OutputNotJson
        })
    }
}

fn number(v: &Value, field: &str) -> Result<u64, ProviderError> {
    v[field].as_u64().ok_or_else(|| {
        fault(
            JobFailureKind::ProviderFailed,
            "thread/tokenUsage/updated",
            field,
        )
    })
}

fn usage(value: &Value) -> Result<Usage, ProviderError> {
    let result = Usage {
        input_tokens: number(value, "inputTokens")?,
        cached_input_tokens: number(value, "cachedInputTokens")?,
        output_tokens: number(value, "outputTokens")?,
        requests: None,
    };
    result.delta_from(Usage::default()).map_err(|_| {
        fault(
            JobFailureKind::ProviderFailed,
            "thread/tokenUsage/updated",
            "invalid usage total",
        )
    })
}

pub(super) fn failure(error: &Value) -> ProviderError {
    let message = error["message"].as_str().unwrap_or("");
    let embedded = message
        .find('{')
        .and_then(|start| serde_json::from_str::<Value>(&message[start..]).ok());
    let code = embedded
        .as_ref()
        .and_then(|v| v.pointer("/error/code").or_else(|| v.get("code")))
        .and_then(Value::as_str)
        .or_else(|| error.get("code").and_then(Value::as_str))
        .or_else(|| {
            [
                "invalid_json_schema",
                "credential_unlinked",
                "model_not_found",
                "model_unavailable",
                "upstream_rejected",
                "usage_limit_reached",
                "usage_not_included",
                "quota_exceeded",
            ]
            .into_iter()
            .find(|code| {
                message
                    .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                    .any(|word| word == *code)
            })
        })
        .unwrap_or(match error["codexErrorInfo"].as_str() {
            Some("usageLimitExceeded") => "usage_limit_reached",
            Some("modelNotFound") => "model_not_found",
            _ => "",
        });
    let kind = match code {
        "invalid_json_schema" => JobFailureKind::InvalidJsonSchema,
        "credential_unlinked" => JobFailureKind::CredentialRelink,
        "model_not_found" | "model_unavailable" => JobFailureKind::ModelUnavailable,
        "upstream_rejected" => JobFailureKind::UpstreamRejected,
        "usage_limit_reached" | "usage_not_included" | "quota_exceeded" => {
            let resets_at = embedded
                .as_ref()
                .and_then(|v| v.pointer("/error/resets_at").or_else(|| v.get("resets_at")))
                .and_then(Value::as_i64)
                .and_then(Timestamp::from_second);
            return ProviderError::UsageLimit { resets_at };
        }
        _ => JobFailureKind::ProviderFailed,
    };
    runtime(kind)
}
