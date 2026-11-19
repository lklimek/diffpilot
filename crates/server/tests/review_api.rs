//! Review-state and classic-diff routes through the public router over the fixture PR `acme/node-client#482`: a
//! finished analysis stored directly (agent outputs of `fixtures/pr-482/agent`) and the fixture repository
//! served from a `file://` clone base.
#![expect(clippy::expect_used, reason = "test helpers outside #[test] functions")]

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use contracts::api::analyses::AnalysisState;
use contracts::api::files::{FilesResponse, PartOwner, ViewedStatus};
use contracts::api::pr::PrSummary;
use contracts::api::review::{BlockRef, QuietRowRef, ReviewState};
use contracts::api::tour::Tour;
use contracts::common::{LineSide, LlmLanguage, NotAnalysedCategory};
use contracts::ids::{AnalysisId, CommitSha, FlowId, StepId};
use contracts::time::Timestamp;
use db::repo::analyses::NewAnalysis;
use db::repo::documents::DocumentKind;
use db::repo::sessions::NewSession;
use db::repo::testing::{file_backed_with_busy_timeout, hold_write_lock};
use db::repo::users::GithubProfile;
use db::repo::{DEFAULT_TENANT, Db};
use http_body_util::BodyExt;
use jiff::SignedDuration;
use serde_json::{Value, json};
use server::api;
use server::config::Config;
use server::secrets::sha256;
use server::state::AppState;
use tempfile::TempDir;
use tower::ServiceExt;

const CSRF: &str = "fake-csrf-token";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture file")).expect("JSON")
}

struct Harness {
    state: AppState,
    repo: PathBuf,
    pr: PrSummary,
    _dir: TempDir,
}

fn config(dir: &Path) -> Config {
    let env: HashMap<&str, String> = HashMap::from([
        ("SR_PUBLIC_URL", "http://review.example.test".to_owned()),
        ("SR_DATA_DIR", dir.join("data").display().to_string()),
        (
            "SR_SECRET_KEY",
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=".to_owned(),
        ),
        ("SR_RUNNER_SECRET", "fake-runner-secret".to_owned()),
        ("SR_GITHUB_APP_ID", "1".to_owned()),
        ("SR_GITHUB_CLIENT_ID", "Iv1.fake".to_owned()),
        ("SR_GITHUB_CLIENT_SECRET", "fake-client-secret".to_owned()),
        (
            "SR_GITHUB_APP_PRIVATE_KEY_FILE",
            "/nonexistent.pem".to_owned(),
        ),
        (
            "SR_GITHUB_CLONE_URL",
            format!("file://{}", dir.join("remotes").display()),
        ),
        ("SR_MEMCAN_URL", "http://memcan.example.test/mcp".to_owned()),
        // Nothing listens there: a GitHub read with a user token fails at once, never reaching GitHub.
        ("SR_GITHUB_API_URL", "http://127.0.0.1:9".to_owned()),
        ("SR_MEMCAN_TOKEN", "fake-memcan-token".to_owned()),
    ]);
    Config::from_lookup(|key| Ok(env.get(key).cloned())).expect("test configuration")
}

impl Harness {
    async fn new() -> Self {
        Self::build(None).await
    }

    /// On a file-backed database whose writers give up after `busy_timeout`.
    async fn file_backed(busy_timeout: Duration) -> Self {
        Self::build(Some(busy_timeout)).await
    }

