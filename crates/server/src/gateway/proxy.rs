//! Model boundary, streaming SSE tap and response-ID accounting.

use super::{Error, Gateway, auth::Access};
use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use contracts::{ids::UserId, job::result::Usage};
use futures::StreamExt;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

pub(super) struct Models {
    etag: String,
    body: Bytes,
    headers: HeaderMap,
    generation: u64,
}

pub(super) fn validate_body(value: &Value) -> Result<(), Error> {
    if value.get("store") != Some(&Value::Bool(false))
        || value.get("stream") != Some(&Value::Bool(true))
        || value.get("previous_response_id").is_some()
    {
        return Err(Error::Body);
    }
    if let Some(tools) = value.get("tools") {
        validate_tools(tools, 0)?;
    }
    if let Some(input) = value.get("input") {
        for item in input.as_array().ok_or(Error::Body)? {
            if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                validate_tools(item.get("tools").ok_or(Error::Body)?, 0)?;
            }
        }
    }
    Ok(())
}

fn validate_tools(value: &Value, depth: usize) -> Result<(), Error> {
    if depth > 16 {
        return Err(Error::Body);
    }
    for tool in value.as_array().ok_or(Error::Body)? {
        match tool.get("type").and_then(Value::as_str) {
            Some("function" | "custom") => (),
            Some("namespace") => validate_tools(tool.get("tools").ok_or(Error::Body)?, depth + 1)?,
            kind => {
                let kind = kind
                    .filter(|s| {
                        s.len() <= 64 && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    })
                    .unwrap_or("unknown");
                tracing::warn!(
                    tool_type = kind,
                    "gateway refused hosted or unsupported model tool"
                );
                return Err(Error::Body);
            }
        }
    }
    Ok(())
}

