//! Transport of the scan: the user's token with one refresh after a 401, searches one at a time and paced, at
//! most four other calls at once, the secondary-limit retry ladder, primary-limit failure, pagination and GraphQL
//! answers with partial data.

use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::header::{ACCEPT, HeaderMap, LINK, RETRY_AFTER, USER_AGENT};
use reqwest::{Client, RequestBuilder, Response, StatusCode, redirect};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use tokio::sync::{Mutex, Semaphore};
use tokio::time::{Instant, sleep, sleep_until};
use url::Url;

use crate::auth::{UserTokenError, user_access_token, user_token_rejected};
use crate::github::oauth::endpoint;
use crate::secrets::secrets_equal;
use crate::state::AppState;

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const CLIENT_NAME: &str = "smart-review";
const GITHUB_API_VERSION: &str = "2022-11-28";
/// Searches of one user run one at a time, at least this far apart (S11-2: 3 s never tripped the limit).
pub(crate) const SEARCH_INTERVAL: Duration = Duration::from_secs(3);
/// Waits before the first and the second retry after a secondary rate limit without `retry-after`; the third
/// hit fails.
pub(crate) const SECONDARY_RETRY_WAITS: [Duration; 2] =
    [Duration::from_secs(60), Duration::from_secs(120)];
/// Non-search calls in flight at once (plan §A.3).
pub(crate) const MAX_CONCURRENT_CALLS: usize = 4;
/// GitHub's largest page.
const PAGE_SIZE: u32 = 100;
/// GitHub serves the first 1000 results of a search only.
const SEARCH_CAP: usize = 1000;

