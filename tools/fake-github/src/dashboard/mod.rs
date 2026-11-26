//! The dashboard scan's GitHub (plan v1.1 §A.2, §A.3) over the synthetic world of `fixtures/dashboard/github`
//! (`--dashboard-fixture-dir`): REST search, releases, milestones, contents, pull request files and user events,
//! and GraphQL alias batches with `rateLimit`. Batches of 30 or more `pullRequest` lookups fail like GitHub's
//! (HTML 502, 504 from 50; spike S11-2). Test controls: `POST /_fake/dashboard/faults` queues injected answers
//! (secondary or primary rate limit, 5xx) and `GET /_fake/dashboard/requests` lists every request served.

mod execute;
mod query;
mod rest;
mod schema;
mod search;
mod world;

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::header::{ACCEPT, CONTENT_TYPE, HOST, RETRY_AFTER};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use contracts::time::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::Instant;

use self::rest::{Answer, Endpoint};
use self::world::World;
use crate::graphql::{self, Graphql, GraphqlRequest};
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
    /// `x-ratelimit-remaining: 0` with `x-ratelimit-reset: reset` (epoch seconds). `status` 403 or 429 (REST
    /// default 403); GraphQL defaults to 200 with a `RATE_LIMITED` error.
    PrimaryRateLimit {
        reset: i64,
        #[serde(default)]
        status: Option<u16>,
    },
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
    /// GraphQL: whether the document selects `rateLimit` (plan §A.2: every query does).
    #[serde(skip_serializing_if = "Option::is_none")]
    rate_limit_selected: Option<bool>,
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
    /// Pull requests whose lazily computed merge state was read once.
    merge_reads: HashSet<(String, u64)>,
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
    /// The instant the world was written for, and how far its dates moved (`--dashboard-world-at`).
    world_at: Option<jiff::Timestamp>,
    shift_seconds: i64,
    /// Arrival counter of `RequestRecord::seq`.
    arrivals: AtomicUsize,
    live: Mutex<Live>,
}

