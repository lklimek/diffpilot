//! Personal access tokens (ADR-019, threat model T20): issuing `srp_<id>_<secret>`, and the check on the public
//! API router that turns `Authorization: Bearer srp_…` into a [`TokenPrincipal`] for the session extractor.
//!
//! The check runs after routing ([`axum::Router::route_layer`]), so it knows the matched route and decides from
//! [`ROUTES`] alone: a token is accepted only on a `GET` route marked `SessionOrToken`, on a route marked
//! `TokenOnly` (the decision feed's claim, report and push-target routes, ADR-023) with that route's scope, and
//! identifies its user on `GET /api/v1/me`; every other route answers 403 `token_not_accepted` before its handler
//! runs. Each request
//! carrying a bearer token writes one `info` line on target `audit` (token id, user id, method, route template,
//! status, duration); never the token, its hash, other headers or the query string.

use std::any::Any;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, MatchedPath, Request, State};
use axum::http::header::{AUTHORIZATION, RETRY_AFTER, WWW_AUTHENTICATE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use contracts::api::access_tokens::{AccessTokenId, AccessTokenSecret, TokenScope};
use contracts::api::error::ApiErrorCode;
use contracts::routes::{GET_ME, HttpMethod, Listener, ROUTES, RouteAuth};
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::Db;
use db::repo::access_tokens::{TOUCH_INTERVAL, TokenCredential};
use subtle::ConstantTimeEq;

use super::{Cookie, Session};
use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::config::Config;
use crate::secrets::{SecretError, random_token, sha256};
use crate::state::AppState;

/// Start of every personal access token; tells it apart from runner bearers and GitHub tokens.
pub const TOKEN_PREFIX: &str = "srp_";
/// Length of the public id (base62).
const ID_LEN: usize = 8;
/// Length of the secret: 32 random bytes (256 bits) from the OS generator, base64url without padding.
const SECRET_LEN: usize = 43;
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Accepted requests per token and minute.
pub const REQUESTS_PER_TOKEN_PER_MINUTE: u32 = 120;
/// Failed bearer attempts per client address and minute; a failed attempt past it answers 429 instead of 401.
/// Malformed tokens are refused before any lookup; a well-formed one is looked up by id and compared in constant
/// time first, so a valid token is never refused by this limit.
pub const FAILED_ATTEMPTS_PER_CLIENT_PER_MINUTE: u32 = 30;
const WINDOW: Duration = Duration::from_secs(60);
/// Counters kept per limiter; beyond this, ended windows are dropped and new clients share one counter.
const MAX_TRACKED: usize = 4096;
/// Query parameter of RFC 6750 §2.3 (token in the URI), refused so a token never lands in a URL or a log.
const QUERY_TOKEN_PARAMETER: &str = "access_token";

/// A newly issued token: shown once, stored only as its hash.
pub(crate) struct IssuedToken {
    /// Public id.
    pub(crate) id: AccessTokenId,
    /// The whole token `srp_<id>_<secret>`.
    pub(crate) token: AccessTokenSecret,
    /// SHA-256 of the whole token.
    pub(crate) hash: [u8; 32],
}

/// A fresh token from the OS random generator: an 8-character base62 id (about 47.6 bits, public) and a
/// 256-bit secret.
pub(crate) fn issue() -> Result<IssuedToken, SecretError> {
    let mut id = String::with_capacity(ID_LEN);
    while id.len() < ID_LEN {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(SecretError::Random)?;
        // Rejection sampling keeps every character equally likely (248 = 4 × 62).
        for byte in bytes.into_iter().filter(|byte| *byte < 248) {
            if id.len() < ID_LEN {
                id.push(char::from(BASE62[usize::from(byte % 62)]));
            }
        }
    }
    let token = format!("{TOKEN_PREFIX}{id}_{}", random_token()?);
    Ok(IssuedToken {
        hash: sha256(token.as_bytes()),
        id: AccessTokenId(id),
        token: AccessTokenSecret(token),
    })
}

/// Whether `id` has the form of a token id (8 base62 characters).
pub(crate) fn is_token_id(id: &str) -> bool {
    id.len() == ID_LEN && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// The id of a well-formed token `srp_<id>_<secret>`; `None` for anything else.
fn token_id(presented: &str) -> Option<AccessTokenId> {
    let rest = presented.strip_prefix(TOKEN_PREFIX)?;
    let (id, secret) = (rest.get(..ID_LEN)?, rest.get(ID_LEN..)?);
    let secret = secret.strip_prefix('_')?;
    let well_formed = is_token_id(id)
        && secret.len() == SECRET_LEN
        && secret
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    well_formed.then(|| AccessTokenId(id.to_owned()))
}

/// The user a valid personal access token speaks for; inserted by [`check`] for the session extractor.
#[derive(Clone)]
pub struct TokenPrincipal {
    /// The token's public id.
    pub token_id: AccessTokenId,
    /// The token's tenant.
    pub tenant_id: String,
    /// The token's user.
    pub user_id: String,
    /// The token's scopes.
    pub scopes: Vec<TokenScope>,
    pub(crate) token_hash: [u8; 32],
}

impl fmt::Debug for TokenPrincipal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenPrincipal")
            .field("token_id", &self.token_id)
            .field("tenant_id", &self.tenant_id)
            .field("user_id", &self.user_id)
            .field("scopes", &self.scopes)
            .finish_non_exhaustive()
    }
}

