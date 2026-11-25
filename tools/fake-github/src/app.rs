//! GitHub App endpoints, called with an App JWT checked like GitHub does: RS256 signature against the App's public
//! key, `iss` = the App id, `iat` not in the future (hence GitHub's advice to backdate it by 60 s), `exp` in the
//! future and at most 10 minutes from now. Endpoints: the repository installation (`rest/repo-installation.json`;
//! other repositories get `rest/repo-installation-missing.json`), an installation token, and
//! `DELETE /installation/token`.
//!
//! A token request must name its scope explicitly, as the server's job tokens do (read-only, the pull request's
//! repository only): without `repositories` or `permissions` GitHub would grant every repository or the App's write
//! access, so the fake refuses such requests (422), and write permissions, permissions the installation lacks and
//! other repositories as well. Each minted token is unique and valid one hour; `GET /_fake/app-credentials` lists
//! the JWTs accepted and the tokens minted (for log scans).

use std::collections::BTreeMap;
use std::path::Path as FilePath;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use contracts::time::Timestamp;
use jiff::SignedDuration;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fixtures::{RawRecording, Recording, load};
use crate::{Config, StartError, github_error, json_body, require_user_agent};

const INSTALLATION: &str = "rest/repo-installation.json";
const INSTALLATION_MISSING: &str = "rest/repo-installation-missing.json";
const INSTALLATION_TOKEN: &str = "rest/installation-token.json";
/// GitHub's longest App JWT lifetime, counted from now.
const MAX_JWT_LIFETIME_SECS: i64 = 600;
/// GitHub's 401 answer for a JWT it cannot verify.
const UNDECODABLE: &str = "A JSON web token could not be decoded";

/// What the App endpoints accepted and issued (`GET /_fake/app-credentials`).
#[derive(Clone, Debug, Default, Serialize)]
struct Issued {
    /// App JWTs accepted, in order.
    jwts: Vec<String>,
    /// Installation tokens minted, in order.
    installation_tokens: Vec<String>,
    /// Installation tokens revoked.
    revoked: Vec<String>,
}

/// The recorded App answers and the App's identity.
pub(crate) struct App {
    /// `owner/name` of the repository the App is installed on.
    repository: String,
    installation_id: u64,
    /// Permissions granted to the installation (`rest/repo-installation.json`).
    granted: BTreeMap<String, String>,
    /// Recorded bodies, served byte for byte.
    installation: Bytes,
    missing: Bytes,
    token: Value,
    /// App id and public key the JWTs must match; `None`: not configured, every App call is refused.
    verifier: Option<(u64, DecodingKey)>,
    issued: Mutex<Issued>,
}

#[derive(Deserialize)]
struct InstallationAnswer {
    id: u64,
    permissions: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Claims {
    iss: Value,
    iat: i64,
    exp: i64,
}

/// The body of a token request (GitHub's fields; unknown ones are refused).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MintRequest {
    repositories: Option<Vec<String>>,
    repository_ids: Option<Vec<u64>>,
    permissions: Option<BTreeMap<String, String>>,
}

impl App {
    /// Loads the App recordings and the App's public key.
    pub(crate) fn load(config: &Config) -> Result<Self, StartError> {
        let dir = &config.fixture_dir;
        let installation: RawRecording = load(dir, INSTALLATION)?;
        let missing: RawRecording = load(dir, INSTALLATION_MISSING)?;
        let answer: InstallationAnswer = serde_json::from_str(installation.response.get())
            .map_err(|source| StartError::ParseFixture {
                path: dir.join(INSTALLATION),
                source,
            })?;
        let token: Recording<Value, Value> = load(dir, INSTALLATION_TOKEN)?;
        let repository = installation
            .request
            .url
            .path()
            .trim_start_matches("/repos/")
            .trim_end_matches("/installation")
            .to_owned();
        let verifier = match (config.app_id, &config.app_public_key_file) {
            (Some(id), Some(file)) => Some((id, public_key(file)?)),
            _ => {
                tracing::warn!(
                    "fake-github has no App id or public key; every GitHub App call is refused"
                );
                None
            }
        };
        Ok(Self {
            repository,
            installation_id: answer.id,
            granted: answer.permissions,
            installation: Bytes::from(installation.response.get().to_owned()),
            missing: Bytes::from(missing.response.get().to_owned()),
            token: token.response,
            verifier,
            issued: Mutex::default(),
        })
    }

