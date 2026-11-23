//! Acceptance tests of the refresh through the HTTP API (R1–R5): the real scan client against the in-process fake
//! GitHub (`tools/fake-github` with `fixtures/dashboard/github`), an in-process fake MemCan serving
//! `fixtures/dashboard/scan/todos.json` and the injected clock reading 2026-11-18 09:00 UTC at the start.
//!
//! Real time, not paused: under tokio's paused clock sqlx's in-memory pool times out (its acquire timeout fires
//! while the sqlite worker thread answers). Searches are not paced here (`DashboardHub::testing`), every wait is on
//! hub events bounded by [`WAIT`], and a gate in front of the fake GitHub holds requests to stop a scan mid-way.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use contracts::api::dashboard::{
    AwaitingReason, DashboardEvent, DashboardSettings, DashboardSummary, ExcludedItem,
    ExclusionCount, ExclusionReason, Exclusions, HistoryKind, ItemCounts, ItemKey, ItemKind,
    ItemPage, ItemTarget, KindCounts, PolicyRef, RankingState, RefreshFailureCode, RefreshState,
    RefreshStatus, RepositoryRule, ScanStepKey, ScoreState, StepState, Tier, TodosStatus, WorkItem,
};
use contracts::common::RepoRef;
use contracts::ids::RefreshId;
use contracts::time::Timestamp;
use db::repo::users::GithubProfile;
use db::repo::{DEFAULT_TENANT, Db};
use futures::StreamExt;
use futures::future::BoxFuture;
use http_body_util::BodyExt;
use jiff::SignedDuration;
use reqwest::header::{ACCEPT, LOCATION};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ServerCapabilities,
    ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use secrecy::SecretString;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;
use tower::ServiceExt;
use url::Url;

use super::rank::{ItemJudgement, NoopRanker, RankRun, Ranker};
use super::*;
use crate::api;
use crate::auth::{lock_user, store_sign_in_tokens};
use crate::config::MemcanConfig;
use crate::config::tests::test_config;
use crate::dashboard::hub::{Clock, DashboardHub};
use crate::github::oauth::TokenGrant;
use crate::secrets::sha256;

const VERIFIER: &str = "refresh-test-verifier-0123456789-0123456789-0123456789";
const MEMCAN_TOKEN: &str = "memcan-token-of-the-refresh-tests";
/// Bound of every wait on a refresh (real time; a refresh against the in-process fakes takes well under a second).
const WAIT: Duration = Duration::from_secs(20);

fn now() -> Timestamp {
    "2026-11-18T09:00:00Z".parse::<Timestamp>().expect("now")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn repo(owner: &str, name: &str) -> RepoRef {
    RepoRef {
        owner: owner.to_owned(),
        name: name.to_owned(),
    }
}

fn row(name: &str, alias: &str, watch_issues: bool) -> RepositoryRule {
    RepositoryRule {
        repo: repo("acme", name),
        alias: Some(alias.to_owned()),
        watch_issues,
        ownership_policy: None,
        branch_patterns: Vec::new(),
        ignored_checks: Vec::new(),
    }
}

/// Settings of the dashboard world (`fixtures/dashboard/github/README.md`).
fn settings() -> DashboardSettings {
    let mut platform = row("platform", "PLAT", true);
    platform.ownership_policy = Some(PolicyRef {
        repo: repo("acme", "review-policies"),
        path: "policies/platform.json".to_owned(),
    });
    DashboardSettings {
        linked_logins: vec!["alice-helper".to_owned()],
        repositories: vec![
            platform,
            row("wallet-ui", "WUI", false),
            row("node-client", "NC", false),
            row("consensus", "CON", false),
            row("docs-site", "DOC", false),
            row("ops-runbooks", "OPS", false),
            row("secret", "SEC", false),
        ],
        enumerate_owners: vec!["acme".to_owned()],
        time_zone: Some("Europe/Warsaw".to_owned()),
        ..DashboardSettings::default()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Fakes

/// Holds the fake GitHub's dashboard requests while closed, and counts them.
#[derive(Default)]
struct Gate {
    closed: AtomicBool,
    requests: AtomicUsize,
    arrived: Notify,
    opened: Notify,
}

impl Gate {
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    fn open(&self) {
        self.closed.store(false, Ordering::SeqCst);
        self.opened.notify_waiters();
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    /// Waits until a request is held at the closed gate.
    async fn held(&self) {
        tokio::time::timeout(WAIT, self.arrived.notified())
            .await
            .expect("a GitHub request reaches the gate");
    }
}

async fn github_gate(State(gate): State<Arc<Gate>>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if !path.starts_with("/_fake") && !path.starts_with("/login") {
        gate.requests.fetch_add(1, Ordering::SeqCst);
        loop {
            let opened = gate.opened.notified();
            tokio::pin!(opened);
            opened.as_mut().enable();
            if !gate.closed.load(Ordering::SeqCst) {
                break;
            }
            gate.arrived.notify_one();
            opened.await;
        }
    }
    next.run(request).await
}

async fn spawn_github(gate: Arc<Gate>, fixture: PathBuf) -> String {
    let config = fake_github::Config {
        fixture_dir: repo_root().join("fixtures/pr-482/github"),
        app_url: Url::parse("https://review.example.test").expect("url"),
        user_login: None,
        user_id: None,
        app_id: None,
        app_public_key_file: None,
        dashboard_fixture_dir: Some(fixture),
        dashboard_world_at: None,
        review_loop_fixture_dir: None,
    };
    let router = fake_github::router(&config)
        .expect("fake GitHub")
        .layer(from_fn_with_state(gate, github_gate));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, router).await });
    format!("http://{addr}")
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client")
}

/// The fake's OAuth web flow; returns the token pair.
async fn sign_in(base: &str) -> TokenGrant {
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()));
    let mut authorize = Url::parse(&format!("{base}/login/oauth/authorize")).expect("url");
    authorize.query_pairs_mut().extend_pairs([
        ("client_id", "fixture-app-client-id"),
        (
            "redirect_uri",
            "https://review.example.test/auth/github/callback",
        ),
        ("state", "s"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
    ]);
    let response = http().get(authorize).send().await.expect("authorize");
    let location =
        Url::parse(response.headers()[LOCATION].to_str().expect("location")).expect("url");
    let code = location
        .query_pairs()
        .find(|(k, _)| k == "code")
        .expect("code")
        .1
        .into_owned();
    let body: Value = http()
        .post(format!("{base}/login/oauth/access_token"))
        .header(ACCEPT, "application/json")
        .form(&[
            ("client_id", "fixture-app-client-id"),
            ("client_secret", "fixture-client-secret"),
            ("code", &code),
            (
                "redirect_uri",
                "https://review.example.test/auth/github/callback",
            ),
            ("code_verifier", VERIFIER),
        ])
        .send()
        .await
        .expect("token")
        .json()
        .await
        .expect("json");
    TokenGrant {
        access_token: SecretString::from(body["access_token"].as_str().expect("access").to_owned()),
        expires_in: body["expires_in"].as_i64().unwrap_or(28_800),
        refresh_token: SecretString::from(
            body["refresh_token"].as_str().expect("refresh").to_owned(),
        ),
        refresh_token_expires_in: body["refresh_token_expires_in"]
            .as_i64()
            .unwrap_or(15_811_200),
    }
}

/// MemCan's `list_todos` over MCP from `fixtures/dashboard/scan/todos.json`, in-process; `down` answers 503.
#[derive(Clone)]
struct FakeTodos {
    url: Url,
    down: Arc<AtomicBool>,
}

#[derive(Clone)]
struct TodoHandler(Arc<HashMap<String, Value>>);

impl ServerHandler for TodoHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name != "list_todos" {
            return Err(ErrorData::invalid_params("unknown tool", None));
        }
        let arguments = request.arguments.unwrap_or_default();
        let project = arguments
            .get("project")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let status = arguments.get("status").and_then(Value::as_str);
        let todos: Vec<Value> = self
            .0
            .get(project)
            .and_then(|page| page["todos"].as_array())
            .into_iter()
            .flatten()
            .filter(|todo| status.is_none_or(|s| todo["status"] == s))
            .cloned()
            .collect();
        let answer = json!({"todos": todos, "has_more": false});
        Ok(CallToolResult::success(vec![ContentBlock::text(answer.to_string())]).into())
    }
}

async fn todo_gate(State(down): State<Arc<AtomicBool>>, request: Request, next: Next) -> Response {
    let bearer = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if down.load(Ordering::SeqCst) {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    } else if bearer.as_deref() != Some(&format!("Bearer {MEMCAN_TOKEN}")) {
        StatusCode::UNAUTHORIZED.into_response()
    } else {
        next.run(request).await
    }
}

impl FakeTodos {
    async fn start() -> Self {
        let text = std::fs::read_to_string(repo_root().join("fixtures/dashboard/scan/todos.json"))
            .expect("todos fixture");
        let pages: HashMap<String, Value> = serde_json::from_str(&text).expect("todos json");
        let handler = TodoHandler(Arc::new(pages));
        let service: StreamableHttpService<TodoHandler, LocalSessionManager> =
            StreamableHttpService::new(
                move || Ok(handler.clone()),
                Arc::default(),
                StreamableHttpServerConfig::default(),
            );
        let down = Arc::new(AtomicBool::new(false));
        let router = axum::Router::new()
            .nest_service("/mcp", service)
            .layer(from_fn_with_state(Arc::clone(&down), todo_gate));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, router).await });
        Self {
            url: Url::parse(&format!("http://{addr}/mcp")).expect("url"),
            down,
        }
    }

    fn config(&self) -> MemcanConfig {
        MemcanConfig {
            observed: Default::default(),
            url: self.url.clone(),
            token: SecretString::from(MEMCAN_TOKEN),
        }
    }

    fn set_down(&self, down: bool) {
        self.down.store(down, Ordering::SeqCst);
    }
}

