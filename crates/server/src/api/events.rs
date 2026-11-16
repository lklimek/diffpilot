//! Server-sent events of an analysis (`docs/contracts/sse-events.md`): subscribe, snapshot, then live events.

use std::borrow::Cow;
use std::convert::Infallible;
use std::time::Duration;

use axum::Router;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use contracts::api::error::ApiErrorCode;
use contracts::api::events::AnalysisEvent;
use contracts::api::questions::AskQuestion;
use contracts::ids::AnalysisId;
use contracts::routes::ANALYSIS_EVENTS;
use db::error::DbError;
use futures::stream::{self, Stream, StreamExt};
use tokio::sync::broadcast;

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::Session;
use crate::state::AppState;

/// Keep-alive comment interval.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// The event stream could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum EventsError {
    /// The analysis id in the path is not valid.
    #[error("the analysis id is not valid")]
    Path(#[from] PathRejection),
    /// Reading the snapshot failed.
    #[error("the analysis could not be read")]
    Db(#[from] DbError),
}

impl ApiErrorSource for EventsError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Path(rejection) => rejection.code(),
            Self::Db(error) => error.code(),
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Path(rejection) => rejection.public_message(),
            Self::Db(error) => error.public_message(),
        }
    }
}

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new().route(ANALYSIS_EVENTS.path, get(analysis_events))
}