    async fn build(busy_timeout: Option<Duration>) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo = dir.path().join("remotes/acme/node-client.git");
        let built = Command::new(fixtures().join("repo/build-repo.sh"))
            .arg(&repo)
            .output()
            .expect("build-repo.sh");
        assert!(built.status.success(), "{built:?}");
        let db = match busy_timeout {
            None => Db::in_memory().await,
            Some(timeout) => file_backed_with_busy_timeout(dir.path(), timeout).await,
        }
        .expect("db");
        let state = AppState::with_queue(config(dir.path()), db);
        let pr: PrSummary = serde_json::from_value(
            read_json(&fixtures().join("api-seed/default/pr-resolve.json"))["pr"].clone(),
        )
        .expect("PrSummary");
        let h = Self {
            state,
            repo,
            pr,
            _dir: dir,
        };
        for (user, github_id) in [("u-alice", 3001), ("u-bob", 3002)] {
            h.sign_in(DEFAULT_TENANT, user, github_id).await;
        }
        h
    }

    /// Stores a GitHub token pair for `user` (the API URL answers nothing, so its reads are "not checked").
    async fn github_tokens(&self, user: &str) {
        let lock = server::auth::lock_user(user).await;
        let grant = server::github::oauth::TokenGrant {
            access_token: secrecy::SecretString::from(format!("ghu_test_{user}")),
            expires_in: 8 * 3600,
            refresh_token: secrecy::SecretString::from(format!("ghr_test_{user}")),
            refresh_token_expires_in: 180 * 86_400,
        };
        server::auth::store_sign_in_tokens(
            &self.state,
            &lock,
            DEFAULT_TENANT,
            &grant,
            Timestamp::now(),
        )
        .await
        .expect("tokens");
    }

    async fn sign_in(&self, tenant: &str, user: &str, github_id: i64) {
        let now = Timestamp::now();
        db::repo::tenants::testing::allow_login(&self.state.db, tenant, user, github_id)
            .await
            .expect("allowlist");
        self.state
            .db
            .users()
            .upsert_member(
                tenant,
                user,
                &GithubProfile {
                    github_id,
                    login: user.to_owned(),
                    name: None,
                    avatar_url: None,
                },
                now,
            )
            .await
            .expect("user");
        self.state
            .db
            .sessions()
            .create(
                &NewSession {
                    token_hash: &sha256(cookie_token(tenant, user).as_bytes()),
                    tenant_id: tenant,
                    user_id: user,
                    csrf_token: CSRF,
                    expires_at: now + SignedDuration::from_hours(24),
                },
                now,
            )
            .await
            .expect("session");
    }

    /// An analysis of `pr` stored directly (no pipeline run), with `pr.base_sha` as its stored base.
    async fn stored(&self, pr: &PrSummary, language: LlmLanguage) -> AnalysisId {
        let db = &self.state.db;
        let repo = db
            .repos()
            .upsert(DEFAULT_TENANT, &pr.repo, None)
            .await
            .expect("repo");
        let pr_id = db
            .pull_requests()
            .upsert(DEFAULT_TENANT, repo.id, pr, Timestamp::now())
            .await
            .expect("pr");
        let id = AnalysisId::new_random();
        db.analyses()
            .create(
                NewAnalysis {
                    id,
                    tenant_id: DEFAULT_TENANT.to_owned(),
                    pull_request_id: pr_id,
                    base_sha: pr.base_sha.clone(),
                    head_sha: pr.head_sha.clone(),
                    language,
                    created_by: "u-alice".to_owned(),
                },
                Timestamp::now(),
            )
            .await
            .expect("analysis");
        id
    }

    /// An analysis of the fixture PR; `finished` ones carry the fixture agent outputs, the others are `planning`
    /// (in Polish: one live analysis per head and language).
    async fn analysis(&self, finished: bool) -> AnalysisId {
        let language = if finished {
            LlmLanguage::En
        } else {
            LlmLanguage::Pl
        };
        let id = self.stored(&self.pr, language).await;
        let db = &self.state.db;
        if !finished {
            return id;
        }
        let now = Timestamp::now();
        let documents = db.documents();
        let agent = |file: &str| read_json(&fixtures().join("agent").join(file));
        documents
            .put(id, DocumentKind::Plan, "", &agent("plan.json"), now)
            .await
            .expect("plan");
        documents
            .put(
                id,
                DocumentKind::PrContext,
                "",
                &agent("pr_context.json"),
                now,
            )
            .await
            .expect("pr context");
        // The GitHub snapshot the pipeline stores at its start: flow 1 rates threads t1 and t2, which
        // verification accepts only for threads listed in the flow's prompt.
        let golden = read_json(&fixtures().join("api/default/tour.json"));
        let mut threads = golden["threads"].clone();
        for thread in threads.as_array_mut().expect("threads") {
            for key in ["step_id", "llm_status", "llm_reason"] {
                thread[key] = Value::Null;
            }
        }
        let snapshot = json!({
            "ci": golden["pr_context"]["facts"]["ci"],
            "threads": threads,
            "threads_status": "checked",
        });
        documents
            .put(id, DocumentKind::GithubSnapshot, "", &snapshot, now)
            .await
            .expect("snapshot");
        for n in 1..=5 {
            documents
                .put(
                    id,
                    DocumentKind::Flow,
                    &format!("f{n}"),
                    &agent(&format!("flow-{n}.json")),
                    now,
                )
                .await
                .expect("flow");
        }
        db.analyses()
            .set_state(id, &AnalysisState::Finished { total: 5 }, now)
            .await
            .expect("finished");
        id
    }

    async fn send(
        &self,
        method: &str,
        uri: &str,
        user: Option<(&str, &str)>,
        csrf: bool,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        json_of(self.respond(method, uri, user, csrf, body).await).await
    }

    async fn respond(
        &self,
        method: &str,
        uri: &str,
        user: Option<(&str, &str)>,
        csrf: bool,
        body: Option<Value>,
    ) -> Response {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some((tenant, user)) = user {
            request = request.header(
                "cookie",
                format!("sr_session={}", cookie_token(tenant, user)),
            );
        }
        if csrf {
            request = request.header("x-csrf-token", CSRF);
        }
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .expect("request");
        api::router(self.state.clone())
            .oneshot(request)
            .await
            .expect("infallible")
    }

    async fn get(&self, uri: &str) -> (StatusCode, Value) {
        self.send("GET", uri, Some(ALICE), false, None).await
    }

    async fn put_state(&self, id: AnalysisId, state: &ReviewState) -> (StatusCode, Value) {
        let body = serde_json::to_value(state).expect("state");
        self.send("PUT", &review_state(id), Some(ALICE), true, Some(body))
            .await
    }

    async fn tour(&self, id: AnalysisId) -> Tour {
        let (status, tour) = self.get(&format!("/api/v1/analyses/{id}/tour")).await;
        assert_eq!(status, StatusCode::OK, "{tour}");
        serde_json::from_value(tour).expect("tour")
    }
}