/// The access token of a `TokenOnly` route (ADR-023): admitted by [`check`] with the route's scope. A session
/// cookie never passes: without a bearer token the route answers 401.
#[derive(Clone, Debug)]
pub struct TokenOnly(pub TokenPrincipal);

impl axum::extract::FromRequestParts<AppState> for TokenOnly {
    type Rejection = TokenAuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<TokenPrincipal>()
            .cloned()
            .map(Self)
            .ok_or(TokenAuthError::Missing)
    }
}

/// Whether the token still passes the request check apart from its secret: not revoked, not expired, and its
/// user on the tenant's allow-list (the SSE heartbeat re-check).
pub(crate) async fn token_live(
    db: &Db,
    id: &AccessTokenId,
    now: Timestamp,
) -> Result<bool, DbError> {
    Ok(db
        .access_tokens()
        .credential(id)
        .await?
        .is_some_and(|credential| usable(&credential, now)))
}

fn usable(credential: &TokenCredential, now: Timestamp) -> bool {
    credential.record.revoked_at.is_none() && credential.record.expires_at > now
}

/// What a token may do on a request, decided from the matched route in [`ROUTES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TokenUse {
    /// A read route (`SessionOrToken`) needing this scope.
    Read(TokenScope),
    /// A `TokenOnly` route needing this scope, any method.
    Execute(TokenScope),
    /// `GET /api/v1/me`: names the token's user.
    Identify,
    /// Any other route or method.
    Refused,
}

fn token_use(method: &Method, matched: Option<&str>) -> TokenUse {
    let method = match *method {
        Method::GET | Method::HEAD => HttpMethod::Get,
        Method::POST => HttpMethod::Post,
        Method::PUT => HttpMethod::Put,
        Method::PATCH => HttpMethod::Patch,
        Method::DELETE => HttpMethod::Delete,
        _ => return TokenUse::Refused,
    };
    let Some(route) = matched.and_then(|path| {
        ROUTES.iter().find(|route| {
            route.listener == Listener::Public && route.method == method && route.path == path
        })
    }) else {
        return TokenUse::Refused;
    };
    match route.auth {
        RouteAuth::SessionOrToken(scope) if route.method == HttpMethod::Get => {
            TokenUse::Read(scope)
        }
        RouteAuth::TokenOnly(scope) => TokenUse::Execute(scope),
        RouteAuth::Public if *route == GET_ME => TokenUse::Identify,
        RouteAuth::SessionOrToken(_)
        | RouteAuth::Public
        | RouteAuth::Session
        | RouteAuth::SessionCsrf
        | RouteAuth::RunnerSecret
        | RouteAuth::JobToken
        | RouteAuth::GithubSignature => TokenUse::Refused,
    }
}

