//! GitHub App endpoints, called with an App JWT (`Bearer` with an RS256 JWT header; the signature is not checked):
//! the repository installation (`rest/repo-installation.json`; other repositories get
//! `rest/repo-installation-missing.json`), an installation token valid one hour from now
//! (`rest/installation-token.json` with a fresh `expires_at`), and `DELETE /installation/token` (204 for the
//! minted token).

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{delete, get, post};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{SecondsFormat, TimeDelta, Utc};
use serde_json::Value;

use crate::StartError;
use crate::fixtures::{RawRecording, Recording, load};
use crate::{github_error, json_body, require_user_agent};

const INSTALLATION: &str = "rest/repo-installation.json";
const INSTALLATION_MISSING: &str = "rest/repo-installation-missing.json";
const INSTALLATION_TOKEN: &str = "rest/installation-token.json";

/// The recorded App answers.
pub(crate) struct App {
    /// `owner/name` of the repository the App is installed on.
    repository: String,
    installation_id: Option<u64>,
    /// Recorded bodies, served byte for byte.
    installation: Bytes,
    missing: Bytes,
    token: Value,
}

impl App {
    /// Loads the App recordings.
    pub(crate) fn load(dir: &std::path::Path) -> Result<Self, StartError> {
        let installation: RawRecording = load(dir, INSTALLATION)?;
        let missing: RawRecording = load(dir, INSTALLATION_MISSING)?;
        let installation_id = serde_json::from_str::<Value>(installation.response.get())
            .ok()
            .and_then(|answer| answer["id"].as_u64());
        let token: Recording<Value, Value> = load(dir, INSTALLATION_TOKEN)?;
        let repository = installation
            .request
            .url
            .path()
            .trim_start_matches("/repos/")
            .trim_end_matches("/installation")
            .to_owned();
        Ok(Self {
            repository,
            installation_id,
            installation: Bytes::from(installation.response.get().to_owned()),
            missing: Bytes::from(missing.response.get().to_owned()),
            token: token.response,
        })
    }
}

/// The App routes.
pub(crate) fn routes(app: Arc<App>) -> Router {
    Router::new()
        .route("/repos/{owner}/{repo}/installation", get(installation))
        .route("/app/installations/{id}/access_tokens", post(access_token))
        .route("/installation/token", delete(revoke))
        .with_state(app)
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// Whether the request carries an App JWT (three parts, RS256 header).
fn has_app_jwt(headers: &HeaderMap) -> bool {
    let Some(token) = bearer(headers) else {
        return false;
    };
    let parts: Vec<&str> = token.split('.').collect();
    let [header, _, _] = parts.as_slice() else {
        return false;
    };
    URL_SAFE_NO_PAD
        .decode(header)
        .ok()
        .and_then(|header| serde_json::from_slice::<Value>(&header).ok())
        .is_some_and(|header| header["alg"] == "RS256")
}

/// The checks of every App endpoint; `Some` is the refusal.
fn refuse(headers: &HeaderMap) -> Option<Response> {
    if let Some(rejection) = require_user_agent(headers) {
        return Some(rejection);
    }
    if has_app_jwt(headers) {
        return None;
    }
    tracing::debug!("fake-github App endpoint called without an App JWT");
    Some(github_error(
        StatusCode::UNAUTHORIZED,
        "A JSON web token could not be decoded",
    ))
}

async fn installation(
    State(app): State<Arc<App>>,
    Path((owner, repo)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Some(refusal) = refuse(&headers) {
        return refusal;
    }
    if format!("{owner}/{repo}").eq_ignore_ascii_case(&app.repository) {
        json_body(StatusCode::OK, app.installation.clone())
    } else {
        json_body(StatusCode::NOT_FOUND, app.missing.clone())
    }
}

async fn access_token(
    State(app): State<Arc<App>>,
    Path(id): Path<u64>,
    headers: HeaderMap,
) -> Response {
    if let Some(refusal) = refuse(&headers) {
        return refusal;
    }
    if Some(id) != app.installation_id {
        return github_error(StatusCode::NOT_FOUND, "Not Found");
    }
    let mut token = app.token.clone();
    token["expires_at"] = Value::String(
        (Utc::now() + TimeDelta::hours(1)).to_rfc3339_opts(SecondsFormat::Secs, true),
    );
    tracing::info!(
        installation = id,
        "fake-github minted an installation token"
    );
    json_body(StatusCode::CREATED, Bytes::from(token.to_string()))
}

async fn revoke(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if let Some(rejection) = require_user_agent(&headers) {
        return rejection;
    }
    if bearer(&headers).is_some() && bearer(&headers) == app.token["token"].as_str() {
        tracing::info!("fake-github revoked the installation token");
        return json_body(StatusCode::NO_CONTENT, Bytes::new());
    }
    github_error(StatusCode::UNAUTHORIZED, "Bad credentials")
}