/// Never ends its ranking (a ranking waiting for the LLM).
#[derive(Debug)]
struct StallingRanker;

impl Ranker for StallingRanker {
    fn rank(&self, _state: AppState, _run: RankRun) -> BoxFuture<'static, ()> {
        Box::pin(std::future::pending())
    }

    fn judgements<'a>(
        &'a self,
        _state: &'a AppState,
        _scope: &'a Scope,
        _snapshot: &'a crate::dashboard::snapshot::Snapshot,
        _overlay: &'a crate::dashboard::plugin::Overlay,
    ) -> BoxFuture<'a, Result<HashMap<contracts::api::dashboard::ItemKey, ItemJudgement>, DbError>>
    {
        Box::pin(async { Ok(HashMap::new()) })
    }
}

/// Panics in its ranking (a bug in the ranking step).
#[derive(Debug)]
struct PanickingRanker;

impl Ranker for PanickingRanker {
    fn rank(&self, _state: AppState, _run: RankRun) -> BoxFuture<'static, ()> {
        Box::pin(async { panic!("ranking bug") })
    }

    fn judgements<'a>(
        &'a self,
        _state: &'a AppState,
        _scope: &'a Scope,
        _snapshot: &'a crate::dashboard::snapshot::Snapshot,
        _overlay: &'a crate::dashboard::plugin::Overlay,
    ) -> BoxFuture<'a, Result<HashMap<contracts::api::dashboard::ItemKey, ItemJudgement>, DbError>>
    {
        Box::pin(async { Ok(HashMap::new()) })
    }
}

// ---------------------------------------------------------------------------------------------------------------
// World

struct World {
    state: AppState,
    github: String,
    gate: Arc<Gate>,
    todos: FakeTodos,
}

const ALICE: &str = "__Host-sr_session=alice-session";
const COLLEAGUE: &str = "__Host-sr_session=colleague-session";

async fn add_user(state: &AppState, user_id: &str, login: &str, github_id: i64, token: &str) {
    let real_now = Timestamp::now();
    db::repo::tenants::testing::allow_login(&state.db, DEFAULT_TENANT, login, github_id)
        .await
        .expect("allow");
    state
        .db
        .users()
        .upsert_member(
            DEFAULT_TENANT,
            user_id,
            &GithubProfile {
                github_id,
                login: login.to_owned(),
                name: None,
                avatar_url: None,
            },
            real_now,
        )
        .await
        .expect("user");
    state
        .db
        .sessions()
        .create(
            &db::repo::sessions::NewSession {
                token_hash: &sha256(token.as_bytes()),
                tenant_id: DEFAULT_TENANT,
                user_id,
                csrf_token: "csrf",
                expires_at: real_now + SignedDuration::from_hours(1),
            },
            real_now,
        )
        .await
        .expect("session");
}

async fn world_with(ranker: Arc<dyn Ranker>, scan_limit: Duration) -> World {
    world_in(
        ranker,
        scan_limit,
        repo_root().join("fixtures/dashboard/github"),
    )
    .await
}

async fn world_in(ranker: Arc<dyn Ranker>, scan_limit: Duration, fixture: PathBuf) -> World {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("warn,server=debug")
        .try_init();
    let gate = Arc::new(Gate::default());
    let github = spawn_github(Arc::clone(&gate), fixture).await;
    let todos = FakeTodos::start().await;
    let mut config = test_config();
    config.github.api_url = Url::parse(&github).expect("url");
    config.github.oauth_url = Url::parse(&github).expect("url");
    config.memcan = todos.config();
    config.memcan_ui_url = Some("http://memcan.example.test/ui/".to_owned());
    let mut state = AppState::with_queue(config, Db::in_memory().await.expect("db"));
    add_user(&state, "u1", "alice", 4_820_300, "alice-session").await;
    add_user(&state, "u2", "colleague", 77, "colleague-session").await;
    let grant = sign_in(&github).await;
    let lock = lock_user("u1").await;
    store_sign_in_tokens(&state, &lock, DEFAULT_TENANT, &grant, Timestamp::now())
        .await
        .expect("token pair");
    drop(lock);
    state
        .db
        .dashboard()
        .save_settings(DEFAULT_TENANT, "u1", &settings(), Timestamp::now())
        .await
        .expect("settings");
    state.dashboard = DashboardHub::testing(
        Clock::starting_at(now()),
        scan_limit,
        ranker,
        Duration::ZERO,
    );
    World {
        state,
        github,
        gate,
        todos,
    }
}

fn alice() -> Scope {
    Scope {
        tenant_id: DEFAULT_TENANT.to_owned(),
        user_id: "u1".to_owned(),
    }
}

async fn world() -> World {
    world_with(Arc::new(NoopRanker), Duration::from_secs(24 * 3600)).await
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl World {
    async fn send(&self, method: &str, uri: &str, cookie: &str, extra: &[(&str, &str)]) -> Answer {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header("cookie", cookie);
        if method != "GET" && !extra.iter().any(|(name, _)| *name == "x-csrf-token") {
            request = request.header("x-csrf-token", "csrf");
        }
        for (name, value) in extra {
            request = request.header(*name, *value);
        }
        let response = api::router(self.state.clone())
            .oneshot(request.body(Body::empty()).expect("request"))
            .await
            .expect("response");
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("JSON body")
        };
        Answer {
            status,
            headers,
            body,
        }
    }

    async fn start(&self) -> RefreshStatus {
        let answer = self
            .send("POST", "/api/v1/dashboard/refreshes", ALICE, &[])
            .await;
        assert_eq!(answer.status, StatusCode::CREATED, "{}", answer.body);
        serde_json::from_value(answer.body).expect("status")
    }

    /// The stored status of refresh `id` of either user.
    async fn status(&self, id: RefreshId) -> RefreshStatus {
        let dashboard = self.state.db.dashboard();
        let record = match dashboard.refresh(DEFAULT_TENANT, "u1", id).await {
            Err(DbError::NotFound) => dashboard.refresh(DEFAULT_TENANT, "u2", id).await,
            other => other,
        };
        status_of(&record.expect("refresh"))
    }

    /// Waits until the refresh's status satisfies `done`.
    async fn wait_until(
        &self,
        id: RefreshId,
        done: impl Fn(&RefreshStatus) -> bool,
    ) -> RefreshStatus {
        let mut events = self.state.dashboard.subscribe();
        let mut status = self.status(id).await;
        let reached = tokio::time::timeout(WAIT, async {
            while !done(&status) {
                match events.recv().await {
                    Ok(event) if event.id == id => status = event,
                    Ok(_) => {}
                    Err(_) => status = self.status(id).await,
                }
            }
        })
        .await;
        assert!(
            reached.is_ok(),
            "refresh never reached the state: {status:?}"
        );
        status
    }

    /// Waits until the refresh ended and its task too (no database work left behind).
    async fn settle(&self, id: RefreshId) -> RefreshStatus {
        let mut stopped = self.state.dashboard.stopped();
        self.wait_until(id, |s| {
            matches!(
                s.state,
                RefreshState::Finished | RefreshState::Failed | RefreshState::Cancelled
            )
        })
        .await;
        if self.state.dashboard.is_running(id) {
            task_stopped(&mut stopped, id).await;
        }
        self.status(id).await
    }

    async fn refreshed(&self) -> RefreshStatus {
        let started = self.start().await;
        self.settle(started.id).await
    }

    async fn summary(&self, cookie: &str) -> DashboardSummary {
        self.summary_with(cookie, "").await
    }

    async fn summary_with(&self, cookie: &str, query: &str) -> DashboardSummary {
        let answer = self
            .send("GET", &format!("/api/v1/dashboard{query}"), cookie, &[])
            .await;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
        serde_json::from_value(answer.body).expect("summary")
    }

    async fn items(&self, query: &str) -> ItemPage {
        let answer = self
            .send(
                "GET",
                &format!("/api/v1/dashboard/items{query}"),
                ALICE,
                &[],
            )
            .await;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
        serde_json::from_value(answer.body).expect("page")
    }

    async fn faults(&self, faults: Value) {
        let status = http()
            .post(format!("{}/_fake/dashboard/faults", self.github))
            .json(&json!({ "faults": faults }))
            .send()
            .await
            .expect("faults")
            .status();
        assert_eq!(status, 204);
    }
}

/// Waits until the task of refresh `id` ended (completed or aborted).
async fn task_stopped(stopped: &mut tokio::sync::broadcast::Receiver<RefreshId>, id: RefreshId) {
    let ended = tokio::time::timeout(WAIT, async {
        while stopped.recv().await.expect("stop signals") != id {}
    })
    .await;
    assert!(ended.is_ok(), "the task of refresh {id} never ended");
}

fn path(id: RefreshId, action: &str) -> String {
    format!("/api/v1/dashboard/refreshes/{id}{action}")
}

fn alias_ids(items: &[WorkItem]) -> Vec<&str> {
    items.iter().map(|item| item.id.as_str()).collect()
}