const ALICE: (&str, &str) = (DEFAULT_TENANT, "u-alice");
const BOB: (&str, &str) = (DEFAULT_TENANT, "u-bob");

fn cookie_token(tenant: &str, user: &str) -> String {
    format!("session-{tenant}-{user}")
}

fn review_state(id: AnalysisId) -> String {
    format!("/api/v1/analyses/{id}/review-state")
}

fn files(id: AnalysisId) -> String {
    format!("/api/v1/analyses/{id}/files")
}

fn files_viewed(id: AnalysisId) -> String {
    format!("/api/v1/analyses/{id}/files/viewed")
}

fn diff(id: AnalysisId, path: &str, side: &str, from: u32, to: u32) -> String {
    format!("/api/v1/analyses/{id}/diff?path={path}&side={side}&from={from}&to={to}")
}

async fn json_of(response: Response) -> (StatusCode, Value) {
    let status = response.status();
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
    (status, body)
}

fn state(ticks: &[&str], last_viewed: Option<BlockRef>) -> ReviewState {
    ReviewState {
        ticked_steps: ticks.iter().map(|id| StepId((*id).to_owned())).collect(),
        last_viewed,
    }
}

fn step(id: &str) -> Option<BlockRef> {
    Some(BlockRef::Step {
        step_id: StepId(id.to_owned()),
    })
}

#[tokio::test]
async fn review_state_round_trip_is_idempotent_and_per_user() {
    let h = Harness::new().await;
    let id = h.analysis(true).await;
    let (status, body) = h.get(&review_state(id)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "ticked_steps": [], "last_viewed": null }));

    let resumed = state(&["f2.1", "f2.2"], step("f1.5"));
    for _ in 0..2 {
        let (status, body) = h.put_state(id, &resumed).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        let (status, body) = h.get(&review_state(id)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_value::<ReviewState>(body).expect("state"),
            resumed
        );
    }
    let (_, other) = h
        .send("GET", &review_state(id), Some(BOB), false, None)
        .await;
    assert_eq!(other, json!({ "ticked_steps": [], "last_viewed": null }));

    // Every block kind of the served Tour is accepted; a repeated tick is stored once.
    let tour = h.tour(id).await;
    let mut blocks = vec![
        Some(BlockRef::PrContext),
        Some(BlockRef::PendingReview),
        None,
    ];
    blocks.extend(tour.flows.iter().map(|flow| {
        Some(BlockRef::FlowIntro {
            flow_id: flow.id.clone(),
        })
    }));
    for block in blocks {
        let (status, body) = h
            .put_state(id, &state(&["f1.1", "f1.1", "f5.2"], block.clone()))
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{block:?}: {body}");
        let (_, body) = h.get(&review_state(id)).await;
        assert_eq!(
            serde_json::from_value::<ReviewState>(body).expect("state"),
            state(&["f1.1", "f5.2"], block)
        );
    }
}

