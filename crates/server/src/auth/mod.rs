//! GitHub App sign-in, cookie sessions and CSRF (ADR-007, threat model T7a).
//!
//! Protect handlers with an extractor matching the route's `RouteAuth`:
//! - [`Session`] for `session` and `read` routes: a valid session cookie, or the [`TokenPrincipal`] the token
//!   check ([`access_tokens::check`], `read` routes only) admitted; else 401 `unauthenticated`;
//! - [`SessionCsrf`] for `session_csrf` routes (every non-GET public route): a session cookie (never a token)
//!   whose CSRF token the `X-CSRF-Token` header carries, else 403 `csrf_rejected`.
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
use contracts::api::access_tokens::AccessTokenId;
use contracts::api::error::ApiErrorCode;
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::sessions::Liveness;
use db::repo::tenants::AllowlistChange;
use db::repo::{DEFAULT_TENANT, Db};
use jiff::SignedDuration;

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::config::Config;
use crate::secrets::{secrets_equal, sha256};
use crate::state::AppState;

pub mod access_tokens;
#[cfg(test)]
mod tests;
mod tokens;

pub use access_tokens::{TokenGate, TokenPrincipal};

pub use tokens::{
    PendingPairs, UserLock, UserTokenError, end_sign_in, lock_user, store_sign_in_tokens,
    user_access_token, user_token_rejected,
};
pub(crate) use tokens::{end_session, store_sign_in};

/// Lifetime of a session (absolute; the user signs in again afterwards).
pub const SESSION_TTL: SignedDuration = SignedDuration::from_hours(24 * 7);
/// A session unused for this long ends (idle timeout).
pub const SESSION_IDLE_TIMEOUT: SignedDuration = SignedDuration::from_hours(24);
/// A use is recorded only when the last recorded one is older, so the idle timeout may end a session up to this
/// much early, never late.
pub const SESSION_TOUCH_INTERVAL: SignedDuration = SignedDuration::from_secs(60);
/// Lifetime of an OAuth login between redirect to GitHub and callback.
pub const OAUTH_LOGIN_TTL: SignedDuration = SignedDuration::from_mins(10);
/// Header carrying the CSRF token on mutating requests.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// Which sessions are live at `now`: unexpired, used within [`SESSION_IDLE_TIMEOUT`], and of a user the
/// session's tenant still allows (a login removed from the allowlist loses access on its next request).
pub(crate) fn liveness(now: Timestamp) -> Liveness {
    Liveness {
        now,
        seen_after: now - SESSION_IDLE_TIMEOUT,
    }
}

/// Makes the default tenant's sign-in allowlist equal `SR_ALLOWED_LOGINS` (ADR-007); run at start, before serving.
pub async fn apply_allowed_logins(db: &Db, logins: &[String]) -> Result<AllowlistChange, DbError> {
    let change = db
        .tenants()
        .reconcile_logins(DEFAULT_TENANT, logins, Timestamp::now())
        .await?;
    tracing::info!(
        added = change.added.len(),
        removed = change.removed.len(),
        added_logins = ?change.added,
        removed_logins = ?change.removed,
        "sign-in allowlist set from SR_ALLOWED_LOGINS"
    );
    Ok(change)
}

/// Logs a start-up warning when `SR_PUBLIC_URL` is plain HTTP on a host other than localhost: cookies then lack
/// Secure unless a trusted proxy forwards HTTPS. Returns whether it warned.
pub fn warn_if_public_url_insecure(config: &Config) -> bool {
    let url = &config.public_url;
    let local = match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    let insecure = url.scheme() == "http" && !local;
    if insecure {
        tracing::warn!(
            public_url = %url,
            "SR_PUBLIC_URL is plain http on a non-local host: session cookies are not Secure unless a trusted proxy forwards https; use an https public URL in production"
        );
    }
    insecure
}

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
    pub(crate) fn set(
        self,
        value: &str,
        max_age: SignedDuration,
        https: bool,
    ) -> Option<HeaderValue> {
        let secure = if https { "; Secure" } else { "" };
        HeaderValue::from_str(&format!(
            "{}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{secure}",
            self.name(https),
            max_age.as_secs()
        ))
        .ok()
    }

    /// `Set-Cookie` value removing the cookie.
    pub(crate) fn clear(self, https: bool) -> Option<HeaderValue> {
        self.set("", SignedDuration::ZERO, https)
    }
}

