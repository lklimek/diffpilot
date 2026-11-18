//! The dashboard scan's GitHub (plan v1.1 §A.2, §A.3) over the synthetic world of `fixtures/dashboard/github`
//! (`--dashboard-fixture-dir`): REST search, releases, milestones, contents, pull request files and user events,
//! and GraphQL alias batches with `rateLimit`. Batches of 30 or more `pullRequest` lookups fail like GitHub's
//! (HTML 502, 504 from 50; spike S11-2). Test controls: `POST /_fake/dashboard/faults` queues injected answers
//! (secondary or primary rate limit, 5xx) and `GET /_fake/dashboard/requests` lists every request served.

mod execute;
mod query;
mod rest;
mod search;
mod world;

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::{ACCEPT, CONTENT_TYPE, HOST, RETRY_AFTER};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::Instant;

use self::rest::{Answer, Endpoint};
use self::world::World;
use crate::graphql::Graphql;
use crate::oauth::{Credentials, Oauth};
use crate::{RecordedState, StartError, github_error, require_user_agent};

/// `pullRequest` lookups per GraphQL request from which GitHub answers an HTML error page (spike S11-2: 30).
const HTML_ERROR_FROM: usize = 30;
/// `pullRequest` lookups per GraphQL request from which GitHub times out with 504 (spike S11-2: 50).
const TIMEOUT_FROM: usize = 50;
const SECONDARY_MESSAGE: &str = "You have exceeded a secondary rate limit. Please wait a few minutes before you try \
                                 again. If you reach out to GitHub Support for help, please include the request ID \
                                 FAKE:0000:0000.";
const PRIMARY_MESSAGE: &str = "API rate limit exceeded for user ID 4820300.";
const TIMEOUT_MESSAGE: &str = "We couldn't respond to your request in time. Sorry about that. Please try \
                               resubmitting your request and contact us if the problem persists.";
const UNICORN_PAGE: &str = "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>Unicorn! &middot; GitHub</title>\
                            </head><body><p>No server is currently available to service your request.</p>\
                            <p>Sorry about that. Please try refreshing and contact us if the problem persists.</p>\
                            </body></html>\n";

/// The kind of request a fault or record applies to.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Target {
    /// `GET /search/issues`.
    Search,
    /// The other dashboard REST reads.
    Rest,
    /// Dashboard GraphQL documents.
    Graphql,
}

/// An injected answer.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum FaultAnswer {
    /// 403 (or `status`) with GitHub's secondary-limit message and, if given, `retry-after`.
    SecondaryRateLimit {
        #[serde(default)]
        retry_after: Option<u64>,
        #[serde(default = "forbidden")]
        status: u16,
    },
    /// `x-ratelimit-remaining: 0` with `x-ratelimit-reset: reset` (epoch seconds): REST 403, GraphQL 200 with a
    /// `RATE_LIMITED` error.
    PrimaryRateLimit { reset: i64 },
    /// `status` (default 502) with GitHub's HTML error page, or a JSON message when `html` is false.
    ServerError {
        #[serde(default = "bad_gateway")]
        status: u16,
        #[serde(default = "yes")]
        html: bool,
    },
}

fn forbidden() -> u16 {
    403
}

fn bad_gateway() -> u16 {
    502
}

fn yes() -> bool {
    true
}

fn one() -> u32 {
    1
}

impl FaultAnswer {
    fn name(&self) -> &'static str {
        match self {
            Self::SecondaryRateLimit { .. } => "secondary_rate_limit",
            Self::PrimaryRateLimit { .. } => "primary_rate_limit",
            Self::ServerError { .. } => "server_error",
        }
    }
}

/// A queued fault: the next `times` requests of kind `on` whose text contains `matching` get `answer`. The text
/// is the path and decoded query (REST) or the GraphQL document followed by its variables.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fault {
    on: Target,
    #[serde(default)]
    matching: Option<String>,
    #[serde(default = "one")]
    times: u32,
    answer: FaultAnswer,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Faults {
    faults: Vec<Fault>,
}