fn connection_header(headers: &HeaderMap, name: &str) -> bool {
    headers
        .get_all("connection")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|v| v.trim().eq_ignore_ascii_case(name))
}
fn forward_request(name: &str) -> bool {
    matches!(
        name,
        "originator"
            | "user-agent"
            | "session-id"
            | "thread-id"
            | "x-client-request-id"
            | "x-openai-internal-codex-responses-lite"
            | "accept"
    ) || name.starts_with("x-codex-")
}
fn forward_response(name: &str) -> bool {
    matches!(name, "x-models-etag" | "x-oai-request-id" | "retry-after")
        || name.starts_with("x-codex-")
}
pub(super) fn response_headers(headers: &HeaderMap) -> HeaderMap {
    headers
        .iter()
        .filter(|(name, _)| {
            forward_response(name.as_str()) && !connection_header(headers, name.as_str())
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

impl Gateway {
    async fn send_model(
        &self,
        route: &str,
        query: Option<&str>,
        body: Option<&Value>,
        headers: &HeaderMap,
        access: &Access,
        etag: Option<&str>,
    ) -> Result<reqwest::Response, Error> {
        let mut url = reqwest::Url::parse(&format!("{}/{}", self.upstream.backend, route))
            .map_err(|_| Error::Configuration)?;
        if let Some(version) = query {
            url.query_pairs_mut().append_pair("client_version", version);
        }
        let mut request = if body.is_some() {
            self.upstream.model_client.post(url)
        } else {
            self.upstream.model_client.get(url)
        };
        for (name, value) in headers
            .iter()
            .filter(|(n, _)| forward_request(n.as_str()) && !connection_header(headers, n.as_str()))
        {
            request = request.header(name, value);
        }
        request = request
            .bearer_auth(&access.bundle.access)
            .header("chatgpt-account-id", &access.bundle.account)
            .header("accept-encoding", "identity");
        if access.bundle.fedramp {
            request = request.header("X-OpenAI-Fedramp", "true");
        }
        if let Some(etag) = etag {
            request = request.header("if-none-match", etag);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        Ok(request.send().await?)
    }

    pub(super) async fn proxy(
        self: &Arc<Self>,
        headers: HeaderMap,
        query: Option<String>,
        body: Option<Value>,
    ) -> Result<Response, Error> {
        let bearer = super::http::bearer(&headers).ok_or(Error::Key)?;
        let (user, key) = self.authenticate(bearer).await?;
        if let Some(body) = &body {
            validate_body(body)?;
        }
        let _permit = self
            .streams
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Configuration)?;
        let access = self.access(&user, None).await?;
        let models = body.is_none();
        let etag = if models {
            self.models
                .lock()
                .await
                .get(&user)
                .filter(|m| m.generation == access.generation)
                .map(|m| m.etag.clone())
        } else {
            None
        };
        let route = if models { "models" } else { "responses" };
        let mut response = self
            .send_model(
                route,
                query.as_deref(),
                body.as_ref(),
                &headers,
                &access,
                etag.as_deref(),
            )
            .await?;
        let generation = if response.status() == StatusCode::UNAUTHORIZED {
            let access = self.access(&user, Some(access.generation)).await?;
            response = self
                .send_model(
                    route,
                    query.as_deref(),
                    body.as_ref(),
                    &headers,
                    &access,
                    None,
                )
                .await?;
            if response.status() == StatusCode::UNAUTHORIZED {
                let mut tx = self.store.lock(&user).await?;
                // A rejection of an older generation must not poison a newer link/refresh.
                if tx.record.generation == access.generation {
                    tx.record.reason = Some(contracts::gateway::RelinkReason::Terminal);
                    tx.save()?;
                }
                return Err(Error::Unlinked);
            }
            access.generation
        } else {
            access.generation
        };
        let status = response.status();
        let output_headers = response_headers(response.headers());
        self.capture_limits(&user, response.headers()).await?;
        if models {
            if status == StatusCode::NOT_MODIFIED {
                let cache = self.models.lock().await;
                if let Some(model) = cache.get(&user).filter(|m| m.generation == generation) {
                    let mut headers = model.headers.clone();
                    headers.extend(output_headers);
                    return Ok((StatusCode::OK, headers, model.body.clone()).into_response());
                }
                return Err(Error::Body);
            }
            let bytes = bounded_bytes(response, 4 * 1024 * 1024).await?;
            if status.is_success()
                && let Some(etag) = output_headers
                    .get("x-models-etag")
                    .and_then(|v| v.to_str().ok())
            {
                self.models.lock().await.insert(
                    user,
                    Models {
                        etag: etag.into(),
                        body: bytes.clone(),
                        headers: output_headers.clone(),
                        generation,
                    },
                );
            }
            return Ok((status, output_headers, bytes).into_response());
        }
        if !status.is_success() {
            {
                let mut tx = self.store.lock(&user).await?;
                tx.record.last_call = Some(contracts::job::result::LlmCallOutcome {
                    at: contracts::time::Timestamp::now(),
                    status: contracts::job::result::LlmCallStatus::NotWorking {
                        message: format!("Model request returned HTTP {}", status.as_u16()),
                    },
                });
                tx.save()?;
            }
            let bytes = bounded_bytes(response, 4 * 1024 * 1024).await?;
            if status == StatusCode::TOO_MANY_REQUESTS {
                let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                let mut tx = self.store.lock(&user).await?;
                tx.record.resets_at = Some(
                    value
                        .pointer("/error/resets_at")
                        .and_then(Value::as_i64)
                        .unwrap_or(super::now() + 1800),
                );
                tx.save()?;
            }
            return Ok((status, output_headers, bytes).into_response());
        }
        let store = self.store.clone();
        let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);
        let mut tasks = self.tasks.lock().await;
        while tasks.try_join_next().is_some() {}
        tasks.spawn(async move {
            let _permit = _permit;
            let mut stream = response.bytes_stream();
            let mut tap = Tap::default();
            let mut sender = Some(sender);
            while let Some(chunk) = stream.next().await {
                let bytes = match chunk {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        if let Some(sender) = sender.take() {
                            let _ = sender
                                .send(Err(std::io::Error::other("model stream interrupted")))
                                .await;
                        }
                        break;
                    }
                };
                let events = match tap.push(&bytes) {
                    Ok(events) => events,
                    Err(_) => {
                        if let Some(sender) = sender.take() {
                            let _ = sender
                                .send(Err(std::io::Error::other(
                                    "model event exceeds gateway bounds",
                                )))
                                .await;
                        }
                        break;
                    }
                };
                for event in events {
                    let completed = completion(&event);
                    if completed.is_err() {
                        if let Some(sender) = sender.take() {
                            let _ = sender
                                .send(Err(std::io::Error::other("model usage event is malformed")))
                                .await;
                        }
                        return;
                    }
                    if event.get("type").and_then(Value::as_str) == Some("response.failed")
                        && let Ok(mut tx) = store.lock(&user).await
                    {
                        tx.record.last_call = Some(contracts::job::result::LlmCallOutcome {
                            at: contracts::time::Timestamp::now(),
                            status: contracts::job::result::LlmCallStatus::NotWorking {
                                message: "Model request failed".into(),
                            },
                        });
                        let code = event
                            .pointer("/response/error/code")
                            .and_then(Value::as_str);
                        if matches!(
                            code,
                            Some(
                                "usage_limit_reached"
                                    | "insufficient_quota"
                                    | "usage_not_included"
                                    | "credit_balance_exhausted"
                                    | "spend_limit_exceeded"
                            )
                        ) {
                            tx.record.resets_at = Some(
                                event
                                    .pointer("/response/error/resets_at")
                                    .and_then(Value::as_i64)
                                    .unwrap_or(super::now() + 1800),
                            );
                        }
                        if tx.save().is_err() {
                            return;
                        }
                    }
                    if let Ok(Some((id, usage))) = completed
                        && store.meter(&user, &key, &id, usage).await.is_err()
                    {
                        if let Some(sender) = sender.take() {
                            let _ = sender
                                .send(Err(std::io::Error::other("model usage persistence failed")))
                                .await;
                        }
                        return;
                    }
                }
                // If the worker disconnects, drain the upstream to capture its final response ID.
                if let Some(channel) = sender.take() {
                    let live = store.lock(&user).await.is_ok_and(|tx| {
                        tx.record
                            .keys
                            .get(&key)
                            .is_some_and(|k| !k.revoked && k.expires_at > super::now())
                    });
                    if !live {
                        let _ = channel
                            .send(Err(std::io::Error::other("virtual key retired")))
                            .await;
                    } else if channel.send(Ok(bytes)).await.is_ok() {
                        sender = Some(channel);
                    }
                }
            }
        });
        let stream = futures::stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|item| (item, receiver))
        });
        let mut response = (status, output_headers, Body::from_stream(stream)).into_response();
        response.headers_mut().insert(
            "content-type",
            HeaderValue::from_static("text/event-stream"),
        );
        Ok(response)
    }

    async fn capture_limits(&self, user: &UserId, headers: &HeaderMap) -> Result<(), Error> {
        let limits: BTreeMap<String, String> = headers
            .iter()
            .filter(|(n, _)| {
                n.as_str().starts_with("x-codex-primary-")
                    || n.as_str().starts_with("x-codex-secondary-")
            })
            .filter_map(|(n, v)| v.to_str().ok().map(|v| (n.to_string(), v.to_owned())))
            .collect();
        if !limits.is_empty() {
            let mut tx = self.store.lock(user).await?;
            tx.record.limits = limits;
            tx.save()?;
        }
        Ok(())
    }
}