impl World {
    /// The server after a restart: same database, a fresh hub, startup recovery run.
    async fn restarted(&self, ranker: Arc<dyn Ranker>) -> Self {
        let mut state = self.state.clone();
        state.dashboard = DashboardHub::testing(
            Clock::starting_at(now() + SignedDuration::from_mins(5)),
            Duration::from_secs(24 * 3600),
            ranker,
            Duration::ZERO,
        );
        recover(&state).await.expect("recover");
        Self {
            state,
            github: self.github.clone(),
            gate: Arc::clone(&self.gate),
            todos: self.todos.clone(),
        }
    }

    /// Starts a refresh and waits until GitHub holds its first request.
    async fn start_held(&self) -> RefreshStatus {
        self.gate.close();
        let started = self.start().await;
        self.gate.held().await;
        started
    }

    async fn error_code(&self, method: &str, uri: &str, cookie: &str) -> (StatusCode, Value) {
        let answer = self.send(method, uri, cookie, &[]).await;
        (answer.status, answer.body["code"].clone())
    }
}

/// The dashboard world as the scan sees it on 2026-11-18 (`fixtures/dashboard/github/README.md`). The ownership
/// policy of `acme/platform` finds only minor owned changes in #1176, #1199 and #1203, but all three are directed
/// at alice (authored by her, or her review requested directly), so the policy keeps them (D19). #1176 (hers,
/// ready, reviews requested from others, nothing her move) waits only on others: Waiting on others, not ranked (B10).
/// Every scanned item is in exactly one place: Needs you, Waiting on others or Excluded.
#[tokio::test]
async fn full_refresh_stores_the_expected_snapshot() {
    let w = world().await;
    let done = w.refreshed().await;
    assert_eq!(done.state, RefreshState::Finished, "{done:?}");
    assert_eq!(done.failure, None);
    assert_eq!(
        done.ranking.as_ref().map(|r| r.state),
        Some(RankingState::Finished)
    );
    assert_eq!(done.percent, 100);
    assert!(
        done.steps.iter().all(|step| step.state == StepState::Done),
        "{:?}",
        done.steps
    );
    assert!(
        done.log.iter().any(|line| line.contains("scan complete")),
        "{:?}",
        done.log
    );

    let summary = w.summary(ALICE).await;
    let meta = summary.snapshot.expect("snapshot");
    assert_eq!(meta.refresh_id, done.id);
    assert_eq!(
        meta.exclusions,
        Some(Exclusions {
            total: 1,
            counts: vec![ExclusionCount {
                reason: ExclusionReason::ExcludedAuthor,
                count: 1
            }],
            items: None,
        }),
        "counts only without ?excluded=true"
    );
    let node_client = || RepoRef {
        owner: "acme".to_owned(),
        name: "node-client".to_owned(),
    };
    let listed = w.summary_with(ALICE, "?excluded=true").await;
    let items = listed
        .snapshot
        .and_then(|s| s.exclusions)
        .and_then(|e| e.items)
        .expect("listed");
    assert_eq!(
        items,
        [ExcludedItem {
            key: ItemKey::new(&ItemTarget::PullRequest {
                repo: node_client(),
                number: 495
            })
            .expect("key"),
            kind: ItemKind::Pr,
            id: "NC-495".to_owned(),
            repo: Some(node_client()),
            number: Some(495),
            url: Some("https://github.com/acme/node-client/pull/495".to_owned()),
            title: "chore(deps): bump serde from 1.0.228 to 1.0.229".to_owned(),
            author: Some("dependabot[bot]".to_owned()),
            reason: ExclusionReason::ExcludedAuthor,
            merged_into: None,
            snoozed_until: None,
        }]
    );
    assert_eq!(summary.latest_refresh.as_ref().map(|r| r.id), Some(done.id));
    assert_eq!(
        meta.item_counts,
        ItemCounts {
            total: 18,
            pr: 11,
            issue: 5,
            todo: 2,
            p0: 0,
            p1: 7,
            p2: 5,
            p3: 6,
            not_judged: 18,
        }
    );
    assert_eq!(meta.excluded, 1, "the excluded bot author");
    assert_eq!(
        meta.checked,
        KindCounts {
            pr: 14,
            issue: 5,
            todo: 2
        }
    );
    let awaiting: Vec<_> = meta
        .awaiting
        .iter()
        .map(|a| {
            (
                a.id.as_str(),
                a.waiting_on.clone(),
                a.reason.clone(),
                a.waiting_since,
            )
        })
        .collect();
    assert_eq!(
        awaiting,
        [
            (
                "PLAT-1176",
                vec!["jun-park".to_owned(), "team:core".to_owned()],
                AwaitingReason::ReadyLabel {
                    label: "ready-for-human".to_owned()
                },
                at("2026-11-07T10:00:00Z")
            ),
            (
                "CON-1541",
                vec!["jun-park".to_owned()],
                AwaitingReason::ReviewRequested,
                at("2026-11-17T12:00:00Z")
            )
        ],
        "K15 W3a (the label) and W3c (a requested review, not approved, not mergeable; no label needed)"
    );
    assert_eq!(meta.awaiting[0].total_tenths, None, "not ranked");
    let ready: Vec<_> = meta
        .ready_to_merge
        .iter()
        .map(|r| (r.id.as_str(), r.url.as_str()))
        .collect();
    assert_eq!(
        ready,
        [("NC-476", "https://github.com/acme/node-client/pull/476")],
        "only the user's PR into the default branch that GitHub lets the user merge (WUI-298 targets a feature branch)"
    );
    assert_eq!(
        meta.merge_state_pending,
        Some(0),
        "NC-476 was UNKNOWN at the details read and CLEAN when read again; WUI-298 targets a feature branch"
    );
    assert_eq!(
        meta.checked.pr,
        meta.item_counts.pr + u32::try_from(meta.awaiting.len()).expect("count") + meta.excluded,
        "scanned = Needs you + Waiting on others + Excluded"
    );
    let history: Vec<_> = meta
        .history
        .iter()
        .map(|e| (e.kind, e.item_id.as_str()))
        .collect();
    use HistoryKind::{PrClosedUnmerged, PrMerged, PrOpened, ReviewGiven};
    assert_eq!(
        history,
        [
            (ReviewGiven, "acme/consensus#1510"),
            (PrOpened, "acme/legacy-wallet#9"),
            (PrOpened, "acme/consensus#1501"),
            (PrOpened, "acme/platform#1190"),
            (PrOpened, "acme/wallet-ui#300"),
            (PrMerged, "acme/consensus#1501"),
            (PrMerged, "acme/platform#1190"),
            (PrOpened, "acme/platform#1176"),
            (PrOpened, "acme/node-client#490"),
            (PrOpened, "acme/platform#1231"),
            (PrClosedUnmerged, "acme/wallet-ui#300"),
            (PrOpened, "acme/wallet-ui#311"),
            (PrOpened, "acme/platform#1233"),
            (PrOpened, "acme/consensus#1536"),
            (ReviewGiven, "acme/platform#1195"),
            (PrOpened, "acme/node-client#476"),
            (PrOpened, "acme/wallet-ui#298"),
            (PrMerged, "acme/platform#1233"),
            (ReviewGiven, "acme/consensus#1534"),
            (PrOpened, "acme/consensus#1541"),
            (PrMerged, "acme/platform#1231"),
            (PrOpened, "acme/review-policies#7"),
            (PrMerged, "acme/consensus#1536"),
            (PrMerged, "acme/review-policies#7"),
        ],
        "12 weeks back from local midnight; acme/platform#1100 is older"
    );
    assert_eq!(meta.unreachable, ["acme/secret"]);
    let notes: Vec<&str> = meta.notes.iter().map(|n| n.text.as_str()).collect();
    assert!(notes.iter().any(|n| n.contains("acme/secret")), "{notes:?}");
    assert!(
        notes.iter().any(|n| n.contains("alice-helper")),
        "{notes:?}"
    );
    assert_eq!(meta.todos.status, TodosStatus::Read);
    assert_eq!(meta.todos.failed_at, None);

    let page = w.items("?limit=500").await;
    assert_eq!(page.revision, summary.revision);
    assert_eq!(page.next_cursor, None);
    assert_eq!(
        alias_ids(&page.items),
        [
            "PLAT-1203",
            "WUI-320",
            "NC-482",
            "WUI-311",
            "DOC-77",
            "NC-476",
            "CON-1534",
            "OPS-14",
            "PLAT-1199",
            "PLAT-1187",
            "WUI-298",
            "NC-490",
            "PLAT-1210",
            "TODO-3fa91c2e",
            "CON-1520",
            "CON-1525",
            "PLAT-1215",
            "TODO-91bd0a44",
        ]
    );
    let first = &page.items[0];
    assert_eq!(first.score.tier, Tier::P1);
    assert_eq!(first.score.state, ScoreState::Unjudged);
    assert_eq!(
        first.url.as_deref(),
        Some("https://github.com/acme/platform/pull/1203")
    );
    let ops = page
        .items
        .iter()
        .find(|i| i.id == "OPS-14")
        .expect("OPS-14");
    assert!(ops.private, "private repository");
    let todo = page
        .items
        .iter()
        .find(|i| i.kind == ItemKind::Todo)
        .expect("a TODO");
    assert!(
        todo.url
            .as_deref()
            .is_some_and(|u| u.starts_with("http://memcan.example.test/ui/tasks/")),
        "{:?}",
        todo.url
    );
    assert!(
        page.items.iter().all(|item| item.review.is_none()),
        "no analysis exists"
    );
}