/// One served request, as `GET /_fake/dashboard/requests` lists it.
#[derive(Clone, Debug, Serialize)]
struct RequestRecord {
    /// 1-based order of arrival.
    seq: usize,
    /// Milliseconds since the fake started, on tokio's clock (follows paused test time in-process).
    at_ms: u64,
    on: Target,
    method: &'static str,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    query: Option<String>,
    /// The decoded search query.
    #[serde(skip_serializing_if = "Option::is_none")]
    q: Option<String>,
    /// GraphQL: top-level lookups (aliases) without `rateLimit`.
    #[serde(skip_serializing_if = "Option::is_none")]
    lookups: Option<usize>,
    /// GraphQL: `pullRequest(number:)` lookups among them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pull_requests: Option<usize>,
    /// GraphQL: points charged.
    #[serde(skip_serializing_if = "Option::is_none")]
    cost: Option<u32>,
    status: u16,
    /// The injected or simulated failure served, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    fault: Option<&'static str>,
}

/// A rate-limit resource and its window.
#[derive(Clone, Copy)]
enum Resource {
    Search,
    Core,
    Graphql,
}

impl Resource {
    fn limit(self) -> u32 {
        match self {
            Self::Search => 30,
            Self::Core | Self::Graphql => 5000,
        }
    }

    fn window_secs(self) -> u64 {
        match self {
            Self::Search => 60,
            Self::Core | Self::Graphql => 3600,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Core => "core",
            Self::Graphql => "graphql",
        }
    }
}

/// Usage of one resource in its current window.
#[derive(Default)]
struct Window {
    index: u64,
    used: u32,
}

/// The `x-ratelimit-*` values of one answer.
struct Rate {
    resource: Resource,
    limit: u32,
    remaining: u32,
    used: u32,
    reset: i64,
}

impl Rate {
    fn exhausted(mut self, reset: i64) -> Self {
        self.remaining = 0;
        self.used = self.limit;
        self.reset = reset;
        self
    }

    fn apply(&self, headers: &mut HeaderMap) {
        for (name, value) in [
            ("x-ratelimit-limit", self.limit.to_string()),
            ("x-ratelimit-remaining", self.remaining.to_string()),
            ("x-ratelimit-used", self.used.to_string()),
            ("x-ratelimit-reset", self.reset.to_string()),
            ("x-ratelimit-resource", self.resource.name().to_owned()),
        ] {
            if let Ok(value) = HeaderValue::from_str(&value) {
                headers.insert(HeaderName::from_static(name), value);
            }
        }
    }
}

#[derive(Default)]
struct Live {
    faults: Vec<Fault>,
    requests: Vec<RequestRecord>,
    search: Window,
    core: Window,
    graphql: Window,
}

impl Live {
    /// Takes one use of the first queued fault for `on` whose filter matches `text`.
    fn take_fault(&mut self, on: Target, text: &str) -> Option<FaultAnswer> {
        let index = self.faults.iter().position(|fault| {
            fault.on == on
                && fault.times > 0
                && fault.matching.as_deref().is_none_or(|m| text.contains(m))
        })?;
        let fault = &mut self.faults[index];
        fault.times -= 1;
        let answer = fault.answer.clone();
        if fault.times == 0 {
            self.faults.remove(index);
        }
        Some(answer)
    }
}

/// The dashboard world and its live state.
pub(crate) struct Dashboard {
    world: World,
    graphql: Arc<Graphql>,
    /// The recorded pull request (`owner/name`, number); its head follows `POST /_fake/head`.
    recorded_pull: Option<(String, u64)>,
    started: Instant,
    started_epoch: i64,
    live: Mutex<Live>,
}

