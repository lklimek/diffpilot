//! Stage-2 slice through the public router: PR resolve against a fake GitHub (wiremock serving the recorded
//! fixtures), ingest into the bare mirror from a `file://` clone base, the pipeline over a scripted
//! `JobExecutor` returning the fixture agent outputs, and the tour and lines endpoints.
#![expect(clippy::expect_used, reason = "test helpers outside #[test] functions")]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use chrono::{TimeDelta, Utc};
use contracts::agent::JobKind;
use contracts::api::analyses::{AnalysisState, AnalysisSummary};
use contracts::api::pr::PrSummary;
use contracts::ids::{AnalysisId, CommitSha, JobId};
use contracts::job::result::{JobFailureKind, JobOutcome, JobResult};
use contracts::job::spec::JobPriority;
use contracts::job::state::JobState;
use db::repo::analyses::NewAnalysis;
use db::repo::documents::DocumentKind;
use db::repo::jobs::JobStage;
use db::repo::sessions::NewSession;
use db::repo::users::GithubProfile;
use db::repo::{DEFAULT_TENANT, Db};
use futures::future::BoxFuture;
use futures::stream::{self, BoxStream, StreamExt};
use http_body_util::BodyExt;
use secrecy::SecretString;
use serde_json::{Value, json};
use server::api;
use server::auth::{lock_user, store_sign_in_tokens};
use server::config::Config;
use server::github::oauth::TokenGrant;
use server::jobs::executor::{ExecutorError, JobExecutor, JobHandle, JobRequest, JobUpdate};
use server::jobs::queue::QueueExecutor;
use server::secrets::sha256;
use server::state::AppState;
use tempfile::TempDir;
use tower::ServiceExt;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const USER_TOKEN: &str = "ghu_fake_slice_token";
const SESSION_TOKEN: &str = "fake-session-token";
const CSRF: &str = "fake-csrf-token";
const PR_URL: &str = "https://github.com/acme/node-client/pull/482";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture file")).expect("JSON")
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

/// Writes `files` (path, content) into the work tree and commits everything; returns the commit id.
fn commit(repo: &Path, files: &[(&str, &str)], message: &str) -> String {
    for (name, content) in files {
        std::fs::write(repo.join(name), content).expect("write");
    }
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

fn numbered(lines: std::ops::RangeInclusive<u32>) -> String {
    lines.map(|n| format!("line {n}\n")).collect()
}

/// Answers every job with the fixture agent output of its stage.
#[derive(Debug, Default)]
struct ScriptedExecutor {
    stages: Mutex<HashMap<JobId, String>>,
    /// Analysis of each submitted job.
    analyses: Mutex<HashMap<JobId, AnalysisId>>,
    /// Jobs whose result was delivered.
    finished: Mutex<HashSet<JobId>>,
    submitted: Mutex<Vec<String>>,
    /// Stages answered other than with their fixture output.
    scripts: Mutex<HashMap<String, Script>>,
    cancelled: Mutex<Vec<String>>,
}

/// How a stage's job ends instead of returning its fixture output.
#[derive(Clone, Debug)]
enum Script {
    Output(Value),
    Fail,
    Pending,
}

impl ScriptedExecutor {
    fn submitted(&self) -> Vec<String> {
        let mut stages = self.submitted.lock().expect("lock").clone();
        stages.sort();
        stages
    }

    fn cancelled(&self) -> Vec<String> {
        let mut stages = self.cancelled.lock().expect("lock").clone();
        stages.sort();
        stages
    }

    fn script(&self, stage: &str, script: Script) {
        self.scripts
            .lock()
            .expect("lock")
            .insert(stage.to_owned(), script);
    }
}

fn agent_output(stage: &str) -> Value {
    let file = match stage {
        "plan:1" => "plan.json".to_owned(),
        "pr_context" => "pr_context.json".to_owned(),
        flow => format!(
            "flow-{}.json",
            flow.strip_prefix("flow:f").expect("flow stage")
        ),
    };
    read_json(&fixtures().join("agent").join(file))
}

impl JobExecutor for ScriptedExecutor {
    fn submit(&self, request: JobRequest) -> BoxFuture<'_, Result<JobHandle, ExecutorError>> {
        let stage = request.stage.key();
        let job_id = request.spec.job_id;
        self.stages
            .lock()
            .expect("lock")
            .insert(job_id, stage.clone());
        self.submitted.lock().expect("lock").push(stage);
        self.analyses
            .lock()
            .expect("lock")
            .insert(job_id, request.spec.analysis_id);
        Box::pin(async move { Ok(JobHandle { job_id }) })
    }

    fn events(&self, handle: &JobHandle) -> BoxStream<'static, Result<JobUpdate, ExecutorError>> {
        let stage = self.stages.lock().expect("lock")[&handle.job_id].clone();
        let script = self.scripts.lock().expect("lock").get(&stage).cloned();
        let outcome = match script {
            None => JobOutcome::Succeeded {
                output: agent_output(&stage),
            },
            Some(Script::Output(output)) => JobOutcome::Succeeded { output },
            Some(Script::Fail) => JobOutcome::Failed {
                kind: JobFailureKind::ProviderFailed,
                message: "the provider exited with an error".to_owned(),
            },
            Some(Script::Pending) => return stream::pending().boxed(),
        };
        self.finished.lock().expect("lock").insert(handle.job_id);
        let result = JobResult {
            job_id: handle.job_id,
            attempt: 1,
            outcome,
            session: None,
            usage: None,
            skills_commit: None,
            llm_call: None,
        };
        stream::iter([Ok(JobUpdate::Finished(result))]).boxed()
    }

    fn cancel(&self, handle: &JobHandle) -> BoxFuture<'_, Result<(), ExecutorError>> {
        let stage = self.stages.lock().expect("lock")[&handle.job_id].clone();
        self.cancelled.lock().expect("lock").push(stage);
        Box::pin(async { Ok(()) })
    }

    fn cancel_analysis(&self, analysis: AnalysisId) -> BoxFuture<'_, Result<(), ExecutorError>> {
        let finished = self.finished.lock().expect("lock").clone();
        let stages = self.stages.lock().expect("lock").clone();
        let mut cancelled = self.cancelled.lock().expect("lock");
        for (job, analysis_of_job) in self.analyses.lock().expect("lock").iter() {
            if *analysis_of_job == analysis && !finished.contains(job) {
                cancelled.push(stages[job].clone());
            }
        }
        Box::pin(async { Ok(()) })
    }
}