    fn issued(&self) -> MutexGuard<'_, Issued> {
        self.issued.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// GitHub's checks of an App JWT; `Err` is GitHub's 401 message.
    fn verify(&self, jwt: &str) -> Result<(), &'static str> {
        let Some((app_id, key)) = &self.verifier else {
            return Err(UNDECODABLE);
        };
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_required_spec_claims(&["iat", "exp", "iss"]);
        validation.validate_exp = false;
        let claims = jsonwebtoken::decode::<Claims>(jwt, key, &validation)
            .map_err(|_| UNDECODABLE)?
            .claims;
        let now = Timestamp::now().as_second();
        let issuer = match &claims.iss {
            Value::String(iss) => iss.parse::<u64>().ok(),
            Value::Number(iss) => iss.as_u64(),
            _ => None,
        };
        if issuer != Some(*app_id) {
            return Err("'Issuer' claim ('iss') must name this GitHub App");
        }
        if claims.iat > now {
            return Err(
                "'Issued at' claim ('iat') must be an Integer representing a time in the past",
            );
        }
        if claims.exp <= now {
            return Err(
                "'Expiration time' claim ('exp') must be a numeric value representing the future time at which the assertion expires",
            );
        }
        if claims.exp > now + MAX_JWT_LIFETIME_SECS {
            return Err("'Expiration time' claim ('exp') is too far in the future");
        }
        Ok(())
    }

    /// The checks of every App endpoint; `Some` is the refusal.
    fn refuse(&self, headers: &HeaderMap) -> Option<Response> {
        if let Some(rejection) = require_user_agent(headers) {
            return Some(rejection);
        }
        let jwt = bearer(headers).unwrap_or_default();
        match self.verify(jwt) {
            Ok(()) => {
                self.issued().jwts.push(jwt.to_owned());
                None
            }
            Err(message) => {
                tracing::info!(reason = message, "fake-github refused an App JWT");
                Some(github_error(StatusCode::UNAUTHORIZED, message))
            }
        }
    }

    /// The permissions a token request asks for, if the fake grants them; `Err` is the 422 message.
    fn scope(&self, body: &[u8]) -> Result<BTreeMap<String, String>, String> {
        let request: MintRequest =
            serde_json::from_slice(body).map_err(|error| format!("Invalid request: {error}"))?;
        if request.repository_ids.is_some() {
            return Err("fake-github: name the repository in `repositories`, not by id".to_owned());
        }
        let Some(repositories) = request.repositories else {
            return Err(
                "fake-github: the token request must name the pull request's repository \
                 (`repositories`); without it GitHub grants every repository of the installation"
                    .to_owned(),
            );
        };
        let own = self.repository.rsplit('/').next().unwrap_or_default();
        if !matches!(repositories.as_slice(), [only] if only.eq_ignore_ascii_case(own)) {
            return Err("There is at least one repository that does not exist or is not accessible to the parent installation.".to_owned());
        }
        let Some(permissions) = request
            .permissions
            .filter(|permissions| !permissions.is_empty())
        else {
            return Err(
                "fake-github: the token request must list read-only `permissions`; without \
                 them GitHub grants the App's write access"
                    .to_owned(),
            );
        };
        if !permissions
            .keys()
            .all(|name| self.granted.contains_key(name))
        {
            return Err(
                "The permissions requested are not granted to this installation.".to_owned(),
            );
        }
        if let Some((name, level)) = permissions.iter().find(|(_, level)| *level != "read") {
            return Err(format!(
                "fake-github: job tokens are read-only; `{name}: {level}` was requested"
            ));
        }
        Ok(permissions)
    }
}

/// The App's public key (PEM: `PUBLIC KEY` or `RSA PUBLIC KEY`).
fn public_key(file: &FilePath) -> Result<DecodingKey, StartError> {
    let pem = std::fs::read(file).map_err(|source| StartError::ReadAppKey {
        path: file.to_path_buf(),
        source,
    })?;
    DecodingKey::from_rsa_pem(&pem).map_err(|source| StartError::AppKey {
        path: file.to_path_buf(),
        source,
    })
}

/// The App routes.
pub(crate) fn routes(app: Arc<App>) -> Router {
    Router::new()
        .route("/repos/{owner}/{repo}/installation", get(installation))
        .route("/app/installations", get(installations))
        .route("/app/installations/{id}/access_tokens", post(access_token))
        .route("/installation/token", delete(revoke))
        .route("/_fake/app-credentials", get(credentials))
        .with_state(app)
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

async fn installations(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if let Some(refusal) = app.refuse(&headers) {
        return refusal;
    }
    let mut body = Vec::with_capacity(app.installation.len() + 2);
    body.push(b'[');
    body.extend_from_slice(&app.installation);
    body.push(b']');
    json_body(StatusCode::OK, Bytes::from(body))
}

async fn installation(
    State(app): State<Arc<App>>,
    Path((owner, repo)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Some(refusal) = app.refuse(&headers) {
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
    body: Bytes,
) -> Response {
    if let Some(refusal) = app.refuse(&headers) {
        return refusal;
    }
    if id != app.installation_id {
        return github_error(StatusCode::NOT_FOUND, "Not Found");
    }
    let permissions = match app.scope(&body) {
        Ok(permissions) => permissions,
        Err(message) => {
            tracing::warn!(%message, "fake-github refused an installation token request");
            return github_error(StatusCode::UNPROCESSABLE_ENTITY, &message);
        }
    };
    let mut token = app.token.clone();
    let value = {
        let mut issued = app.issued();
        let value = format!(
            "{}-{}",
            token["token"].as_str().unwrap_or("fake-installation-token"),
            issued.installation_tokens.len() + 1
        );
        issued.installation_tokens.push(value.clone());
        value
    };
    token["token"] = Value::String(value);
    token["permissions"] = serde_json::to_value(permissions).unwrap_or_default();
    token["expires_at"] = Value::String(format!(
        "{:.0}",
        (Timestamp::now() + SignedDuration::from_hours(1)).get()
    ));
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
    let presented = bearer(&headers).unwrap_or_default();
    let mut issued = app.issued();
    let minted = issued
        .installation_tokens
        .iter()
        .any(|token| token == presented);
    if minted && !issued.revoked.iter().any(|token| token == presented) {
        issued.revoked.push(presented.to_owned());
        tracing::info!("fake-github revoked an installation token");
        return json_body(StatusCode::NO_CONTENT, Bytes::new());
    }
    github_error(StatusCode::UNAUTHORIZED, "Bad credentials")
}

async fn credentials(State(app): State<Arc<App>>) -> Json<Issued> {
    Json(app.issued().clone())
}