impl Dashboard {
    /// Loads the world of `dir`; `pull_path` is the recorded pull request's REST path.
    pub(crate) fn load(
        dir: &Path,
        graphql: Arc<Graphql>,
        pull_path: &str,
    ) -> Result<Self, StartError> {
        let world = World::load(dir)?;
        let segments: Vec<&str> = pull_path.trim_start_matches('/').split('/').collect();
        let recorded_pull = match segments.as_slice() {
            ["repos", owner, name, "pulls", number] => {
                number.parse().ok().map(|n| (format!("{owner}/{name}"), n))
            }
            _ => None,
        };
        tracing::info!(
            repositories = world.repositories.len(),
            pull_requests = world.pull_requests.len(),
            issues = world.issues.len(),
            dir = %dir.display(),
            "fake-github loaded the dashboard world"
        );
        Ok(Self {
            world,
            graphql,
            recorded_pull,
            started: Instant::now(),
            started_epoch: Utc::now().timestamp(),
            live: Mutex::default(),
        })
    }

    fn live(&self) -> MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn record(&self, mut record: RequestRecord, status: StatusCode) {
        record.status = status.as_u16();
        let mut live = self.live();
        record.seq = live.requests.len() + 1;
        live.requests.push(record);
    }

    fn new_record(&self, on: Target, method: &'static str, path: &str) -> RequestRecord {
        RequestRecord {
            seq: 0,
            at_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            on,
            method,
            path: path.to_owned(),
            query: None,
            q: None,
            lookups: None,
            pull_requests: None,
            cost: None,
            status: 0,
            fault: None,
        }
    }

    /// Charges `cost` to `resource` and returns the headers' values.
    fn charge(&self, live: &mut Live, resource: Resource, cost: u32) -> Rate {
        let window_secs = resource.window_secs();
        let index = self.started.elapsed().as_secs() / window_secs;
        let window = match resource {
            Resource::Search => &mut live.search,
            Resource::Core => &mut live.core,
            Resource::Graphql => &mut live.graphql,
        };
        if window.index != index {
            *window = Window { index, used: 0 };
        }
        window.used = window.used.saturating_add(cost);
        let limit = resource.limit();
        let window_end = (index + 1).saturating_mul(window_secs);
        Rate {
            resource,
            limit,
            remaining: limit.saturating_sub(window.used),
            used: window.used,
            reset: self.started_epoch + i64::try_from(window_end).unwrap_or(i64::MAX / 2),
        }
    }

    fn moved_head(&self) -> execute::MovedHead {
        self.recorded_pull
            .as_ref()
            .map(|(repository, number)| (repository.clone(), *number, self.graphql.head()))
    }

    fn rest(&self, endpoint: &Endpoint, uri: &Uri, headers: &HeaderMap, oauth: &Oauth) -> Response {
        let on = if *endpoint == Endpoint::Search {
            Target::Search
        } else {
            Target::Rest
        };
        let pairs: Vec<(String, String)> =
            url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
                .into_owned()
                .collect();
        let mut record = self.new_record(on, "GET", uri.path());
        record.query = uri.query().map(str::to_owned);
        record.q = pairs
            .iter()
            .find(|(k, _)| k == "q")
            .map(|(_, v)| v.clone())
            .filter(|_| on == Target::Search);
        let response = self.rest_answer(endpoint, uri, headers, oauth, &pairs, &mut record);
        self.record(record, response.status());
        response
    }