/// Another connection holds the write lock past the busy timeout: reading a finished analysis and its tour (whose
/// pre-pass is computed on read, as for an analysis finished without one) still answers.
#[tokio::test]
async fn reads_answer_while_another_connection_holds_the_write_lock() {
    let h = Harness::file_backed(Duration::from_millis(100)).await;
    let id = h.analysis(true).await;
    let _lock = hold_write_lock(&h.state.db).await.expect("write lock");
    let (status, body) = h.get(&format!("/api/v1/analyses/{id}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A real write that cannot take the write lock within the busy timeout answers 503 `unavailable` with
/// `Retry-After`, not 500.
#[tokio::test]
async fn write_under_a_held_write_lock_is_503_unavailable() {
    let h = Harness::file_backed(Duration::from_millis(100)).await;
    let id = h.analysis(true).await;
    h.tour(id).await;
    let _lock = hold_write_lock(&h.state.db).await.expect("write lock");
    let body = serde_json::to_value(state(&[], None)).expect("state");
    let response = h
        .respond("PUT", &review_state(id), Some(ALICE), true, Some(body))
        .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok()),
        Some("1")
    );
    let (_, body) = json_of(response).await;
    assert_eq!(
        body,
        json!({"code": "unavailable", "message": "The server is busy. Try again in a moment."})
    );
}

#[tokio::test]
async fn review_state_rejects_stale_block_ids() {
    let h = Harness::new().await;
    let id = h.analysis(true).await;
    let kept = state(&["f1.1"], step("f1.2"));
    assert_eq!(h.put_state(id, &kept).await.0, StatusCode::NO_CONTENT);
    let tour = h.tour(id).await;
    let mut stale = vec![
        state(&["f1.1", "f9.1"], None),
        state(&[], step("f1.99")),
        state(
            &[],
            Some(BlockRef::FlowIntro {
                flow_id: FlowId("f6".to_owned()),
            }),
        ),
    ];
    // Quiet rows are accepted exactly when the Tour shows them.
    let quiet = [
        (QuietRowRef::Unexplained, !tour.quiet.unexplained.is_empty()),
        (QuietRowRef::Moves, !tour.quiet.moves.is_empty()),
    ]
    .into_iter()
    .chain(
        [
            NotAnalysedCategory::Tests,
            NotAnalysedCategory::DocsComments,
            NotAnalysedCategory::GeneratedMechanical,
        ]
        .map(|category| {
            let shown = tour
                .quiet
                .not_analysed
                .iter()
                .any(|section| section.category == category);
            (QuietRowRef::NotAnalysed { category }, shown)
        }),
    );
    for (row, shown) in quiet {
        let block = state(&[], Some(BlockRef::Quiet { row: row.clone() }));
        if shown {
            assert_eq!(h.put_state(id, &block).await.0, StatusCode::NO_CONTENT);
            assert_eq!(h.put_state(id, &kept).await.0, StatusCode::NO_CONTENT);
        } else {
            stale.push(block);
        }
    }
    for body in stale {
        let (status, error) = h.put_state(id, &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
        assert_eq!(error["code"], json!("invalid_request"));
        let message = error["message"].as_str().expect("message");
        assert!(message.contains("not part of this analysis"), "{message}");
        let (_, stored) = h.get(&review_state(id)).await;
        assert_eq!(
            serde_json::from_value::<ReviewState>(stored).expect("state"),
            kept,
            "a rejected write changes nothing"
        );
    }
}

#[tokio::test]
async fn review_state_write_before_the_analysis_finished_is_409() {
    let h = Harness::new().await;
    let id = h.analysis(false).await;
    let (status, body) = h.put_state(id, &state(&[], None)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], json!("analysis_not_finished"));
    let (status, body) = h.get(&review_state(id)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "ticked_steps": [], "last_viewed": null }));
}

#[tokio::test]
async fn review_state_concurrent_tick_writes_keep_one_whole_state() {
    let h = Harness::new().await;
    let id = h.analysis(true).await;
    let steps = [
        "f1.1", "f1.2", "f1.3", "f1.4", "f1.5", "f1.6", "f1.7", "f1.8",
    ];
    let writes: Vec<ReviewState> = (1..=steps.len())
        .map(|n| state(&steps[..n], step(steps[n - 1])))
        .collect();
    let results = futures::future::join_all(writes.iter().map(|body| h.put_state(id, body))).await;
    for (status, body) in results {
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }
    let (_, stored) = h.get(&review_state(id)).await;
    let stored: ReviewState = serde_json::from_value(stored).expect("state");
    assert!(
        writes.contains(&stored),
        "the stored state is one of the writes, never a mix: {stored:?}"
    );
}

/// `(old_start, old_count, new_start, new_count)` of a `@@ -a,b +c,d @@` header (counts default to 1).
fn header_numbers(header: &str) -> (u32, u32, u32, u32) {
    let ranges: Vec<&str> = header.split(' ').skip(1).take(2).collect();
    let parse = |range: &str| {
        let mut parts = range[1..].split(',');
        let start = parts.next().expect("start").parse().expect("number");
        let count = parts
            .next()
            .map_or(1, |count| count.parse().expect("count"));
        (start, count)
    };
    let (old, new) = (parse(ranges[0]), parse(ranges[1]));
    (old.0, old.1, new.0, new.1)
}

/// One diff line: marker, text without its ending, and the ending when it is not LF (`crlf`, `no_newline`).
type GitLine = (char, String, Option<String>);
/// Hunks of `git diff -U3` of one file as (header numbers, lines).
type GitHunk = ((u32, u32, u32, u32), Vec<GitLine>);

