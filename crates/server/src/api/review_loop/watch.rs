//! Watch routes (RW1–RW3, `docs/contracts/http-api.md` "Review loop", `docs/contracts/sse-events.md` "User events"):
//! watch and unwatch a pull request, the reader's watches, and the reader's event stream. The work lives in
//! `review_loop::watch`.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use contracts::api::llm::LlmActivity;
use contracts::api::review_loop::{UserStreamEvent, WatchList};
use contracts::routes::{LIST_WATCHES, USER_EVENTS, WATCH_PR};
use contracts::time::Timestamp;
use db::error::DbError;
use futures::stream::{self, Stream, StreamExt};
use tokio::sync::watch::Receiver;
use tokio::time::{Instant, Interval, MissedTickBehavior};

use super::{ApiResult, PrPath, handle_of};
use crate::api::error::ApiFailure;
use crate::api::events::{HEARTBEAT, StreamSlot, holding, session_live};
use crate::api::llm_activity;
use crate::auth::{Session, SessionCsrf};
use crate::review_loop::watch;
use crate::state::AppState;

/// How often a user's event stream looks for new stored events.
const POLL: Duration = Duration::from_secs(5);
/// Most stored events one look sends.
const POLL_BATCH: u32 = 50;
const ACTIVITY_INTERVAL: Duration = Duration::from_secs(1);

/// Routes of this module.
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(WATCH_PR.path, put(watch_pr).delete(unwatch_pr))
        .route(LIST_WATCHES.path, get(list))
        .route(USER_EVENTS.path, get(user_events))
}