    fn rest_answer(
        &self,
        endpoint: &Endpoint,
        uri: &Uri,
        headers: &HeaderMap,
        oauth: &Oauth,
        pairs: &[(String, String)],
        record: &mut RequestRecord,
    ) -> Response {
        if let Some(rejection) = require_user_agent(headers) {
            return rejection;
        }
        let credentials = oauth.credentials(headers);
        if credentials == Credentials::Invalid {
            return github_error(StatusCode::UNAUTHORIZED, "Bad credentials");
        }
        let resource = if record.on == Target::Search {
            Resource::Search
        } else {
            Resource::Core
        };
        let text = format!(
            "{} {}",
            uri.path(),
            pairs
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&")
        );
        let rate = {
            let mut live = self.live();
            let fault = live.take_fault(record.on, &text);
            let rate = self.charge(&mut live, resource, 1);
            if let Some(fault) = fault {
                record.fault = Some(fault.name());
                return fault_response(&fault, record.on, rate);
            }
            rate
        };
        let host = headers
            .get(HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("localhost");
        let base = format!("http://{host}{}", uri.path());
        let authenticated = credentials == Credentials::Valid;
        let answer = if *endpoint == Endpoint::Search {
            self.search(pairs, authenticated, &base)
        } else {
            let raw = headers
                .get(ACCEPT)
                .and_then(|a| a.to_str().ok())
                .is_some_and(|a| a.contains("application/vnd.github.raw"));
            rest::answer(endpoint, &self.world, authenticated, &base, pairs, raw)
        };
        respond(answer, &rate)
    }

    fn search(&self, pairs: &[(String, String)], authenticated: bool, base: &str) -> Answer {
        let param = |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        let invalid = |message: String| Answer::Json {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            body: json!({"message": "Validation Failed", "errors": [{"message": message, "resource": "Search",
                "field": "q", "code": "invalid"}], "documentation_url": "https://docs.github.com/v3/search/",
                "status": "422"}),
            link: None,
        };
        let search = match search::parse(param("q").unwrap_or_default())
            .and_then(|search| search.check(&self.world, authenticated).map(|()| search))
        {
            Ok(search) => search,
            Err(error) => {
                if matches!(error, search::SearchError::Unsupported(_)) {
                    tracing::warn!(%error, "fake-github refused a search term it does not implement");
                }
                return invalid(error.to_string());
            }
        };
        let (per_page, page) = rest::page_params(pairs);
        if (page - 1).saturating_mul(per_page) >= 1000 {
            return Answer::Json {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                body: json!({"message": "Only the first 1000 search results are available",
                    "documentation_url": "https://docs.github.com/v3/search/", "status": "422"}),
                link: None,
            };
        }
        let nodes = if search.pull_requests {
            &self.world.pull_requests
        } else {
            &self.world.issues
        };
        let mut hits: Vec<&Value> = nodes
            .iter()
            .filter(|node| authenticated || node["repository"]["isPrivate"] != true)
            .filter(|node| search.matches(node))
            .collect();
        let field = if param("sort") == Some("created") {
            "createdAt"
        } else {
            "updatedAt"
        };
        hits.sort_by(|a, b| {
            b[field]
                .as_str()
                .cmp(&a[field].as_str())
                .then_with(|| world::name_with_owner(a).cmp(world::name_with_owner(b)))
                .then_with(|| world::number(a).cmp(&world::number(b)))
        });
        if param("order") == Some("asc") {
            hits.reverse();
        }
        let items: Vec<Value> = hits
            .iter()
            .map(|node| search::item(node, &self.world))
            .collect();
        let (slice, link) = rest::page(&items, 1000, base, pairs);
        Answer::Json {
            status: StatusCode::OK,
            body: json!({"total_count": items.len(), "incomplete_results": false, "items": slice}),
            link,
        }
    }

    fn graphql(
        &self,
        doc: &query::Document,
        request: &GraphqlRequest,
        headers: &HeaderMap,
        oauth: &Oauth,
    ) -> Response {
        let (lookups, pulls) = execute::batch_size(doc);
        let mut record = self.new_record(Target::Graphql, "POST", "/graphql");
        record.lookups = Some(lookups);
        record.pull_requests = Some(pulls);
        let response = self.graphql_answer(doc, request, headers, oauth, &mut record);
        self.record(record, response.status());
        response
    }

    fn graphql_answer(
        &self,
        doc: &query::Document,
        request: &GraphqlRequest,
        headers: &HeaderMap,
        oauth: &Oauth,
        record: &mut RequestRecord,
    ) -> Response {
        if oauth.credentials(headers) != Credentials::Valid {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"message": "Bad credentials", "documentation_url": "https://docs.github.com/graphql"})),
            )
                .into_response();
        }
        let pulls = record.pull_requests.unwrap_or_default();
        let text = format!("{} {}", request.query, request.variables);
        let (rate, cost) = {
            let mut live = self.live();
            if let Some(fault) = live.take_fault(Target::Graphql, &text) {
                record.fault = Some(fault.name());
                let rate = self.charge(&mut live, Resource::Graphql, 0);
                return fault_response(&fault, Target::Graphql, rate);
            }
            if pulls >= HTML_ERROR_FROM {
                record.fault = Some("oversized_batch");
                tracing::info!(
                    pulls,
                    "fake-github fails an oversized GraphQL batch as GitHub does"
                );
                return if pulls >= TIMEOUT_FROM {
                    (
                        StatusCode::GATEWAY_TIMEOUT,
                        Json(json!({"message": TIMEOUT_MESSAGE})),
                    )
                        .into_response()
                } else {
                    html_error(StatusCode::BAD_GATEWAY)
                };
            }
            let cost = u32::try_from((pulls + pulls / 5).max(1)).unwrap_or(u32::MAX);
            (self.charge(&mut live, Resource::Graphql, cost), cost)
        };
        record.cost = Some(cost);
        let rate_limit = json!({"__typename": "RateLimit", "cost": cost, "limit": rate.limit,
            "remaining": rate.remaining, "used": rate.used, "nodeCount": record.lookups,
            "resetAt": DateTime::<Utc>::from_timestamp(rate.reset, 0).map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true))});
        let body = execute::execute(doc, &self.world, rate_limit, &self.moved_head());
        let mut response = (StatusCode::OK, Json(body)).into_response();
        rate.apply(response.headers_mut());
        response
    }
}