async fn analysis_events(
    State(state): State<AppState>,
    session: Session,
    path: Result<Path<AnalysisId>, PathRejection>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiFailure<EventsError>> {
    let Path(analysis) = path.map_err(EventsError::from)?;
    // Another tenant's analysis answers like an unknown one (no existence leak).
    let record = state
        .db
        .analyses()
        .get(analysis)
        .await
        .map_err(EventsError::from)?;
    if record.tenant_id != session.tenant_id {
        return Err(EventsError::Db(DbError::NotFound).into());
    }
    // Subscribe first: an update written after the snapshot read is then buffered, never lost.
    let live = state.progress.subscribe(analysis);
    let summary = state
        .db
        .analyses()
        .summary(analysis)
        .await
        .map_err(EventsError::from)?;
    let mut snapshot = vec![AnalysisEvent::Analysis { summary }];
    snapshot.extend(
        question_snapshot(&state, analysis)
            .await
            .map_err(EventsError::from)?
            .into_iter()
            .map(|question| AnalysisEvent::Question { question }),
    );
    let events = stream::iter(snapshot).chain(live_events(live));
    Ok(Sse::new(
        events
            .filter_map(|event| async move { sse_event(&event) })
            .map(Ok),
    )
    .keep_alive(KeepAlive::new().interval(KEEP_ALIVE)))
}

/// Every question of the analysis in any state, for the snapshot.
// TODO(WP3-ASK): read all questions of the analysis (every state) from the questions repository.
async fn question_snapshot(
    _state: &AppState,
    _analysis: AnalysisId,
) -> Result<Vec<AskQuestion>, DbError> {
    Ok(Vec::new())
}

/// Live events in channel order; a subscriber that fell behind loses its stream and reconnects for a snapshot.
fn live_events(receiver: broadcast::Receiver<AnalysisEvent>) -> impl Stream<Item = AnalysisEvent> {
    stream::unfold(receiver, |mut receiver| async move {
        match receiver.recv().await {
            Ok(event) => Some((event, receiver)),
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::debug!(
                    skipped,
                    "SSE subscriber fell behind; closing so the browser reconnects"
                );
                None
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    })
}

fn sse_event(event: &AnalysisEvent) -> Option<Event> {
    match Event::default().event(event.name()).json_data(event) {
        Ok(sse) => Some(sse),
        Err(error) => {
            tracing::error!(%error, "could not encode an analysis event for SSE; the event was skipped");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use chrono::Utc;
    use contracts::api::analyses::AnalysisState;
    use db::repo::DEFAULT_TENANT;
    use db::repo::analyses::testing::{seed_analysis, seed_tenant};
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::api::router;
    use crate::state::tests::test_state;
    use crate::test_support::session_cookie;

    const WAIT: Duration = Duration::from_secs(10);

    fn events_request(analysis: AnalysisId, cookie: Option<&str>) -> Request<Body> {
        let mut request = Request::get(ANALYSIS_EVENTS.path.replace("{id}", &analysis.to_string()));
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        request.body(Body::empty()).expect("request")
    }

    /// Reads SSE frames until `count` events arrived; returns (event name, data) pairs.
    async fn read_events(body: Body, count: usize) -> Vec<(String, Value)> {
        let mut body = body.into_data_stream();
        let mut text = String::new();
        let mut events = Vec::new();
        while events.len() < count {
            let chunk = tokio::time::timeout(WAIT, body.next())
                .await
                .expect("an SSE frame in time")
                .expect("stream open")
                .expect("frame");
            text.push_str(std::str::from_utf8(&chunk).expect("utf-8"));
            while let Some(end) = text.find("\n\n") {
                let frame: String = text.drain(..end + 2).collect();
                let mut name = None;
                let mut data = None;
                for line in frame.lines() {
                    if let Some(value) = line.strip_prefix("event: ") {
                        name = Some(value.to_owned());
                    } else if let Some(value) = line.strip_prefix("data: ") {
                        data = Some(serde_json::from_str(value).expect("JSON data"));
                    }
                }
                if let (Some(name), Some(data)) = (name, data) {
                    events.push((name, data));
                }
            }
        }
        events
    }

    #[tokio::test]
    async fn sse_emits_snapshot_then_progress_for_a_scripted_sequence() {
        let state = test_state().await;
        let analysis = seed_analysis(&state.db).await.expect("seed");
        let cookie = session_cookie(&state, DEFAULT_TENANT).await;
        let request = events_request(analysis, Some(&cookie));
        let response = router(state.clone())
            .oneshot(request)
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            "text/event-stream",
            "SSE content type"
        );
        let script = [
            AnalysisState::Running { done: 0, total: 2 },
            AnalysisState::Running { done: 1, total: 2 },
            AnalysisState::Finished { total: 2 },
        ];
        let writer = {
            let state = state.clone();
            let script = script.clone();
            tokio::spawn(async move {
                for step in script {
                    state
                        .progress
                        .set_analysis_state(&state.db, analysis, &step, Utc::now())
                        .await
                        .expect("update");
                }
            })
        };
        let events = read_events(response.into_body(), 1 + script.len()).await;
        writer.await.expect("writer");
        let states: Vec<Value> = events
            .iter()
            .map(|(name, data)| {
                assert_eq!(name, "analysis");
                assert_eq!(data["event"], "analysis");
                data["summary"]["state"].clone()
            })
            .collect();
        assert_eq!(states[0]["state"], "planning", "snapshot first");
        let expected: Vec<Value> = script
            .iter()
            .map(|step| serde_json::to_value(step).expect("json"))
            .collect();
        assert_eq!(states[1..], expected[..]);
        assert_eq!(events[0].1["summary"]["pr"]["number"], 482);
    }

    #[tokio::test]
    async fn unknown_or_other_tenants_analysis_is_not_found() {
        let state = test_state().await;
        let analysis = seed_analysis(&state.db).await.expect("seed");
        seed_tenant(&state.db, "other").await.expect("tenant");
        let own = session_cookie(&state, DEFAULT_TENANT).await;
        let foreign = session_cookie(&state, "other").await;
        for (id, cookie) in [(AnalysisId::new_random(), &own), (analysis, &foreign)] {
            let response = router(state.clone())
                .oneshot(events_request(id, Some(cookie)))
                .await
                .expect("infallible");
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{cookie}");
        }
    }

    #[tokio::test]
    async fn sse_requires_session() {
        let state = test_state().await;
        let analysis = seed_analysis(&state.db).await.expect("seed");
        for cookie in [None, Some("__Host-sr_session=not-a-session")] {
            let response = router(state.clone())
                .oneshot(events_request(analysis, cookie))
                .await
                .expect("infallible");
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{cookie:?}");
        }
    }
}