struct Harness {
    state: AppState,
    executor: Arc<ScriptedExecutor>,
    github: MockServer,
    user_id: String,
    _dir: TempDir,
}

fn config(dir: &Path, github: &MockServer) -> Config {
    let remotes = dir.join("remotes");
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
        ("SR_GITHUB_API_URL", format!("{}/api", github.uri())),
        ("SR_GITHUB_OAUTH_URL", github.uri()),
        (
            "SR_GITHUB_CLONE_URL",
            format!("file://{}", remotes.display()),
        ),
        ("SR_MEMCAN_URL", "http://memcan.example.test/mcp".to_owned()),
        ("SR_MEMCAN_TOKEN", "fake-memcan-token".to_owned()),
    ]);
    Config::from_lookup(|key| Ok(env.get(key).cloned())).expect("test configuration")
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().expect("temp dir");
    let built = Command::new(fixtures().join("repo/build-repo.sh"))
        .arg(dir.path().join("remotes/acme/node-client.git"))
        .output()
        .expect("build-repo.sh");
    assert!(built.status.success(), "{built:?}");
    let github = MockServer::start().await;
    let recorded = read_json(&fixtures().join("github/rest/pull.json"));
    Mock::given(method("GET"))
        .and(path("/api/repos/acme/node-client/pulls/482"))
        .and(header(
            "authorization",
            format!("Bearer {USER_TOKEN}").as_str(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(&recorded["response"]))
        .mount(&github)
        .await;

    let db = Db::in_memory().await.expect("db");
    let executor = Arc::new(ScriptedExecutor::default());
    let state = AppState::new(config(dir.path(), &github), db, executor.clone());
    let now = Utc::now();
    // The fixture user `alice` is not in the seeded allowlist: sign her in directly.
    let user = state
        .db
        .users()
        .upsert_member(
            DEFAULT_TENANT,
            "u-alice",
            &GithubProfile {
                github_id: 3001,
                login: "alice".to_owned(),
                name: None,
                avatar_url: None,
            },
            now,
        )
        .await
        .expect("user");
    let lock = lock_user(&user.id).await;
    let grant = TokenGrant {
        access_token: SecretString::from(USER_TOKEN),
        expires_in: 8 * 3600,
        refresh_token: SecretString::from("ghr_fake_refresh"),
        refresh_token_expires_in: 180 * 86_400,
    };
    store_sign_in_tokens(&state, &lock, DEFAULT_TENANT, &grant, now)
        .await
        .expect("tokens");
    drop(lock);
    state
        .db
        .sessions()
        .create(
            &NewSession {
                token_hash: &sha256(SESSION_TOKEN.as_bytes()),
                tenant_id: DEFAULT_TENANT,
                user_id: &user.id,
                csrf_token: CSRF,
                expires_at: now + TimeDelta::days(1),
            },
            now,
        )
        .await
        .expect("session");
    Harness {
        state,
        executor,
        github,
        user_id: user.id,
        _dir: dir,
    }
}

enum Auth {
    None,
    Session,
    SessionCsrf,
}

impl Harness {
    async fn send(&self, method: &str, uri: &str, auth: Auth, body: Option<Value>) -> Response {
        let mut request = Request::builder().method(method).uri(uri);
        if !matches!(auth, Auth::None) {
            request = request.header("cookie", format!("sr_session={SESSION_TOKEN}"));
        }
        if matches!(auth, Auth::SessionCsrf) {
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
        json_of(self.send("GET", uri, Auth::Session, None).await).await
    }

    async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        json_of(self.send("POST", uri, Auth::SessionCsrf, Some(body)).await).await
    }

    /// An analysis of the fixture PR stored directly (no pipeline run).
    async fn seed_analysis(&self) -> AnalysisId {
        self.seed_analysis_at(None).await
    }

    /// Like [`Harness::seed_analysis`], at another head commit when given.
    async fn seed_analysis_at(&self, head: Option<String>) -> AnalysisId {
        let pr: PrSummary = serde_json::from_value(
            read_json(&fixtures().join("api-seed/default/pr-resolve.json"))["pr"].clone(),
        )
        .expect("PrSummary");
        let repo = self
            .state
            .db
            .repos()
            .upsert(DEFAULT_TENANT, &pr.repo, None)
            .await
            .expect("repo");
        let pr_id = self
            .state
            .db
            .pull_requests()
            .upsert(DEFAULT_TENANT, repo.id, &pr, Utc::now())
            .await
            .expect("pr");
        self.state
            .db
            .analyses()
            .create(
                NewAnalysis {
                    id: AnalysisId::new_random(),
                    tenant_id: DEFAULT_TENANT.to_owned(),
                    pull_request_id: pr_id,
                    base_sha: pr.base_sha,
                    head_sha: head.map_or(pr.head_sha, CommitSha),
                    language: Default::default(),
                    created_by: self.user_id.clone(),
                },
                Utc::now(),
            )
            .await
            .expect("analysis")
            .id
    }

    async fn wait_until_finished(&self, id: &str) -> AnalysisSummary {
        let summary = self.wait_until_ended(id).await;
        assert!(
            matches!(summary.state, AnalysisState::Finished { .. }),
            "analysis ended as {:?}",
            summary.state
        );
        summary
    }

    /// Waits for `finished`, `failed` or `cancelled`.
    async fn wait_until_ended(&self, id: &str) -> AnalysisSummary {
        for _ in 0..600 {
            let (status, body) = self.get(&format!("/api/v1/analyses/{id}")).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let summary: AnalysisSummary = serde_json::from_value(body).expect("summary");
            match summary.state {
                AnalysisState::Finished { .. }
                | AnalysisState::Failed { .. }
                | AnalysisState::Cancelled => return summary,
                _ => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
        panic!("analysis did not end in time");
    }

    /// Creates the remote repository `acme/<name>` with `build`, which returns the pull request's base tip and
    /// head.
    fn remote_repo(
        &self,
        name: &str,
        build: impl FnOnce(&Path) -> (String, String),
    ) -> (String, String) {
        let repo = self._dir.path().join(format!("remotes/acme/{name}.git"));
        git(&repo, &["init", "-q", "-b", "main"]);
        build(&repo)
    }

    /// Creates pull request `number` of a new remote `acme/<name>` and analyses it through the API; returns the
    /// analysis once its pipeline has ended (prepared, with the merge base stored).
    async fn analyse_repo(
        &self,
        name: &str,
        number: u32,
        build: impl FnOnce(&Path) -> (String, String),
    ) -> AnalysisSummary {
        let (base, head) = self.remote_repo(name, build);
        let mut pull = read_json(&fixtures().join("github/rest/pull.json"))["response"].clone();
        pull["number"] = json!(number);
        pull["html_url"] = json!(format!("https://github.com/acme/{name}/pull/{number}"));
        pull["base"]["sha"] = json!(base);
        pull["head"]["sha"] = json!(head);
        pull["base"]["repo"]["name"] = json!(name);
        Mock::given(method("GET"))
            .and(path(format!("/api/repos/acme/{name}/pulls/{number}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(pull))
            .mount(&self.github)
            .await;
        let (status, created) = self
            .post(
                "/api/v1/analyses",
                json!({
                    "pr": { "repo": { "owner": "acme", "name": name }, "number": number },
                    "head_sha": head,
                }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        assert_eq!(created["state"], json!({ "state": "planning" }));
        let id = created["id"].as_str().expect("id").to_owned();
        self.wait_until_ended(&id).await
    }

    /// An analysis of `acme/<name>` stored directly with the base tip as its base (no pipeline run).
    async fn seed_unprepared(&self, name: &str, base: &str, head: &str) -> AnalysisId {
        let mut pr: PrSummary = serde_json::from_value(
            read_json(&fixtures().join("api-seed/default/pr-resolve.json"))["pr"].clone(),
        )
        .expect("PrSummary");
        pr.repo.name = name.to_owned();
        pr.base_sha = CommitSha(base.to_owned());
        pr.head_sha = CommitSha(head.to_owned());
        let repo = self
            .state
            .db
            .repos()
            .upsert(DEFAULT_TENANT, &pr.repo, None)
            .await
            .expect("repo");
        let pr_id = self
            .state
            .db
            .pull_requests()
            .upsert(DEFAULT_TENANT, repo.id, &pr, Utc::now())
            .await
            .expect("pr");
        self.state
            .db
            .analyses()
            .create(
                NewAnalysis {
                    id: AnalysisId::new_random(),
                    tenant_id: DEFAULT_TENANT.to_owned(),
                    pull_request_id: pr_id,
                    base_sha: pr.base_sha,
                    head_sha: pr.head_sha,
                    language: Default::default(),
                    created_by: self.user_id.clone(),
                },
                Utc::now(),
            )
            .await
            .expect("analysis")
            .id
    }
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

/// Paths where two JSON documents differ (`/`-joined keys and indexes).
fn differences(path: &str, golden: &Value, seed: &Value, out: &mut Vec<String>) {
    match (golden, seed) {
        (Value::Object(a), Value::Object(b)) => {
            for key in a.keys().chain(b.keys().filter(|key| !a.contains_key(*key))) {
                let (x, y) = (a.get(key), b.get(key));
                match (x, y) {
                    (Some(x), Some(y)) => differences(&format!("{path}/{key}"), x, y, out),
                    _ => out.push(format!("{path}/{key}: {x:?} vs {y:?}")),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (index, (x, y)) in a.iter().zip(b).enumerate() {
                differences(&format!("{path}/{index}"), x, y, out);
            }
        }
        _ if golden != seed => out.push(format!("{path}: {golden} vs {seed}")),
        _ => {}
    }
}

#[tokio::test]
async fn slice_pipeline() {
    let h = harness().await;
    let (status, resolved) = h.post("/api/v1/pr/resolve", json!({ "url": PR_URL })).await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    let seed_resolve = read_json(&fixtures().join("api-seed/default/pr-resolve.json"));
    assert_eq!(resolved["pr"], seed_resolve["pr"]);
    assert_eq!(resolved["analysis"], Value::Null);
    assert_eq!(resolved["head_moved"], json!(false));

    let (status, created) = h
        .post(
            "/api/v1/analyses",
            json!({
                "pr": { "repo": { "owner": "acme", "name": "node-client" }, "number": 482 },
                "head_sha": resolved["pr"]["head_sha"],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["state"], json!({ "state": "planning" }));
    let id = created["id"].as_str().expect("id").to_owned();

    let finished = h.wait_until_finished(&id).await;
    assert_eq!(finished.state, AnalysisState::Finished { total: 5 });
    assert_eq!(
        h.executor.submitted(),
        [
            "flow:f1",
            "flow:f2",
            "flow:f3",
            "flow:f4",
            "flow:f5",
            "plan:1",
            "pr_context"
        ]
    );

    let (status, mut tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(status, StatusCode::OK, "{tour}");
    // The analysis id and times are the only run-dependent values: pin them to the seed's.
    let seed = read_json(&fixtures().join("api-seed/default/tour.json"));
    for key in ["id", "created_at", "finished_at"] {
        tour["analysis"][key] = seed["analysis"][key].clone();
    }
    let golden_path = fixtures().join("api/default/tour.json");
    let rendered = format!("{}\n", serde_json::to_string_pretty(&tour).expect("render"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(golden_path.parent().expect("dir")).expect("golden dir");
        std::fs::write(&golden_path, &rendered).expect("write golden");
    }
    let golden = std::fs::read_to_string(&golden_path)
        .expect("golden fixtures/pr-482/api/default/tour.json; create it with UPDATE_GOLDEN=1");
    assert_eq!(rendered, golden, "tour differs from the golden");

    let mut diffs = Vec::new();
    differences("", &tour, &seed, &mut diffs);
    eprintln!(
        "golden vs api-seed: {} differences\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}

#[tokio::test]
async fn pipeline_resumes_unfinished_analysis_from_stored_documents() {
    let h = harness().await;
    let id = h.seed_analysis().await;
    let documents = h.state.db.documents();
    let now = Utc::now();
    documents
        .put(id, DocumentKind::Plan, "", &agent_output("plan:1"), now)
        .await
        .expect("plan");
    documents
        .put(
            id,
            DocumentKind::PrContext,
            "",
            &agent_output("pr_context"),
            now,
        )
        .await
        .expect("pr context");
    for flow in ["f1", "f3"] {
        documents
            .put(
                id,
                DocumentKind::Flow,
                flow,
                &agent_output(&format!("flow:{flow}")),
                now,
            )
            .await
            .expect("flow");
    }

    let resumed = server::pipeline::resume_unfinished(&h.state)
        .await
        .expect("resume");
    assert_eq!(resumed, 1);
    let finished = h.wait_until_finished(&id.to_string()).await;
    assert_eq!(finished.state, AnalysisState::Finished { total: 5 });
    assert_eq!(h.executor.submitted(), ["flow:f2", "flow:f4", "flow:f5"]);
}

#[tokio::test]
async fn lines_match_the_seed_for_whole_files() {
    let h = harness().await;
    let id = h.seed_analysis().await;
    let root = fixtures().join("api-seed/default/lines");
    let mut seeds = Vec::new();
    let mut dirs = vec![root.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).expect("dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                seeds.push(path);
            }
        }
    }
    assert!(seeds.len() >= 10);
    for seed_path in seeds {
        let mut seed = read_json(&seed_path);
        let uri = format!(
            "/api/v1/analyses/{id}/lines?path={}&side={}&from=1&to={}",
            seed["path"].as_str().expect("path"),
            seed["side"].as_str().expect("side"),
            seed["to"]
        );
        let (status, lines) = h.get(&uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {lines}");
        // Moved blocks come from the deterministic move detection of WP3-PREPASS.
        seed["moved"] = json!([]);
        assert_eq!(lines, seed, "{}", seed_path.display());
    }
}

#[tokio::test]
async fn lines_caps_the_range() {
    let h = harness().await;
    let id = h.seed_analysis().await;
    let lines = |from: u32, to: u32| {
        format!("/api/v1/analyses/{id}/lines?path=src/rpc/client.rs&side=head&from={from}&to={to}")
    };
    let (status, body) = h.get(&lines(1, 2000)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (body["from"].clone(), body["to"].clone()),
        (json!(1), json!(224))
    );
    assert_eq!(body["lines"].as_array().map(Vec::len), Some(224));

    let (status, body) = h.get(&lines(1, 2001)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], json!("range_too_large"));
    let (status, body) = h.get(&lines(5000, 6000)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["lines"], json!([]));
    for (from, to) in [(0, 10), (20, 10)] {
        let (status, body) = h.get(&lines(from, to)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{from}..{to}");
        assert_eq!(body["code"], json!("invalid_request"));
    }

    let (status, body) = h.get(&lines(95, 99)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["changed"], json!([95, 96, 97, 98, 99]));
    assert_eq!(body["lines"].as_array().map(Vec::len), Some(5));
    assert_eq!(
        body["comments"]["line_thread_ranges"],
        json!([{ "start_line": 95, "end_line": 99 }])
    );
}

#[tokio::test]
async fn lines_side_base_serves_deleted_code() {
    let h = harness().await;
    let id = h.seed_analysis().await;
    let uri = |side: &str| {
        format!("/api/v1/analyses/{id}/lines?path=tests/send_once.rs&side={side}&from=1&to=50")
    };
    let (status, body) = h.get(&uri("base")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total_lines"], json!(10));
    assert_eq!(body["changed"], json!((1..=10).collect::<Vec<u32>>()));
    assert_eq!(body["deleted"], json!([]));
    assert_eq!(
        body["comments"],
        json!({ "scope": "in_diff", "line_thread_ranges": [{ "start_line": 1, "end_line": 10 }] })
    );
    let (status, body) = h.get(&uri("head")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // A removed block inside a kept file: head side shows it at its marker.
    let (status, body) = h
        .get(&format!(
            "/api/v1/analyses/{id}/lines?path=src/rpc/client.rs&side=head&from=50&to=55"
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deleted"][0]["after_line"], json!(52));
    assert_eq!(
        body["deleted"][0]["lines"].as_array().map(Vec::len),
        Some(11)
    );
}

#[tokio::test]
async fn lines_rejects_path_traversal() {
    let h = harness().await;
    let id = h.seed_analysis().await;
    for side in ["head", "base"] {
        for path in [
            "../etc/passwd",
            "/etc/passwd",
            "src/../../Cargo.toml",
            "src/./lib.rs",
            "src//lib.rs",
            "src%0Alib.rs",
        ] {
            let (status, body) = h
                .get(&format!(
                    "/api/v1/analyses/{id}/lines?path={path}&side={side}&from=1&to=5"
                ))
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{side} {path}: {body}");
            assert_eq!(body["code"], json!("invalid_request"));
        }
        // Valid paths outside the pinned tree of that side are unknown, not read from elsewhere.
        for path in [".git/config", "HEAD", "refs/heads/main"] {
            let (status, _) = h
                .get(&format!(
                    "/api/v1/analyses/{id}/lines?path={path}&side={side}&from=1&to=5"
                ))
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{side} {path}");
        }
    }
    // A file that exists only at head is not served from the base tree.
    let (status, _) = h
        .get(&format!(
            "/api/v1/analyses/{id}/lines?path=src/rpc/retry.rs&side=base&from=1&to=5"
        ))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unauthenticated_requests_get_401() {
    let h = harness().await;
    let id = h.seed_analysis().await;
    for (method, uri) in [
        ("GET", format!("/api/v1/analyses/{id}")),
        ("GET", format!("/api/v1/analyses/{id}/tour")),
        (
            "GET",
            format!("/api/v1/analyses/{id}/lines?path=README.md&side=head&from=1&to=2"),
        ),
        ("POST", "/api/v1/pr/resolve".to_owned()),
        ("POST", "/api/v1/analyses".to_owned()),
    ] {
        let (status, body) = json_of(h.send(method, &uri, Auth::None, Some(json!({}))).await).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        assert_eq!(body["code"], json!("unauthenticated"));
    }
}

#[tokio::test]
async fn mutations_without_csrf_get_403() {
    let h = harness().await;
    for uri in ["/api/v1/pr/resolve", "/api/v1/analyses"] {
        let body = json!({ "url": PR_URL });
        let (status, body) = json_of(h.send("POST", uri, Auth::Session, Some(body)).await).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(body["code"], json!("csrf_rejected"));
    }
    assert!(
        h.github
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "no GitHub call before the CSRF check"
    );
}

#[tokio::test]
async fn resolve_and_create_reject_invalid_names_and_private_repositories() {
    let h = harness().await;
    let (status, body) = h
        .post(
            "/api/v1/pr/resolve",
            json!({ "url": "https://example.test/acme/node-client/pull/482" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], json!("not_a_pr_url"));

    let (status, body) = h
        .post(
            "/api/v1/analyses",
            json!({
                "pr": { "repo": { "owner": "acme", "name": ".." }, "number": 482 },
                "head_sha": "0".repeat(40),
            }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], json!("invalid_request"));

    let mut private = read_json(&fixtures().join("github/rest/pull.json"))["response"].clone();
    private["number"] = json!(7);
    private["base"]["repo"]["private"] = json!(true);
    Mock::given(method("GET"))
        .and(path("/api/repos/acme/node-client/pulls/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(private))
        .mount(&h.github)
        .await;
    let (status, body) = h
        .post(
            "/api/v1/pr/resolve",
            json!({ "url": "https://github.com/acme/node-client/pull/7" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], json!("private_repository"));
}

#[tokio::test]
async fn tour_before_finish_is_409_and_unknown_analysis_404() {
    let h = harness().await;
    let id = h.seed_analysis().await;
    let (status, body) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], json!("analysis_not_finished"));
    let unknown = AnalysisId::new_random();
    let (status, body) = h.get(&format!("/api/v1/analyses/{unknown}/tour")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], json!("not_found"));
}

#[tokio::test]
async fn other_tenants_analysis_is_not_found() {
    let h = harness().await;
    let id = h.seed_analysis().await;
    db::repo::analyses::testing::seed_tenant(&h.state.db, "other")
        .await
        .expect("tenant");
    let now = Utc::now();
    h.state
        .db
        .sessions()
        .create(
            &NewSession {
                token_hash: &sha256(b"other-tenant-session"),
                tenant_id: "other",
                user_id: &h.user_id,
                csrf_token: CSRF,
                expires_at: now + TimeDelta::days(1),
            },
            now,
        )
        .await
        .expect("session");
    for uri in [
        format!("/api/v1/analyses/{id}"),
        format!("/api/v1/analyses/{id}/tour"),
        format!("/api/v1/analyses/{id}/lines?path=README.md&side=head&from=1&to=2"),
    ] {
        let request = Request::get(&uri)
            .header("cookie", "sr_session=other-tenant-session")
            .body(Body::empty())
            .expect("request");
        let response = api::router(h.state.clone())
            .oneshot(request)
            .await
            .expect("infallible");
        let (status, body) = json_of(response).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
        assert_eq!(body["code"], json!("not_found"));
    }
}

/// A pull request whose base branch moved after the fork point: returns (fork point, base tip, head).
fn diverged(repo: &Path) -> (String, String, String) {
    let fork = commit(
        repo,
        &[("a.txt", &numbered(1..=10)), ("b.txt", &numbered(1..=10))],
        "fork point",
    );
    git(repo, &["checkout", "-q", "-b", "feature"]);
    let head = commit(
        repo,
        &[(
            "a.txt",
            &numbered(1..=10).replace("line 2\n", "changed 2\n"),
        )],
        "change on the pull request",
    );
    git(repo, &["checkout", "-q", "main"]);
    let base_tip = commit(
        repo,
        &[(
            "b.txt",
            &numbered(1..=10).replace("line 1\n", "changed on main\n"),
        )],
        "change only on the base branch",
    );
    (fork, base_tip, head)
}

#[tokio::test]
async fn diverged_base_uses_the_merge_base() {
    let h = harness().await;
    let mut fork = String::new();
    let summary = h
        .analyse_repo("diverged", 9, |repo| {
            let (fork_point, base_tip, head) = diverged(repo);
            fork = fork_point;
            (base_tip, head)
        })
        .await;
    assert_eq!(
        summary.pr.base_sha.0, fork,
        "the analysis base is the merge base"
    );

    let lines = |file: &str, side: &str| {
        format!(
            "/api/v1/analyses/{}/lines?path={file}&side={side}&from=1&to=10",
            summary.id
        )
    };
    let (status, a) = h.get(&lines("a.txt", "head")).await;
    assert_eq!(status, StatusCode::OK, "{a}");
    assert_eq!(a["changed"], json!([2]));
    // b.txt changed only on the base branch: not part of the pull request.
    let (status, b) = h.get(&lines("b.txt", "head")).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b["changed"], json!([]));
    assert_eq!(b["comments"], json!({ "scope": "not_in_diff" }));
    let (status, b_base) = h.get(&lines("b.txt", "base")).await;
    assert_eq!(status, StatusCode::OK, "{b_base}");
    assert_eq!(
        b_base["lines"][0],
        json!("line 1"),
        "base side reads the merge base"
    );
}

/// Pull request #10 of `acme/big`: a file over the byte limit, a file whose first line is 1 MiB, and from
/// `keep.txt` a 5,000-line block removed after its line 1 and one line removed after its line 3.
async fn big_repo(h: &Harness) -> AnalysisSummary {
    h.analyse_repo("big", 10, |repo| {
        let huge = "x".repeat(1023) + "\n";
        let long = "y".repeat(1024 * 1024) + "\nend\n";
        let base = commit(
            repo,
            &[
                ("huge.txt", &huge.repeat(9 * 1024)),
                ("long.txt", &long),
                ("keep.txt", &numbered(1..=5006)),
            ],
            "base",
        );
        let kept = numbered(1..=1) + &numbered(5002..=5003) + &numbered(5005..=5006);
        let head = commit(repo, &[("keep.txt", &kept)], "remove a large block");
        (base, head)
    })
    .await
}

#[tokio::test]
async fn lines_refuses_blobs_over_the_byte_limit() {
    let h = harness().await;
    let summary = big_repo(&h).await;
    let (status, body) = h
        .get(&format!(
            "/api/v1/analyses/{}/lines?path=huge.txt&side=head&from=1&to=1",
            summary.id
        ))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], json!("not_rendered"));
}

#[tokio::test]
async fn lines_reads_a_window_of_a_file_with_a_very_long_line() {
    let h = harness().await;
    let summary = big_repo(&h).await;
    let (status, body) = h
        .get(&format!(
            "/api/v1/analyses/{}/lines?path=long.txt&side=head&from=2&to=2",
            summary.id
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total_lines"], json!(2));
    assert_eq!(body["lines"], json!(["end"]));
}

#[tokio::test]
async fn lines_caps_deleted_blocks() {
    let h = harness().await;
    let summary = big_repo(&h).await;
    let (status, body) = h
        .get(&format!(
            "/api/v1/analyses/{}/lines?path=keep.txt&side=head&from=1&to=4",
            summary.id
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let deleted = body["deleted"].as_array().expect("deleted");
    assert_eq!(
        deleted.len(),
        2,
        "a marker past the budget is still reported"
    );
    assert_eq!(deleted[0]["after_line"], json!(1));
    assert_eq!(deleted[0]["base_start"], json!(2));
    assert_eq!(deleted[0]["total_lines"], json!(5000));
    let removed = deleted[0]["lines"].as_array().expect("lines");
    assert_eq!(removed.len(), 2000, "the block is visibly truncated");
    assert_eq!(removed[0], json!("line 2"));
    assert_eq!(
        deleted[1],
        json!({ "after_line": 3, "base_start": 5004, "lines": [], "total_lines": 1 })
    );
}

async fn fail_flow_2(h: &Harness, script: Script) -> AnalysisSummary {
    h.executor.script("flow:f2", script);
    for flow in ["flow:f3", "flow:f4", "flow:f5"] {
        h.executor.script(flow, Script::Pending);
    }
    let (status, resolved) = h.post("/api/v1/pr/resolve", json!({ "url": PR_URL })).await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    let (status, created) = h
        .post(
            "/api/v1/analyses",
            json!({
                "pr": { "repo": { "owner": "acme", "name": "node-client" }, "number": 482 },
                "head_sha": resolved["pr"]["head_sha"],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    h.wait_until_ended(created["id"].as_str().expect("id"))
        .await
}

fn assert_failed_in_f2_and_rest_cancelled(h: &Harness, summary: &AnalysisSummary, message: &str) {
    match &summary.state {
        AnalysisState::Failed {
            flow: Some(flow),
            message: text,
        } => {
            assert_eq!(flow.id.0, "f2");
            assert!(text.contains(message), "{text}");
        }
        other => panic!("expected a failure in f2, got {other:?}"),
    }
    let cancelled = h.executor.cancelled();
    for flow in ["flow:f3", "flow:f4", "flow:f5"] {
        assert!(
            cancelled.contains(&flow.to_owned()),
            "{flow} not cancelled: {cancelled:?}"
        );
    }
    assert!(!cancelled.contains(&"flow:f2".to_owned()));
}

#[tokio::test]
async fn failed_job_cancels_the_other_jobs() {
    let h = harness().await;
    let summary = fail_flow_2(&h, Script::Fail).await;
    assert_failed_in_f2_and_rest_cancelled(&h, &summary, "The agent run failed");
}

#[tokio::test]
async fn schema_invalid_output_fails_and_cancels_the_other_jobs() {
    let h = harness().await;
    let summary = fail_flow_2(&h, Script::Output(json!({ "flow_id": "f2" }))).await;
    assert_failed_in_f2_and_rest_cancelled(&h, &summary, "did not have the expected structure");
}

#[tokio::test]
async fn restart_with_failing_preparation_cancels_live_jobs() {
    let h = harness().await;
    // The head commit is not on the remote: preparing the analysis (fetch) fails at restart.
    let id = h.seed_analysis_at(Some("f".repeat(40))).await;
    let queue: Arc<QueueExecutor> = Arc::new(QueueExecutor::new(h.state.db.clone()));
    let state = AppState::new(
        config(h._dir.path(), &h.github),
        h.state.db.clone(),
        queue.clone(),
    );
    let pending = queue
        .submit(JobRequest {
            spec: db::repo::analyses::testing::job_spec(id, JobKind::Plan),
            priority: JobPriority::Batch,
            stage: JobStage::Plan { batch: 1 },
            generation: 0,
            pr_repo: contracts::common::RepoRef {
                owner: "acme".to_owned(),
                name: "node-client".to_owned(),
            },
        })
        .await
        .expect("submit");

    server::pipeline::resume_unfinished(&state)
        .await
        .expect("resume");
    let summary = h.wait_until_ended(&id.to_string()).await;
    assert!(
        matches!(summary.state, AnalysisState::Failed { .. }),
        "{:?}",
        summary.state
    );
    let job = state.db.jobs().get(pending.job_id).await.expect("job");
    assert_eq!(job.state, JobState::Cancelled);
}

#[tokio::test]
async fn create_returns_at_once_and_lines_wait_for_preparation() {
    let h = harness().await;
    // No remote repository exists for acme/missing: the ingest fails after the analysis exists.
    let mut pull = read_json(&fixtures().join("github/rest/pull.json"))["response"].clone();
    pull["number"] = json!(11);
    pull["base"]["repo"]["name"] = json!("missing");
    let head = pull["head"]["sha"].clone();
    Mock::given(method("GET"))
        .and(path("/api/repos/acme/missing/pulls/11"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull))
        .mount(&h.github)
        .await;
    let (status, created) = h
        .post(
            "/api/v1/analyses",
            json!({
                "pr": { "repo": { "owner": "acme", "name": "missing" }, "number": 11 },
                "head_sha": head,
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["state"], json!({ "state": "planning" }));
    let id = created["id"].as_str().expect("id").to_owned();
    let ended = h.wait_until_ended(&id).await;
    match ended.state {
        AnalysisState::Failed {
            flow: None,
            message,
        } => {
            assert!(
                message.contains("repository could not be fetched"),
                "{message}"
            );
        }
        other => panic!("expected a failed preparation, got {other:?}"),
    }
    assert!(
        h.executor.submitted().is_empty(),
        "no job before preparation"
    );
}

#[tokio::test]
async fn lines_answer_409_until_the_merge_base_is_stored() {
    let h = harness().await;
    let (fork, base_tip, head) = {
        let mut out = (String::new(), String::new(), String::new());
        h.remote_repo("diverged", |repo| {
            out = diverged(repo);
            (out.1.clone(), out.2.clone())
        });
        out
    };
    let id = h.seed_unprepared("diverged", &base_tip, &head).await;
    let lines = format!("/api/v1/analyses/{id}/lines?path=a.txt&side=head&from=1&to=10");
    let (status, body) = h.get(&lines).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], json!("analysis_not_finished"));

    server::pipeline::start(&h.state, id);
    let ended = h.wait_until_ended(&id.to_string()).await;
    assert_eq!(ended.pr.base_sha.0, fork);
    let (status, body) = h.get(&lines).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["changed"], json!([2]));
}
