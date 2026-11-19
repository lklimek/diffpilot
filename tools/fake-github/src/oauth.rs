//! OAuth web flow (`/login/oauth/authorize`, `/login/oauth/access_token`) and `GET /user` for the fake user;
//! `POST /_fake/user` makes later sign-ins another user (multi-user tests).
//! Behaviour follows spike S4: PKCE S256, exact callback URL, refresh rotation that invalidates the old pair,
//! OAuth errors as HTTP 200 with an `error` field.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::header::{ACCEPT, AUTHORIZATION, LOCATION};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use contracts::routes::AUTH_CALLBACK;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::fixtures::{ACCESS_TOKEN, RawRecording, Recording, USER, load};
use crate::{Config, StartError, github_error, json_body, require_user_agent};

const TROUBLESHOOTING_URI: &str = "https://docs.github.com/apps/managing-oauth-apps/troubleshooting-oauth-app-access-token-request-errors/";

/// Client credentials and first authorisation code of the recorded exchange.
#[derive(Deserialize)]
struct TokenRequestBody {
    client_id: String,
    client_secret: String,
    code: String,
}

/// Token response; the recorded one is the template of every grant.
#[derive(Clone, Deserialize, Serialize)]
struct TokenGrant {
    access_token: String,
    expires_in: i64,
    refresh_token: String,
    refresh_token_expires_in: i64,
    scope: String,
    token_type: String,
}

/// Codes and tokens issued so far.
#[derive(Default)]
struct Issued {
    codes: u64,
    grants: u64,
    /// Open authorisation codes and their PKCE challenge.
    pending: HashMap<String, Option<String>>,
    /// Valid refresh token → its access token.
    refresh: HashMap<String, String>,
    /// Valid access tokens.
    access: HashSet<String>,
    /// The `GET /user` body of later code exchanges, once `POST /_fake/user` set one.
    next_user: Option<Bytes>,
    /// Access token → its `GET /user` body, for tokens not of the configured user.
    users: HashMap<String, Bytes>,
}

/// Body of `POST /_fake/user`: who signs in from now on.
#[derive(Deserialize)]
struct Identity {
    login: String,
    id: i64,
}

/// What a request's `Authorization` header amounts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Credentials {
    /// No `Authorization` header.
    Anonymous,
    /// A currently valid access token.
    Valid,
    /// Malformed, unknown or rotated-out token.
    Invalid,
}

pub(crate) struct Oauth {
    client_id: String,
    client_secret: String,
    first_code: String,
    callback: Url,
    grant: TokenGrant,
    user: Bytes,
    issued: Mutex<Issued>,
}

/// Value number `n` of a deterministic sequence: the recorded value first, then `<value>-<n>`.
fn nth(value: &str, n: u64) -> String {
    if n == 1 {
        value.to_owned()
    } else {
        format!("{value}-{n}")
    }
}

impl Oauth {
    pub(crate) fn load(config: &Config) -> Result<Self, StartError> {
        let token: Recording<TokenRequestBody, TokenGrant> =
            load(&config.fixture_dir, ACCESS_TOKEN)?;
        let user: RawRecording = load(&config.fixture_dir, USER)?;
        let user = if config.user_login.is_none() && config.user_id.is_none() {
            user.response.get().to_owned()
        } else {
            let mut profile: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(user.response.get()).map_err(|source| {
                    StartError::ParseFixture {
                        path: config.fixture_dir.join(USER),
                        source,
                    }
                })?;
            if let Some(login) = &config.user_login {
                profile.insert("login".to_owned(), login.clone().into());
            }
            if let Some(id) = config.user_id {
                profile.insert("id".to_owned(), id.into());
            }
            serde_json::Value::Object(profile).to_string()
        };
        let mut callback = config.app_url.clone();
        let path = format!(
            "{}{}",
            config.app_url.path().trim_end_matches('/'),
            AUTH_CALLBACK.path
        );
        callback.set_path(&path);
        callback.set_query(None);
        callback.set_fragment(None);
        Ok(Self {
            client_id: token.request.body.client_id,
            client_secret: token.request.body.client_secret,
            first_code: token.request.body.code,
            callback,
            grant: token.response,
            user: Bytes::from(user),
            issued: Mutex::default(),
        })
    }

    /// The request's `Authorization` against the currently valid access tokens.
    pub(crate) fn credentials(&self, headers: &HeaderMap) -> Credentials {
        if !headers.contains_key(AUTHORIZATION) {
            return Credentials::Anonymous;
        }
        if bearer(headers).is_some_and(|token| self.issued().access.contains(token)) {
            Credentials::Valid
        } else {
            Credentials::Invalid
        }
    }

