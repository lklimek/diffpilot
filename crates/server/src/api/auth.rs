//! Sign-in, callback, logout and `/api/v1/me` (`docs/contracts/http-api.md`, ADR-007).

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Query, State};
use axum::http::header::{CACHE_CONTROL, LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use contracts::api::auth::{LogoutRequest, Me, MeResponse};
use contracts::api::error::LoginError;
use contracts::routes::{AUTH_CALLBACK, AUTH_LOGIN, AUTH_LOGOUT, GET_ME};
use db::error::DbError;
use db::repo::DEFAULT_TENANT;
use db::repo::sessions::NewSession;
use db::repo::tenants::LoginAdmission;
use db::repo::users::GithubProfile;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use url::Url;

use crate::api::error::ApiFailure;
use crate::auth::{
    BrowserTransport, Cookie, OAUTH_LOGIN_TTL, SESSION_TTL, Session, SessionCsrf, UserTokenError,
    end_session, end_sign_in, liveness, lock_user, store_sign_in,
};
use crate::config::Config;
use crate::github::oauth::{self, OauthError};
use crate::secrets::{Aad, SecretError, TokenCipher, random_token, secrets_equal, sha256};
use crate::state::AppState;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(AUTH_LOGIN.path, get(login))
        .route(AUTH_CALLBACK.path, get(callback))
        .route(AUTH_LOGOUT.path, post(logout))
        .route(GET_ME.path, get(me))
}

/// The exact OAuth callback URL registered on the GitHub App: `<SR_PUBLIC_URL>/auth/github/callback`.
fn callback_url(config: &Config) -> String {
    let mut url = config.public_url.clone();
    let path = format!(
        "{}{}",
        config.public_url.path().trim_end_matches('/'),
        AUTH_CALLBACK.path
    );
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    url.into()
}

fn verifier_aad(state: &str) -> Aad<'_> {
    Aad {
        table: "oauth_logins",
        column: "code_verifier_ct",
        row_id: state,
    }
}

fn redirect(location: &str, cookies: Vec<Option<HeaderValue>>) -> Response {
    let mut response = (
        StatusCode::FOUND,
        [(LOCATION, location), (CACHE_CONTROL, "no-store")],
    )
        .into_response();
    for cookie in cookies.into_iter().flatten() {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    response
}

fn login_error_location(error: LoginError) -> String {
    let code = serde_json::to_value(error)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "oauth_failed".to_owned());
    format!("/?login_error={code}")
}

