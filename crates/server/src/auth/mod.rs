//! GitHub App sign-in, cookie sessions and CSRF (ADR-007, threat model T7a).
//!
//! Protect handlers with an extractor matching the route's `RouteAuth`:
//! - [`Session`] for `session` routes: a valid session cookie, else 401 `unauthenticated`;
//! - [`SessionCsrf`] for `session_csrf` routes (every non-GET public route): additionally the
//!   `X-CSRF-Token` header must equal the session's CSRF token, else 403 `csrf_rejected`.
//!
//! The signed-in user's GitHub token comes from [`user_access_token`], which refreshes it when due.
//!
//! ```ignore
//! async fn create(
//!     State(state): State<AppState>,
//!     SessionCsrf(session): SessionCsrf, // before any body extractor
//! ) -> Result<Response, ApiFailure<UserTokenError>> {
//!     let token = user_access_token(&state, &session.user_id).await?; // tenant: session.tenant_id
//! }
//! ```

use std::borrow::Cow;
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRequestParts, OptionalFromRequestParts};
use axum::http::header::COOKIE;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue};
use chrono::{TimeDelta, Utc};
use contracts::api::error::ApiErrorCode;
use db::error::DbError;

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::config::Config;
use crate::secrets::{secrets_equal, sha256};
use crate::state::AppState;

pub mod oauth;
#[cfg(test)]
mod tests;
mod tokens;

pub use tokens::{UserTokenError, end_sign_in, store_sign_in_tokens, user_access_token};

/// Lifetime of a session (absolute; the user signs in again afterwards).
pub const SESSION_TTL: TimeDelta = TimeDelta::days(7);
/// Lifetime of an OAuth login between redirect to GitHub and callback.
pub const OAUTH_LOGIN_TTL: TimeDelta = TimeDelta::minutes(10);
/// Header carrying the CSRF token on mutating requests.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// A cookie set by this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cookie {
    /// The session token.
    Session,
    /// The OAuth `state` of a login in progress, binding the callback to the browser that started it.
    OauthState,
}

impl Cookie {
    /// Cookie name; `__Host-` prefixed when Secure, so no subdomain or plain-HTTP origin can set it.
    fn name(self, https: bool) -> &'static str {
        match (self, https) {
            (Self::Session, true) => "__Host-sr_session",
            (Self::Session, false) => "sr_session",
            (Self::OauthState, true) => "__Host-sr_oauth_state",
            (Self::OauthState, false) => "sr_oauth_state",
        }
    }

    /// The cookie's value in the request, if present.
    pub(crate) fn read(self, headers: &HeaderMap, https: bool) -> Option<String> {
        let name = self.name(https);
        headers
            .get_all(COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_owned())
    }

    /// `Set-Cookie` value: HttpOnly, SameSite=Lax, Path=/, Secure over HTTPS. `value` must be a cookie-safe
    /// token (base64url).
    pub(crate) fn set(self, value: &str, max_age: TimeDelta, https: bool) -> Option<HeaderValue> {
        let secure = if https { "; Secure" } else { "" };
        HeaderValue::from_str(&format!(
            "{}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{secure}",
            self.name(https),
            max_age.num_seconds()
        ))
        .ok()
    }

    /// `Set-Cookie` value removing the cookie.
    pub(crate) fn clear(self, https: bool) -> Option<HeaderValue> {
        self.set("", TimeDelta::zero(), https)
    }
}

/// How the browser reached the server: HTTPS when the public URL is https, or when a trusted reverse proxy
/// (`SR_TRUSTED_PROXIES`) forwarded the request with `X-Forwarded-Proto: https`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrowserTransport {
    /// Cookies are Secure and `__Host-` prefixed.
    pub https: bool,
}

impl BrowserTransport {
    fn of(parts: &Parts, config: &Config) -> Self {
        if config.public_url.scheme() == "https" {
            return Self { https: true };
        }
        let trusted_peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(peer)| peer.ip().to_canonical())
            .is_some_and(|ip| config.trusted_proxies.iter().any(|net| net.contains(&ip)));
        let forwarded_https = parts
            .headers
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .is_some_and(|proto| proto.trim().eq_ignore_ascii_case("https"));
        Self {
            https: trusted_peer && forwarded_https,
        }
    }
}

impl FromRequestParts<AppState> for BrowserTransport {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self::of(parts, &state.config))
    }
}

/// A signed-in browser session (`session` routes).
#[derive(Clone)]
pub struct Session {
    /// Signed-in user.
    pub user_id: String,
    /// Tenant of the session.
    pub tenant_id: String,
    csrf_token: String,
    token_hash: [u8; 32],
    transport: BrowserTransport,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("user_id", &self.user_id)
            .field("tenant_id", &self.tenant_id)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// The CSRF token the browser must echo in `X-CSRF-Token`.
    pub fn csrf_token(&self) -> &str {
        &self.csrf_token
    }

    pub(crate) fn token_hash(&self) -> &[u8] {
        &self.token_hash
    }

    pub(crate) fn transport(&self) -> BrowserTransport {
        self.transport
    }

    async fn load(parts: &Parts, state: &AppState) -> Result<Option<Self>, AuthError> {
        let transport = BrowserTransport::of(parts, &state.config);
        let Some(token) = Cookie::Session.read(&parts.headers, transport.https) else {
            return Ok(None);
        };
        let token_hash = sha256(token.as_bytes());
        let record = state.db.sessions().touch(&token_hash, Utc::now()).await?;
        Ok(record.map(|record| Self {
            user_id: record.user_id,
            tenant_id: record.tenant_id,
            csrf_token: record.csrf_token,
            token_hash,
            transport,
        }))
    }
}

impl FromRequestParts<AppState> for Session {
    type Rejection = ApiFailure<AuthError>;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Self::load(parts, state)
            .await?
            .ok_or(ApiFailure(AuthError::Unauthenticated))
    }
}

impl OptionalFromRequestParts<AppState> for Session {
    type Rejection = ApiFailure<AuthError>;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Option<Self>, Self::Rejection> {
        Ok(Self::load(parts, state).await?)
    }
}

/// A signed-in session whose request carries the matching `X-CSRF-Token` (`session_csrf` routes).
#[derive(Clone, Debug)]
pub struct SessionCsrf(pub Session);

impl FromRequestParts<AppState> for SessionCsrf {
    type Rejection = ApiFailure<AuthError>;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let session =
            <Session as FromRequestParts<AppState>>::from_request_parts(parts, state).await?;
        let presented = parts
            .headers
            .get(CSRF_HEADER)
            .and_then(|value| value.to_str().ok());
        match presented {
            Some(token) if secrets_equal(token, session.csrf_token()) => Ok(Self(session)),
            _ => Err(ApiFailure(AuthError::CsrfRejected)),
        }
    }
}

/// A request was refused by the session or CSRF check.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// No valid session cookie.
    #[error("the request has no valid session")]
    Unauthenticated,
    /// Missing or wrong `X-CSRF-Token`.
    #[error("the request has a missing or wrong CSRF token")]
    CsrfRejected,
    /// The session store failed.
    #[error("the session could not be loaded")]
    Db(#[from] DbError),
}

impl ApiErrorSource for AuthError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Unauthenticated => ApiErrorCode::Unauthenticated,
            Self::CsrfRejected => ApiErrorCode::CsrfRejected,
            Self::Db(error) => error.code(),
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Unauthenticated => Cow::Borrowed("Sign in to continue."),
            Self::CsrfRejected => {
                Cow::Borrowed("The request could not be verified. Reload the page and try again.")
            }
            Self::Db(error) => error.public_message(),
        }
    }
}
