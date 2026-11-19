//! MCP client of MemCan over streamable HTTP with the configured bearer token. Every call is time-bounded, nothing
//! is retried here, and no error carries the token or the MemCan address (upstream text is scrubbed and cut before
//! it is kept).

use std::sync::Arc;
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResponse};
use rmcp::service::{RoleClient, RunningService, ServiceError};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::common::client_side_sse::NeverRetry;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use secrecy::ExposeSecret;
use serde_json::Value;

use crate::config::MemcanConfig;

/// Longest wait for connecting and the MCP handshake.
const CONNECT_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_secs(3)
} else {
    Duration::from_secs(5)
};
/// Longest wait for one tool call (MemCan embeds the text before it answers).
pub(crate) const CALL_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_secs(3)
} else {
    Duration::from_secs(20)
};
/// Largest tool answer accepted (all text blocks together).
pub(crate) const MAX_ANSWER_BYTES: usize = 256 * 1024;
/// Largest server-sent event accepted by the transport.
const MAX_SSE_EVENT_BYTES: usize = 512 * 1024;
/// Upstream error text kept for diagnosis, in characters.
const MAX_DETAIL_CHARS: usize = 300;

/// A MemCan call failed. Neither `Display` nor `Debug` contains the bearer token.
#[derive(Debug, thiserror::Error)]
pub enum MemcanError {
    /// No connection or no MCP session (network, TLS, authentication or protocol failure).
    #[error("MemCan could not be reached; check SR_MEMCAN_URL and SR_MEMCAN_TOKEN")]
    Unreachable {
        /// Scrubbed, shortened upstream error.
        detail: String,
    },
    /// MemCan did not answer within the time limit.
    #[error("MemCan did not answer in time")]
    Timeout,
    /// MemCan answered the call with an error.
    #[error("MemCan rejected the {tool} call")]
    Rejected {
        /// The tool.
        tool: &'static str,
        /// JSON-RPC error code; `None` for a tool result marked as error.
        code: Option<i32>,
        /// Scrubbed, shortened error text.
        detail: String,
    },
    /// MemCan answered with something other than the expected text.
    #[error("MemCan answered the {tool} call with an unexpected response")]
    InvalidResponse {
        /// The tool.
        tool: &'static str,
    },
}

impl MemcanError {
    /// Whether MemCan itself is unavailable, so further calls now would fail as well.
    pub(crate) fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unreachable { .. } | Self::Timeout)
    }
}

/// A connected MCP session; dropped to close.
pub(crate) struct MemcanClient<'a> {
    service: RunningService<RoleClient, ()>,
    config: &'a MemcanConfig,
}

impl<'a> MemcanClient<'a> {
    /// Connects and completes the MCP handshake.
    pub(crate) async fn connect(config: &'a MemcanConfig) -> Result<Self, MemcanError> {
        let result = Self::connect_inner(config).await;
        if let Err(error) = &result {
            config.observe::<()>(&Err(error));
        }
        result
    }

    async fn connect_inner(config: &'a MemcanConfig) -> Result<Self, MemcanError> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(CALL_TIMEOUT)
            .build()
            .map_err(|error| unreachable_error(config, &error))?;
        let mut transport = StreamableHttpClientTransportConfig::with_uri(config.url.as_str())
            .auth_header(config.token.expose_secret())
            .max_concurrent_requests(1);
        transport.retry_config = Arc::new(NeverRetry::default());
        transport.max_sse_event_size = MAX_SSE_EVENT_BYTES;
        let transport = StreamableHttpClientTransport::with_client(http, transport);
        let service = tokio::time::timeout(CONNECT_TIMEOUT, ().serve(transport))
            .await
            .map_err(|_| MemcanError::Timeout)?
            .map_err(|error| unreachable_error(config, &error))?;
        Ok(Self { service, config })
    }

