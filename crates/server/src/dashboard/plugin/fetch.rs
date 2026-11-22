//! The HTTP read every plug-in shares (ADR-022 §4): one unauthenticated JSON `GET` with `If-None-Match`, no
//! redirects, a timeout and a size bound.

use std::time::Duration;

use reqwest::header::{ACCEPT, ETAG, IF_NONE_MATCH};
use reqwest::{Client, StatusCode, redirect};
use serde::de::DeserializeOwned;
use url::Url;

use super::PluginError;

/// Bound of one request.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest answer read.
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// One request's answer.
pub(super) enum Answer<T> {
    /// 304 to the validator sent.
    NotModified,
    /// The body and its validator, if the source sent one.
    Body { etag: Option<String>, body: T },
}

/// The client of a plug-in: no redirects, [`HTTP_TIMEOUT`], no credentials.
pub(super) fn client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .redirect(redirect::Policy::none())
        .timeout(HTTP_TIMEOUT)
        .user_agent("diffpilot")
        .build()
}

/// Reads `url` as JSON, sending `etag` as `If-None-Match`.
pub(super) async fn get_json<T: DeserializeOwned>(
    http: &Client,
    url: Url,
    etag: Option<&str>,
) -> Result<Answer<T>, PluginError> {
    let mut request = http.get(url).header(ACCEPT, "application/json");
    if let Some(etag) = etag {
        request = request.header(IF_NONE_MATCH, etag);
    }
    let mut response = request.send().await.map_err(PluginError::Http)?;
    let status = response.status();
    if status == StatusCode::NOT_MODIFIED {
        return if etag.is_some() {
            Ok(Answer::NotModified)
        } else {
            Err(PluginError::UnexpectedNotModified)
        };
    }
    if !status.is_success() {
        return Err(PluginError::Status(status.as_u16()));
    }
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(PluginError::Http)? {
        if bytes.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(PluginError::TooLarge {
                limit: MAX_BODY_BYTES,
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    let body = serde_json::from_slice(&bytes).map_err(PluginError::Malformed)?;
    Ok(Answer::Body { etag, body })
}