/// A request with a bearer token was refused.
#[derive(Debug, thiserror::Error)]
pub enum TokenAuthError {
    /// The request carries a session cookie as well.
    #[error("the request carries both a session cookie and a bearer token")]
    CookieAndToken,
    /// A token in the query string.
    #[error("the request carries an access token in its query string")]
    TokenInQuery,
    /// The route or method does not accept tokens.
    #[error("the route does not accept access tokens")]
    NotAccepted,
    /// A `TokenOnly` route without a bearer token.
    #[error("the route needs an access token")]
    Missing,
    /// Malformed, unknown, wrong, expired or revoked token, or its user is off the allow-list.
    #[error("the bearer token is not a valid access token")]
    Invalid,
    /// Valid, without the route's scope.
    #[error("the access token lacks the route's scope")]
    InsufficientScope,
    /// Over a request limit.
    #[error("too many access token requests; retry after {retry_after_seconds} s")]
    RateLimited {
        /// Seconds until the limit's window ends.
        retry_after_seconds: u64,
    },
    /// The token store failed.
    #[error("the access token could not be checked")]
    Db(#[from] DbError),
}

impl ApiErrorSource for TokenAuthError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::CookieAndToken | Self::TokenInQuery => ApiErrorCode::InvalidRequest,
            Self::NotAccepted => ApiErrorCode::TokenNotAccepted,
            Self::Invalid | Self::Missing => ApiErrorCode::Unauthenticated,
            Self::InsufficientScope => ApiErrorCode::InsufficientScope,
            Self::RateLimited { .. } => ApiErrorCode::RateLimited,
            Self::Db(error) => error.code(),
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        Cow::Borrowed(match self {
            Self::CookieAndToken => "Send either a session cookie or an access token, not both.",
            Self::TokenInQuery => {
                "Send the access token in the Authorization header, never in the URL."
            }
            Self::NotAccepted => "This request does not accept access tokens.",
            Self::Missing => "This request needs an access token in the Authorization header.",
            Self::Invalid => "The access token is not valid.",
            Self::InsufficientScope => "The access token does not allow this request.",
            Self::RateLimited { .. } => "Too many requests. Try again later.",
            Self::Db(error) => return error.public_message(),
        })
    }
}

impl IntoResponse for TokenAuthError {
    fn into_response(self) -> Response {
        let header = match &self {
            Self::Invalid | Self::Missing => {
                Some((WWW_AUTHENTICATE, HeaderValue::from_static("Bearer")))
            }
            Self::RateLimited {
                retry_after_seconds,
            } => Some((RETRY_AFTER, HeaderValue::from(*retry_after_seconds))),
            _ => None,
        };
        let mut response = ApiFailure(self).into_response();
        if let Some((name, value)) = header {
            response.headers_mut().insert(name, value);
        }
        response
    }
}

/// Request counters of one minute per key.
#[derive(Debug)]
struct Limiter<K> {
    limit: u32,
    table: Mutex<Table<K>>,
}

#[derive(Debug)]
struct Table<K> {
    windows: HashMap<K, Window>,
    /// Last drop of ended windows, at most one scan a second.
    pruned_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug)]
struct Window {
    started: Instant,
    count: u32,
}

impl<K: Eq + Hash + Clone> Limiter<K> {
    fn new(limit: u32) -> Self {
        Self {
            limit,
            table: Mutex::new(Table {
                windows: HashMap::new(),
                pruned_at: None,
            }),
        }
    }

    /// Counts one event of `key` and decides under one lock; `Err(seconds to wait)` when it is over the limit.
    /// Once [`MAX_TRACKED`] keys have open windows, a new key counts on the shared `overflow` key.
    fn hit(&self, key: K, overflow: K, now: Instant) -> Result<(), u64> {
        let mut table = self.table.lock().unwrap_or_else(PoisonError::into_inner);
        let Table { windows, pruned_at } = &mut *table;
        let full = |windows: &HashMap<K, Window>| {
            windows.len() >= MAX_TRACKED && !windows.contains_key(&key)
        };
        if full(windows)
            && pruned_at.is_none_or(|at| now.duration_since(at) >= Duration::from_secs(1))
        {
            windows.retain(|_, window| now.duration_since(window.started) < WINDOW);
            *pruned_at = Some(now);
        }
        let key = if full(windows) { overflow } else { key };
        let window = windows.entry(key).or_insert(Window {
            started: now,
            count: 0,
        });
        if now.duration_since(window.started) >= WINDOW {
            *window = Window {
                started: now,
                count: 0,
            };
        }
        if window.count >= self.limit {
            return Err(retry_after(window, now));
        }
        window.count += 1;
        Ok(())
    }
}