impl Dashboard {
    /// Loads the world of `dir`, its dates moved so that `world_at` is now; `pull_path` is the recorded pull
    /// request's REST path.
    pub(crate) fn load(
        dir: &Path,
        graphql: Arc<Graphql>,
        pull_path: &str,
        world_at: Option<jiff::Timestamp>,
    ) -> Result<Self, StartError> {
        let started_epoch = Timestamp::now().as_second();
        let shift_seconds = world_at.map_or(0, |at| started_epoch - at.as_second());
        let world = World::load(dir, shift_seconds)?;
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
            shift_seconds,
            "fake-github loaded the dashboard world"
        );
        Ok(Self {
            world,
            graphql,
            recorded_pull,
            started: Instant::now(),
            started_epoch,
            world_at,
            shift_seconds,
            arrivals: AtomicUsize::new(0),
            live: Mutex::default(),
        })
    }

    fn live(&self) -> MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn record(&self, mut record: RequestRecord, status: StatusCode) {
        record.status = status.as_u16();
        self.live().requests.push(record);
    }

    /// Every request served so far, in order of arrival.
    fn request_log(&self) -> Json<Vec<RequestRecord>> {
        let mut log = self.live().requests.clone();
        log.sort_by_key(|record| record.seq);
        Json(log)
    }

    /// A record of a request arriving now.
    fn new_record(&self, on: Target, method: &'static str, path: &str) -> RequestRecord {
        RequestRecord {
            seq: self.arrivals.fetch_add(1, Ordering::Relaxed) + 1,
            at_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            on,
            method,
            path: path.to_owned(),
            query: None,
            q: None,
            lookups: None,
            pull_requests: None,
            cost: None,
            rate_limit_selected: None,
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

    /// Answers a dashboard document, or GitHub's refusal of an invalid one (`errors` only, HTTP 200).
    fn graphql(
        &self,
        doc: Result<&query::Document, &query::DocumentError>,
        request: &GraphqlRequest,
        headers: &HeaderMap,
    ) -> Response {
        let mut record = self.new_record(Target::Graphql, "POST", "/graphql");
        if let Ok(doc) = doc {
            let (lookups, pulls, rate_limit) = execute::batch_size(doc);
            record.lookups = Some(lookups);
            record.pull_requests = Some(pulls);
            record.rate_limit_selected = Some(rate_limit);
        } else {
            record.rate_limit_selected = Some(false);
        }
        let response = self.graphql_answer(doc, request, headers, &mut record);
        self.record(record, response.status());
        response
    }

    /// GitHub's 401 for a GraphQL request without a valid token (checked before parsing).
    fn unauthorized(&self) -> Response {
        let record = self.new_record(Target::Graphql, "POST", "/graphql");
        let response = (
            StatusCode::UNAUTHORIZED,
            Json(json!({"message": "Bad credentials", "documentation_url": "https://docs.github.com/graphql"})),
        )
            .into_response();
        self.record(record, response.status());
        response
    }

    fn graphql_answer(
        &self,
        doc: Result<&query::Document, &query::DocumentError>,
        request: &GraphqlRequest,
        headers: &HeaderMap,
        record: &mut RequestRecord,
    ) -> Response {
        if let Some(rejection) = require_user_agent(headers) {
            return rejection;
        }
        let pulls = record.pull_requests.unwrap_or_default();
        let text = format!("{} {}", request.query, request.variables);
        let (rate, cost, doc) = {
            let mut live = self.live();
            if let Some(fault) = live.take_fault(Target::Graphql, &text) {
                record.fault = Some(fault.name());
                let rate = self.charge(&mut live, Resource::Graphql, 0);
                return fault_response(&fault, Target::Graphql, rate);
            }
            let doc = match doc {
                Ok(doc) => doc,
                Err(error) => {
                    tracing::debug!(%error, "fake-github refused an invalid dashboard GraphQL document");
                    return (
                        StatusCode::OK,
                        Json(json!({"errors": [{"message": error.to_string(), "locations": []}]})),
                    )
                        .into_response();
                }
            };
            let problems = self.world.schema.validate(doc);
            if !problems.is_empty() {
                tracing::debug!(
                    count = problems.len(),
                    "fake-github refused a dashboard GraphQL document"
                );
                return (StatusCode::OK, Json(json!({ "errors": problems }))).into_response();
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
            (self.charge(&mut live, Resource::Graphql, cost), cost, doc)
        };
        record.cost = Some(cost);
        let rate_limit = json!({"__typename": "RateLimit", "cost": cost, "limit": rate.limit,
            "remaining": rate.remaining, "used": rate.used, "nodeCount": record.lookups,
            "resetAt": Timestamp::from_second(rate.reset).map(|t| format!("{:.0}", t.get()))});
        let moved_head = self.moved_head();
        let body = execute::execute(
            doc,
            &self.world,
            rate_limit,
            &moved_head,
            &mut self.live().merge_reads,
        );
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
        FaultAnswer::PrimaryRateLimit { reset, status } => {
            let status = status.and_then(|s| StatusCode::from_u16(s).ok());
            let mut response =
                if on == Target::Graphql && status.is_none_or(|s| s == StatusCode::OK) {
                    (
                        StatusCode::OK,
                        Json(json!({"data": null, "errors": [{"type": "RATE_LIMITED",
                    "message": "API rate limit already exceeded for user ID 4820300."}]})),
                    )
                        .into_response()
                } else {
                    (
                        status.unwrap_or(StatusCode::FORBIDDEN),
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

/// State of `POST /graphql`: the v1 review endpoint and, when loaded, the dashboard world and the review-loop
/// recordings.
#[derive(Clone)]
pub(crate) struct GraphqlState {
    pub(crate) graphql: Arc<Graphql>,
    pub(crate) oauth: Arc<Oauth>,
    pub(crate) dashboard: Option<Arc<Dashboard>>,
    pub(crate) review_loop: Option<Arc<crate::review_loop::ReviewLoop>>,
}

/// `POST /graphql`: with the dashboard world loaded, only what v1 sends (mutations, `node` lookups, one
/// unaliased lookup of the recorded pull request) reaches the v1 review endpoint; every other document,
/// including an unparsable or invalid one, is answered by the dashboard world.
pub(crate) async fn serve_graphql(
    State(state): State<GraphqlState>,
    headers: HeaderMap,
    request: Result<Json<GraphqlRequest>, JsonRejection>,
) -> Response {
    let request = match request {
        Err(JsonRejection::BytesRejection(error)) => return error.into_response(),
        request => request.ok(),
    };
    let request = request.as_ref().map(|Json(request)| request);
    if let Some(review_loop) = &state.review_loop
        && let Some(request) = request
        && let Some(answer) = review_loop.answer(&request.query, &request.variables)
    {
        if state.oauth.credentials(&headers) != Credentials::Valid {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"message": "Bad credentials", "documentation_url": "https://docs.github.com/graphql"})),
            )
                .into_response();
        }
        return answer;
    }
    if let Some(dashboard) = &state.dashboard
        && let Some(request) = request
    {
        if state.oauth.credentials(&headers) != Credentials::Valid {
            return dashboard.unauthorized();
        }
        let doc = query::parse(&request.query)
            .map_err(query::DocumentError::from)
            .and_then(|mut doc| {
                if doc.kind == query::Kind::Query {
                    doc.resolve(&request.variables)?;
                }
                Ok(doc)
            });
        match doc {
            Ok(doc) if execute::routes_to_v1(&doc, dashboard.recorded_pull.as_ref()) => {}
            Ok(doc) => return dashboard.graphql(Ok(&doc), request, &headers),
            Err(error) => return dashboard.graphql(Err(&error), request, &headers),
        }
    }
    graphql::serve(&state.graphql, &headers, request)
}

/// Test controls of the dashboard world.
pub(crate) fn control_routes(dashboard: Arc<Dashboard>) -> Router {
    Router::new()
        .route("/_fake/dashboard/requests", get(requests))
        .route("/_fake/dashboard/faults", post(add_faults))
        .route("/_fake/dashboard/clock", get(clock))
        .with_state(dashboard)
}

/// `GET /_fake/dashboard/clock`: the instant the world was written for and how far its dates moved.
async fn clock(State(dashboard): State<Arc<Dashboard>>) -> Json<Value> {
    Json(json!({
        "world_at": dashboard.world_at.map(|at| at.to_string()),
        "shift_seconds": dashboard.shift_seconds,
    }))
}

/// `GET /_fake/dashboard/requests`: every dashboard request served so far, in order.
async fn requests(State(dashboard): State<Arc<Dashboard>>) -> Response {
    dashboard.request_log().into_response()
}

/// `POST /_fake/dashboard/faults`: queues faults (`{"faults": [{on, matching?, times?, answer}]}`).
async fn add_faults(
    State(dashboard): State<Arc<Dashboard>>,
    Json(request): Json<Faults>,
) -> StatusCode {
    let valid =
        request.faults.iter().all(|fault| match &fault.answer {
            FaultAnswer::SecondaryRateLimit { status, .. }
            | FaultAnswer::ServerError { status, .. } => StatusCode::from_u16(*status)
                .is_ok_and(|s| s.is_client_error() || s.is_server_error()),
            FaultAnswer::PrimaryRateLimit { status, .. } => status
                .is_none_or(|s| matches!((fault.on, s), (_, 403 | 429) | (Target::Graphql, 200))),
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn dashboard() -> Dashboard {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let config = crate::Config {
            fixture_dir: root.join("fixtures/pr-482/github"),
            app_url: url::Url::parse("http://app.test:3000").unwrap(),
            user_login: None,
            user_id: None,
            app_id: None,
            app_public_key_file: None,
            dashboard_fixture_dir: None,
            dashboard_world_at: None,
            review_loop_fixture_dir: None,
        };
        let oauth = Arc::new(Oauth::load(&config).unwrap());
        let graphql = Arc::new(Graphql::load(&config, oauth).unwrap());
        Dashboard::load(
            &root.join("fixtures/dashboard/github"),
            graphql,
            "/repos/acme/node-client/pulls/482",
            None,
        )
        .unwrap()
    }

    #[test]
    fn seq_is_the_order_of_arrival() {
        let dashboard = dashboard();
        let first = dashboard.new_record(Target::Rest, "GET", "/first");
        let second = dashboard.new_record(Target::Rest, "GET", "/second");
        dashboard.record(second, StatusCode::OK);
        dashboard.record(first, StatusCode::OK);
        let Json(log) = dashboard.request_log();
        let order: Vec<(String, usize)> = log.iter().map(|r| (r.path.clone(), r.seq)).collect();
        assert_eq!(
            order,
            vec![("/first".to_owned(), 1), ("/second".to_owned(), 2)]
        );
    }
}