    /// Calls `tool` and returns its text answer; an error result is [`MemcanError::Rejected`].
    pub(crate) async fn call(
        &self,
        tool: &'static str,
        arguments: Value,
    ) -> Result<String, MemcanError> {
        let result = self.call_inner(tool, arguments).await;
        self.config.observe(&result.as_ref());
        result
    }

    async fn call_inner(
        &self,
        tool: &'static str,
        arguments: Value,
    ) -> Result<String, MemcanError> {
        let Value::Object(arguments) = arguments else {
            return Err(MemcanError::InvalidResponse { tool });
        };
        let params = CallToolRequestParams::new(tool).with_arguments(arguments);
        let response = tokio::time::timeout(CALL_TIMEOUT, self.service.call_tool_once(params))
            .await
            .map_err(|_| MemcanError::Timeout)?;
        let result = match response {
            Ok(CallToolResponse::Complete(result)) => result,
            Ok(_) => return Err(MemcanError::InvalidResponse { tool }),
            Err(ServiceError::McpError(error)) => {
                return Err(MemcanError::Rejected {
                    tool,
                    code: Some(error.code.0),
                    detail: scrub(self.config, &error.message),
                });
            }
            Err(ServiceError::Timeout { .. }) => return Err(MemcanError::Timeout),
            Err(error) => return Err(unreachable_error(self.config, &error)),
        };
        let mut text = String::new();
        for block in &result.content {
            let Some(block) = block.as_text() else {
                return Err(MemcanError::InvalidResponse { tool });
            };
            if text.len() + block.text.len() > MAX_ANSWER_BYTES {
                return Err(MemcanError::InvalidResponse { tool });
            }
            text.push_str(&block.text);
        }
        if result.is_error == Some(true) {
            return Err(MemcanError::Rejected {
                tool,
                code: None,
                detail: scrub(self.config, &text),
            });
        }
        Ok(text)
    }
}

fn unreachable_error(config: &MemcanConfig, error: &dyn std::error::Error) -> MemcanError {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    MemcanError::Unreachable {
        detail: scrub(config, &text),
    }
}

/// Removes the bearer token, the MemCan address and any URL, then cuts the text to [`MAX_DETAIL_CHARS`].
fn scrub(config: &MemcanConfig, text: &str) -> String {
    let url = &config.url;
    let host = url.host_str().unwrap_or_default();
    let authority = url
        .port_or_known_default()
        .map(|port| format!("{host}:{port}"))
        .unwrap_or_default();
    let mut text = text.to_owned();
    for secret in [
        config.token.expose_secret(),
        url.as_str(),
        url.as_str().trim_end_matches('/'),
        &authority,
        host,
    ] {
        if !secret.is_empty() {
            text = text.replace(secret, "[redacted]");
        }
    }
    strip_urls(&text).chars().take(MAX_DETAIL_CHARS).collect()
}

/// Replaces every `http://` or `https://` URL (up to whitespace, a quote or a closing bracket) with `[url]`.
fn strip_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = ["http://", "https://"]
        .iter()
        .filter_map(|scheme| rest.find(scheme))
        .min()
    {
        out.push_str(&rest[..start]);
        out.push_str("[url]");
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | ']' | '>' | '"' | '\''))
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use secrecy::SecretString;

    use super::*;

    #[test]
    fn scrub_removes_token_address_and_urls() {
        let config = MemcanConfig {
            observed: Default::default(),
            url: "http://memcan.lan:8190/mcp-url-CANARY"
                .parse()
                .expect("url"),
            token: SecretString::from("tok-CANARY"),
        };
        let text = scrub(
            &config,
            "error sending request for url (http://memcan.lan:8190/mcp-url-CANARY): Bearer tok-CANARY; \
             memcan.lan:8190 refused; see https://example.com/x?y=1 and memcan.lan",
        );
        for leak in ["CANARY", "memcan.lan", "8190", "example.com"] {
            assert!(!text.contains(leak), "{leak} in {text}");
        }
        assert!(text.contains("refused"), "{text}");
    }
}