fn retry_after(window: &Window, now: Instant) -> u64 {
    WINDOW
        .saturating_sub(now.duration_since(window.started))
        .as_secs()
        .max(1)
}

/// State of the token check: the application state and the in-process rate limits (one server process owns
/// the database, as for the per-user locks).
#[derive(Clone)]
pub struct TokenGate {
    state: AppState,
    limits: Arc<Limits>,
}

#[derive(Debug)]
struct Limits {
    tokens: Limiter<AccessTokenId>,
    /// Keyed by client address (IPv6: its /64); `None` = unknown address or the shared overflow counter.
    failures: Limiter<Option<IpAddr>>,
}

impl fmt::Debug for TokenGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenGate").finish_non_exhaustive()
    }
}

impl TokenGate {
    /// A gate with fresh limits; build one per router.
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            limits: Arc::new(Limits {
                tokens: Limiter::new(REQUESTS_PER_TOKEN_PER_MINUTE),
                failures: Limiter::new(FAILED_ATTEMPTS_PER_CLIENT_PER_MINUTE),
            }),
        }
    }

    /// Checks the bearer token of `parts` for its route; on success the principal it speaks for.
    async fn admit(
        &self,
        parts: &Parts,
        audit: &mut AuditIds,
    ) -> Result<TokenPrincipal, TokenAuthError> {
        if Cookie::Session.read(&parts.headers, true).is_some()
            || Cookie::Session.read(&parts.headers, false).is_some()
        {
            return Err(TokenAuthError::CookieAndToken);
        }
        let matched = parts
            .extensions
            .get::<MatchedPath>()
            .map(MatchedPath::as_str);
        let needed = match token_use(&parts.method, matched) {
            TokenUse::Refused => return Err(TokenAuthError::NotAccepted),
            TokenUse::Read(scope) | TokenUse::Execute(scope) => Some(scope),
            TokenUse::Identify => None,
        };
        // Authentication first: a valid token is never refused by the failure limit of its address, which only
        // counts and throttles failures. Malformed tokens never reach the database; a well-formed one costs one
        // primary-key read, as a session cookie does.
        let Some((credential, principal)) = self.authenticate(parts, audit).await? else {
            let client = client_address(parts, &self.state.config);
            return Err(
                match self.limits.failures.hit(client, None, Instant::now()) {
                    Ok(()) => TokenAuthError::Invalid,
                    Err(retry_after_seconds) => TokenAuthError::RateLimited {
                        retry_after_seconds,
                    },
                },
            );
        };
        if needed.is_some_and(|scope| !credential.record.scopes.contains(&scope)) {
            return Err(TokenAuthError::InsufficientScope);
        }
        self.limits
            .tokens
            .hit(
                principal.token_id.clone(),
                principal.token_id.clone(),
                Instant::now(),
            )
            .map_err(|retry_after_seconds| TokenAuthError::RateLimited {
                retry_after_seconds,
            })?;
        self.record_use(&credential).await;
        Ok(principal)
    }

    /// The valid token of the request with its stored record; `None` when absent, malformed, unknown, wrong,
    /// expired, revoked or of a user off the allow-list.
    async fn authenticate(
        &self,
        parts: &Parts,
        audit: &mut AuditIds,
    ) -> Result<Option<(TokenCredential, TokenPrincipal)>, TokenAuthError> {
        let Some(presented) = bearer(parts) else {
            return Ok(None);
        };
        let Some(id) = token_id(presented) else {
            return Ok(None);
        };
        audit.token_id = Some(id.clone());
        // The lookup by id tells only whether a (public) id exists; the secret is compared in constant time
        // through the SHA-256 digests.
        let hash = sha256(presented.as_bytes());
        let Some(credential) = self.state.db.access_tokens().credential(&id).await? else {
            return Ok(None);
        };
        let matches: bool = credential.token_hash.ct_eq(&hash).into();
        if !matches || !usable(&credential, Timestamp::now()) {
            return Ok(None);
        }
        audit.user_id = Some(credential.record.user_id.clone());
        let principal = TokenPrincipal {
            token_id: id,
            tenant_id: credential.record.tenant_id.clone(),
            user_id: credential.record.user_id.clone(),
            scopes: credential.record.scopes.clone(),
            token_hash: hash,
        };
        Ok(Some((credential, principal)))
    }

    /// Records the use at most once a minute per token, so a burst of requests writes nothing; best-effort.
    async fn record_use(&self, credential: &TokenCredential) {
        let now = Timestamp::now();
        let due = credential
            .record
            .last_used_at
            .is_none_or(|last| now - last >= TOUCH_INTERVAL);
        if !due {
            return;
        }
        if let Err(error) = self
            .state
            .db
            .access_tokens()
            .touch(&credential.record.id, now)
            .await
        {
            tracing::warn!(
                sqlite_code = ?error.sqlite_code(),
                ?error,
                token_id = %credential.record.id,
                "the use of an access token could not be recorded (database busy?); the request proceeds"
            );
        }
    }
}