fn at(text: &str) -> Timestamp {
    text.parse().expect("time")
}

#[tokio::test]
async fn cancel_keeps_previous_snapshot() {
    let w = world().await;
    let first = w.refreshed().await;
    let running = w.start_held().await;
    assert_eq!(running.state, RefreshState::Scanning);
    let mut stopped = w.state.dashboard.stopped();
    let calls = w.gate.requests();
    let answer = w
        .send("POST", &path(running.id, "/cancel"), ALICE, &[])
        .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let cancelled: RefreshStatus = serde_json::from_value(answer.body).expect("status");
    assert_eq!(cancelled.state, RefreshState::Cancelled);
    // The scan's task is dropped while its GitHub call is held: it can make no further call.
    task_stopped(&mut stopped, running.id).await;
    w.gate.open();
    assert_eq!(w.gate.requests(), calls, "no GitHub call after the cancel");
    let cancel_line = cancelled.log.len();
    assert_eq!(
        w.status(running.id).await.log.len(),
        cancel_line,
        "no progress written after the cancel"
    );
    assert_eq!(w.status(running.id).await.state, RefreshState::Cancelled);
    let summary = w.summary(ALICE).await;
    assert_eq!(summary.snapshot.expect("snapshot").refresh_id, first.id);
    assert_eq!(summary.latest_refresh.map(|r| r.id), Some(running.id));
    let (status, code) = w
        .error_code("POST", &path(running.id, "/cancel"), ALICE)
        .await;
    assert_eq!((status, code), (StatusCode::CONFLICT, json!("conflict")));
}

/// A cancel that lands after the refresh is stored but before its task runs (no task to abort yet) still stops
/// the scan before its first GitHub call.
#[tokio::test]
async fn cancel_before_the_task_runs_stops_the_scan() {
    let w = world().await;
    let mut stopped = w.state.dashboard.stopped();
    let started = start(&w.state, &alice(), RefreshTrigger::Manual)
        .await
        .expect("start");
    // The spawned task has not run yet (current-thread runtime): end the scan as R3 does in that window.
    w.state
        .db
        .dashboard()
        .end_scan(
            DEFAULT_TENANT,
            "u1",
            started.id,
            &db::repo::dashboard::ScanEnd::Cancelled,
            now(),
        )
        .await
        .expect("cancel");
    task_stopped(&mut stopped, started.id).await;
    assert_eq!(w.gate.requests(), 0, "no GitHub call");
    let status = w.status(started.id).await;
    assert_eq!(status.state, RefreshState::Cancelled);
    assert_eq!(w.summary(ALICE).await.snapshot, None);
    assert_eq!(
        w.refreshed().await.state,
        RefreshState::Finished,
        "a new refresh may start"
    );
}

/// Cancelling a ranking keeps the judgement counts stored meanwhile.
#[tokio::test]
async fn cancel_of_a_ranking_keeps_its_stored_counts() {
    let w = world_with(Arc::new(StallingRanker), Duration::from_secs(24 * 3600)).await;
    let started = w.start().await;
    w.wait_until(started.id, |s| s.state == RefreshState::Ranking)
        .await;
    let counts = db::repo::dashboard::JudgeCounts {
        judged: 4,
        reused: 2,
        left_provisional: 1,
    };
    let reported = w
        .state
        .db
        .dashboard()
        .set_ranking(
            DEFAULT_TENANT,
            "u1",
            started.id,
            0,
            RankingState::Running,
            counts,
        )
        .await
        .expect("report");
    assert_eq!(reported, db::repo::dashboard::RankingUpdate::Applied);
    let answer = w
        .send("POST", &path(started.id, "/cancel"), ALICE, &[])
        .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let cancelled: RefreshStatus = serde_json::from_value(answer.body).expect("status");
    assert_eq!(
        (cancelled.state, cancelled.ranking.map(|r| r.state)),
        (RefreshState::Finished, Some(RankingState::Cancelled))
    );
    assert_eq!(
        (
            cancelled.judged,
            cancelled.reused,
            cancelled.left_provisional
        ),
        (4, 2, 1)
    );
}

/// A panicking refresh task ends its refresh instead of leaving it live in the database.
#[tokio::test]
async fn a_panicking_ranking_ends_failed_and_a_new_refresh_may_start() {
    let w = world_with(Arc::new(PanickingRanker), Duration::from_secs(24 * 3600)).await;
    let started = w.start().await;
    let ended = w
        .wait_until(started.id, |s| {
            s.ranking.as_ref().map(|r| r.state) == Some(RankingState::Failed)
        })
        .await;
    assert_eq!(ended.state, RefreshState::Finished);
    let mut stopped = w.state.dashboard.stopped();
    if w.state.dashboard.is_running(started.id) {
        task_stopped(&mut stopped, started.id).await;
    }
    assert!(!w.state.dashboard.is_running(started.id));
    let again = w.start().await;
    assert_ne!(again.id, started.id);
}

#[tokio::test]
async fn failed_scan_keeps_previous_snapshot_and_rate_limit_blocks_a_new_start() {
    let w = world().await;
    let first = w.refreshed().await;
    let reset = at("2026-11-18T10:41:00Z");
    w.faults(json!([{"on": "search", "times": 100,
        "answer": {"kind": "primary_rate_limit", "reset": reset.as_second()}}]))
        .await;
    let failed = w.refreshed().await;
    assert_eq!(failed.state, RefreshState::Failed);
    let failure = failed.failure.expect("failure");
    assert_eq!(failure.code, RefreshFailureCode::GithubRateLimited);
    assert_eq!(failure.retry_at, Some(reset));
    assert_eq!(
        failure.message,
        "GitHub's rate limit is used up. Retry at 11:41."
    );
    let summary = w.summary(ALICE).await;
    assert_eq!(summary.snapshot.expect("snapshot").refresh_id, first.id);
    let answer = w
        .send("POST", "/api/v1/dashboard/refreshes", ALICE, &[])
        .await;
    assert_eq!(
        answer.status,
        StatusCode::TOO_MANY_REQUESTS,
        "{}",
        answer.body
    );
    assert_eq!(answer.body["code"], "github_rate_limited");
    assert!(answer.headers.contains_key("retry-after"));
}

/// GitHub's `retry-after` is a number from the network: one past the representable time range leaves the retry
/// time unknown instead of panicking the refresh.
#[tokio::test]
async fn an_out_of_range_retry_after_fails_without_a_retry_time() {
    let w = world().await;
    w.faults(json!([{"on": "search", "times": 100,
        "answer": {"kind": "secondary_rate_limit", "retry_after": 400_000_000_000_u64}}]))
        .await;
    let failed = w.refreshed().await;
    let failure = failed.failure.expect("failure");
    assert_eq!(failure.code, RefreshFailureCode::GithubRateLimited);
    assert_eq!(failure.retry_at, None);
    assert_eq!(
        failure.message,
        "GitHub's rate limit is used up. Try again later."
    );
}

#[tokio::test]
async fn second_refresh_while_scanning_is_refused() {
    let w = world().await;
    let running = w.start_held().await;
    let (status, code) = w
        .error_code("POST", "/api/v1/dashboard/refreshes", ALICE)
        .await;
    assert_eq!(
        (status, code),
        (StatusCode::CONFLICT, json!("refresh_running"))
    );
    w.gate.open();
    assert_eq!(w.settle(running.id).await.state, RefreshState::Finished);
}

#[tokio::test]
async fn new_refresh_supersedes_one_that_is_ranking() {
    let w = world_with(Arc::new(StallingRanker), Duration::from_secs(24 * 3600)).await;
    let first = w.start().await;
    w.wait_until(first.id, |s| s.state == RefreshState::Ranking)
        .await;
    assert!(w.state.dashboard.is_running(first.id), "ranking runs");
    let second = w.start().await;
    assert_ne!(second.id, first.id);
    let superseded = w.status(first.id).await;
    assert_eq!(superseded.state, RefreshState::Finished);
    assert_eq!(
        superseded.ranking.map(|r| r.state),
        Some(RankingState::Cancelled)
    );
    assert!(
        !w.state.dashboard.is_running(first.id),
        "the old ranking is stopped"
    );
    let ranking = w
        .wait_until(second.id, |s| s.state == RefreshState::Ranking)
        .await;
    assert_eq!(
        ranking.ranking.map(|r| r.state),
        Some(RankingState::Running)
    );
    let summary = w.summary(ALICE).await;
    assert_eq!(summary.snapshot.expect("snapshot").refresh_id, second.id);
}

#[tokio::test]
async fn restart_marks_scanning_refresh_failed() {
    let w = world().await;
    let running = w.start_held().await;
    // The old process dies with its tasks.
    w.state.dashboard.abort(running.id);
    let after = w.restarted(Arc::new(NoopRanker)).await;
    w.gate.open();
    let status = after.status(running.id).await;
    assert_eq!(status.state, RefreshState::Failed);
    assert_eq!(
        status.failure.map(|f| f.code),
        Some(RefreshFailureCode::Interrupted)
    );
    assert_eq!(after.summary(ALICE).await.snapshot, None);
    let fresh = after.refreshed().await;
    assert_eq!(fresh.state, RefreshState::Finished);
}

