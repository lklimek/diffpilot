//! Bounded body draining for webhook requests sent to the public listener.

use std::time::Duration;

use axum::Router;
use axum::extract::Request;
use axum::http::header::{CONNECTION, CONTENT_LENGTH};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use futures::StreamExt;
use tokio::time::timeout;

use crate::state::AppState;

const MAX_BYTES: usize = 4 * 1024 * 1024;
const DRAIN_TIMEOUT: Duration = Duration::from_secs(8);

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/webhooks", any(not_found))
        .route("/webhooks/", any(not_found))
        .route("/webhooks/{*path}", any(not_found))
}

async fn not_found(request: Request) -> Response {
    let oversized = request
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > MAX_BYTES as u64);
    if oversized {
        return close(StatusCode::PAYLOAD_TOO_LARGE);
    }

    // Drop each transport chunk as it arrives, with no body buffer of our own.
    let drain = async {
        let mut body = request.into_body().into_data_stream();
        let mut remaining = MAX_BYTES;
        while let Some(frame) = body.next().await {
            let Ok(chunk) = frame else {
                return false;
            };
            let Some(left) = remaining.checked_sub(chunk.len()) else {
                return false;
            };
            remaining = left;
        }
        true
    };

    if matches!(timeout(DRAIN_TIMEOUT, drain).await, Ok(true)) {
        StatusCode::NOT_FOUND.into_response()
    } else {
        close(StatusCode::NOT_FOUND)
    }
}

fn close(status: StatusCode) -> Response {
    (status, [(CONNECTION, HeaderValue::from_static("close"))]).into_response()
}