/// The token of the single `Authorization: Bearer <token>` header; `None` for any other form.
fn bearer(parts: &Parts) -> Option<&str> {
    let mut values = parts.headers.get_all(AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, token) = value.to_str().ok()?.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

/// Whether the request presents a personal access token: an `Authorization: Bearer srp_…` header. Any other
/// `Authorization` value (an authenticating proxy's Basic or bearer credential) is not ours and passes untouched.
fn is_token_attempt(headers: &HeaderMap) -> bool {
    headers.get_all(AUTHORIZATION).iter().any(|value| {
        value
            .to_str()
            .ok()
            .and_then(|value| value.split_once(' '))
            .is_some_and(|(scheme, token)| {
                scheme.eq_ignore_ascii_case("bearer")
                    && token.trim_start().starts_with(TOKEN_PREFIX)
            })
    })
}

/// The client key of the failed-attempt limit: the TCP peer, or, when the peer is a trusted proxy
/// (`SR_TRUSTED_PROXIES`), the last `X-Forwarded-For` entry, which that proxy appended; an IPv6 address counts as
/// its /64 network (one subscriber's prefix).
fn client_address(parts: &Parts, config: &Config) -> Option<IpAddr> {
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(peer)| peer.ip().to_canonical())?;
    let forwarded = || {
        parts
            .headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .last()
            .and_then(|address| address.trim().parse::<IpAddr>().ok())
            .map(|address| address.to_canonical())
    };
    let client = if config.trusted_proxies.iter().any(|net| net.contains(&peer)) {
        forwarded().unwrap_or(peer)
    } else {
        peer
    };
    Some(match client {
        IpAddr::V6(address) => IpAddr::V6(Ipv6Addr::from_bits(
            address.to_bits() & !u128::from(u64::MAX),
        )),
        address @ IpAddr::V4(_) => address,
    })
}

fn query_carries_token(uri: &Uri) -> bool {
    uri.query().is_some_and(|query| {
        query
            .split('&')
            .any(|pair| pair.split('=').next() == Some(QUERY_TOKEN_PARAMETER))
    })
}

/// Ids named in the audit line, as far as the check got.
#[derive(Debug, Default)]
struct AuditIds {
    token_id: Option<AccessTokenId>,
    user_id: Option<String>,
}

