//! Delays early responses until the transport has consumed the request or reached its budget.

use std::future::poll_fn;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http_body::{Body as _, Frame};
use tokio::time::{Instant, timeout_at};

pub(super) const MAX_BYTES: usize = 4 * 1024 * 1024;
pub(super) const BODY_TIMEOUT: Duration = Duration::from_secs(8);

fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

/// Runs the handler and drains the rest of the request within the shared webhook budget.
pub(crate) async fn drain_unread_body(request: Request, next: Next) -> Response {
    if content_length(request.headers()).is_some_and(|len| len > MAX_BYTES as u64) {
        let mut response = StatusCode::PAYLOAD_TOO_LARGE.into_response();
        close(&mut response);
        return response;
    }

    if request.body().is_end_stream() {
        return next.run(request).await;
    }

    let deadline = Instant::now() + BODY_TIMEOUT;
    let (parts, body) = request.into_parts();
    let body = SharedBody(Arc::new(Mutex::new(Transport {
        body,
        remaining: Some(MAX_BYTES),
    })));
    let mut response = next
        .run(Request::from_parts(parts, Body::new(body.clone())))
        .await;
    let expired = Instant::now() >= deadline && !body.is_end_stream();
    if expired || !matches!(timeout_at(deadline, body.drain()).await, Ok(true)) {
        close(&mut response);
    }
    response
}

fn close(response: &mut Response) {
    response
        .headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("close"));
}

struct Transport {
    body: Body,
    remaining: Option<usize>,
}

// The receiver and drain poll the same body in sequence. Neither buffers discarded frames.
#[derive(Clone)]
struct SharedBody(Arc<Mutex<Transport>>);

impl SharedBody {
    async fn drain(mut self) -> bool {
        while !self.is_end_stream() {
            match poll_fn(|cx| Pin::new(&mut self).poll_frame(cx)).await {
                Some(Ok(_)) => {}
                None => break,
                Some(Err(_)) => return false,
            }
        }
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remaining
            .is_some()
    }
}

#[derive(Debug, thiserror::Error)]
enum ReadError {
    #[error("the webhook body could not be read")]
    Transport(#[source] axum::Error),
    #[error("the webhook body exceeded its size limit")]
    Limit,
}

impl http_body::Body for SharedBody {
    type Data = Bytes;
    type Error = ReadError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, ReadError>>> {
        let mut transport = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(remaining) = transport.remaining else {
            return Poll::Ready(Some(Err(ReadError::Limit)));
        };
        let next = std::task::ready!(Pin::new(&mut transport.body).poll_frame(cx));
        let Some(next) = next else {
            return Poll::Ready(None);
        };
        let frame = match next {
            Ok(frame) => frame,
            Err(error) => {
                transport.remaining = None;
                return Poll::Ready(Some(Err(ReadError::Transport(error))));
            }
        };
        transport.remaining = remaining.checked_sub(frame.data_ref().map_or(0, Bytes::len));
        // Let the route's extractor handle this frame's limit rejection.
        Poll::Ready(Some(Ok(frame)))
    }

    fn is_end_stream(&self) -> bool {
        let transport = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        transport.remaining.is_none() || transport.body.is_end_stream()
    }
}