    fn issued(&self) -> MutexGuard<'_, Issued> {
        self.issued.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Issues the next token pair, of `user` (a `GET /user` body) or else the configured user.
    fn mint(&self, issued: &mut Issued, user: Option<Bytes>) -> TokenGrant {
        issued.grants += 1;
        let grant = TokenGrant {
            access_token: nth(&self.grant.access_token, issued.grants),
            refresh_token: nth(&self.grant.refresh_token, issued.grants),
            ..self.grant.clone()
        };
        issued
            .refresh
            .insert(grant.refresh_token.clone(), grant.access_token.clone());
        issued.access.insert(grant.access_token.clone());
        if let Some(user) = user {
            issued.users.insert(grant.access_token.clone(), user);
        }
        grant
    }
}

/// The token of a `Bearer` or `token` `Authorization` header.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("token "))
}

pub(crate) fn routes(oauth: Arc<Oauth>) -> Router {
    Router::new()
        .route("/login/oauth/authorize", get(authorize))
        .route("/login/oauth/access_token", post(access_token))
        .route("/user", get(user))
        .route("/_fake/user", post(set_user))
        .with_state(oauth)
}

fn found(location: &Url) -> Response {
    (StatusCode::FOUND, [(LOCATION, location.as_str())]).into_response()
}