/// A GitHub read of the scan failed. No variant carries the token or GitHub's message text.
#[derive(Debug, thiserror::Error)]
pub enum WorkError {
    /// The HTTP client could not be built.
    #[error("the HTTP client for GitHub could not be created")]
    Client(#[source] reqwest::Error),
    /// The request did not complete.
    #[error("the request to GitHub did not complete")]
    Http(#[source] reqwest::Error),
    /// No usable user token.
    #[error("no usable GitHub token")]
    Token(#[from] UserTokenError),
    /// GitHub rejected the user token, also after a refresh.
    #[error("GitHub rejected the user token; sign in again")]
    Unauthorized,
    /// GitHub's primary rate limit is exhausted until `reset`.
    #[error("the GitHub rate limit is exhausted")]
    PrimaryRateLimit {
        /// When the limit resets (`x-ratelimit-reset`), if GitHub said.
        reset: Option<DateTime<Utc>>,
    },
    /// GitHub's secondary rate limit was hit a third time.
    #[error("GitHub's secondary rate limit persisted after two retries")]
    SecondaryRateLimit,
    /// GitHub search refuses a login or repository (unknown, or not visible to the token).
    #[error("GitHub search refuses this login or repository")]
    Unsearchable,
    /// GitHub does not know the item or hides it from the token.
    #[error("GitHub answered 404")]
    NotFound,
    /// GitHub failed (5xx).
    #[error("GitHub failed with HTTP status {0}")]
    ServerError(StatusCode),
    /// GitHub answered with another unexpected status.
    #[error("GitHub answered with HTTP status {0}")]
    Status(StatusCode),
    /// The answer is not JSON (e.g. GitHub's HTML error page).
    #[error("GitHub answered with something other than JSON")]
    NotJson(#[source] serde_json::Error),
    /// The JSON does not have the expected shape.
    #[error("GitHub answered with unexpected JSON")]
    Malformed(#[source] serde_json::Error),
    /// GitHub refused the GraphQL document before executing it; the error types (never messages).
    #[error("GitHub refused the query ({0:?})")]
    Refused(Vec<String>),
    /// Only queries are sent (T18).
    #[error("the scan sends GraphQL queries only")]
    NotAQuery,
    /// The scan's call limiter was closed.
    #[error("the GitHub scan was interrupted")]
    Interrupted,
}

impl WorkError {
    /// The refresh fails: no token, rejected token, or a rate limit.
    pub fn ends_refresh(&self) -> bool {
        matches!(
            self,
            Self::Token(_)
                | Self::Unauthorized
                | Self::PrimaryRateLimit { .. }
                | Self::SecondaryRateLimit
                | Self::Interrupted
        )
    }

    /// A batch failing this way is retried once as two halves (5xx, non-JSON, timeout; S11-2).
    pub(crate) fn splits_batch(&self) -> bool {
        match self {
            Self::ServerError(_) | Self::NotJson(_) => true,
            Self::Http(error) => error.is_timeout(),
            _ => false,
        }
    }
}

/// The user's GitHub access token for the scan.
pub trait TokenSource: Send + Sync {
    /// The current access token, refreshed when due.
    fn token(&self) -> impl Future<Output = Result<SecretString, UserTokenError>> + Send;
    /// GitHub answered 401 to `token`; the next [`TokenSource::token`] must refresh instead of returning it.
    fn rejected(&self, token: &SecretString) -> impl Future<Output = ()> + Send;
}

/// The stored token pair of a user (ADR-007: refresh serialised per user).
#[derive(Clone, Debug)]
pub struct UserTokens {
    /// Server state holding the token store.
    pub state: AppState,
    /// The user.
    pub user_id: String,
}

impl TokenSource for UserTokens {
    async fn token(&self) -> Result<SecretString, UserTokenError> {
        user_access_token(&self.state, &self.user_id).await
    }

    async fn rejected(&self, token: &SecretString) {
        user_token_rejected(&self.state, &self.user_id, token).await;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CallKind {
    Search,
    Other,
}

/// GitHub client of one user's scan (one per refresh).
pub struct WorkClient<T> {
    http: Client,
    api_url: Url,
    tokens: T,
    token: Mutex<Option<SecretString>>,
    /// Start of the last search attempt; held for a whole search, so searches run one at a time.
    last_search: Mutex<Option<Instant>>,
    calls: Semaphore,
    pub(crate) page_size: u32,
}

impl<T> std::fmt::Debug for WorkClient<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkClient")
            .field("api_url", &self.api_url.as_str())
            .finish_non_exhaustive()
    }
}

/// How a 403/429 answer limits the caller.
enum Limit {
    Primary(Option<DateTime<Utc>>),
    Secondary(Option<Duration>),
    None,
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn reset_at(headers: &HeaderMap) -> Option<DateTime<Utc>> {
    header(headers, "x-ratelimit-reset")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .and_then(|epoch| DateTime::from_timestamp(epoch, 0))
}

/// Classifies a 403/429 answer, reading the body only when the headers do not tell.
async fn limit(response: Response) -> Limit {
    let status = response.status();
    let headers = response.headers().clone();
    if header(&headers, "x-ratelimit-remaining").is_some_and(|value| value.trim() == "0") {
        return Limit::Primary(reset_at(&headers));
    }
    if let Some(seconds) = header(&headers, RETRY_AFTER.as_str()) {
        return Limit::Secondary(seconds.trim().parse().ok().map(Duration::from_secs));
    }
    let body = response.text().await.unwrap_or_default();
    if status == StatusCode::TOO_MANY_REQUESTS
        || body.to_ascii_lowercase().contains("secondary rate limit")
    {
        Limit::Secondary(None)
    } else {
        Limit::None
    }
}

/// Whether the `Link` header names a next page. The next URL itself is never followed: the token goes only to
/// the configured API host.
fn has_next(headers: &HeaderMap) -> bool {
    header(headers, LINK.as_str()).is_some_and(|link| link.contains("rel=\"next\""))
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    data: Option<Map<String, Value>>,
    #[serde(default)]
    errors: Vec<ErrorEntry>,
}

#[derive(Deserialize)]
struct ErrorEntry {
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct SearchPage<I> {
    items: Vec<I>,
}

impl<T: TokenSource> WorkClient<T> {
    /// A client for `<api_url>` reading with the tokens of `tokens`.
    pub fn new(api_url: Url, tokens: T) -> Result<Self, WorkError> {
        let http = Client::builder()
            .redirect(redirect::Policy::none())
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(WorkError::Client)?;
        Ok(Self {
            http,
            api_url,
            tokens,
            token: Mutex::new(None),
            last_search: Mutex::new(None),
            calls: Semaphore::new(MAX_CONCURRENT_CALLS),
            page_size: PAGE_SIZE,
        })
    }

    async fn current_token(&self) -> Result<SecretString, WorkError> {
        let mut held = self.token.lock().await;
        if let Some(token) = held.as_ref() {
            return Ok(token.clone());
        }
        let token = self.tokens.token().await?;
        *held = Some(token.clone());
        Ok(token)
    }

    /// Replaces `rejected` with a refreshed token; concurrent callers with the same rejected token refresh once.
    async fn replace_rejected(&self, rejected: &SecretString) -> Result<(), WorkError> {
        let mut held = self.token.lock().await;
        if let Some(token) = held.as_ref()
            && !secrets_equal(token.expose_secret(), rejected.expose_secret())
        {
            return Ok(());
        }
        *held = None;
        self.tokens.rejected(rejected).await;
        *held = Some(self.tokens.token().await?);
        Ok(())
    }

    /// Sends one call with the user's token: searches serial and paced, other calls limited to four at once; a
    /// 401 refreshes the token once, a secondary limit is retried after `retry-after` or 60 s, then 120 s.
    async fn send(
        &self,
        kind: CallKind,
        build: impl Fn(&Client) -> RequestBuilder,
    ) -> Result<Response, WorkError> {
        let mut last_search = match kind {
            CallKind::Search => Some(self.last_search.lock().await),
            CallKind::Other => None,
        };
        let _permit = match kind {
            CallKind::Other => Some(
                self.calls
                    .acquire()
                    .await
                    .map_err(|_| WorkError::Interrupted)?,
            ),
            CallKind::Search => None,
        };
        let mut refreshed = false;
        let mut secondary_hits = 0;
        loop {
            if let Some(last) = last_search.as_mut() {
                if let Some(at) = **last {
                    sleep_until(at + SEARCH_INTERVAL).await;
                }
                **last = Some(Instant::now());
            }
            let token = self.current_token().await?;
            let response = build(&self.http)
                .bearer_auth(token.expose_secret())
                .header(USER_AGENT, CLIENT_NAME)
                .header("x-github-api-version", GITHUB_API_VERSION)
                .send()
                .await
                .map_err(WorkError::Http)?;
            let status = response.status();
            if status == StatusCode::UNAUTHORIZED {
                if refreshed {
                    return Err(WorkError::Unauthorized);
                }
                tracing::debug!(
                    "GitHub rejected the user token during the scan; refreshing it once"
                );
                refreshed = true;
                self.replace_rejected(&token).await?;
                continue;
            }
            if status != StatusCode::FORBIDDEN && status != StatusCode::TOO_MANY_REQUESTS {
                return Ok(response);
            }
            match limit(response).await {
                Limit::Primary(reset) => return Err(WorkError::PrimaryRateLimit { reset }),
                Limit::Secondary(retry_after) => {
                    let Some(ladder) = SECONDARY_RETRY_WAITS.get(secondary_hits) else {
                        return Err(WorkError::SecondaryRateLimit);
                    };
                    let wait = retry_after.unwrap_or(*ladder);
                    secondary_hits += 1;
                    tracing::info!(
                        wait_secs = wait.as_secs(),
                        attempt = secondary_hits,
                        "GitHub secondary rate limit during the scan; retrying after the wait"
                    );
                    sleep(wait).await;
                }
                Limit::None => return Err(WorkError::Status(status)),
            }
        }
    }

    /// `GET <api>/<path>?<query>`; statuses other than 2xx become errors.
    async fn get(
        &self,
        kind: CallKind,
        path: &str,
        query: &[(&str, String)],
        accept: &str,
    ) -> Result<Response, WorkError> {
        let mut url = endpoint(&self.api_url, path);
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(key, value)| (*key, value.as_str())));
        }
        let response = self
            .send(kind, |http| http.get(url.clone()).header(ACCEPT, accept))
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        Err(match status {
            StatusCode::NOT_FOUND => WorkError::NotFound,
            StatusCode::UNPROCESSABLE_ENTITY => {
                let body = response.text().await.unwrap_or_default();
                if body.contains("cannot be searched") {
                    WorkError::Unsearchable
                } else {
                    WorkError::Status(status)
                }
            }
            status if status.is_server_error() => WorkError::ServerError(status),
            status => WorkError::Status(status),
        })
    }

    /// A REST read answering JSON.
    pub(crate) async fn rest_json<D: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<D, WorkError> {
        let response = self
            .get(CallKind::Other, path, query, "application/vnd.github+json")
            .await?;
        let bytes = response.bytes().await.map_err(WorkError::Http)?;
        serde_json::from_slice(&bytes).map_err(WorkError::Malformed)
    }

    /// A REST read answering raw text (`application/vnd.github.raw`).
    pub(crate) async fn rest_raw(&self, path: &str) -> Result<String, WorkError> {
        let response = self
            .get(CallKind::Other, path, &[], "application/vnd.github.raw")
            .await?;
        response.text().await.map_err(WorkError::Http)
    }

    /// Every page of a REST list, up to `cap` items.
    pub(crate) async fn rest_pages<D: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        cap: usize,
    ) -> Result<Vec<D>, WorkError> {
        self.pages(CallKind::Other, path, query, cap, |bytes| {
            serde_json::from_slice::<Vec<D>>(bytes)
        })
        .await
    }

    /// Every page of `GET /search/issues?q=<q>`, up to GitHub's 1000 results.
    pub(crate) async fn search<D: DeserializeOwned>(&self, q: &str) -> Result<Vec<D>, WorkError> {
        let query = [("q", q.to_owned())];
        self.pages(
            CallKind::Search,
            "search/issues",
            &query,
            SEARCH_CAP,
            |bytes| serde_json::from_slice::<SearchPage<D>>(bytes).map(|page| page.items),
        )
        .await
    }

    async fn pages<D>(
        &self,
        kind: CallKind,
        path: &str,
        query: &[(&str, String)],
        cap: usize,
        parse: impl Fn(&[u8]) -> Result<Vec<D>, serde_json::Error>,
    ) -> Result<Vec<D>, WorkError> {
        let mut items = Vec::new();
        let mut page = 1_u32;
        loop {
            let mut paged = query.to_vec();
            paged.push(("per_page", self.page_size.to_string()));
            paged.push(("page", page.to_string()));
            let response = self
                .get(kind, path, &paged, "application/vnd.github+json")
                .await?;
            let next = has_next(response.headers());
            let bytes = response.bytes().await.map_err(WorkError::Http)?;
            items.extend(parse(&bytes).map_err(WorkError::Malformed)?);
            if !next || items.len() >= cap {
                items.truncate(cap);
                return Ok(items);
            }
            page += 1;
        }
    }

    /// Runs a GraphQL query; returns `data` (an unresolved lookup is `null`). Errors next to data are dropped
    /// (their lookups are `null`); errors without data fail, `RATE_LIMITED` as the primary limit.
    pub(crate) async fn graphql(&self, document: &str) -> Result<Map<String, Value>, WorkError> {
        if !document.trim_start().starts_with("query") {
            return Err(WorkError::NotAQuery);
        }
        let url = endpoint(&self.api_url, "graphql");
        let body = json!({ "query": document });
        let response = self
            .send(CallKind::Other, |http| {
                http.post(url.clone())
                    .header(ACCEPT, "application/json")
                    .json(&body)
            })
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(if status.is_server_error() {
                WorkError::ServerError(status)
            } else {
                WorkError::Status(status)
            });
        }
        let reset = reset_at(response.headers());
        let bytes = response.bytes().await.map_err(WorkError::Http)?;
        let envelope: Envelope = serde_json::from_slice(&bytes).map_err(WorkError::NotJson)?;
        let kinds: Vec<String> = envelope
            .errors
            .iter()
            .map(|error| error.kind.clone().unwrap_or_default())
            .collect();
        match envelope.data {
            Some(data) => {
                if !kinds.is_empty() {
                    tracing::debug!(?kinds, "GitHub answered a scan query with partial errors");
                }
                if let Some(rate) = data.get("rateLimit") {
                    tracing::trace!(cost = %rate["cost"], remaining = %rate["remaining"],
                        "GitHub GraphQL cost of a scan query");
                }
                Ok(data)
            }
            None if kinds.iter().any(|kind| kind == "RATE_LIMITED") => {
                Err(WorkError::PrimaryRateLimit { reset })
            }
            None => Err(WorkError::Refused(kinds)),
        }
    }
}
