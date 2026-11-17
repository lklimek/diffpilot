//! Fake GitHub for development and tests: serves the recordings of `fixtures/pr-482/github` — the OAuth web
//! flow, `GET /user`, the recorded REST endpoints, the GitHub App endpoints (installation, installation token) and
//! a GraphQL endpoint with review threads and an in-memory pending review. Deterministic; never contacts GitHub.

mod app;
mod fixtures;
mod graphql;
mod oauth;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::{CONTENT_TYPE, USER_AGENT};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use url::Url;

use crate::fixtures::{RECORDED_ENDPOINTS, RawRecording, load};

/// Configuration of the fake GitHub.
#[derive(Clone, Debug, clap::Args)]
pub struct Config {
    /// Directory of the recordings, e.g. `fixtures/pr-482/github`.
    #[arg(long, env = "FAKE_GITHUB_FIXTURE_DIR")]
    pub fixture_dir: PathBuf,
    /// Public URL of the app (`SR_PUBLIC_URL`); its `/auth/github/callback` is the registered callback.
    #[arg(long, env = "FAKE_GITHUB_APP_URL")]
    pub app_url: Url,
    /// Login of the fake user; default: the recorded user (`alice`).
    #[arg(long, env = "FAKE_GITHUB_USER_LOGIN")]
    pub user_login: Option<String>,
    /// Numeric id of the fake user; default: the recorded id.
    #[arg(long, env = "FAKE_GITHUB_USER_ID")]
    pub user_id: Option<i64>,
}

/// The fake GitHub could not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The fixture directory does not exist.
    #[error(
        "the fixture directory {} does not exist; pass --fixture-dir (e.g. fixtures/pr-482/github)",
        path.display()
    )]
    FixtureDirMissing {
        /// The configured directory.
        path: PathBuf,
    },
    /// A recording could not be read.
    #[error("could not read the recording {}", path.display())]
    ReadFixture {
        /// The recording file.
        path: PathBuf,
        /// The I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A recording is not the expected JSON.
    #[error("the recording {} is not valid", path.display())]
    ParseFixture {
        /// The recording file.
        path: PathBuf,
        /// The JSON error.
        #[source]
        source: serde_json::Error,
    },
    /// A recording has an unusable method or status.
    #[error("the recording {} has an invalid {field}", path.display())]
    BadRecording {
        /// The recording file.
        path: PathBuf,
        /// The offending field.
        field: &'static str,
    },
}

/// A recorded response.
struct Recorded {
    status: StatusCode,
    body: Bytes,
}

/// Recorded REST responses by method and path.
type RecordedEndpoints = HashMap<(Method, String), Recorded>;

/// State of the recorded REST endpoints: the responses and the OAuth state that checks present tokens.
struct RecordedState {
    endpoints: RecordedEndpoints,
    oauth: Arc<oauth::Oauth>,
}

/// Loads every recording and builds the router; fails when the fixture directory or a recording is missing.
pub fn router(config: &Config) -> Result<Router, StartError> {
    if !config.fixture_dir.is_dir() {
        return Err(StartError::FixtureDirMissing {
            path: config.fixture_dir.clone(),
        });
    }
    let oauth = Arc::new(oauth::Oauth::load(config)?);
    let graphql = Arc::new(graphql::Graphql::load(config, Arc::clone(&oauth))?);
    let mut recorded = RecordedEndpoints::new();
    for file in RECORDED_ENDPOINTS {
        let recording: RawRecording = load(&config.fixture_dir, file)?;
        let invalid = |field| StartError::BadRecording {
            path: config.fixture_dir.join(file),
            field,
        };
        let method = Method::from_bytes(recording.request.method.as_bytes())
            .map_err(|_| invalid("request.method"))?;
        let status = StatusCode::from_u16(recording.status).map_err(|_| invalid("status"))?;
        recorded.insert(
            (method, recording.request.url.path().to_owned()),
            Recorded {
                status,
                body: Bytes::from(recording.response.get().to_owned()),
            },
        );
    }
    tracing::info!(
        endpoints = recorded.len(),
        dir = %config.fixture_dir.display(),
        "fake-github loaded the recordings"
    );
    Ok(Router::new()
        .fallback(serve_recorded)
        .with_state(Arc::new(RecordedState {
            endpoints: recorded,
            oauth: Arc::clone(&oauth),
        }))
        .merge(oauth::routes(oauth))
        .merge(app::routes(Arc::new(app::App::load(&config.fixture_dir)?)))
        .merge(
            Router::new()
                .route("/graphql", post(graphql::serve))
                .with_state(graphql),
        ))
}

/// A JSON response with a pre-serialised body.
fn json_body(status: StatusCode, body: Bytes) -> Response {
    (
        status,
        [(CONTENT_TYPE, "application/json; charset=utf-8")],
        body,
    )
        .into_response()
}

/// A REST API error in GitHub's shape.
fn github_error(status: StatusCode, message: &str) -> Response {
    let body = serde_json::json!({
        "message": message,
        "documentation_url": "https://docs.github.com/rest",
        "status": status.as_str(),
    });
    json_body(status, Bytes::from(body.to_string()))
}

/// GitHub's REST API refuses requests without a `User-Agent`.
fn require_user_agent(headers: &HeaderMap) -> Option<Response> {
    if headers.contains_key(USER_AGENT) {
        return None;
    }
    tracing::warn!("fake-github REST request without a User-Agent header; GitHub answers 403");
    Some(github_error(
        StatusCode::FORBIDDEN,
        "Request forbidden by administrative rules. Please make sure your request has a User-Agent header",
    ))
}

/// Serves a recorded REST response by method and path (the query is ignored); 404 otherwise. Anonymous
/// reads are allowed (public repository); a present but invalid token gets 401, as on GitHub.
async fn serve_recorded(
    State(state): State<Arc<RecordedState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    if let Some(rejection) = require_user_agent(&headers) {
        return rejection;
    }
    if state.oauth.credentials(&headers) == oauth::Credentials::Invalid {
        tracing::debug!(
            path = uri.path(),
            "fake-github REST request with an unknown or rotated token"
        );
        return github_error(StatusCode::UNAUTHORIZED, "Bad credentials");
    }
    match state
        .endpoints
        .get(&(method.clone(), uri.path().to_owned()))
    {
        Some(response) => json_body(response.status, response.body.clone()),
        None => {
            tracing::warn!(
                %method,
                path = uri.path(),
                "fake-github has no recording for this request; add its fixture to RECORDED_ENDPOINTS"
            );
            github_error(StatusCode::NOT_FOUND, "Not Found")
        }
    }
}