async fn bounded_bytes(response: reqwest::Response, bound: usize) -> Result<Bytes, Error> {
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len().saturating_add(chunk.len()) > bound {
            return Err(Error::Body);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(bytes))
}

#[derive(Default)]
pub(super) struct Tap {
    line: Vec<u8>,
    data: Vec<u8>,
}
impl Tap {
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<Vec<Value>, Error> {
        let mut events = Vec::new();
        for byte in bytes {
            if *byte != b'\n' {
                self.line.push(*byte);
            } else {
                if self.line.last() == Some(&b'\r') {
                    self.line.pop();
                }
                if self.line.is_empty() {
                    if !self.data.is_empty() {
                        if let Ok(value) = serde_json::from_slice(&self.data) {
                            events.push(value);
                        }
                        self.data.clear();
                    }
                } else if let Some(data) = self.line.strip_prefix(b"data:") {
                    if !self.data.is_empty() {
                        self.data.push(b'\n');
                    }
                    self.data
                        .extend_from_slice(data.strip_prefix(b" ").unwrap_or(data));
                }
                self.line.clear();
            }
            if self.line.len().saturating_add(self.data.len()) > 4 * 1024 * 1024 {
                return Err(Error::Body);
            }
        }
        Ok(events)
    }
}

pub(super) fn completion(event: &Value) -> Result<Option<(String, Usage)>, Error> {
    if event.get("type").and_then(Value::as_str) != Some("response.completed") {
        return Ok(None);
    }
    let response = event.get("response").ok_or(Error::Usage)?;
    let id = response
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or(Error::Usage)?;
    let usage = response.get("usage").ok_or(Error::Usage)?;
    let usage = Usage {
        input_tokens: usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .ok_or(Error::Usage)?,
        cached_input_tokens: match usage.pointer("/input_tokens_details/cached_tokens") {
            None => 0,
            Some(value) => value.as_u64().ok_or(Error::Usage)?,
        },
        output_tokens: usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .ok_or(Error::Usage)?,
        requests: Some(1),
    };
    usage
        .checked_add(Usage::default())
        .map_err(|_| Error::Usage)?;
    Ok(Some((id.into(), usage)))
}