#[tokio::test]
async fn restart_interrupts_a_ranking_and_rank_again_finishes_it() {
    let w = world_with(Arc::new(StallingRanker), Duration::from_secs(24 * 3600)).await;
    let first = w.start().await;
    w.wait_until(first.id, |s| s.state == RefreshState::Ranking)
        .await;
    w.state.dashboard.abort(first.id);
    let after = w.restarted(Arc::new(NoopRanker)).await;
    let interrupted = after.status(first.id).await;
    assert_eq!(interrupted.state, RefreshState::Finished);
    let ranking = interrupted.ranking.expect("ranking");
    assert_eq!(ranking.state, RankingState::Interrupted);
    assert!(ranking.message.is_some());
    let answer = after
        .send("POST", &path(first.id, "/rank"), ALICE, &[])
        .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.body);
    let ranked = after
        .wait_until(first.id, |s| {
            s.ranking.as_ref().map(|r| r.state) == Some(RankingState::Finished)
        })
        .await;
    assert_eq!(ranked.state, RefreshState::Finished);
    let (status, code) = after
        .error_code("POST", &path(first.id, "/rank"), ALICE)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{code}");
}

#[tokio::test]
async fn items_page_with_cursor_etag_and_expired_cursor() {
    let w = world().await;
    w.refreshed().await;
    let all = w.items("").await;
    assert_eq!(all.items.len(), 18, "default limit 100");
    let first = w.items("?limit=5").await;
    assert_eq!(alias_ids(&first.items), alias_ids(&all.items[..5]));
    let cursor = first.next_cursor.clone().expect("next page");
    let second = w.items(&format!("?limit=5&cursor={cursor}")).await;
    assert_eq!(alias_ids(&second.items), alias_ids(&all.items[5..10]));
    let last = w
        .items(&format!(
            "?limit=100&cursor={}",
            second.next_cursor.expect("third page")
        ))
        .await;
    assert_eq!(alias_ids(&last.items), alias_ids(&all.items[10..]));
    assert_eq!(last.next_cursor, None);

    let answer = w
        .send("GET", "/api/v1/dashboard/items?limit=5", ALICE, &[])
        .await;
    let etag = answer.headers["etag"].to_str().expect("etag").to_owned();
    assert!(etag.starts_with("W/\""), "{etag}");
    let again = w
        .send(
            "GET",
            "/api/v1/dashboard/items?limit=5",
            ALICE,
            &[("if-none-match", etag.as_str())],
        )
        .await;
    assert_eq!(again.status, StatusCode::NOT_MODIFIED);
    assert_eq!(again.body, Value::Null);
    let other = w
        .send(
            "GET",
            "/api/v1/dashboard/items?limit=6",
            ALICE,
            &[("if-none-match", etag.as_str())],
        )
        .await;
    assert_eq!(other.status, StatusCode::OK);

    let (status, code) = w
        .error_code("GET", "/api/v1/dashboard/items?cursor=garbage!", ALICE)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{code}");
    w.refreshed().await;
    let (status, code) = w
        .error_code(
            "GET",
            &format!("/api/v1/dashboard/items?cursor={cursor}"),
            ALICE,
        )
        .await;
    assert_eq!(
        (status, code),
        (StatusCode::CONFLICT, json!("cursor_expired"))
    );
    let fresh = w.summary(ALICE).await;
    assert_ne!(
        fresh.revision, first.revision,
        "a new snapshot, a new revision"
    );
}