/// How the browser reached the server: HTTPS when the public URL is https, or when a trusted reverse proxy
/// (`SR_TRUSTED_PROXIES`, matched against the TCP peer) forwarded the request with `X-Forwarded-Proto: https`.
/// No other forwarding header is read; redirect and callback URLs come from `SR_PUBLIC_URL` only.
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

/// A signed-in browser session, or on `read` routes a personal access token speaking for its user.
#[derive(Clone)]
pub struct Session {
    /// Signed-in user.
    pub user_id: String,
    /// Tenant of the session.
    pub tenant_id: String,
    credential: Credential,
    /// SHA-256 of the presented credential (session token or access token).
    token_hash: [u8; 32],
    transport: BrowserTransport,
}

/// How the request authenticated.
#[derive(Clone)]
enum Credential {
    /// A session cookie with its CSRF token.
    Cookie { csrf_token: String },
    /// A personal access token (ADR-019).
    AccessToken(AccessTokenId),
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
    /// The CSRF token the browser must echo in `X-CSRF-Token`; `None` for an access token.
    pub fn csrf_token(&self) -> Option<&str> {
        match &self.credential {
            Credential::Cookie { csrf_token } => Some(csrf_token),
            Credential::AccessToken(_) => None,
        }
    }

    /// Whether the request authenticated with a personal access token.
    pub(crate) fn is_access_token(&self) -> bool {
        matches!(self.credential, Credential::AccessToken(_))
    }

    /// Whether the session or token still passes the checks of a new request (SSE heartbeat): the session is
    /// live, or the token is neither revoked nor expired; either way the user is still on the allow-list.
    pub(crate) async fn is_live(&self, state: &AppState) -> Result<bool, DbError> {
        let now = Timestamp::now();
        match &self.credential {
            Credential::Cookie { .. } => Ok(state
                .db
                .sessions()
                .live(&self.token_hash, liveness(now))
                .await?
                .is_some()),
            Credential::AccessToken(id) => access_tokens::token_live(&state.db, id, now).await,
        }
    }

    fn of_token(principal: &TokenPrincipal, transport: BrowserTransport) -> Self {
        Self {
            user_id: principal.user_id.clone(),
            tenant_id: principal.tenant_id.clone(),
            credential: Credential::AccessToken(principal.token_id.clone()),
            token_hash: principal.token_hash,
            transport,
        }
    }

    /// The token's principal if the token check admitted one, else the session cookie's session.
    async fn load_any(parts: &Parts, state: &AppState) -> Result<Option<Self>, AuthError> {
        match parts.extensions.get::<TokenPrincipal>() {
            Some(principal) => Ok(Some(Self::of_token(
                principal,
                BrowserTransport::of(parts, &state.config),
            ))),
            None => Self::load(parts, state).await,
        }
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
        let now = Timestamp::now();
        let sessions = state.db.sessions();
        // Validity is decided by this read alone; the use record below is best-effort.
        let record = sessions.live(&token_hash, liveness(now)).await?;
        let seen_before = now - SESSION_TOUCH_INTERVAL;
        if let Some(record) = &record
            && record.last_seen_at < seen_before
            && let Err(error) = sessions
                .touch(&token_hash, liveness(now), seen_before)
                .await
        {
            tracing::warn!(
                sqlite_code = ?error.sqlite_code(),
                ?error,
                "the use of a session could not be recorded (database busy?); the request proceeds and the next \
                 use records it"
            );
        }
        Ok(record.map(|record| Self {
            user_id: record.user_id,
            tenant_id: record.tenant_id,
            credential: Credential::Cookie {
                csrf_token: record.csrf_token,
            },
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
        Self::load_any(parts, state)
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
        Ok(Self::load_any(parts, state).await?)
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
        // The cookie only: a token principal never authorises a mutation.
        let session = Session::load(parts, state)
            .await?
            .ok_or(ApiFailure(AuthError::Unauthenticated))?;
        let presented = parts
            .headers
            .get(CSRF_HEADER)
            .and_then(|value| value.to_str().ok());
        match (presented, session.csrf_token()) {
            (Some(token), Some(expected)) if secrets_equal(token, expected) => Ok(Self(session)),
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