/// Why a sign-in did not complete.
#[derive(Debug, thiserror::Error)]
enum SignInError {
    #[error(
        "the OAuth state is missing, unknown, expired, already used or not bound to this browser"
    )]
    StateMismatch,
    #[error("GitHub returned an authorisation error to the callback")]
    Denied,
    #[error("the callback carries no authorisation code")]
    MissingCode,
    #[error("the GitHub login '{login}' may not sign in ({admission:?})")]
    NotAllowed {
        login: String,
        admission: LoginAdmission,
    },
    #[error("the OAuth exchange with GitHub failed")]
    Oauth(#[from] OauthError),
    #[error("a sign-in secret could not be produced or read")]
    Secret(#[from] SecretError),
    #[error("the sign-in could not be stored")]
    Db(#[from] DbError),
    #[error("the GitHub tokens could not be stored")]
    Tokens(#[from] UserTokenError),
}

impl SignInError {
    fn login_error(&self) -> LoginError {
        match self {
            Self::StateMismatch => LoginError::StateMismatch,
            Self::NotAllowed { .. } => LoginError::LoginNotAllowed,
            Self::Denied
            | Self::MissingCode
            | Self::Oauth(_)
            | Self::Secret(_)
            | Self::Db(_)
            | Self::Tokens(_) => LoginError::OauthFailed,
        }
    }

    fn log(&self) {
        match self {
            Self::Secret(_) | Self::Db(_) | Self::Tokens(_) => {
                tracing::error!(error = ?self, "sign-in failed on the server");
            }
            Self::NotAllowed { .. } => {
                tracing::warn!(error = %self, "sign-in refused by the tenant allowlist");
            }
            Self::StateMismatch | Self::Denied | Self::MissingCode | Self::Oauth(_) => {
                tracing::warn!(error = ?self, "sign-in callback rejected");
            }
        }
    }
}

/// `GET /auth/github/login`: starts the web flow (state + PKCE S256) and redirects to GitHub.
async fn login(State(state): State<AppState>, transport: BrowserTransport) -> Response {
    match start_login(&state).await {
        Ok((oauth_state, authorize)) => redirect(
            authorize.as_str(),
            vec![Cookie::OauthState.set(&oauth_state, OAUTH_LOGIN_TTL, transport.https)],
        ),
        Err(error) => {
            error.log();
            redirect(&login_error_location(LoginError::OauthFailed), Vec::new())
        }
    }
}

async fn start_login(state: &AppState) -> Result<(String, Url), SignInError> {
    let now = Utc::now();
    let sessions = state.db.sessions();
    sessions.delete_expired(liveness(now)).await?;
    let oauth_state = random_token()?;
    let verifier = SecretString::from(random_token()?);
    let challenge = URL_SAFE_NO_PAD.encode(sha256(verifier.expose_secret().as_bytes()));
    let verifier_ct = TokenCipher::new(&state.config.secret_key)
        .encrypt(verifier_aad(&oauth_state), verifier.expose_secret())?;
    sessions
        .create_oauth_login(
            &oauth_state,
            DEFAULT_TENANT,
            &verifier_ct,
            now,
            now + OAUTH_LOGIN_TTL,
        )
        .await?;
    let authorize = oauth::authorize_url(
        &state.config.github,
        &callback_url(&state.config),
        &oauth_state,
        &challenge,
    );
    Ok((oauth_state, authorize))
}

/// Query of the OAuth callback.
#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// `GET /auth/github/callback`: completes the web flow; redirects to `/` or `/?login_error=<code>`.
async fn callback(
    State(state): State<AppState>,
    transport: BrowserTransport,
    headers: HeaderMap,
    query: Result<Query<CallbackQuery>, QueryRejection>,
) -> Response {
    let query = query.ok().map(|Query(query)| query);
    let clear_state = Cookie::OauthState.clear(transport.https);
    match complete_login(&state, transport, &headers, query).await {
        Ok(session_cookie) => redirect("/", vec![clear_state, session_cookie]),
        Err(error) => {
            error.log();
            redirect(
                &login_error_location(error.login_error()),
                vec![clear_state],
            )
        }
    }
}

/// Validates the callback and, for an allowed login, stores user, tokens and a new session; returns the
/// session cookie.
async fn complete_login(
    state: &AppState,
    transport: BrowserTransport,
    headers: &HeaderMap,
    query: Option<CallbackQuery>,
) -> Result<Option<HeaderValue>, SignInError> {
    let query = query.ok_or(SignInError::StateMismatch)?;
    let oauth_state = query.state.ok_or(SignInError::StateMismatch)?;
    let bound_state = Cookie::OauthState
        .read(headers, transport.https)
        .ok_or(SignInError::StateMismatch)?;
    if !secrets_equal(&bound_state, &oauth_state) {
        return Err(SignInError::StateMismatch);
    }
    let pending = state
        .db
        .sessions()
        .take_oauth_login(&oauth_state, Utc::now())
        .await?
        .ok_or(SignInError::StateMismatch)?;
    if query.error.is_some() {
        return Err(SignInError::Denied);
    }
    let code = query.code.ok_or(SignInError::MissingCode)?;
    let cipher = TokenCipher::new(&state.config.secret_key);
    let verifier = cipher.decrypt(verifier_aad(&oauth_state), &pending.code_verifier_ct)?;

    let github = &state.config.github;
    let grant =
        oauth::exchange_code(github, &code, &callback_url(&state.config), &verifier).await?;
    let github_user = oauth::fetch_user(github, &grant.access_token).await?;

    let now = Utc::now();
    let tenant_id = pending.tenant_id;
    let admission = state
        .db
        .tenants()
        .admit_login(&tenant_id, &github_user.login, github_user.id, now)
        .await?;
    if admission != LoginAdmission::Allowed {
        return Err(SignInError::NotAllowed {
            login: github_user.login,
            admission,
        });
    }
    let profile = GithubProfile {
        github_id: github_user.id,
        login: github_user.login,
        name: github_user.name,
        avatar_url: github_user.avatar_url,
    };
    let user = state
        .db
        .users()
        .upsert_member(&tenant_id, &uuid::Uuid::new_v4().to_string(), &profile, now)
        .await?;
    // Session and tokens are written together under the user's lock, never interleaved with a refresh or
    // sign-out; a failed write leaves neither.
    let lock = lock_user(&user.id).await;
    let session_token = random_token()?;
    let csrf_token = random_token()?;
    let session = NewSession {
        token_hash: &sha256(session_token.as_bytes()),
        tenant_id: &tenant_id,
        user_id: &user.id,
        csrf_token: &csrf_token,
        expires_at: now + SESSION_TTL,
    };
    store_sign_in(state, &lock, &session, &grant, now).await?;
    drop(lock);
    tracing::info!(login = %user.profile.login, user_id = %user.id, "user signed in");
    Ok(Cookie::Session.set(&session_token, SESSION_TTL, transport.https))
}

/// `POST /auth/logout`: ends this session, or with `everywhere` every session of the user; the stored GitHub
/// tokens end with the user's last live session.
async fn logout(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    body: Result<Json<LogoutRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match body {
        Ok(body) => body,
        Err(rejection) => return ApiFailure(rejection).into_response(),
    };
    let ended = if request.everywhere {
        end_sign_in(&state, &session.user_id).await
    } else {
        end_session(&state, &session).await
    };
    if let Err(error) = ended {
        return ApiFailure(error).into_response();
    }
    tracing::info!(user_id = %session.user_id, everywhere = request.everywhere, "user signed out");
    no_content(Cookie::Session.clear(session.transport().https))
}

fn no_content(cookie: Option<HeaderValue>) -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Some(cookie) = cookie {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    response
}

/// `GET /api/v1/me`: the signed-in user and CSRF token, or nulls when signed out.
async fn me(
    State(state): State<AppState>,
    session: Option<Session>,
) -> Result<Response, ApiFailure<DbError>> {
    let signed_out = MeResponse {
        user: None,
        csrf_token: None,
    };
    let body = match session {
        None => signed_out,
        Some(session) => match state.db.users().get(&session.user_id).await {
            Ok(user) => MeResponse {
                user: Some(Me {
                    login: user.profile.login,
                    name: user.profile.name,
                    avatar_url: user.profile.avatar_url,
                    language: user.language,
                }),
                csrf_token: Some(session.csrf_token().to_owned()),
            },
            Err(DbError::NotFound) => signed_out,
            Err(error) => return Err(error.into()),
        },
    };
    Ok(([(CACHE_CONTROL, "no-store")], Json(body)).into_response())
}