#[tokio::test]
async fn items_filters() {
    let w = world().await;
    w.refreshed().await;
    let p1 = w.items("?tier=p1").await;
    assert_eq!(
        alias_ids(&p1.items),
        [
            "PLAT-1203",
            "WUI-320",
            "NC-482",
            "WUI-311",
            "DOC-77",
            "NC-476",
            "CON-1534"
        ]
    );
    let issues = w.items("?kind=issue").await;
    assert_eq!(
        alias_ids(&issues.items),
        [
            "PLAT-1187",
            "PLAT-1210",
            "TODO-3fa91c2e",
            "CON-1520",
            "CON-1525",
            "PLAT-1215",
            "TODO-91bd0a44",
        ],
        "TODO items whatever kind says"
    );
    let prs = w.items("?kind=pr&todos=false").await;
    assert_eq!(prs.items.len(), 11);
    assert!(prs.items.iter().all(|i| i.kind == ItemKind::Pr));
    let wallet = w.items("?repo=ACME/wallet-ui").await;
    assert_eq!(alias_ids(&wallet.items), ["WUI-320", "WUI-311", "WUI-298"]);
    let unjudged = w.items("?state=unjudged&tier=p3&todos=false").await;
    assert_eq!(
        alias_ids(&unjudged.items),
        ["PLAT-1210", "CON-1520", "CON-1525", "PLAT-1215",]
    );
    let all = w.items("").await;
    let since = all.items.iter().map(|i| i.updated_at).max().expect("items");
    let recent = w.items(&format!("?updated_since={:.0}", since.get())).await;
    assert!(!recent.items.is_empty());
    assert!(recent.items.iter().all(|i| i.updated_at >= since));
    let (status, _) = w
        .error_code("GET", "/api/v1/dashboard/items?tier=p9", ALICE)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sse_sends_the_status_then_each_change_until_finished() {
    let w = world().await;
    let running = w.start_held().await;
    let request = Request::builder()
        .uri(path(running.id, "/events"))
        .header("cookie", ALICE)
        .body(Body::empty())
        .expect("request");
    let response = api::router(w.state.clone())
        .oneshot(request)
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let mut events: Vec<RefreshStatus> = Vec::new();
    let mut text = String::new();
    w.gate.open();
    let read = tokio::time::timeout(WAIT, async {
        while events
            .last()
            .is_none_or(|s| s.state != RefreshState::Finished)
        {
            let chunk = body.next().await.expect("stream open").expect("chunk");
            text.push_str(std::str::from_utf8(&chunk).expect("utf-8"));
            while let Some(end) = text.find("\n\n") {
                let frame: String = text.drain(..end + 2).collect();
                let name = frame
                    .lines()
                    .find_map(|l| l.strip_prefix("event: "))
                    .expect("event name");
                let data = frame
                    .lines()
                    .find_map(|l| l.strip_prefix("data: "))
                    .expect("data");
                match serde_json::from_str::<DashboardEvent>(data).expect("event") {
                    DashboardEvent::Refresh { status } => {
                        assert_eq!(name, "refresh", "{frame}");
                        events.push(status);
                    }
                    // A slow host may reach the 15 s heartbeat; it carries no status.
                    DashboardEvent::Heartbeat { .. } => assert_eq!(name, "heartbeat"),
                }
            }
        }
    })
    .await;
    assert!(read.is_ok(), "{events:?}");
    assert_eq!(events[0].state, RefreshState::Scanning, "the status first");
    assert!(events.iter().all(|s| s.id == running.id));
    assert!(
        events.windows(2).all(|p| p[0].percent <= p[1].percent),
        "{:?}",
        events.iter().map(|s| s.percent).collect::<Vec<_>>()
    );
    let states: Vec<RefreshState> = events.iter().map(|s| s.state).collect();
    let ranking_at = states.iter().position(|s| *s == RefreshState::Ranking);
    assert!(ranking_at.is_some(), "{states:?}");
    assert!(
        states[..ranking_at.unwrap_or(0)]
            .iter()
            .all(|s| *s == RefreshState::Scanning),
        "{states:?}"
    );
    assert!(
        events.len() > 7,
        "progress events while scanning: {}",
        events.len()
    );
    let current_item = events.iter().find_map(|s| {
        s.steps
            .iter()
            .find(|step| step.key == ScanStepKey::PrDetails && step.state == StepState::Running)
            .and_then(|step| step.detail.clone())
    });
    assert!(
        current_item.is_some_and(|item| item.starts_with("acme/") && item.contains('#')),
        "pr_details names the current item"
    );
    let last = events.last().expect("last");
    assert_eq!(last.percent, 100);
    assert_eq!(last, &w.status(running.id).await);
}

#[tokio::test]
async fn dashboard_of_another_user_is_never_served() {
    let w = world().await;
    let done = w.refreshed().await;
    let summary = w.summary(COLLEAGUE).await;
    assert_eq!(summary.snapshot, None);
    assert_eq!(summary.latest_refresh, None);
    let answer = w
        .send("GET", "/api/v1/dashboard/items", COLLEAGUE, &[])
        .await;
    assert_eq!(answer.status, StatusCode::OK);
    let page: ItemPage = serde_json::from_value(answer.body).expect("page");
    assert!(page.items.is_empty());
    for (method, action) in [
        ("GET", ""),
        ("POST", "/cancel"),
        ("POST", "/rank"),
        ("GET", "/events"),
    ] {
        let (status, code) = w
            .error_code(method, &path(done.id, action), COLLEAGUE)
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {action}: {code}");
    }
    let (status, _) = w.error_code("GET", "/api/v1/dashboard", "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// The refresh statuses of an R5 stream until one is ended.
async fn statuses_until_ended(w: &World, cookie: &str, id: RefreshId) -> Vec<RefreshStatus> {
    let request = Request::builder()
        .uri(path(id, "/events"))
        .header("cookie", cookie)
        .body(Body::empty())
        .expect("request");
    let response = api::router(w.state.clone())
        .oneshot(request)
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let mut statuses: Vec<RefreshStatus> = Vec::new();
    let mut text = String::new();
    let ended = |s: &RefreshStatus| {
        matches!(s.state, RefreshState::Failed | RefreshState::Cancelled)
            || s.ranking
                .as_ref()
                .is_some_and(|r| r.state == RankingState::Finished)
    };
    let read = tokio::time::timeout(WAIT, async {
        while !statuses.last().is_some_and(ended) {
            let chunk = body.next().await.expect("stream open").expect("chunk");
            text.push_str(std::str::from_utf8(&chunk).expect("utf-8"));
            while let Some(end) = text.find("\n\n") {
                let frame: String = text.drain(..end + 2).collect();
                let data = frame
                    .lines()
                    .find_map(|l| l.strip_prefix("data: "))
                    .expect("data");
                if let DashboardEvent::Refresh { status } =
                    serde_json::from_str::<DashboardEvent>(data).expect("event")
                {
                    statuses.push(status);
                }
            }
        }
    })
    .await;
    assert!(read.is_ok(), "{statuses:?}");
    statuses
}

/// T16: each stream carries only its own user's refresh, while both users' refreshes run at once.
#[tokio::test]
async fn dashboard_events_are_per_user() {
    let w = world().await;
    let alices = w.start_held().await;
    let (status, _) = w
        .error_code("GET", &path(alices.id, "/events"), COLLEAGUE)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "another user's stream");
    let answer = w
        .send("POST", "/api/v1/dashboard/refreshes", COLLEAGUE, &[])
        .await;
    assert_eq!(answer.status, StatusCode::CREATED, "{}", answer.body);
    let colleagues: RefreshStatus = serde_json::from_value(answer.body).expect("status");
    let alice_stream = statuses_until_ended(&w, ALICE, alices.id);
    let colleague_stream = statuses_until_ended(&w, COLLEAGUE, colleagues.id);
    w.gate.open();
    let (alice_events, colleague_events) = tokio::join!(alice_stream, colleague_stream);
    assert!(
        alice_events.iter().all(|s| s.id == alices.id),
        "{alice_events:?}"
    );
    assert!(
        colleague_events.iter().all(|s| s.id == colleagues.id),
        "{colleague_events:?}"
    );
    assert!(alice_events.len() > 2 && !colleague_events.is_empty());
    assert_eq!(
        colleague_events.last().map(|s| s.state),
        Some(RefreshState::Failed),
        "the colleague has no GitHub sign-in (which also ends their session)"
    );
}

#[tokio::test]
async fn memcan_down_keeps_previous_todos_with_note() {
    let w = world().await;
    w.refreshed().await;
    let before = w.summary(ALICE).await.snapshot.expect("snapshot");
    w.todos.set_down(true);
    let done = w.refreshed().await;
    assert_eq!(done.state, RefreshState::Finished);
    let todos_step = done
        .steps
        .iter()
        .find(|s| s.key == ScanStepKey::Todos)
        .expect("todos step");
    assert_eq!(todos_step.state, StepState::Failed);
    let meta = w.summary(ALICE).await.snapshot.expect("snapshot");
    assert_eq!(meta.refresh_id, done.id);
    assert_eq!(meta.todos.status, TodosStatus::NotRefreshed);
    assert_eq!(meta.todos.read_at, before.todos.read_at);
    assert!(meta.todos.failed_at.is_some());
    assert!(
        meta.notes
            .iter()
            .any(|n| n.text.contains("TODOs not refreshed")),
        "{:?}",
        meta.notes
    );
    assert_eq!(meta.item_counts.todo, 2, "previous TODOs kept");
    let page = w.items("").await;
    assert_eq!(
        page.items
            .iter()
            .filter(|i| i.kind == ItemKind::Todo)
            .count(),
        2
    );
}

#[tokio::test]
async fn review_link_reports_head_moved() {
    let w = world().await;
    let analysis = db::repo::analyses::testing::seed_analysis(&w.state.db)
        .await
        .expect("seed");
    w.refreshed().await;
    let page = w.items("").await;
    let linked: Vec<_> = page
        .items
        .iter()
        .filter_map(|i| i.review.as_ref().map(|r| (i.id.as_str(), r)))
        .collect();
    assert_eq!(linked.len(), 1, "{linked:?}");
    let (id, link) = linked[0];
    assert_eq!(id, "NC-482");
    assert_eq!(link.analysis_id, analysis);
    assert!(
        link.head_moved,
        "the fixture head differs from the analysed one"
    );
    assert!(!link.has_my_state);
}

#[tokio::test]
async fn refresh_without_a_github_sign_in_fails_with_sign_in_required() {
    let w = world().await;
    let answer = w
        .send("POST", "/api/v1/dashboard/refreshes", COLLEAGUE, &[])
        .await;
    assert_eq!(answer.status, StatusCode::CREATED, "{}", answer.body);
    let started: RefreshStatus = serde_json::from_value(answer.body).expect("status");
    let failed = w.settle(started.id).await;
    assert_eq!(failed.state, RefreshState::Failed);
    assert_eq!(
        failed.failure.map(|f| f.code),
        Some(RefreshFailureCode::SignInRequired)
    );
}

/// A GitHub search outage (every search 502) does not replace the previous snapshot with an empty one (Refresh
/// states: a failed scan keeps the previous snapshot; `github_unavailable`).
#[tokio::test]
async fn search_outage_keeps_previous_snapshot() {
    let w = world().await;
    let first = w.refreshed().await;
    w.faults(json!([{"on": "search", "times": 1000, "answer": {"kind": "server_error"}}]))
        .await;
    let second = w.refreshed().await;
    let summary = w.summary(ALICE).await;
    let meta = summary.snapshot.expect("snapshot");
    assert_eq!(
        (second.state, meta.refresh_id, meta.item_counts.pr),
        (RefreshState::Failed, first.id, 11),
        "outage refresh: {:?}; current snapshot items {:?}",
        second.failure,
        meta.item_counts
    );
    assert_eq!(
        second.failure.map(|f| f.code),
        Some(RefreshFailureCode::GithubUnavailable)
    );
}

/// Some searches failing is a partial scan: the snapshot is stored with a note per missing search.
#[tokio::test]
async fn partial_search_failure_stores_a_snapshot_with_notes() {
    let w = world().await;
    w.faults(
        json!([{"on": "search", "matching": "mentions:alice", "times": 10,
        "answer": {"kind": "server_error"}}]),
    )
    .await;
    let done = w.refreshed().await;
    assert_eq!(done.state, RefreshState::Finished, "{:?}", done.failure);
    let meta = w.summary(ALICE).await.snapshot.expect("snapshot");
    assert_eq!(meta.refresh_id, done.id);
    assert!(
        meta.notes
            .iter()
            .any(|n| n.text.contains("issues mentioning alice")),
        "{:?}",
        meta.notes
    );
}

/// A login GitHub search refuses, with no owners to list: the note names it and the Settings group to fill in.
#[tokio::test]
async fn refused_search_without_owners_says_where_to_add_them() {
    let w = world().await;
    let mut none = settings();
    none.enumerate_owners.clear();
    w.state
        .db
        .dashboard()
        .save_settings(DEFAULT_TENANT, "u1", &none, Timestamp::now())
        .await
        .expect("settings");
    let done = w.refreshed().await;
    assert_eq!(done.state, RefreshState::Finished, "{:?}", done.failure);
    let meta = w.summary(ALICE).await.snapshot.expect("snapshot");
    let expected = "GitHub search refuses alice-helper. Their work is missing from this list. In Settings → \
                    Dashboard, add the organisations or users that hold it under \"When GitHub search refuses an \
                    account\".";
    assert!(
        meta.notes.iter().any(|n| n.text == expected),
        "{:?}",
        meta.notes
    );
}

/// T19: TODOs are shown only while the user's current settings include them, whatever the snapshot holds.
#[tokio::test]
async fn todos_hidden_when_switched_off() {
    let w = world().await;
    w.refreshed().await;
    let before = w.summary(ALICE).await;
    let mut off = settings();
    off.include_todos = false;
    w.state
        .db
        .dashboard()
        .save_settings(DEFAULT_TENANT, "u1", &off, Timestamp::now())
        .await
        .expect("settings");
    let page = w.items("").await;
    let todos = page
        .items
        .iter()
        .filter(|i| i.kind == ItemKind::Todo)
        .count();
    assert_eq!(
        todos, 0,
        "TODOs still served after include_todos was switched off"
    );
    let after = w.summary(ALICE).await;
    let meta = after.snapshot.expect("snapshot");
    assert_eq!((meta.item_counts.todo, meta.checked.todo), (0, 0));
    assert_eq!(meta.todos.status, TodosStatus::Off);
    assert_ne!(
        after.revision, before.revision,
        "hiding TODOs is a new revision"
    );
    assert_eq!(page.revision, after.revision);
}

#[tokio::test]
async fn scan_over_the_time_limit_fails_as_timed_out() {
    let w = world_with(Arc::new(NoopRanker), Duration::from_millis(300)).await;
    let started = w.start_held().await;
    let failed = w.settle(started.id).await;
    w.gate.open();
    assert_eq!(failed.state, RefreshState::Failed);
    let failure = failed.failure.expect("failure");
    assert_eq!(failure.code, RefreshFailureCode::TimedOut);
    assert_eq!(w.summary(ALICE).await.snapshot, None);
}

mod judged;
mod plugin;
mod snooze;

/// Pull requests the refused linked login `alice-helper` authored count in the charts whoever merged them: by the
/// user (`acme/platform#1231`), by a third person (`acme/consensus#1536`, absent from its event feed), outside the
/// user's owned paths (`acme/platform#1233`), older (`acme/consensus#1501`), and opened the day before and merged
/// by the user in a repository only enumerated, with only the `opened` event in the feed
/// (`acme/review-policies#7`). GitHub's feed reports a merge as action `merged` with a slim payload (no `merged`
/// flag, no author), and not always.
#[tokio::test]
async fn history_counts_merges_of_a_refused_login_whoever_merged_them() {
    let w = world().await;
    let done = w.refreshed().await;
    assert_eq!(done.state, RefreshState::Finished, "{:?}", done.failure);
    let meta = w.summary(ALICE).await.snapshot.expect("snapshot");
    let history: Vec<(HistoryKind, &str)> = meta
        .history
        .iter()
        .map(|e| (e.kind, e.item_id.as_str()))
        .collect();
    for item in [
        "acme/consensus#1501",
        "acme/platform#1231",
        "acme/consensus#1536",
        "acme/platform#1233",
        "acme/review-policies#7",
    ] {
        assert!(
            history.contains(&(HistoryKind::PrMerged, item)),
            "{item} merged missing: {history:?}"
        );
        assert!(
            history.contains(&(HistoryKind::PrOpened, item)),
            "{item} opened missing: {history:?}"
        );
    }
    // The note says where the history may be incomplete and which setting completes it.
    let note = meta
        .notes
        .iter()
        .map(|n| n.text.as_str())
        .find(|n| n.contains("alice-helper") && n.contains("public events"))
        .expect("history note");
    assert!(
        note.contains("Settings") && note.contains("\"When GitHub search refuses an account\""),
        "{note}"
    );
}

/// Look-back days (G9, K5) mark an issue opened within them, counted back from the scan, as new (the reference
/// skill's `isNew`: age ≤ look-back days).
#[tokio::test]
async fn issues_opened_within_the_look_back_days_are_marked_new() {
    let w = world().await;
    let done = w.refreshed().await;
    assert_eq!(done.state, RefreshState::Finished, "{:?}", done.failure);
    let issues = w.items("?kind=issue").await;
    let new: Vec<&str> = issues
        .items
        .iter()
        .filter(|item| item.tags.iter().any(|tag| tag.text == "new"))
        .map(|item| item.id.as_str())
        // Opened exactly 7 days before the test clock: on the edge, the scan runs a moment later.
        .filter(|id| *id != "CON-1520")
        .collect();
    // Scanned at 2026-11-18T09:00Z with the default 7 days: 1187 (1 day), 1525 (3), 1215 (4); not 1210 (9),
    // 305 (12), 1150 (39).
    assert_eq!(new, ["PLAT-1187", "CON-1525", "PLAT-1215"]);
    let tip = &issues.items[0]
        .tags
        .iter()
        .find(|t| t.text == "new")
        .expect("tag")
        .tip;
    assert!(
        tip.contains("2026-11-17") && tip.contains("7 days"),
        "{tip}"
    );
}

/// The public events of a login search refuses could not be read: no note may claim they reach back to the window
/// start (they were not read at all).
#[tokio::test]
async fn unread_event_feed_is_not_reported_as_reaching_back_to_the_window_start() {
    let w = world().await;
    w.faults(
        json!([{"on": "rest", "matching": "users/alice-helper/events", "times": 100,
        "answer": {"kind": "server_error", "html": false}}]),
    )
    .await;
    let done = w.refreshed().await;
    assert_eq!(done.state, RefreshState::Finished, "{:?}", done.failure);
    let meta = w.summary(ALICE).await.snapshot.expect("snapshot");
    let notes: Vec<&str> = meta.notes.iter().map(|n| n.text.as_str()).collect();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("public events of alice-helper could not be read")),
        "{notes:?}"
    );
    assert!(
        !notes.iter().any(|n| n.contains("which reach back to")),
        "an unread feed is described as read back to a date: {notes:?}"
    );
}