fn html_error(status: StatusCode) -> Response {
    (
        status,
        [(CONTENT_TYPE, "text/html; charset=utf-8")],
        UNICORN_PAGE,
    )
        .into_response()
}

fn respond(answer: Answer, rate: &Rate) -> Response {
    let mut response = match answer {
        Answer::Json { status, body, link } => {
            let mut response = (status, Json(body)).into_response();
            if let Some(link) = link.and_then(|l| HeaderValue::from_str(&l).ok()) {
                response
                    .headers_mut()
                    .insert(axum::http::header::LINK, link);
            }
            response
        }
        Answer::Raw(text) => (
            StatusCode::OK,
            [(CONTENT_TYPE, "application/vnd.github.raw")],
            text,
        )
            .into_response(),
    };
    rate.apply(response.headers_mut());
    response
}

fn fault_response(fault: &FaultAnswer, on: Target, rate: Rate) -> Response {
    tracing::info!(
        fault = fault.name(),
        ?on,
        "fake-github served an injected fault"
    );
    let docs = if on == Target::Graphql {
        "https://docs.github.com/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api"
    } else {
        "https://docs.github.com/rest/using-the-rest-api/rate-limits-for-the-rest-api"
    };
    match fault {
        FaultAnswer::SecondaryRateLimit {
            retry_after,
            status,
        } => {
            let status = StatusCode::from_u16(*status).unwrap_or(StatusCode::FORBIDDEN);
            let mut response = (
                status,
                Json(json!({"message": SECONDARY_MESSAGE, "documentation_url": docs})),
            )
                .into_response();
            rate.apply(response.headers_mut());
            if let Some(value) =
                retry_after.and_then(|s| HeaderValue::from_str(&s.to_string()).ok())
            {
                response.headers_mut().insert(RETRY_AFTER, value);
            }
            response
        }
        FaultAnswer::PrimaryRateLimit { reset } => {
            let mut response = if on == Target::Graphql {
                (
                    StatusCode::OK,
                    Json(json!({"data": null, "errors": [{"type": "RATE_LIMITED",
                    "message": "API rate limit already exceeded for user ID 4820300."}]})),
                )
                    .into_response()
            } else {
                (
                    StatusCode::FORBIDDEN,
                    Json(json!({"message": PRIMARY_MESSAGE, "documentation_url": docs})),
                )
                    .into_response()
            };
            rate.exhausted(*reset).apply(response.headers_mut());
            response
        }
        FaultAnswer::ServerError { status, html } => {
            let status = StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY);
            if *html {
                html_error(status)
            } else {
                (status, Json(json!({"message": TIMEOUT_MESSAGE}))).into_response()
            }
        }
    }
}