/// OAuth error the GitHub way: HTTP 200 with `error`.
fn oauth_error(error: &str, description: &str) -> Response {
    Json(serde_json::json!({
        "error": error,
        "error_description": description,
        "error_uri": format!("{TROUBLESHOOTING_URI}#{}", error.replace('_', "-")),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct AuthorizeQuery {
    client_id: Option<String>,
    redirect_uri: Option<String>,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
}

/// Grants at once (the user is always signed in and consenting) and redirects to the callback.
async fn authorize(
    State(oauth): State<Arc<Oauth>>,
    Query(query): Query<AuthorizeQuery>,
) -> Response {
    if query.client_id.as_deref() != Some(oauth.client_id.as_str()) {
        tracing::warn!(
            expected = %oauth.client_id,
            "fake-github authorize: unknown client_id; set SR_GITHUB_CLIENT_ID to the recorded client id"
        );
        return (StatusCode::NOT_FOUND, "unknown OAuth client_id").into_response();
    }
    let mut location = oauth.callback.clone();
    if let Some(uri) = &query.redirect_uri
        && uri != oauth.callback.as_str()
    {
        tracing::warn!(
            %uri,
            callback = %oauth.callback,
            "fake-github authorize: redirect_uri is not the registered callback; check --app-url"
        );
        let mut pairs = location.query_pairs_mut();
        pairs
            .append_pair("error", "redirect_uri_mismatch")
            .append_pair(
                "error_description",
                "The redirect_uri MUST match the registered callback URL for this application.",
            )
            .append_pair("error_uri", TROUBLESHOOTING_URI);
        if let Some(state) = &query.state {
            pairs.append_pair("state", state);
        }
        drop(pairs);
        return found(&location);
    }
    let pkce_valid = matches!(
        (
            &query.code_challenge,
            query.code_challenge_method.as_deref()
        ),
        (None, None) | (Some(_), Some("S256"))
    );
    if !pkce_valid {
        tracing::debug!("fake-github authorize with incomplete or non-S256 PKCE parameters");
        return (
            StatusCode::BAD_REQUEST,
            "send code_challenge with code_challenge_method=S256, or neither",
        )
            .into_response();
    }
    let code = {
        let mut issued = oauth.issued();
        issued.codes += 1;
        let code = nth(&oauth.first_code, issued.codes);
        issued.pending.insert(code.clone(), query.code_challenge);
        code
    };
    tracing::info!("fake-github issued an authorization code");
    let mut pairs = location.query_pairs_mut();
    pairs.append_pair("code", &code);
    if let Some(state) = &query.state {
        pairs.append_pair("state", state);
    }
    drop(pairs);
    found(&location)
}

#[derive(Deserialize)]
struct TokenForm {
    client_id: Option<String>,
    client_secret: Option<String>,
    grant_type: Option<String>,
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
}

/// Code exchange (with PKCE) and refresh-token rotation.
async fn access_token(
    State(oauth): State<Arc<Oauth>>,
    headers: HeaderMap,
    Form(form): Form<TokenForm>,
) -> Response {
    let accepts_json = headers
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("application/json"));
    if !accepts_json {
        tracing::warn!(
            "fake-github token request without Accept: application/json; GitHub would answer form-encoded"
        );
        return (
            StatusCode::NOT_ACCEPTABLE,
            "send Accept: application/json; only JSON token responses are faked",
        )
            .into_response();
    }
    if form.client_id.as_deref() != Some(oauth.client_id.as_str())
        || form.client_secret.as_deref() != Some(oauth.client_secret.as_str())
    {
        tracing::debug!("fake-github token request with wrong client credentials");
        return oauth_error(
            "incorrect_client_credentials",
            "The client_id and/or client_secret passed are incorrect.",
        );
    }
    match form.grant_type.as_deref() {
        None | Some("authorization_code") => exchange_code(&oauth, &form),
        Some("refresh_token") => refresh(&oauth, form.refresh_token.as_deref()),
        Some(_) => oauth_error("unsupported_grant_type", "The grant type is not supported."),
    }
}

fn exchange_code(oauth: &Oauth, form: &TokenForm) -> Response {
    if let Some(uri) = &form.redirect_uri
        && uri != oauth.callback.as_str()
    {
        tracing::debug!(%uri, "fake-github code exchange with a foreign redirect_uri");
        return oauth_error(
            "redirect_uri_mismatch",
            "The redirect_uri MUST match the registered callback URL for this application.",
        );
    }
    let mut issued = oauth.issued();
    let Some((code, challenge)) = form
        .code
        .as_ref()
        .and_then(|code| Some((code, issued.pending.get(code)?.clone())))
    else {
        tracing::debug!("fake-github code exchange with an unknown or used code");
        return bad_verification_code();
    };
    if let Some(challenge) = challenge {
        let matches = form.code_verifier.as_ref().is_some_and(|verifier| {
            URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) == challenge
        });
        if !matches {
            // GitHub documents no separate PKCE error; a failed check leaves the code usable.
            tracing::debug!("fake-github code exchange with a wrong PKCE verifier");
            return bad_verification_code();
        }
    }
    issued.pending.remove(code);
    let user = issued.next_user.clone();
    let grant = oauth.mint(&mut issued, user);
    tracing::info!("fake-github exchanged a code for a token pair");
    Json(grant).into_response()
}

fn bad_verification_code() -> Response {
    oauth_error(
        "bad_verification_code",
        "The code passed is incorrect or expired.",
    )
}

fn refresh(oauth: &Oauth, refresh_token: Option<&str>) -> Response {
    let mut issued = oauth.issued();
    let Some(access) = refresh_token.and_then(|token| issued.refresh.remove(token)) else {
        tracing::debug!("fake-github refresh with an unknown or rotated refresh token");
        return oauth_error(
            "bad_refresh_token",
            "The refresh token passed is incorrect or expired.",
        );
    };
    issued.access.remove(&access);
    let user = issued.users.remove(&access);
    let grant = oauth.mint(&mut issued, user);
    tracing::info!("fake-github rotated a token pair");
    Json(grant).into_response()
}

/// The fake user, for a currently valid access token.
async fn user(State(oauth): State<Arc<Oauth>>, headers: HeaderMap) -> Response {
    if let Some(rejection) = require_user_agent(&headers) {
        return rejection;
    }
    if oauth.credentials(&headers) != Credentials::Valid {
        tracing::debug!("fake-github GET /user with a missing, unknown or rotated token");
        return github_error(StatusCode::UNAUTHORIZED, "Bad credentials");
    }
    let body = bearer(&headers)
        .and_then(|token| oauth.issued().users.get(token).cloned())
        .unwrap_or_else(|| oauth.user.clone());
    json_body(StatusCode::OK, body)
}

/// Test control: later sign-ins are `login`/`id` (the configured profile otherwise); earlier tokens keep their user.
async fn set_user(State(oauth): State<Arc<Oauth>>, Json(identity): Json<Identity>) -> Response {
    let Ok(serde_json::Value::Object(mut profile)) = serde_json::from_slice(&oauth.user) else {
        tracing::warn!("fake-github cannot switch users: the recorded user is not a JSON object");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    profile.insert("login".to_owned(), identity.login.clone().into());
    profile.insert("id".to_owned(), identity.id.into());
    oauth.issued().next_user = Some(Bytes::from(serde_json::Value::Object(profile).to_string()));
    tracing::info!(login = %identity.login, "fake-github signs in another user from now on");
    StatusCode::NO_CONTENT.into_response()
}