/// RW1 PUT: the pull request is read (as on its page), then watched.
async fn watch_pr(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: PrPath,
) -> ApiResult<StatusCode> {
    let handle = handle_of(&state, &session, path).await?;
    watch::watch(&state, &session, &handle).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// RW1 DELETE.
async fn unwatch_pr(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: PrPath,
) -> ApiResult<StatusCode> {
    let Ok(axum::extract::Path((owner, name, number))) = path else {
        return Err(crate::review_loop::LoopError::NotFound.into());
    };
    let pr = super::pr_ref(owner, name, number);
    let handle = crate::review_loop::find(&state, &session.tenant_id, &pr).await?;
    watch::unwatch(&state, &session, &handle).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// RW2.
async fn list(State(state): State<AppState>, session: Session) -> ApiResult<Json<WatchList>> {
    Ok(Json(watch::list(&state, &session).await?))
}

/// RW3: the reader's events, activity updates, and a heartbeat every 15 s.
async fn user_events(State(state): State<AppState>, session: Session) -> Response {
    let slot = match StreamSlot::open(&session) {
        Ok(slot) => slot,
        Err(full) => return full.into_response(),
    };
    let events = match user_stream(state, session, POLL, HEARTBEAT).await {
        Ok(events) => events,
        Err(error) => return ApiFailure(error).into_response(),
    };
    Sse::new(
        holding(slot, events)
            .filter_map(|event| async move { sse_event(&event) })
            .map(Ok::<_, Infallible>),
    )
    .into_response()
}

fn sse_event(event: &UserStreamEvent) -> Option<Event> {
    match Event::default().event(event.name()).json_data(event) {
        Ok(sse) => Some(sse),
        Err(error) => {
            tracing::error!(%error, "could not encode a user event for SSE; the event was skipped");
            None
        }
    }
}

/// What a user's stream holds between two items.
struct Cursor {
    state: AppState,
    session: Session,
    /// The newest event sent (or the connection time).
    after: Timestamp,
    pending: VecDeque<UserStreamEvent>,
    poll: Interval,
    heartbeat: Interval,
    stopping: Receiver<bool>,
    activity_ticks: Receiver<()>,
    last_activity: LlmActivity,
    next_activity: Instant,
}

enum Wake {
    Stop,
    Heartbeat,
    Poll,
    Activity,
}

fn interval(period: Duration) -> Interval {
    let mut ticks = tokio::time::interval_at(Instant::now() + period, period);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticks
}

/// The user's stored events, rate-limited activity updates, and heartbeats that recheck the session.
async fn user_stream(
    state: AppState,
    session: Session,
    poll: Duration,
    heartbeat: Duration,
) -> Result<impl Stream<Item = UserStreamEvent>, DbError> {
    let after = Timestamp::now();
    let activity_ticks = state.db.jobs().subscribe_activity(&session.tenant_id);
    let last_activity = llm_activity::snapshot(&state, &session).await?;
    let cursor = Cursor {
        stopping: state.stopping.subscribe(),
        state,
        session,
        after,
        pending: VecDeque::new(),
        poll: interval(poll),
        heartbeat: interval(heartbeat),
        activity_ticks,
        last_activity,
        next_activity: Instant::now(),
    };
    Ok(stream::unfold(cursor, |mut c| async move {
        loop {
            if let Some(event) = c.pending.pop_front() {
                return Some((event, c));
            }
            let wake = tokio::select! {
                _ = c.stopping.wait_for(|stop| *stop) => Wake::Stop,
                _ = c.heartbeat.tick() => Wake::Heartbeat,
                _ = c.poll.tick() => Wake::Poll,
                changed = activity_tick(&mut c.activity_ticks, c.next_activity) => {
                    if changed.is_ok() { Wake::Activity } else { Wake::Stop }
                },
            };
            match wake {
                Wake::Stop => return None,
                Wake::Heartbeat => {
                    if !session_live(&c.state, &c.session).await {
                        return None;
                    }
                    let at = Timestamp::now();
                    return Some((UserStreamEvent::Heartbeat { at }, c));
                }
                Wake::Activity => {
                    let activity = match llm_activity::snapshot(&c.state, &c.session).await {
                        Ok(activity) => activity,
                        Err(error) => {
                            tracing::warn!(%error, "LLM activity could not be read; closing the user event stream");
                            return None;
                        }
                    };
                    // Only visible changes trigger an event or advance the stream's cooldown.
                    if activity != c.last_activity {
                        if !session_live(&c.state, &c.session).await {
                            return None;
                        }
                        c.next_activity = Instant::now() + ACTIVITY_INTERVAL;
                        c.last_activity = activity.clone();
                        return Some((UserStreamEvent::LlmActivity { activity }, c));
                    }
                }
                Wake::Poll => {
                    let events = c
                        .state
                        .db
                        .review_loop()
                        .user_events(&c.session.user_id, Some(c.after), POLL_BATCH)
                        .await;
                    match events {
                        Ok(events) => {
                            if let Some(last) = events.last() {
                                c.after = last.at;
                            }
                            c.pending.extend(
                                events
                                    .into_iter()
                                    .map(|user_event| UserStreamEvent::UserEvent { user_event }),
                            );
                        }
                        Err(error) => {
                            tracing::warn!(%error, "user events could not be read; closing the stream");
                            return None;
                        }
                    }
                }
            }
        }
    }))
}

async fn activity_tick(
    changes: &mut Receiver<()>,
    next_activity: Instant,
) -> Result<(), tokio::sync::watch::error::RecvError> {
    tokio::time::sleep_until(next_activity).await;
    changes.changed().await
}

#[cfg(test)]
mod tests {
    use axum::extract::FromRequestParts;
    use axum::http::Request;
    use contracts::api::review_loop::UserEventKind;
    use db::repo::DEFAULT_TENANT;
    use db::repo::review_loop::NewUserEvent;
    use db::repo::users::GithubProfile;
    use jiff::SignedDuration;

    use super::*;
    use crate::state::tests::test_state;

    const WAIT: Duration = Duration::from_secs(5);

    async fn member(state: &AppState, user: &str, github_id: i64) {
        state
            .db
            .users()
            .upsert_member(
                DEFAULT_TENANT,
                user,
                &GithubProfile {
                    github_id,
                    login: format!("login-{user}"),
                    name: None,
                    avatar_url: None,
                },
                Timestamp::now(),
            )
            .await
            .expect("user");
    }

    async fn session(state: &AppState) -> Session {
        member(state, "u1", 1).await;
        member(state, "u2", 2).await;
        let cookie = crate::test_support::session_cookie(state, DEFAULT_TENANT).await;
        let (mut parts, ()) = Request::get("/")
            .header("cookie", cookie)
            .body(())
            .expect("request")
            .into_parts();
        <Session as FromRequestParts<AppState>>::from_request_parts(&mut parts, state)
            .await
            .expect("session")
    }

    fn event(user: &str, text: &str) -> NewUserEvent {
        NewUserEvent {
            user_id: user.to_owned(),
            kind: UserEventKind::Test,
            pull_request_id: None,
            count: 0,
            text: text.to_owned(),
            url: None,
        }
    }

    /// RW3 (G4): the stream sends the user's events written after it opened, named `user_event`, never older ones
    /// or another user's, and a `heartbeat`.
    #[tokio::test]
    async fn user_stream_sends_new_events_by_name() {
        let state = test_state().await;
        let session = session(&state).await;
        let rl = state.db.review_loop();
        rl.add_user_event(
            DEFAULT_TENANT,
            &event("u1", "old"),
            Timestamp::now() - SignedDuration::from_mins(1),
        )
        .await
        .expect("old event");
        let mut events = Box::pin(
            user_stream(
                state.clone(),
                session,
                Duration::from_millis(20),
                Duration::from_millis(400),
            )
            .await
            .expect("stream"),
        );
        rl.add_user_event(DEFAULT_TENANT, &event("u2", "other"), Timestamp::now())
            .await
            .expect("other user's event");
        rl.add_user_event(DEFAULT_TENANT, &event("u1", "new"), Timestamp::now())
            .await
            .expect("new event");
        let first = tokio::time::timeout(WAIT, events.next())
            .await
            .expect("in time")
            .expect("an event");
        let UserStreamEvent::UserEvent { user_event } = &first else {
            panic!("expected a user event first: {first:?}");
        };
        assert_eq!(user_event.text, "new");
        assert_eq!(first.name(), "user_event");
        let wire = serde_json::to_value(&first).expect("json");
        assert_eq!(wire["event"], "user_event");
        assert_eq!(wire["user_event"]["kind"], "test");
        let second = tokio::time::timeout(WAIT, events.next())
            .await
            .expect("in time")
            .expect("a heartbeat");
        assert_eq!(second.name(), "heartbeat", "{second:?}");
    }

    /// RW3 ends when the server stops.
    #[tokio::test]
    async fn user_stream_ends_when_the_server_stops() {
        let state = test_state().await;
        let session = session(&state).await;
        let events = user_stream(
            state.clone(),
            session,
            Duration::from_millis(20),
            Duration::from_secs(60),
        )
        .await
        .expect("stream");
        state.stopping.send_replace(true);
        let rest: Vec<_> = tokio::time::timeout(WAIT, events.collect())
            .await
            .expect("the stream ends");
        assert!(rest.is_empty(), "{rest:?}");
    }
}
