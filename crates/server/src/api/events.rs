//! Server-sent events of an analysis (`docs/contracts/sse-events.md`): subscribe, snapshot, then live events.

use std::borrow::Cow;
use std::convert::Infallible;
use std::time::Duration;

use axum::Router;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::response::sse::{Event, Sse};
use axum::routing::get;
use chrono::Utc;
use contracts::api::error::ApiErrorCode;
use contracts::api::events::AnalysisEvent;
use contracts::api::questions::AskQuestion;
use contracts::ids::AnalysisId;
use contracts::routes::ANALYSIS_EVENTS;
use db::error::DbError;
use futures::stream::{self, Stream, StreamExt};
use tokio::sync::broadcast;
use tokio::time::MissedTickBehavior;

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::Session;
use crate::state::AppState;

/// Interval of the `heartbeat` event, which also keeps proxies from closing an idle stream.
const HEARTBEAT: Duration = Duration::from_secs(15);

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
    let events = analysis_stream(snapshot, live);
    Ok(Sse::new(
        events
            .filter_map(|event| async move { sse_event(&event) })
            .map(Ok),
    ))
}

/// Every question of the analysis in any state, for the snapshot.
async fn question_snapshot(
    state: &AppState,
    analysis: AnalysisId,
) -> Result<Vec<AskQuestion>, DbError> {
    state.db.questions().list(analysis, Utc::now()).await
}

/// The snapshot, then live events and a [`AnalysisEvent::Heartbeat`] every [`HEARTBEAT`] (no database read); ends
/// when the live events end.
fn analysis_stream(
    snapshot: Vec<AnalysisEvent>,
    live: broadcast::Receiver<AnalysisEvent>,
) -> impl Stream<Item = AnalysisEvent> {
    let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let heartbeats = stream::unfold(ticks, |mut ticks| async move {
        ticks.tick().await;
        Some((Some(AnalysisEvent::Heartbeat { at: Utc::now() }), ticks))
    });
    // `None` marks the end of the live events, which ends the stream (heartbeats never end).
    let live = live_events(live)
        .map(Some)
        .chain(stream::once(async { None }));
    stream::iter(snapshot).chain(
        stream::select(live, heartbeats)
            .take_while(|event| std::future::ready(event.is_some()))
            .filter_map(std::future::ready),
    )
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
    use chrono::{TimeZone, Utc};
    use contracts::agent::JobKind;
    use contracts::api::analyses::AnalysisState;
    use contracts::job::spec::JobPriority;
    use db::repo::DEFAULT_TENANT;
    use db::repo::analyses::testing::{CREDENTIAL, job_spec, seed_analysis, seed_tenant};
    use db::repo::jobs::{ClaimParams, JobStage, NewJob};
    use serde_json::{Value, json};
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
            AnalysisState::Running {
                done: 0,
                total: 2,
                stage: None,
            },
            AnalysisState::Running {
                done: 1,
                total: 2,
                stage: None,
            },
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

    /// A reconnect reads the stage from the database: `started_at` is the claim time on every snapshot.
    #[tokio::test]
    async fn snapshot_carries_the_stage_started_at_the_claim() {
        let state = test_state().await;
        let analysis = seed_analysis(&state.db).await.expect("seed");
        let claimed_at = Utc
            .with_ymd_and_hms(2026, 11, 17, 9, 30, 0)
            .single()
            .expect("time");
        state
            .db
            .jobs()
            .enqueue(
                NewJob {
                    spec: job_spec(analysis, JobKind::Plan),
                    priority: JobPriority::Batch,
                    stage: JobStage::Plan { batch: 1 },
                    generation: 0,
                },
                claimed_at,
            )
            .await
            .expect("enqueue");
        let cookie = session_cookie(&state, DEFAULT_TENANT).await;
        let stage_on_connect = || async {
            let response = router(state.clone())
                .oneshot(events_request(analysis, Some(&cookie)))
                .await
                .expect("infallible");
            let events = read_events(response.into_body(), 1).await;
            events[0].1["summary"]["state"]["stage"].clone()
        };
        assert_eq!(
            stage_on_connect().await,
            json!({"kind": "plan", "started_at": null}),
            "queued: waiting for the runner"
        );
        state
            .db
            .jobs()
            .claim(ClaimParams {
                credential_ids: &[CREDENTIAL.to_owned()],
                job_token_hash: b"hash",
                lease_until: claimed_at + chrono::TimeDelta::hours(1),
                now: claimed_at,
            })
            .await
            .expect("claim")
            .expect("claimed");
        for connect in ["first", "reconnect"] {
            assert_eq!(
                stage_on_connect().await,
                json!({"kind": "plan", "started_at": claimed_at}),
                "{connect}"
            );
        }
    }

    /// A `heartbeat` every 15 s while the stream is open; the stream ends with the live channel.
    #[tokio::test(start_paused = true)]
    async fn heartbeat_every_15_s_until_the_live_channel_ends() {
        let (sender, receiver) = broadcast::channel(4);
        let start = tokio::time::Instant::now();
        let mut events = Box::pin(analysis_stream(Vec::new(), receiver));
        for n in 1..=2 {
            let event = tokio::time::timeout(Duration::from_secs(16), events.next())
                .await
                .expect("a heartbeat within 15 s")
                .expect("heartbeat");
            assert_eq!(start.elapsed(), Duration::from_secs(15) * n);
            assert_eq!(event.name(), "heartbeat");
            let data = serde_json::to_value(&event).expect("json");
            let mut keys: Vec<&str> = data
                .as_object()
                .expect("object")
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(keys, ["at", "event"], "{data}");
            assert_eq!(data["event"], "heartbeat");
            assert!(data["at"].is_string(), "{data}");
        }
        let live = AnalysisEvent::Heartbeat {
            at: chrono::DateTime::UNIX_EPOCH,
        };
        sender.send(live.clone()).expect("send");
        assert_eq!(events.next().await, Some(live), "live events pass at once");
        assert_eq!(start.elapsed(), Duration::from_secs(30));
        drop(sender);
        assert_eq!(events.next().await, None, "ends with the live channel");
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
