//! An in-process fake of the PR Hygiene service's public API (`/api/v1/prs`, `/api/v1/stages`): strong ETags of the
//! body bytes with `If-None-Match` → 304, a status override (e.g. 503 before the first snapshot), a raw body
//! override (malformed JSON) and a log of every request.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::header::{ETAG, IF_NONE_MATCH};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

/// One request the fake served.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Served {
    /// Path.
    pub path: String,
    /// `If-None-Match` sent.
    pub if_none_match: Option<String>,
    /// Status answered.
    pub status: u16,
}

#[derive(Default)]
struct World {
    prs: Value,
    stages: Value,
    details: HashMap<String, Value>,
    detail_delay: Duration,
    status: Option<StatusCode>,
    raw: Option<String>,
    served: Vec<Served>,
}

/// The fake service; cheap to clone.
#[derive(Clone)]
pub(crate) struct FakePrHygiene {
    world: Arc<Mutex<World>>,
    /// Base URL (with a trailing slash).
    pub url: Url,
}

/// The stage list the real service serves (2026-11-22).
pub(crate) fn stages() -> Value {
    json!({"idle_days": 14, "stages": [
        {"stage": "draft", "owner": "author", "in_flow": false, "late_hours": null, "entry_recorded": true},
        {"stage": "bots", "owner": "bots", "in_flow": true, "late_hours": [24.0, 48.0], "entry_recorded": true},
        {"stage": "self-review", "owner": "author", "in_flow": true, "late_hours": [72.0, 168.0], "entry_recorded": true},
        {"stage": "ci", "owner": "nobody", "in_flow": true, "late_hours": [6.0, 24.0], "entry_recorded": false},
        {"stage": "queued", "owner": "nobody", "in_flow": true, "late_hours": null, "entry_recorded": true},
        {"stage": "review", "owner": "reviewers", "in_flow": true, "late_hours": [24.0, 72.0], "entry_recorded": true},
        {"stage": "mergeable", "owner": "author", "in_flow": true, "late_hours": [24.0, 72.0], "entry_recorded": true},
        {"stage": "blocked", "owner": "maintainers", "in_flow": true, "late_hours": [24.0, 72.0], "entry_recorded": true},
        {"stage": "not-governed", "owner": "author", "in_flow": false, "late_hours": null, "entry_recorded": true},
        {"stage": "unknown", "owner": "nobody", "in_flow": false, "late_hours": null, "entry_recorded": false}
    ]})
}

/// A `/prs` record shaped like the real service's.
pub(crate) fn record(
    repo: &str,
    number: u32,
    engine_state: Option<&str>,
    next_action: &str,
) -> Value {
    json!({
        "key": format!("{repo}#{number}"), "repo": repo, "number": number, "title": "A change",
        "author": "alice", "author_kind": "human", "areas": [], "asks": [], "objectors": [], "base": null,
        "changes_requested": false, "ci_failing": false, "created_at": null, "updated_at": null, "draft": false,
        "engine_state": engine_state, "stage": "self-review", "idle": false, "lateness": "late",
        "merge_conflict": false, "milestone": null, "next_action": next_action,
        "blockers": [next_action, "Human approval or objection resolution is required"],
        "since": "2026-11-18T08:00:00Z", "since_basis": "engine", "tracked": true, "unresolved_comments": 0
    })
}

impl FakePrHygiene {
    /// Starts the fake serving `prs` (records) generated at `generated_at`.
    pub(crate) async fn start(generated_at: &str, prs: Vec<Value>) -> Self {
        let world = Arc::new(Mutex::new(World {
            prs: json!({"generated_at": generated_at, "prs": prs, "stale_repos": []}),
            stages: stages(),
            ..World::default()
        }));
        let router = Router::new().fallback(serve).with_state(Arc::clone(&world));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr: SocketAddr = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, router).await });
        Self {
            world,
            url: Url::parse(&format!("http://{addr}/")).expect("url"),
        }
    }

    fn world(&self) -> std::sync::MutexGuard<'_, World> {
        self.world.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the `/prs` body.
    pub(crate) fn set_prs(&self, body: Value) {
        self.world().prs = body;
    }

    /// Sets the response for a single pull request.
    pub(crate) fn set_detail(&self, repo: &str, number: u32, body: Value) {
        self.world()
            .details
            .insert(format!("/api/v1/prs/{repo}/{number}"), body);
    }

    /// Delays every per-PR response without delaying the aggregate endpoints.
    pub(crate) fn set_detail_delay(&self, delay: Duration) {
        self.world().detail_delay = delay;
    }

    /// The `/prs` body.
    pub(crate) fn prs(&self) -> Value {
        self.world().prs.clone()
    }

    /// Answers every request with `status` (and no body) until cleared.
    pub(crate) fn set_status(&self, status: Option<StatusCode>) {
        self.world().status = status;
    }

    /// Answers every request 200 with `raw` until cleared.
    pub(crate) fn set_raw(&self, raw: Option<&str>) {
        self.world().raw = raw.map(str::to_owned);
    }

    /// Every request served so far.
    pub(crate) fn served(&self) -> Vec<Served> {
        self.world().served.clone()
    }
}

async fn serve(State(world): State<Arc<Mutex<World>>>, request: Request) -> Response {
    let delay = if request.uri().path().starts_with("/api/v1/prs/") {
        world
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .detail_delay
    } else {
        Duration::ZERO
    };
    let response = respond(&world, request);
    tokio::time::sleep(delay).await;
    response
}

fn respond(world: &Mutex<World>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let if_none_match = request
        .headers()
        .get(IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let mut world = world.lock().unwrap_or_else(PoisonError::into_inner);
    let (status, response) = if let Some(status) = world.status {
        (status, status.into_response())
    } else if let Some(raw) = world.raw.clone() {
        (StatusCode::OK, raw.into_response())
    } else {
        let body = match path.as_str() {
            "/api/v1/prs" => Some(world.prs.to_string()),
            "/api/v1/stages" => Some(world.stages.to_string()),
            _ => world.details.get(&path).map(Value::to_string),
        };
        match body {
            None => (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND.into_response()),
            Some(body) => {
                let etag = format!(
                    "\"{}\"",
                    Sha256::digest(body.as_bytes())
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                );
                if if_none_match.as_deref() == Some(etag.as_str()) {
                    (
                        StatusCode::NOT_MODIFIED,
                        StatusCode::NOT_MODIFIED.into_response(),
                    )
                } else {
                    let mut response = body.into_response();
                    if let Ok(value) = HeaderValue::from_str(&etag) {
                        response.headers_mut().insert(ETAG, value);
                    }
                    (StatusCode::OK, response)
                }
            }
        }
    };
    world.served.push(Served {
        path,
        if_none_match,
        status: status.as_u16(),
    });
    response
}