/// `git diff -U3` of one file, read byte-exactly: CR before LF and "\ No newline at end of file" become endings.
fn git_diff(repo: &Path, base: &str, head: &str, path: &str) -> Vec<GitHunk> {
    let output = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .arg("-C")
        .arg(repo)
        .args([
            "diff",
            "-U3",
            "--no-color",
            "--no-ext-diff",
            "--diff-algorithm=myers",
            base,
            head,
            "--",
            path,
        ])
        .output()
        .expect("git diff");
    assert!(output.status.success(), "{output:?}");
    let mut hunks: Vec<GitHunk> = Vec::new();
    // `split('\n')`, not `lines()`: `lines()` would drop a CR and hide a CRLF change.
    for line in String::from_utf8(output.stdout).expect("UTF-8").split('\n') {
        if line.starts_with("@@") {
            hunks.push((header_numbers(line), Vec::new()));
        } else if let Some((_, lines)) = hunks.last_mut() {
            if line.starts_with('\\') {
                let last = lines.last_mut().expect("a line before the marker");
                last.2 = Some("no_newline".to_owned());
            } else if let Some(marker @ (' ' | '-' | '+')) = line.chars().next() {
                lines.push((marker, line[1..].to_owned(), None));
            }
        }
    }
    // Only a line followed by LF can end in CR LF: a CR before "\ No newline at end of file" is text.
    for (_, text, eol) in hunks.iter_mut().flat_map(|(_, lines)| lines.iter_mut()) {
        if eol.is_none() && text.ends_with('\r') {
            text.pop();
            *eol = Some("crlf".to_owned());
        }
    }
    hunks
}

/// The hunks of a `diff` response in the form of [`git_diff`]; asserts the line numbers run on from the header.
fn response_hunks(body: &Value) -> Vec<GitHunk> {
    body["hunks"]
        .as_array()
        .expect("hunks")
        .iter()
        .map(|hunk| {
            let numbers = header_numbers(hunk["header"].as_str().expect("header"));
            let (mut old, mut new) = (numbers.0, numbers.2);
            let lines = hunk["lines"]
                .as_array()
                .expect("lines")
                .iter()
                .map(|line| {
                    let marker = match line["kind"].as_str().expect("kind") {
                        "context" => ' ',
                        "removed" => '-',
                        _ => '+',
                    };
                    let expected_old = (marker != '+').then_some(old);
                    let expected_new = (marker != '-').then_some(new);
                    assert_eq!(line["old_line"], json!(expected_old), "{line}");
                    assert_eq!(line["new_line"], json!(expected_new), "{line}");
                    old += u32::from(marker != '+');
                    new += u32::from(marker != '-');
                    let eol = line["eol"].as_str().map(str::to_owned);
                    (marker, line["text"].as_str().expect("text").to_owned(), eol)
                })
                .collect();
            (numbers, lines)
        })
        .collect()
}

#[tokio::test]
async fn diff_of_whole_files_matches_git_diff() {
    let h = Harness::new().await;
    let id = h.analysis(true).await;
    let hunks = read_json(&fixtures().join("repo/hunks.json"));
    let mut files: Vec<(String, &str)> = hunks
        .as_array()
        .expect("hunks")
        .iter()
        .filter_map(|hunk| {
            let side = match hunk["status"].as_str().expect("status") {
                "modified" | "added" => "head",
                "deleted" => "base",
                _ => return None,
            };
            Some((hunk["path"].as_str().expect("path").to_owned(), side))
        })
        .collect();
    files.dedup();
    assert!(files.len() >= 10, "{files:?}");
    for (path, side) in files {
        let (status, body) = h.get(&diff(id, &path, side, 1, 2000)).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        assert_eq!(body["path"], json!(path));
        assert_eq!(body["side"], json!(side));
        let expected = git_diff(&h.repo, &h.pr.base_sha.0, &h.pr.head_sha.0, &path);
        assert_eq!(response_hunks(&body), expected, "{path}");
    }
}

