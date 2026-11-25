//! An in-process fake of thepastaclaw's review-system status file (`/review-system/data/status.json`, GitHub Pages):
//! a strong ETag of the body bytes with `If-None-Match` → 304, a status override, a raw body override and a log of
//! every request. Records are shaped like the live file (schema version 1, 2026-11-22).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::header::{ETAG, IF_NONE_MATCH};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

use super::pr_hygiene::Served;

/// Path of the status file.
pub(crate) const STATUS_PATH: &str = "/review-system/data/status.json";

#[derive(Default)]
struct World {
    status_body: Value,
    status: Option<StatusCode>,
    raw: Option<String>,
    served: Vec<Served>,
}

/// The fake status host; cheap to clone.
#[derive(Clone)]
pub(crate) struct FakeThepastaclaw {
    world: Arc<Mutex<World>>,
    /// URL of the status file.
    pub url: Url,
}

/// A `live.active[]` run shaped like the live file's: `steps` done or running so far, `upcoming` steps left.
pub(crate) fn active(repo: &str, number: u32, id: u64, phase: &str, steps: usize) -> Value {
    let names = [
        "worktree", "select", "triage", "context", "phase1", "verify1", "gate", "phase2",
        "verify2", "publish",
    ];
    let steps = steps.min(names.len());
    json!({
        "attempt": 1, "author": "alice", "deadline_at": "2026-11-23T01:54:18Z", "deadline_in_seconds": 20284,
        "degraded": false, "elapsed_seconds": 1365, "findings": null, "heartbeat_age_seconds": 11,
        "heartbeat_at": "2026-11-22T20:16:03Z", "id": id, "lanes": [], "live_lanes": [], "number": number,
        "overdue": false, "phase": phase, "pr_url": format!("https://github.com/{repo}/pull/{number}"),
        "priority": true, "progress": 0.484, "queue": "live", "queued_at": "2026-11-22T19:15:03Z",
        "remaining_seconds": 1456, "repo": repo, "sha": "ebe26cc219776dc63c03009d7821a0375e551f45",
        "started_at": "2026-11-22T19:53:29Z", "status": "running",
        "steps": names[..steps].iter().map(|name| json!({
            "info": {}, "name": name, "seconds": 9, "started_at": "2026-11-22T20:16:05Z", "status": "ok"
        })).collect::<Vec<_>>(),
        "tier": "normal", "title": "A change", "tokens_in": 0, "tokens_out": 0, "trigger": "new_push",
        "upcoming": names[steps..], "wait_seconds": 2306
    })
}

/// A `live.queued[]` entry shaped like the live file's.
pub(crate) fn queued(repo: &str, number: u32, reason: &str) -> Value {
    json!({
        "age_seconds": 3671, "attempts": 0, "author": "alice", "eligible": true,
        "eligible_at": "2026-11-22T19:15:03Z", "eligible_in_seconds": 0, "number": number, "position": 1,
        "pr_url": format!("https://github.com/{repo}/pull/{number}"),
        "prioritize_url": format!("https://github.com/{repo}/issues/{number}#issuecomment-1"), "priority": true,
        "queued_at": "2026-11-22T19:15:03Z", "reason": reason, "repo": repo,
        "sha": "7e29566594114e4ae4243db90d51f75f2f5aab37", "title": "A change", "trigger": "new_push"
    })
}

/// A status file with `active` runs and `queued` entries, its data as of `data_as_of`.
pub(crate) fn status(data_as_of: &str, active: Vec<Value>, queued: Vec<Value>) -> Value {
    json!({
        "schema_version": 1, "data_as_of": data_as_of, "generated_at": data_as_of,
        "history": {}, "rounds": {}, "shadow": {},
        "live": {
            "active": active, "queued": queued, "ts": data_as_of, "failed_heads_24h": 0,
            "heads": {"done": 11347, "queued": 3, "running": 4}, "median_run_minutes": 29.5
        }
    })
}

impl FakeThepastaclaw {
    /// Starts the fake serving `body` at [`STATUS_PATH`].
    pub(crate) async fn start(body: Value) -> Self {
        let world = Arc::new(Mutex::new(World {
            status_body: body,
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
            url: Url::parse(&format!("http://{addr}{STATUS_PATH}")).expect("url"),
        }
    }

    fn world(&self) -> MutexGuard<'_, World> {
        self.world.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the status file.
    pub(crate) fn set_body(&self, body: Value) {
        self.world().status_body = body;
    }

    /// The status file.
    pub(crate) fn body(&self) -> Value {
        self.world().status_body.clone()
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
    } else if path != STATUS_PATH {
        (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND.into_response())
    } else {
        let body = world.status_body.to_string();
        let digest: String = Sha256::digest(body.as_bytes())
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();
        let etag = format!("\"{digest}\"");
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
    };
    world.served.push(Served {
        path,
        if_none_match,
        status: status.as_u16(),
    });
    response
}