/// Middleware of the public API router (`route_layer`): requests without `Authorization: Bearer srp_…` pass
/// unchanged; requests with one are admitted as a [`TokenPrincipal`] or refused (ADR-019 §5).
pub async fn check(State(gate): State<TokenGate>, request: Request, next: Next) -> Response {
    if query_carries_token(request.uri()) {
        return TokenAuthError::TokenInQuery.into_response();
    }
    if !is_token_attempt(request.headers()) {
        return next.run(request).await;
    }
    let started = Instant::now();
    let (mut parts, body) = request.into_parts();
    let mut audit = AuditIds::default();
    let admitted = gate.admit(&parts, &mut audit).await;
    let method = parts.method.clone();
    let route = parts
        .extensions
        .get::<MatchedPath>()
        .map_or_else(String::new, |path| path.as_str().to_owned());
    let response = match admitted {
        Ok(principal) => {
            parts.extensions.insert(principal);
            next.run(Request::from_parts(parts, body)).await
        }
        Err(error) => error.into_response(),
    };
    tracing::info!(
        target: "audit",
        token_id = audit.token_id.as_ref().map(|id| id.0.as_str()),
        user_id = audit.user_id.as_deref(),
        method = %method,
        route = %route,
        status = response.status().as_u16(),
        duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "access token request"
    );
    response
}

/// How long a GitHub-backed read is reused for an access token: agents poll, and every GitHub call spends the
/// user's own GitHub budget (5000 requests an hour), which the browser needs too.
pub(crate) const GITHUB_READ_TTL: Duration = Duration::from_secs(60);
/// Most cached GitHub-backed reads; beyond it (after dropping ended ones) a read is not cached.
const MAX_CACHED_READS: usize = 1024;

type CachedRead = (Instant, Arc<dyn Any + Send + Sync>);

/// GitHub-backed reads served to access tokens, by (user id, key). In-process: one server process.
static GITHUB_READS: LazyLock<Mutex<HashMap<(String, String), CachedRead>>> =
    LazyLock::new(Mutex::default);

/// The result of a GitHub-backed `read` for `session`. For an access token a success is reused for
/// [`GITHUB_READ_TTL`] per (user, `key`), so a polling agent does not spend the user's GitHub budget; a browser
/// session always reads.
pub(crate) async fn github_read<T, E>(
    session: &Session,
    key: String,
    read: impl Future<Output = Result<T, E>>,
) -> Result<T, E>
where
    T: Clone + Send + Sync + 'static,
{
    if !session.is_access_token() {
        return read.await;
    }
    let key = (session.user_id.clone(), key);
    let now = Instant::now();
    let fresh = |(at, _): &CachedRead| now.duration_since(*at) < GITHUB_READ_TTL;
    {
        let reads = GITHUB_READS.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(value) = reads
            .get(&key)
            .filter(|entry| fresh(entry))
            .and_then(|(_, value)| value.downcast_ref::<T>())
        {
            return Ok(value.clone());
        }
    }
    let value = read.await?;
    let mut reads = GITHUB_READS.lock().unwrap_or_else(PoisonError::into_inner);
    if reads.len() >= MAX_CACHED_READS {
        reads.retain(|_, entry| fresh(entry));
    }
    if reads.len() < MAX_CACHED_READS || reads.contains_key(&key) {
        reads.insert(key, (now, Arc::new(value.clone())));
    }
    Ok(value)
}

/// Test helper: a token of `user_id` in the default tenant stored with `scopes` and `expires_at`; returns the
/// whole token and its principal.
#[cfg(test)]
pub(crate) async fn stored_token(
    db: &Db,
    user_id: &str,
    scopes: Vec<TokenScope>,
    expires_at: Timestamp,
) -> (String, TokenPrincipal) {
    use db::repo::DEFAULT_TENANT;
    use db::repo::access_tokens::{CreateOutcome, NewAccessToken};
    let issued = issue().expect("token");
    let new = NewAccessToken {
        id: issued.id.clone(),
        name: format!("agent {}", issued.id),
        token_hash: issued.hash,
        scopes,
        expires_at,
    };
    let created = db
        .access_tokens()
        .create(DEFAULT_TENANT, user_id, &new, Timestamp::now())
        .await
        .expect("store token");
    assert!(matches!(created, CreateOutcome::Created(_)), "{created:?}");
    let principal = TokenPrincipal {
        token_id: issued.id,
        tenant_id: DEFAULT_TENANT.to_owned(),
        user_id: user_id.to_owned(),
        scopes: new.scopes,
        token_hash: issued.hash,
    };
    (issued.token.0, principal)
}

#[cfg(test)]
mod tests;