#[tokio::test]
async fn mentions_stay_outside_ranking_and_hide_is_scoped_and_token_readable() {
    use contracts::api::access_tokens::{AccessTokenId, TokenScope};
    use db::repo::access_tokens::NewAccessToken;
    let world = world().await;
    world.refreshed().await;
    let before = world.summary(ALICE).await;
    let github_reads = world.gate.requests();
    let mention = before
        .mentions
        .iter()
        .find(|m| m.number == 305)
        .expect("mention-only issue");
    assert!(!mention.is_bot);
    assert!(mention.url.ends_with("#issuecomment-1"));
    assert!(
        !world
            .items("")
            .await
            .items
            .iter()
            .any(|i| i.key == mention.item_key)
    );
    assert!(
        world
            .items("")
            .await
            .items
            .iter()
            .any(|i| i.number == Some(1187)),
        "watched/assigned issue stays ranked"
    );
    let secret = format!("srp_ABCDEFGH_{}", "a".repeat(43));
    world
        .state
        .db
        .access_tokens()
        .create(
            DEFAULT_TENANT,
            "u1",
            &NewAccessToken {
                id: AccessTokenId("ABCDEFGH".into()),
                name: "mentions test".into(),
                token_hash: sha256(secret.as_bytes()),
                scopes: vec![TokenScope::Read],
                expires_at: Timestamp::now() + SignedDuration::from_hours(24),
            },
            Timestamp::now(),
        )
        .await
        .unwrap();
    let bearer = format!("Bearer {secret}");
    for uri in ["/api/v1/dashboard", "/api/v1/dashboard/items"] {
        let read = world
            .send("GET", uri, "", &[("authorization", &bearer)])
            .await;
        assert_eq!(read.status, StatusCode::OK, "{}", read.body);
        assert_eq!(
            read.body["mentions"],
            serde_json::to_value(&before.mentions).unwrap()
        );
        assert_eq!(
            read.body["mention_coverage"],
            json!({
                "unchecked_items": 0, "unchecked_logins": ["alice-helper"], "timed_out_logins": []
            })
        );
    }
    let uri = format!("/api/v1/dashboard/mentions/{}/hide", mention.id);
    assert_eq!(
        world
            .send("POST", &uri, "", &[("authorization", &bearer)])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        world.send("POST", &uri, COLLEAGUE, &[]).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        world.send("POST", &uri, ALICE, &[]).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        world.send("POST", &uri, ALICE, &[]).await.status,
        StatusCode::NO_CONTENT
    );
    let after = world.summary(ALICE).await;
    assert_ne!(before.revision, after.revision);
    assert!(!after.mentions.iter().any(|m| m.id == mention.id));
    assert_eq!(
        world.gate.requests(),
        github_reads,
        "reads and hides use only stored data"
    );
    world.refreshed().await;
    assert!(
        !world
            .summary(ALICE)
            .await
            .mentions
            .iter()
            .any(|m| m.id == mention.id)
    );
}