/// State of the REST fallback: the recorded endpoints and, when loaded, the dashboard world.
#[derive(Clone)]
pub(crate) struct RestState {
    pub(crate) recorded: Arc<RecordedState>,
    pub(crate) dashboard: Option<Arc<Dashboard>>,
}

/// REST fallback: a recorded endpoint first, then the dashboard world's `GET` endpoints, else 404.
pub(crate) async fn serve_rest(
    State(state): State<RestState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let recorded = state
        .recorded
        .endpoints
        .contains_key(&(method.clone(), uri.path().to_owned()));
    if let Some(dashboard) = &state.dashboard
        && !recorded
        && method == Method::GET
        && let Some(endpoint) = Endpoint::of(uri.path())
    {
        return dashboard.rest(&endpoint, &uri, &headers, &state.recorded.oauth);
    }
    crate::serve_recorded(State(state.recorded), method, uri, headers).await
}

#[derive(Deserialize)]
struct GraphqlRequest {
    query: String,
    #[serde(default)]
    variables: Value,
}

/// State of `POST /graphql`: the v1 review endpoint and, when loaded, the dashboard world.
#[derive(Clone)]
pub(crate) struct GraphqlState {
    pub(crate) graphql: Arc<Graphql>,
    pub(crate) oauth: Arc<Oauth>,
    pub(crate) dashboard: Option<Arc<Dashboard>>,
}

/// `POST /graphql`: dashboard documents (alias batches, `rateLimit`, owner or listing lookups) go to the
/// dashboard world, everything else to the v1 review endpoint.
pub(crate) async fn serve_graphql(
    State(state): State<GraphqlState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(dashboard) = &state.dashboard
        && let Ok(request) = serde_json::from_slice::<GraphqlRequest>(&body)
        && let Ok(doc) = query::parse(&request.query, &request.variables)
        && execute::is_dashboard(&doc)
    {
        return dashboard.graphql(&doc, &request, &headers, &state.oauth);
    }
    crate::graphql::serve(State(state.graphql), headers, body).await
}

/// Test controls of the dashboard world.
pub(crate) fn control_routes(dashboard: Arc<Dashboard>) -> Router {
    Router::new()
        .route("/_fake/dashboard/requests", get(requests))
        .route("/_fake/dashboard/faults", post(add_faults))
        .with_state(dashboard)
}

/// `GET /_fake/dashboard/requests`: every dashboard request served so far, in order.
async fn requests(State(dashboard): State<Arc<Dashboard>>) -> Response {
    Json(dashboard.live().requests.clone()).into_response()
}

/// `POST /_fake/dashboard/faults`: queues faults (`{"faults": [{on, matching?, times?, answer}]}`).
async fn add_faults(
    State(dashboard): State<Arc<Dashboard>>,
    Json(request): Json<Faults>,
) -> StatusCode {
    let valid = request.faults.iter().all(|fault| match &fault.answer {
        FaultAnswer::SecondaryRateLimit { status, .. }
        | FaultAnswer::ServerError { status, .. } => {
            StatusCode::from_u16(*status).is_ok_and(|s| s.is_client_error() || s.is_server_error())
        }
        FaultAnswer::PrimaryRateLimit { .. } => true,
    });
    if !valid {
        return StatusCode::BAD_REQUEST;
    }
    tracing::info!(
        count = request.faults.len(),
        "fake-github queued dashboard faults"
    );
    dashboard.live().faults.extend(request.faults);
    StatusCode::NO_CONTENT
}
