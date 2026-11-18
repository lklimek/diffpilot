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
use crate::auth::{Session, liveness};
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
    let events = reader_events(state, session, analysis, HEARTBEAT).await?;
    Ok(Sse::new(
        events
            .filter_map(|event| async move { sse_event(&event) })
            .map(Ok),
    ))
}

/// The events of `analysis` as the session's user sees them: the snapshot, then live events and a heartbeat
/// every `heartbeat`. Each heartbeat tick checks the session again (sign-out, expiry, allow-list removal): the
/// stream ends once it is gone, and when the server stops.
async fn reader_events(
    state: AppState,
    session: Session,
    analysis: AnalysisId,
    heartbeat: Duration,
) -> Result<impl Stream<Item = AnalysisEvent>, EventsError> {
    // Another tenant's analysis answers like an unknown one (no existence leak).
    let record = state.db.analyses().get(analysis).await?;
    if record.tenant_id != session.tenant_id {
        return Err(EventsError::Db(DbError::NotFound));
    }
    // Subscribe first: an update written after the snapshot read is then buffered, never lost.
    let live = state.progress.subscribe(analysis);
    let summary = state.db.analyses().summary(analysis).await?;
    let mut snapshot = vec![AnalysisEvent::Analysis { summary }];
    snapshot.extend(
        question_snapshot(&state, analysis, &session.user_id)
            .await?
            .into_iter()
            .map(|question| AnalysisEvent::Question { question }),
    );
    let mut stopping = state.stopping.subscribe();
    let stopped = async move {
        // An error means the sender is gone, which also ends the stream.
        let _ = stopping.wait_for(|stop| *stop).await;
    };
    Ok(analysis_stream(snapshot, live, heartbeat)
        .then(move |mut event| {
            let state = state.clone();
            let session = session.clone();
            async move {
                if matches!(event, AnalysisEvent::Heartbeat { .. })
                    && !session_live(&state, &session).await
                {
                    return Err(());
                }
                let user = session.user_id;
                match personalize_event(&state, &user, &mut event).await {
                    Ok(true) => Ok(Some(event)),
                    Ok(false) => Ok(None),
                    Err(error) => {
                        tracing::warn!(%error, "SSE ownership projection failed; closing stream");
                        Err(())
                    }
                }
            }
        })
        .take_while(|event| std::future::ready(event.is_ok()))
        .filter_map(|event| std::future::ready(event.ok().flatten()))
        .take_until(stopped))
}

/// Whether the stream's session still passes the checks of a new request; a failed read ends the stream too
/// (the browser reconnects through the session check).
async fn session_live(state: &AppState, session: &Session) -> bool {
    let sessions = state.db.sessions();
    match sessions
        .live(session.token_hash(), liveness(Utc::now()))
        .await
    {
        Ok(Some(_)) => true,
        Ok(None) => {
            tracing::debug!(user = %session.user_id, "SSE stream closed: its session ended");
            false
        }
        Err(error) => {
            tracing::warn!(%error, "SSE session check failed; closing stream");
            false
        }
    }
}

/// Reader-specific fields are computed after the shared broadcast, never cached in that broadcast.
async fn personalize_event(
    state: &AppState,
    user: &str,
    event: &mut AnalysisEvent,
) -> Result<bool, DbError> {
    match event {
        AnalysisEvent::Analysis { summary } => {
            summary.has_my_state = Some(state.db.review_states().exists(user, summary.id).await?);
        }
        AnalysisEvent::Question { question } => {
            let record = state.db.questions().get(question.id, Utc::now()).await?;
            if record.user_id != user {
                return Ok(false);
            }
            question.asked_by = record.question.asked_by;
            question.mine = Some(record.user_id == user);
        }
        AnalysisEvent::Heartbeat { .. } => {}
    }
    Ok(true)
}

/// Every private question of the analysis in any state, for the snapshot.
async fn question_snapshot(
    state: &AppState,
    analysis: AnalysisId,
    user: &str,
) -> Result<Vec<AskQuestion>, DbError> {
    state
        .db
        .questions()
        .list_for_user(analysis, user, Utc::now())
        .await
}