#[tokio::test]
async fn mention_only_prs_honor_search_coverage_exclusions_and_hide_identity() {
    let dir = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(repo_root().join("fixtures/dashboard/github")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
    }
    let file = dir.path().join("pull-requests.json");
    let mut prs: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let template = prs
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["number"] == 320)
        .unwrap()
        .clone();
    for (number, author, commenter, text) in [
        (321, "bob", "carol", "@alice please"),
        (322, "bob", "carol", "@alice-helper please"),
        (323, "dependabot", "carol", "@alice please"),
        (324, "bob", "dependabot", "@alice please"),
        (325, "bob", "carol", "> @alice quoted"),
        (326, "bob", "KNOWN", "@alice please"),
        (327, "bob", "carol", "@alice draft"),
        (328, "bob", "carol", "@alice answered"),
        (329, "bob", "carol", "@alice newest window"),
    ] {
        let mut pr = template.clone();
        pr["number"] = json!(number);
        pr["author"] = json!({"__typename":"User", "login":author});
        pr["reviewRequests"] = json!({"nodes":[]});
        pr["assignees"] = json!({"nodes":[]});
        pr["reviews"] = json!({"nodes":[]});
        pr["reviewThreads"] = json!({"nodes":[]});
        pr["body"] = json!("");
        pr["comments"] = json!({"nodes":[
            {"author":{"__typename":"User","login":commenter},"body":text,"createdAt":"2026-11-18T08:30:00Z"},
            {"author":{"__typename":"User","login":commenter},"body":text,"createdAt":"2026-11-18T08:40:00Z"}
        ]});
        if number == 327 {
            pr["isDraft"] = json!(true);
        }
        if number == 328 {
            pr["comments"]["nodes"].as_array_mut().unwrap().push(json!({
                "author":{"__typename":"User","login":"alice-helper"},"body":"Answered",
                "createdAt":"2026-11-18T08:50:00Z"
            }));
        }
        if number == 329 {
            let mut comments = Vec::new();
            for n in 0..55 {
                comments.push(json!({"author":{"__typename":"User","login":"carol"},
                    "body":if n == 54 {"@alice newest window"} else {"Older"},
                    "createdAt":format!("2026-11-18T08:{n:02}:00Z")}));
            }
            pr["comments"] = json!({"nodes":comments});
        }
        prs.as_array_mut().unwrap().push(pr);
    }
    // Existing files are minor in the ownership policy; a mention is not a directed review request.
    let owned = prs
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|p| p["number"] == 1203)
        .unwrap();
    owned["reviewRequests"] = json!({"nodes":[]});
    owned["body"] = json!("@alice please");
    std::fs::write(&file, serde_json::to_vec(&prs).unwrap()).unwrap();
    let w = world_in(
        Arc::new(NoopRanker),
        Duration::from_secs(120),
        dir.path().to_owned(),
    )
    .await;
    let mut configured = settings();
    configured.known_bots.push("known".into());
    w.state
        .db
        .dashboard()
        .save_settings(DEFAULT_TENANT, "u1", &configured, now())
        .await
        .unwrap();
    assert_eq!(w.refreshed().await.state, RefreshState::Finished);
    let before = w.summary(ALICE).await;
    for number in [321, 322] {
        assert_eq!(
            before
                .mentions
                .iter()
                .filter(|m| m.number == number)
                .count(),
            if number == 321 { 2 } else { 0 }
        );
        assert!(
            !w.items("")
                .await
                .items
                .iter()
                .any(|i| i.number == Some(number))
        );
    }
    assert_eq!(
        before
            .mentions
            .iter()
            .filter(|m| m.number == 326 && m.is_bot)
            .count(),
        2
    );
    for number in [323, 324, 325, 327, 328, 1203] {
        assert!(
            !before.mentions.iter().any(|m| m.number == number),
            "excluded {number}"
        );
    }
    let partial = before
        .mentions
        .iter()
        .find(|m| m.number == 329)
        .expect("truncated conversation kept");
    assert_eq!(serde_json::to_value(partial).unwrap()["partly_read"], true);
    let old = before.mentions.iter().find(|m| m.number == 321).unwrap();
    let newer = before
        .mentions
        .iter()
        .find(|m| m.number == 321 && m.created_at > old.created_at)
        .unwrap();
    let uri = format!("/api/v1/dashboard/mentions/{}/hide", old.id);
    let no_csrf = api::router(w.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&uri)
                .header("cookie", ALICE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(no_csrf.status(), StatusCode::FORBIDDEN);

    assert_eq!(
        w.send("POST", &uri, ALICE, &[("x-csrf-token", "wrong")])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert!(
        w.summary(ALICE)
            .await
            .mentions
            .iter()
            .any(|m| m.id == old.id)
    );
    assert_eq!(
        w.send("POST", &uri, ALICE, &[]).await.status,
        StatusCode::NO_CONTENT
    );
    w.refreshed().await;
    let after = w.summary(ALICE).await;
    assert!(!after.mentions.iter().any(|m| m.id == old.id));
    assert!(after.mentions.iter().any(|m| m.id == newer.id));
    let first = w.items("?limit=1").await;
    assert!(!first.mentions.is_empty());
    let second = w
        .items(&format!("?limit=1&cursor={}", first.next_cursor.unwrap()))
        .await;
    assert!(second.mentions.is_empty());
}

#[tokio::test]
async fn mention_unreadable_policy_keeps_the_same_unverified_items_as_ranking() {
    let dir = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(repo_root().join("fixtures/dashboard/github")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
    }
    let file = dir.path().join("pull-requests.json");
    let mut prs: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let pr = prs
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|p| p["number"] == 1203)
        .unwrap();
    pr["reviewRequests"] = json!({"nodes":[]});
    pr["body"] = json!("@alice please");
    std::fs::write(&file, serde_json::to_vec(&prs).unwrap()).unwrap();
    let w = world_in(
        Arc::new(NoopRanker),
        Duration::from_secs(120),
        dir.path().to_owned(),
    )
    .await;
    let mut configured = settings();
    configured
        .repositories
        .iter_mut()
        .find(|r| r.repo.name == "platform")
        .unwrap()
        .ownership_policy
        .as_mut()
        .unwrap()
        .path = "missing.json".into();
    w.state
        .db
        .dashboard()
        .save_settings(DEFAULT_TENANT, "u1", &configured, now())
        .await
        .unwrap();
    assert_eq!(w.refreshed().await.state, RefreshState::Finished);
    assert!(
        w.summary(ALICE)
            .await
            .mentions
            .iter()
            .any(|m| m.number == 1203)
    );
}

#[tokio::test]
async fn mention_only_items_are_not_counted_in_ranking_progress() {
    let w = world().await;
    let done = w.refreshed().await;
    let total = |key| done.steps.iter().find(|s| s.key == key).unwrap().total;
    let summary = w.summary(ALICE).await;
    assert_eq!(total(ScanStepKey::SearchPrs), total(ScanStepKey::PrDetails));
    assert_eq!(
        total(ScanStepKey::Issues),
        Some(summary.snapshot.unwrap().checked.issue)
    );
}

#[tokio::test]
async fn unsearchable_mentions_do_not_spend_the_budget_on_unrelated_open_prs() {
    let dir = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(repo_root().join("fixtures/dashboard/github")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
    }
    let file = dir.path().join("pull-requests.json");
    let mut prs: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let mut template = prs
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["number"] == 311)
        .unwrap()
        .clone();
    template["author"]["login"] = json!("bob");
    template["assignees"] = json!({"nodes":[]});
    template["reviewRequests"] = json!({"nodes":[]});
    template["reviews"] = json!({"nodes":[],"totalCount":0});
    template["comments"] = json!({"nodes":[],"totalCount":0});
    template["reviewThreads"] = json!({"nodes":[],"totalCount":0});
    for number in 400..445 {
        let mut pr = template.clone();
        pr["number"] = json!(number);
        pr["body"] = json!("Unrelated work");
        pr["updatedAt"] = json!("2026-11-18T08:00:00Z");
        prs.as_array_mut().unwrap().push(pr);
    }
    template["number"] = json!(450);
    template["body"] = json!("@alice please reply");
    template["comments"] = json!({"nodes":[{
        "author":{"__typename":"User","login":"carol"},
        "body":"@alice-helper please reply", "createdAt":"2026-11-18T08:00:00Z"
    }]});
    template["updatedAt"] = json!("2026-11-16T08:00:00Z");
    prs.as_array_mut().unwrap().push(template.clone());
    template["comments"] = json!({"nodes":[],"totalCount":0});
    template["number"] = json!(451);
    template["body"] = json!("@alice-helper please reply");
    prs.as_array_mut().unwrap().push(template);
    std::fs::write(&file, serde_json::to_vec(&prs).unwrap()).unwrap();
    let file = dir.path().join("issues.json");
    let mut issues: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let issue = issues
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|i| i["number"] == 305)
        .unwrap();
    issue["body"] = json!("@alice-helper please reply");
    issue["comments"] = json!({"nodes":[],"totalCount":0});
    let template = issue.clone();
    for (number, author, commenter, body, text) in [
        (600, "bob", "carol", "", "@alice-helper please reply"),
        (601, "bob", "carol", "> @alice-helper quoted", ""),
        (602, "dependabot", "carol", "@alice-helper please reply", ""),
        (603, "bob", "dependabot", "", "@alice-helper please reply"),
        (
            604,
            "bob",
            "alice",
            "@alice-helper please reply",
            "Answered",
        ),
        (
            605,
            "bob",
            "alice-helper",
            "@alice-helper please reply",
            "Answered",
        ),
        (
            606,
            "bob",
            "carol",
            "@alice-helper-extra",
            "mail@alice-helper.dev",
        ),
    ] {
        let mut extra = template.clone();
        extra["number"] = json!(number);
        extra["author"]["login"] = json!(author);
        extra["body"] = json!(body);
        extra["comments"] = json!({"nodes":[{
            "author":{"__typename":"User","login":commenter}, "body":text,
            "createdAt":"2026-11-18T08:00:00Z"
        }]});
        issues.as_array_mut().unwrap().push(extra);
    }
    for number in 700..745 {
        let mut extra = template.clone();
        extra["number"] = json!(number);
        extra["body"] = json!("Unrelated issue");
        extra["updatedAt"] = json!("2026-11-18T08:00:00Z");
        issues.as_array_mut().unwrap().push(extra);
    }
    issues
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|i| i["number"] == 1525)
        .unwrap()["body"] = json!("@alice-helper please reply");
    std::fs::write(&file, serde_json::to_vec(&issues).unwrap()).unwrap();
    let w = world_in(
        Arc::new(NoopRanker),
        Duration::from_secs(120),
        dir.path().to_owned(),
    )
    .await;
    assert_eq!(w.refreshed().await.state, RefreshState::Finished);
    let ranked = w.items("").await.items;
    let issue = ranked
        .iter()
        .find(|i| i.kind == ItemKind::Issue && i.number == Some(1525))
        .unwrap();
    assert!(issue.tags.iter().any(|tag| tag.text == "mentions you"));
    let summary = w.summary(ALICE).await;
    assert!(
        summary.mentions.iter().any(|m| m.number == 450),
        "the older real candidate must be read"
    );
    assert_eq!(
        summary.mentions.iter().filter(|m| m.number == 450).count(),
        1,
        "a searchable PR must not match the refused login"
    );
    for number in [305, 600] {
        assert!(
            summary.mentions.iter().any(|m| m.number == number),
            "fallback issue {number} must not vanish"
        );
    }
    assert!(
        !ranked
            .iter()
            .any(|i| matches!(i.number, Some(305 | 600..=606 | 700..=744)))
    );
    for number in [451, 601, 602, 603, 604, 605, 606, 1525] {
        assert!(
            !summary.mentions.iter().any(|m| m.number == number),
            "ineligible fallback {number}"
        );
    }
    let expected =
        json!({"unchecked_items":0,"unchecked_logins":["alice-helper"],"timed_out_logins":[]});
    for uri in ["/api/v1/dashboard", "/api/v1/dashboard/items"] {
        let read = w.send("GET", uri, ALICE, &[]).await;
        assert_eq!(read.body["mention_coverage"], expected);
    }
    assert!(
        !summary
            .snapshot
            .unwrap()
            .notes
            .iter()
            .any(|n| n.text.contains("Mentions not checked"))
    );
}