#[tokio::test]
async fn diff_of_a_step_range_shows_only_its_hunks() {
    let h = Harness::new().await;
    let id = h.analysis(true).await;
    // src/rpc/client.rs: 11 lines removed after head line 52; the next change starts at head line 89.
    let (status, body) = h.get(&diff(id, "src/rpc/client.rs", "head", 50, 55)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let hunks = body["hunks"].as_array().expect("hunks");
    assert_eq!(hunks.len(), 1, "{body}");
    assert_eq!(hunks[0]["header"], json!("@@ -49,17 +50,6 @@"));
    let removed = hunks[0]["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .filter(|line| line["kind"] == json!("removed"))
        .count();
    assert_eq!(removed, 11);
    // Unchanged lines and unchanged files have no hunks.
    for uri in [
        diff(id, "src/rpc/client.rs", "head", 1, 30),
        diff(id, "Cargo.toml", "head", 1, 10),
    ] {
        let (status, body) = h.get(&uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        assert_eq!(body["hunks"], json!([]), "{uri}");
    }
}

#[tokio::test]
async fn diff_validates_range_and_path_like_lines() {
    let h = Harness::new().await;
    let id = h.analysis(true).await;
    for (from, to) in [(0, 10), (20, 10)] {
        let (status, body) = h.get(&diff(id, "src/lib.rs", "head", from, to)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{from}..{to}");
        assert_eq!(body["code"], json!("invalid_request"));
    }
    let (status, body) = h.get(&diff(id, "src/lib.rs", "head", 1, 2001)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], json!("range_too_large"));
    for side in ["head", "base"] {
        for path in [
            "../etc/passwd",
            "/etc/passwd",
            "src/../../Cargo.toml",
            "src//lib.rs",
            "src%0Alib.rs",
        ] {
            let (status, body) = h.get(&diff(id, path, side, 1, 5)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{side} {path}: {body}");
            assert_eq!(body["code"], json!("invalid_request"));
        }
        for path in [".git/config", "HEAD", "no/such/file.rs"] {
            let (status, body) = h.get(&diff(id, path, side, 1, 5)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{side} {path}: {body}");
        }
    }
    // Files exist only on their own side: an added file has no base, a deleted one no head.
    for (path, side) in [
        ("tests/rpc_retry.rs", "base"),
        ("tests/send_once.rs", "head"),
    ] {
        let (status, _) = h.get(&diff(id, path, side, 1, 5)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{side} {path}");
    }
}

#[tokio::test]
async fn analysis_routes_check_session_csrf_and_tenant() {
    let h = Harness::new().await;
    let id = h.analysis(true).await;
    let body = || Some(json!({ "ticked_steps": [], "last_viewed": null }));
    let other = ("other", "u-carol");
    db::repo::analyses::testing::seed_tenant(&h.state.db, "other")
        .await
        .expect("tenant");
    h.sign_in("other", "u-carol", 3003).await;
    let get_diff = diff(id, "src/lib.rs", "head", 1, 5);
    let viewed = || Some(json!({ "path": "src/lib.rs", "viewed": true }));
    let ask_lines = || {
        Some(
            json!({ "text": "Why?", "anchor": { "kind": "lines", "path": "src/lib.rs", "side": "head", "range": null } }),
        )
    };
    let questions = format!("/api/v1/analyses/{id}/questions");
    for (method, uri, csrf, body) in [
        ("GET", review_state(id), false, None),
        ("PUT", review_state(id), true, body()),
        ("GET", get_diff.clone(), false, None),
        ("GET", files(id), false, None),
        ("PUT", files_viewed(id), true, viewed()),
        ("POST", questions.clone(), true, ask_lines()),
    ] {
        let (status, error) = h.send(method, &uri, None, csrf, body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        assert_eq!(error["code"], json!("unauthenticated"));
        let (status, error) = h.send(method, &uri, Some(other), csrf, body).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{method} {uri}: other tenant"
        );
        assert_eq!(error["code"], json!("not_found"));
    }
    for (method, uri, body) in [
        ("PUT", review_state(id), body()),
        ("PUT", files_viewed(id), viewed()),
        ("POST", questions.clone(), ask_lines()),
    ] {
        let (status, error) = h.send(method, &uri, Some(ALICE), false, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(error["code"], json!("csrf_rejected"));
    }
    let unknown = AnalysisId::new_random();
    for uri in [
        review_state(unknown),
        diff(unknown, "src/lib.rs", "head", 1, 5),
    ] {
        let (status, _) = h.get(&uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
    let (status, error) = h
        .send(
            "PUT",
            &review_state(id),
            Some(ALICE),
            true,
            Some(json!({ "ticked_steps": "f1.1" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["code"], json!("invalid_request"));
}

/// Runs git in `repo` (created when missing) without user configuration; returns trimmed stdout.
fn git(repo: &Path, args: &[&str]) -> String {
    std::fs::create_dir_all(repo).expect("repo dir");
    let output = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.test")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.test")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git");
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Writes `files` into the work tree and commits everything; returns the commit id.
fn commit(repo: &Path, files: &[(&str, &[u8])], message: &str) -> String {
    for (name, content) in files {
        std::fs::write(repo.join(name), content).expect("write");
    }
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

#[tokio::test]
async fn diff_keeps_line_endings_refuses_binary_files_and_waits_for_the_merge_base() {
    let h = Harness::new().await;
    let repo = h._dir.path().join("remotes/acme/blobs.git");
    git(&repo, &["init", "-q", "-b", "main"]);
    let fork = commit(
        &repo,
        &[
            ("a.txt", b"one\n"),
            ("image.bin", &[0, 1, 2, 0, 255]),
            ("eof.txt", b"1\n2\n3\nlast"),
            ("crlf.txt", b"1\r\n2\r\n3\r\n"),
            ("cr.txt", b"1\nx\r"),
        ],
        "fork point",
    );
    git(&repo, &["checkout", "-q", "-b", "feature"]);
    let head = commit(
        &repo,
        &[
            ("a.txt", b"two\n"),
            ("image.bin", &[0, 9, 9, 0, 255]),
            ("eof.txt", b"1\n2\n3\nlast\n"),
            ("crlf.txt", b"1\r\n2\n3\r\n"),
            ("cr.txt", b"1\nx\r\n"),
        ],
        "change",
    );
    git(&repo, &["checkout", "-q", "main"]);
    let base_tip = commit(&repo, &[("b.txt", b"moved on\n")], "base moved on");
    let mut pr = h.pr.clone();
    pr.repo.name = "blobs".to_owned();
    pr.number = 7;
    pr.html_url = "https://github.com/acme/blobs/pull/7".to_owned();
    pr.base_sha = CommitSha(fork);
    pr.head_sha = CommitSha(head);
    let prepared = h.stored(&pr, LlmLanguage::En).await;
    let (status, body) = h.get(&diff(prepared, "image.bin", "head", 1, 5)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], json!("not_rendered"));
    let (status, body) = h.get(&diff(prepared, "a.txt", "head", 1, 1)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["hunks"][0]["header"], json!("@@ -1,1 +1,1 @@"));
    // Line-ending changes stay visible and match git byte for byte: LF <-> none, CRLF <-> LF.
    for (path, side) in [
        ("eof.txt", "head"),
        ("eof.txt", "base"),
        ("crlf.txt", "head"),
        ("crlf.txt", "base"),
        ("cr.txt", "head"),
        ("cr.txt", "base"),
    ] {
        let (status, body) = h.get(&diff(prepared, path, side, 1, 10)).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        let expected = git_diff(&repo, &pr.base_sha.0, &pr.head_sha.0, path);
        assert_eq!(response_hunks(&body), expected, "{path} {side}");
    }
    let (_, body) = h.get(&diff(prepared, "eof.txt", "head", 4, 4)).await;
    let changed: Vec<(&Value, &Value)> = body["hunks"][0]["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .filter(|line| line["kind"] != json!("context"))
        .map(|line| (&line["text"], &line["eol"]))
        .collect();
    assert_eq!(
        changed,
        [
            (&json!("last"), &json!("no_newline")),
            (&json!("last"), &Value::Null)
        ]
    );
    let (_, body) = h.get(&diff(prepared, "crlf.txt", "head", 2, 2)).await;
    let changed: Vec<&Value> = body["hunks"][0]["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .filter(|line| line["kind"] != json!("context"))
        .map(|line| &line["eol"])
        .collect();
    assert_eq!(changed, [&json!("crlf"), &Value::Null]);
    // A last line "x\r" without LF keeps its CR as text; with LF added it is "x" ending in CR LF.
    let (_, body) = h.get(&diff(prepared, "cr.txt", "head", 2, 2)).await;
    let changed: Vec<(&Value, &Value)> = body["hunks"][0]["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .filter(|line| line["kind"] != json!("context"))
        .map(|line| (&line["text"], &line["eol"]))
        .collect();
    assert_eq!(
        changed,
        [
            (&json!("x\r"), &json!("no_newline")),
            (&json!("x"), &json!("crlf"))
        ]
    );
    // b.txt changed only on the base branch: not part of the pull request.
    let (status, body) = h.get(&diff(prepared, "b.txt", "head", 1, 1)).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the head tree has no b.txt: {body}"
    );

    // Stored with the base tip, the analysis is not prepared yet (its base is not the merge base).
    pr.number = 8;
    pr.base_sha = CommitSha(base_tip);
    let unprepared = h.stored(&pr, LlmLanguage::En).await;
    let (status, body) = h.get(&diff(unprepared, "a.txt", "head", 1, 1)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], json!("analysis_not_finished"));
}

/// Lines of `side` in `range`, as a set of (path, line).
fn line_set(path: &str, start: u32, end: u32) -> impl Iterator<Item = (String, u32)> + '_ {
    (start..=end).map(move |line| (path.to_owned(), line))
}

/// C1 on fixture PR 482: every changed line is in exactly one part of its hunk, and the Unexplained parts are
/// exactly the Tour's Unexplained rows (one source of truth). No user token: the Viewed marks are not read and no
/// GitHub call is made.
#[tokio::test]
async fn files_own_every_changed_line_once_and_agree_with_the_tour() {
    let h = Harness::new().await;
    let id = h.analysis(true).await;
    let tour = h.tour(id).await;
    h.github_tokens("u-alice").await;
    let (status, body) = h.get(&files(id)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let response: FilesResponse = serde_json::from_value(body).expect("files");
    assert_eq!(response.viewed_status, ViewedStatus::NotChecked);
    assert_eq!(response.current_head, None);
    assert!(response.files.iter().all(|file| file.viewed.is_none()));
    let mut unexplained: [BTreeSet<(String, u32)>; 2] = Default::default();
    let mut changed_lines = 0;
    for file in &response.files {
        let base_path = file.old_path.as_deref().unwrap_or(&file.path);
        let (mut added, mut removed) = (0, 0);
        for hunk in &file.hunks {
            for (side, range, path) in [
                (LineSide::Base, hunk.base, base_path),
                (LineSide::Head, hunk.head, file.path.as_str()),
            ] {
                let parts: Vec<_> = hunk.parts.iter().filter(|part| part.side == side).collect();
                let Some(range) = range else {
                    assert!(parts.is_empty(), "{}: {hunk:?}", file.path);
                    continue;
                };
                let mut next = range.start_line;
                for part in &parts {
                    assert_eq!(
                        part.range.start_line, next,
                        "{}: gap or overlap in {hunk:?}",
                        file.path
                    );
                    next = part.range.end_line + 1;
                    if let PartOwner::Unexplained { .. } = part.owner {
                        let set = &mut unexplained[usize::from(side == LineSide::Head)];
                        set.extend(line_set(path, part.range.start_line, part.range.end_line));
                    }
                }
                assert_eq!(
                    next,
                    range.end_line + 1,
                    "{}: parts end early in {hunk:?}",
                    file.path
                );
                let count = range.end_line - range.start_line + 1;
                match side {
                    LineSide::Base => removed += count,
                    LineSide::Head => added += count,
                }
            }
        }
        if file.not_rendered.is_none() {
            assert_eq!(
                (file.added, file.removed),
                (added, removed),
                "{}",
                file.path
            );
        }
        changed_lines += added + removed;
    }
    assert!(changed_lines > 100, "{changed_lines}");
    let mut expected: [BTreeSet<(String, u32)>; 2] = Default::default();
    for row in &tour.quiet.unexplained {
        let set = &mut expected[usize::from(row.side == LineSide::Head)];
        for range in &row.ranges {
            set.extend(line_set(&row.file, range.start_line, range.end_line));
        }
    }
    assert_eq!(unexplained, expected);
    // Every step with changed lines owns or shares at least one, so the Files view can open each one.
    let owners: BTreeSet<&StepId> = response
        .files
        .iter()
        .flat_map(|file| &file.hunks)
        .flat_map(|hunk| &hunk.parts)
        .flat_map(|part| {
            let owner = match &part.owner {
                PartOwner::Step { step_id, .. } => Some(step_id),
                _ => None,
            };
            owner.into_iter().chain(&part.also_in)
        })
        .collect();
    for step in tour.flows.iter().flat_map(|flow| &flow.steps) {
        if !step.facts.changed_lines.is_empty() {
            assert!(owners.contains(&step.id), "{} owns no line", step.id);
        }
    }
}

#[tokio::test]
async fn files_wait_for_the_analysis_and_viewed_refuses_other_paths() {
    let h = Harness::new().await;
    let running = h.analysis(false).await;
    let (status, body) = h.get(&files(running)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], json!("analysis_not_finished"));
    let id = h.analysis(true).await;
    let (status, body) = h
        .send(
            "PUT",
            &files_viewed(id),
            Some(ALICE),
            true,
            Some(json!({ "path": "src/not-in-the-pr.rs", "viewed": true })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], json!("invalid_request"));
    let (status, body) = h
        .send(
            "PUT",
            &files_viewed(id),
            Some(ALICE),
            true,
            Some(json!({ "path": "src/rpc/client.rs" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    // Without a GitHub sign-in the Viewed marks cannot be read: sign in again, not "not checked".
    let (status, body) = h.send("GET", &files(id), Some(BOB), false, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["code"], json!("unauthenticated"));
}

/// C1 contract: a `step` owner is a step of the named flow that holds the lines in the Tour, and head lines it owns
/// are among that step's changed lines; a `flow` owner names a flow of the Tour.
#[tokio::test]
async fn files_step_owners_agree_with_the_tour_steps() {
    let h = Harness::new().await;
    let id = h.analysis(true).await;
    h.github_tokens("u-alice").await;
    let tour = h.tour(id).await;
    let (status, body) = h.get(&files(id)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let response: FilesResponse = serde_json::from_value(body).expect("files");
    let mut step_parts = 0;
    for file in &response.files {
        for part in file.hunks.iter().flat_map(|hunk| &hunk.parts) {
            match &part.owner {
                PartOwner::Step { flow_id, step_id } => {
                    step_parts += 1;
                    let flow = tour
                        .flows
                        .iter()
                        .find(|flow| &flow.id == flow_id)
                        .expect("owner flow in the Tour");
                    let step = flow
                        .steps
                        .iter()
                        .find(|step| &step.id == step_id)
                        .expect("owner step in its flow");
                    assert!(!part.also_in.contains(step_id), "{part:?}");
                    if part.side == LineSide::Head {
                        assert_eq!(step.facts.file, file.path, "{part:?}");
                        assert_eq!(step.facts.side, LineSide::Head, "{part:?}");
                        for line in part.range.start_line..=part.range.end_line {
                            assert!(
                                step.facts
                                    .changed_lines
                                    .iter()
                                    .any(|r| r.start_line <= line && line <= r.end_line),
                                "{}:{line} owned by {step_id} but not among its changed lines",
                                file.path
                            );
                        }
                    }
                }
                PartOwner::Flow { flow_id } => {
                    assert!(
                        tour.flows.iter().any(|flow| &flow.id == flow_id),
                        "{part:?}"
                    );
                }
                _ => {}
            }
        }
    }
    assert!(step_parts > 5, "{step_parts}");
}