/// The snapshot, then live events and a [`AnalysisEvent::Heartbeat`] every `heartbeat`; ends when the live events
/// end.
fn analysis_stream(
    snapshot: Vec<AnalysisEvent>,
    live: broadcast::Receiver<AnalysisEvent>,
    heartbeat: Duration,
) -> impl Stream<Item = AnalysisEvent> {
    let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
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
    async fn private_sse_snapshot_and_live_events_never_reveal_the_other_reader() {
        use contracts::ids::{QuestionId, StepId};
        use db::repo::questions::{NewQuestion, QuestionLimits};
        let state = test_state().await;
        let analysis = seed_analysis(&state.db).await.expect("seed");
        let cookie_a = session_cookie(&state, DEFAULT_TENANT).await;
        let cookie_b = crate::test_support::other_user_cookie(&state).await;
        let mut events = Vec::new();
        for user in ["u1", "u2"] {
            let id = QuestionId::new_random();
            state
                .db
                .questions()
                .create(
                    NewQuestion {
                        id,
                        analysis_id: analysis,
                        step_id: StepId("f1.1".to_owned()),
                        user_id: user.to_owned(),
                        text: format!("PRIVATE_{user}"),
                    },
                    QuestionLimits {
                        per_analysis: 10,
                        per_user: 10,
                        user_window_start: Utc::now(),
                    },
                    Utc::now(),
                )
                .await
                .expect("question");
            let mut question = state
                .db
                .questions()
                .get(id, Utc::now())
                .await
                .expect("question")
                .question;
            question.state = contracts::api::questions::AskState::Answering;
            events.push(AnalysisEvent::Question { question });
        }
        for (index, cookie) in [cookie_a, cookie_b].iter().enumerate() {
            let response = router(state.clone())
                .oneshot(events_request(analysis, Some(cookie)))
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::OK);
            // Foreign first: it must be skipped while the stream stays open for the own event.
            for event in [&events[1 - index], &events[index]] {
                state
                    .progress
                    .update(analysis, || async { Ok::<_, DbError>(event.clone()) })
                    .await
                    .expect("publish");
            }
            let received = read_events(response.into_body(), 3).await;
            assert_eq!(received[0].0, "analysis");
            for (_, data) in &received[1..] {
                assert_eq!(data["question"]["text"], format!("PRIVATE_u{}", index + 1));
                assert_eq!(data["question"]["mine"], true);
            }
            assert_eq!(received[2].1["question"]["state"], "answering");
        }
    }

    #[tokio::test]
    async fn sse_projects_ownership_per_subscriber_without_replacing_the_event_state() {
        use contracts::api::questions::AskState;
        use contracts::ids::{QuestionId, StepId};
        use db::repo::questions::{NewQuestion, QuestionLimits};
        let state = test_state().await;
        let analysis = seed_analysis(&state.db).await.expect("analysis");
        let id = QuestionId::new_random();
        state
            .db
            .questions()
            .create(
                NewQuestion {
                    id,
                    analysis_id: analysis,
                    step_id: StepId("f1.1".to_owned()),
                    user_id: "u1".to_owned(),
                    text: "Question".to_owned(),
                },
                QuestionLimits {
                    per_analysis: 10,
                    per_user: 10,
                    user_window_start: Utc::now(),
                },
                Utc::now(),
            )
            .await
            .expect("question");
        let mut question = state
            .db
            .questions()
            .get(id, Utc::now())
            .await
            .expect("question")
            .question;
        question.state = AskState::Answering;
        let shared = AnalysisEvent::Question { question };
        for (user, mine) in [("u1", true), ("u2", false)] {
            let mut event = shared.clone();
            let visible = personalize_event(&state, user, &mut event)
                .await
                .expect("projection");
            assert_eq!(visible, mine, "foreign questions never leave the stream");
            if !visible {
                continue;
            }
            let wire = serde_json::to_value(event).expect("wire");
            assert_eq!(wire["question"]["mine"], mine);
            assert_eq!(wire["question"]["asked_by"], "lklimek");
            assert_eq!(
                wire["question"]["state"], "answering",
                "ownership reads must not reorder state"
            );
        }
        assert!(
            serde_json::to_value(shared).expect("wire")["question"]
                .get("mine")
                .is_none()
        );
        state
            .db
            .review_states()
            .put(
                "u1",
                analysis,
                &contracts::api::review::ReviewState::default(),
                Utc::now(),
            )
            .await
            .expect("state");
        for (user, mine) in [("u1", true), ("u2", false)] {
            let mut event = AnalysisEvent::Analysis {
                summary: state
                    .db
                    .analyses()
                    .summary(analysis)
                    .await
                    .expect("summary"),
            };
            personalize_event(&state, user, &mut event)
                .await
                .expect("projection");
            assert_eq!(
                serde_json::to_value(event).expect("wire")["summary"]["has_my_state"],
                mine
            );
        }
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
                idle: false,
                stage: None,
            },
            AnalysisState::Running {
                done: 1,
                total: 2,
                idle: false,
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
        let mut events = Box::pin(analysis_stream(Vec::new(), receiver, HEARTBEAT));
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

    /// The session of `cookie`, as the extractor loads it.
    async fn session_of(state: &AppState, cookie: &str) -> Session {
        use axum::extract::FromRequestParts;
        let (mut parts, ()) = Request::get("/")
            .header("cookie", cookie)
            .body(())
            .expect("request")
            .into_parts();
        <Session as FromRequestParts<AppState>>::from_request_parts(&mut parts, state)
            .await
            .expect("session")
    }

    /// Drains `events` until the stream ends; panics when it is still open after [`WAIT`].
    async fn until_end(events: impl Stream<Item = AnalysisEvent>) -> Vec<AnalysisEvent> {
        tokio::time::timeout(WAIT, events.collect())
            .await
            .expect("the stream ends")
    }

    /// Sign-out, expiry or allow-list removal end an open stream at its next heartbeat tick.
    #[tokio::test]
    async fn stream_ends_at_the_next_heartbeat_once_the_session_is_gone() {
        let state = test_state().await;
        let analysis = seed_analysis(&state.db).await.expect("seed");
        let cookie = session_cookie(&state, DEFAULT_TENANT).await;
        let session = session_of(&state, &cookie).await;
        let events = reader_events(
            state.clone(),
            session.clone(),
            analysis,
            Duration::from_millis(50),
        )
        .await
        .expect("stream");
        let mut events = Box::pin(events);
        assert!(matches!(
            events.next().await,
            Some(AnalysisEvent::Analysis { .. })
        ));
        assert!(
            matches!(
                tokio::time::timeout(WAIT, events.next()).await,
                Ok(Some(AnalysisEvent::Heartbeat { .. }))
            ),
            "a live session keeps its heartbeats"
        );
        assert!(
            state
                .db
                .sessions()
                .delete(session.token_hash())
                .await
                .expect("sign out")
        );
        let rest = until_end(events).await;
        assert!(
            rest.len() <= 1,
            "at most the tick already under way: {rest:?}"
        );
    }

    /// The stop signal ends open streams, so graceful shutdown does not wait for the browsers.
    #[tokio::test]
    async fn stream_ends_when_the_server_stops() {
        let state = test_state().await;
        let analysis = seed_analysis(&state.db).await.expect("seed");
        let cookie = session_cookie(&state, DEFAULT_TENANT).await;
        let session = session_of(&state, &cookie).await;
        let events = reader_events(state.clone(), session, analysis, HEARTBEAT)
            .await
            .expect("stream");
        let mut events = Box::pin(events);
        assert!(matches!(
            events.next().await,
            Some(AnalysisEvent::Analysis { .. })
        ));
        state.stopping.send_replace(true);
        assert!(until_end(events).await.is_empty());
    }
}
