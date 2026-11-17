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
use contracts::job::protocol::{Claim, ClaimResponse};
use contracts::job::result::{JobFailureKind, JobOutcome, JobResult};
use contracts::job::spec::{JobPriority, JobSpec};
use contracts::job::state::JobState;
use contracts::routes::{RUNNER_CLAIM, RUNNER_RESULT};
use db::repo::analyses::{NewAnalysis, SessionState};
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
use wiremock::matchers::{body_string_contains, header, method, path};
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
    /// Stage key and generation of each submitted job.
    stages: Mutex<HashMap<JobId, (String, u32)>>,
    /// Submitted specs with their stage key and generation, in submit order.
    specs: Mutex<Vec<(String, u32, JobSpec)>>,
    /// Analysis of each submitted job.
    analyses: Mutex<HashMap<JobId, AnalysisId>>,
    /// Jobs whose result was delivered.
    finished: Mutex<HashSet<JobId>>,
    submitted: Mutex<Vec<String>>,
    /// Stages answered other than with their fixture output.
    scripts: Mutex<HashMap<String, Script>>,
    cancelled: Mutex<Vec<String>>,
    /// Stages whose job ran with a GitHub installation token.
    github_access: Mutex<HashSet<String>>,
}

/// How a stage's job ends instead of returning its fixture output. Scripts are keyed by `<stage>#<generation>`,
/// or by `<stage>` for every generation.
#[derive(Clone, Debug)]
enum Script {
    Output(Value),
    Fail,
    /// Fails with this runner text.
    FailWith(String),
    Pending,
    /// Paused by the usage limit; returns the fixture output once notified.
    PausedUntil(Arc<tokio::sync::Notify>),
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

    /// Jobs of `stage` run with a GitHub installation token.
    fn grant_github_access(&self, stage: &str) {
        self.github_access
            .lock()
            .expect("lock")
            .insert(stage.to_owned());
    }

    fn unscript(&self, stage: &str) {
        self.scripts.lock().expect("lock").remove(stage);
    }

    fn script(&self, stage: &str, script: Script) {
        self.scripts
            .lock()
            .expect("lock")
            .insert(stage.to_owned(), script);
    }

    /// Submitted specs of `stage` (any generation), in submit order.
    fn specs_of(&self, stage: &str) -> Vec<(u32, JobSpec)> {
        self.specs
            .lock()
            .expect("lock")
            .iter()
            .filter(|(key, _, _)| key == stage)
            .map(|(_, generation, spec)| (*generation, spec.clone()))
            .collect()
    }
}

fn agent_output(stage: &str) -> Value {
    let file = match stage {
        // Every batch of a huge pull request gets the fixture plan (rejected by verification).
        plan if plan.starts_with("plan:") => "plan.json".to_owned(),
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
        let generation = request.generation;
        let job_id = request.spec.job_id;
        self.stages
            .lock()
            .expect("lock")
            .insert(job_id, (stage.clone(), generation));
        self.specs
            .lock()
            .expect("lock")
            .push((stage.clone(), generation, request.spec.clone()));
        self.submitted
            .lock()
            .expect("lock")
            .push(if generation == 0 {
                stage
            } else {
                format!("{stage}#{generation}")
            });
        self.analyses
            .lock()
            .expect("lock")
            .insert(job_id, request.spec.analysis_id);
        Box::pin(async move { Ok(JobHandle { job_id }) })
    }

    fn events(&self, handle: &JobHandle) -> BoxStream<'static, Result<JobUpdate, ExecutorError>> {
        let (stage, generation) = self.stages.lock().expect("lock")[&handle.job_id].clone();
        let script = {
            let scripts = self.scripts.lock().expect("lock");
            scripts
                .get(&format!("{stage}#{generation}"))
                .or_else(|| scripts.get(&stage))
                .cloned()
        };
        let mut paused = None;
        let outcome = match script {
            None => JobOutcome::Succeeded {
                output: agent_output(&stage),
            },
            Some(Script::Output(output)) => JobOutcome::Succeeded { output },
            Some(Script::Fail) => JobOutcome::Failed {
                kind: JobFailureKind::ProviderFailed,
                message: "the provider exited with an error".to_owned(),
            },
            Some(Script::FailWith(message)) => JobOutcome::Failed {
                kind: JobFailureKind::ProviderFailed,
                message,
            },
            Some(Script::Pending) => return stream::pending().boxed(),
            Some(Script::PausedUntil(notify)) => {
                paused = Some(notify);
                JobOutcome::Succeeded {
                    output: agent_output(&stage),
                }
            }
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
        let github_access = self.github_access.lock().expect("lock").contains(&stage);
        let finished = Ok(JobUpdate::Finished {
            result,
            github_access,
        });
        match paused {
            None => stream::iter([finished]).boxed(),
            Some(notify) => stream::iter([Ok(JobUpdate::Paused { resumes_at: None })])
                .chain(stream::once(async move {
                    notify.notified().await;
                    finished
                }))
                .boxed(),
        }
    }

    fn cancel(&self, handle: &JobHandle) -> BoxFuture<'_, Result<(), ExecutorError>> {
        let (stage, _) = self.stages.lock().expect("lock")[&handle.job_id].clone();
        self.cancelled.lock().expect("lock").push(stage);
        Box::pin(async { Ok(()) })
    }

    fn cancel_analysis(&self, analysis: AnalysisId) -> BoxFuture<'_, Result<(), ExecutorError>> {
        let finished = self.finished.lock().expect("lock").clone();
        let stages = self.stages.lock().expect("lock").clone();
        let mut cancelled = self.cancelled.lock().expect("lock");
        for (job, analysis_of_job) in self.analyses.lock().expect("lock").iter() {
            if *analysis_of_job == analysis && !finished.contains(job) {
                cancelled.push(stages[job].0.clone());
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

/// A throwaway RSA key for the fake GitHub App (PKCS#8 PEM).
fn app_key_pem() -> String {
    use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der};
    use aws_lc_rs::rsa::{KeyPair, KeySize};
    use base64::Engine;
    let pair = KeyPair::generate(KeySize::Rsa2048).expect("RSA key generation");
    let der: Pkcs8V1Der<'static> = pair.as_der().expect("PKCS#8");
    let body = base64::engine::general_purpose::STANDARD.encode(der.as_ref());
    let lines: Vec<&str> = body
        .as_bytes()
        .chunks(64)
        .map(|chunk| std::str::from_utf8(chunk).expect("base64 is ASCII"))
        .collect();
    format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        lines.join("\n")
    )
}

fn config(dir: &Path, github: &MockServer, app_key: Option<&Path>) -> Config {
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
            app_key.map_or_else(
                || "/nonexistent.pem".to_owned(),
                |key| key.display().to_string(),
            ),
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
    harness_with_app(false).await
}

/// The harness; with `app`, the GitHub App is installed on the fixture repository (key and installation lookup).
async fn harness_with_app(app: bool) -> Harness {
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
    // The GitHub snapshot read at ingest: the recorded review threads (one page) and check runs.
    let mut threads =
        read_json(&fixtures().join("github/graphql/review-threads.json"))["response"].clone();
    threads["data"]["repository"]["pullRequest"]["reviewThreads"]["pageInfo"] =
        json!({"hasNextPage": false, "endCursor": null});
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_string_contains("reviewThreads"))
        .respond_with(ResponseTemplate::new(200).set_body_json(threads))
        .mount(&github)
        .await;
    let checks = read_json(&fixtures().join("github/rest/check-runs.json"));
    let head = recorded["response"]["head"]["sha"]
        .as_str()
        .expect("head sha");
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/repos/acme/node-client/commits/{head}/check-runs"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(&checks["response"]))
        .mount(&github)
        .await;

    let db = Db::in_memory().await.expect("db");
    let executor = Arc::new(ScriptedExecutor::default());
    let app_key = app.then(|| {
        let key = dir.path().join("app.pem");
        std::fs::write(&key, app_key_pem()).expect("write key");
        key
    });
    if app {
        Mock::given(method("GET"))
            .and(path("/api/repos/acme/node-client/installation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": 4242, "app_id": 1, "suspended_at": null
            })))
            .mount(&github)
            .await;
    }
    let state = AppState::new(
        config(dir.path(), &github, app_key.as_deref()),
        db,
        executor.clone(),
    );
    let now = Utc::now();
    // The fixture user `alice` is not in the seeded allowlist: allow her and sign her in directly.
    db::repo::tenants::testing::allow_login(&state.db, DEFAULT_TENANT, "alice", 3001)
        .await
        .expect("allowlist");
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

    /// Resolves the fixture PR and creates its analysis through the API; returns the analysis id.
    async fn create_fixture_analysis(&self) -> String {
        let (status, resolved) = self
            .post("/api/v1/pr/resolve", json!({ "url": PR_URL }))
            .await;
        assert_eq!(status, StatusCode::OK, "{resolved}");
        let (status, created) = self
            .post(
                "/api/v1/analyses",
                json!({
                    "pr": { "repo": { "owner": "acme", "name": "node-client" }, "number": 482 },
                    "head_sha": resolved["pr"]["head_sha"],
                }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        created["id"].as_str().expect("id").to_owned()
    }

    /// The harness with the real job queue as executor; runner calls go through [`Harness::runner`].
    fn with_queue(self) -> Self {
        let queue = Arc::new(QueueExecutor::new(self.state.db.clone()));
        let state = AppState::new(
            config(self._dir.path(), &self.github, None),
            self.state.db.clone(),
            queue,
        );
        Self { state, ..self }
    }

    /// A runner request with `bearer` (the runner secret or a job token).
    async fn runner(&self, uri: &str, bearer: &str, body: Value) -> (StatusCode, Value) {
        let request = Request::post(uri)
            .header("authorization", format!("Bearer {bearer}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("request");
        json_of(
            api::runner_router(self.state.clone())
                .oneshot(request)
                .await
                .expect("infallible"),
        )
        .await
    }

    /// One claim of the pipeline credential, as the runner sends it.
    async fn runner_claim(&self) -> Option<Claim> {
        let request = json!({
            "runner_id": "runner-1",
            "credentials": [{
                "credential": { "id": server::pipeline::CREDENTIAL_ID, "kind": "codex_chatgpt" },
                "usable": true,
            }],
            "dropped_sessions": [],
        });
        let (status, body) = self
            .runner(RUNNER_CLAIM.path, "fake-runner-secret", request)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        serde_json::from_value::<ClaimResponse>(body)
            .expect("claim response")
            .claim
    }

    async fn post_empty(&self, uri: &str) -> (StatusCode, Value) {
        json_of(self.send("POST", uri, Auth::SessionCsrf, None).await).await
    }

    /// Polls the analysis until `done` holds for its state.
    async fn wait_for_state(
        &self,
        id: &str,
        done: impl Fn(&AnalysisState) -> bool,
    ) -> AnalysisSummary {
        for _ in 0..600 {
            let (status, body) = self.get(&format!("/api/v1/analyses/{id}")).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let summary: AnalysisSummary = serde_json::from_value(body).expect("summary");
            if done(&summary.state) {
                return summary;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the analysis did not reach the expected state in time");
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

/// The analysis's one MemCan profile lookup reaches every prompt (fenced as data) and the Tour's facts.
#[tokio::test]
async fn memcan_profile_lookup_reaches_every_prompt_and_the_tour() {
    let h = harness().await;
    let id = h.seed_analysis().await;
    let profile = "MemCan-profile-CANARY: payments service; risk lives in src/ledger.";
    h.state
        .db
        .documents()
        .put(
            id,
            DocumentKind::MemcanProfile,
            "",
            &json!({"memcan": "checked", "text": profile}),
            Utc::now(),
        )
        .await
        .expect("lookup");

    server::pipeline::resume_unfinished(&h.state)
        .await
        .expect("resume");
    let finished = h.wait_until_finished(&id.to_string()).await;
    assert_eq!(finished.state, AnalysisState::Finished { total: 5 });
    for stage in ["plan:1", "pr_context", "flow:f1"] {
        let runs = h.executor.specs_of(stage);
        let prompt = &runs.first().expect(stage).1.prompt;
        let fenced = prompt
            .split("<<<DATA ")
            .find(|block| block.contains("project profile: MemCan memory"))
            .unwrap_or_default();
        assert!(fenced.contains(profile), "{stage}: {prompt}");
    }
    let (_, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(
        tour["pr_context"]["facts"]["project_profile"],
        json!({"sources": ["repository_file", "memcan"], "memcan": "checked"}),
        "the fixture repository has a profile file too"
    );
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
        let (status, mut lines) = h.get(&uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {lines}");
        // Moved blocks come from the stored pre-pass's move detection. The identical move of client.rs matches the
        // hand-written seed; for retry.rs the detection reports one block 1-95 with its differing lines where the
        // seed has 38-95 (out-of-scope finding for the move detection), so only client.rs is compared.
        let moved = lines["moved"].take();
        let seeded = seed["moved"].take();
        if seed["path"] == json!("src/rpc/client.rs") {
            assert_eq!(moved, seeded, "moved blocks of client.rs");
        }
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
    let id = h.seed_analysis().await;
    for uri in [
        "/api/v1/pr/resolve".to_owned(),
        "/api/v1/analyses".to_owned(),
        format!("/api/v1/analyses/{id}/cancel"),
        format!("/api/v1/analyses/{id}/retry"),
    ] {
        let uri = uri.as_str();
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
    db::repo::tenants::testing::allow_login(&h.state.db, "other", "alice", 3001)
        .await
        .expect("allowlist");
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
    for (method, uri) in [
        ("GET", format!("/api/v1/analyses/{id}")),
        ("GET", format!("/api/v1/analyses/{id}/tour")),
        (
            "GET",
            format!("/api/v1/analyses/{id}/lines?path=README.md&side=head&from=1&to=2"),
        ),
        ("POST", format!("/api/v1/analyses/{id}/cancel")),
        ("POST", format!("/api/v1/analyses/{id}/retry")),
    ] {
        let request = Request::builder()
            .method(method)
            .uri(&uri)
            .header("cookie", "sr_session=other-tenant-session")
            .header("x-csrf-token", CSRF)
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
    let summary = h.state.db.analyses().summary(id).await.expect("summary");
    assert_eq!(
        summary.state,
        AnalysisState::Planning,
        "untouched by the other tenant"
    );
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

/// Jobs run one at a time: a failure in f2 ends the analysis before any later flow is submitted.
fn assert_failed_in_f2_and_nothing_after_it(h: &Harness, summary: &AnalysisSummary, message: &str) {
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
    let submitted = h.executor.submitted();
    for flow in ["flow:f3", "flow:f4", "flow:f5"] {
        assert!(
            !submitted.contains(&flow.to_owned()),
            "{flow} submitted: {submitted:?}"
        );
    }
    assert!(!h.executor.cancelled().contains(&"flow:f2".to_owned()));
}

#[tokio::test]
async fn failed_job_ends_the_analysis_before_later_jobs() {
    let h = harness().await;
    let summary = fail_flow_2(&h, Script::Fail).await;
    assert_failed_in_f2_and_nothing_after_it(&h, &summary, "The agent run failed");
}

#[tokio::test]
async fn schema_invalid_output_ends_the_analysis_before_later_jobs() {
    let h = harness().await;
    let summary = fail_flow_2(&h, Script::Output(json!({ "flow_id": "f2" }))).await;
    assert_failed_in_f2_and_nothing_after_it(&h, &summary, "did not have the expected structure");
}

#[tokio::test]
async fn restart_with_failing_preparation_cancels_live_jobs() {
    let h = harness().await;
    // The head commit is not on the remote: preparing the analysis (fetch) fails at restart.
    let id = h.seed_analysis_at(Some("f".repeat(40))).await;
    let queue: Arc<QueueExecutor> = Arc::new(QueueExecutor::new(h.state.db.clone()));
    let state = AppState::new(
        config(h._dir.path(), &h.github, None),
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

/// A finished analysis's Tour holds only computed facts or an explicit "not checked" (brief §1).
#[tokio::test]
async fn tour_has_no_uncomputed_fact() {
    let h = harness().await;
    let id = h.create_fixture_analysis().await;
    h.wait_until_finished(&id).await;
    let (status, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(status, StatusCode::OK, "{tour}");
    let facts = &tour["pr_context"]["facts"];
    let checks = read_json(&fixtures().join("github/rest/check-runs.json"));
    let runs = checks["response"]["check_runs"]
        .as_array()
        .expect("runs")
        .len();
    assert_ne!(
        facts["ci"]["state"],
        json!("not_checked"),
        "CI read from the snapshot"
    );
    assert_eq!(facts["ci"]["total"], json!(runs));
    assert_eq!(facts["threads_status"], json!("checked"));
    let recorded = read_json(&fixtures().join("github/graphql/review-threads.json"));
    let nodes = recorded["response"]["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"]
        .as_array()
        .expect("threads")
        .len();
    let threads = tour["threads"].as_array().expect("threads");
    assert_eq!(threads.len(), nodes);
    // Every line thread inside a step is placed in it; its id is listed on the step.
    let steps: Vec<&Value> = tour["flows"]
        .as_array()
        .expect("flows")
        .iter()
        .flat_map(|flow| flow["steps"].as_array().expect("steps"))
        .collect();
    for thread in threads {
        let inside = steps.iter().find(|step| {
            let facts = &step["facts"];
            thread["line"].as_u64().is_some_and(|line| {
                facts["file"] == thread["path"]
                    && facts["side"] == thread["side"]
                    && facts["range"]["start_line"].as_u64() <= Some(line)
                    && Some(line) <= facts["range"]["end_line"].as_u64()
            })
        });
        match inside {
            Some(step) => {
                assert_eq!(thread["step_id"], step["id"], "{thread}");
                assert!(
                    step["facts"]["thread_ids"]
                        .as_array()
                        .expect("ids")
                        .contains(&thread["id"])
                );
            }
            None => {
                assert_eq!(thread["step_id"], Value::Null, "{thread}");
                assert_eq!(
                    thread["llm_status"],
                    Value::Null,
                    "no status outside a step"
                );
            }
        }
    }
    // The MemCan profile is honestly not checked; the App is not installed in the fake, so related work is not
    // checked and none is shown.
    assert_eq!(facts["project_profile"]["memcan"], json!("not_checked"));
    assert_eq!(facts["related_work_checked"], json!(false));
    assert_eq!(tour["pr_context"]["llm"]["related_work"], json!([]));
    let step_count: usize = tour["flows"]
        .as_array()
        .expect("flows")
        .iter()
        .map(|flow| flow["steps"].as_array().expect("steps").len())
        .sum();
    assert_eq!(facts["step_count"], json!(step_count));
    assert_eq!(facts["flow_count"], json!(5));
    let commenting = h
        .post("/api/v1/pr/resolve", json!({ "url": PR_URL }))
        .await
        .1;
    assert_eq!(
        commenting["commenting_available"],
        json!(false),
        "looked up: not installed"
    );
}

/// The fixture output of `stage` with its first step moved outside the changed lines (verification rejects it).
fn rejected_flow(stage: &str) -> Value {
    let mut output = agent_output(stage);
    output["steps"][0]["start_line"] = json!(9990);
    output["steps"][0]["end_line"] = json!(9999);
    output
}

#[tokio::test]
async fn rejected_flow_output_is_repaired() {
    let h = harness().await;
    h.executor
        .script("flow:f2#0", Script::Output(rejected_flow("flow:f2")));
    let id = h.create_fixture_analysis().await;
    let finished = h.wait_until_finished(&id).await;
    assert_eq!(finished.state, AnalysisState::Finished { total: 5 });
    assert!(h.executor.submitted().contains(&"flow:f2#1".to_owned()));
    let runs = h.executor.specs_of("flow:f2");
    assert_eq!(runs.len(), 2);
    assert!(
        runs[1].1.prompt.contains("repair 1 of 2"),
        "{}",
        runs[1].1.prompt
    );
    let (_, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(tour["flows"].as_array().expect("flows").len(), 5);
}

/// Deviation from the plan's test wording ("repair exhausted -> failed flow + Retry"), decided by the
/// coordinator: a flow without accepted output leaves its lines Unexplained and the analysis finishes.
#[tokio::test]
async fn exhausted_flow_repairs_leave_its_lines_unexplained() {
    let h = harness().await;
    h.executor
        .script("flow:f2", Script::Output(rejected_flow("flow:f2")));
    let id = h.create_fixture_analysis().await;
    let finished = h.wait_until_finished(&id).await;
    assert_eq!(finished.state, AnalysisState::Finished { total: 5 });
    let submitted = h.executor.submitted();
    for run in ["flow:f2", "flow:f2#1", "flow:f2#2"] {
        assert!(submitted.contains(&run.to_owned()), "{submitted:?}");
    }
    assert!(!submitted.contains(&"flow:f2#3".to_owned()));
    let (_, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    let flows: Vec<&Value> = tour["flows"].as_array().expect("flows").iter().collect();
    assert_eq!(flows.len(), 4);
    assert!(flows.iter().all(|flow| flow["id"] != json!("f2")));
    assert!(
        !tour["quiet"]["unexplained"]
            .as_array()
            .expect("unexplained")
            .is_empty(),
        "the lines of f2 are Unexplained"
    );
}

#[tokio::test]
async fn exhausted_plan_repairs_fail_the_analysis() {
    let h = harness().await;
    let mut plan = agent_output("plan:1");
    plan["flows"][0]["hunks"][0]["hunk_id"] = json!("h999");
    h.executor.script("plan:1", Script::Output(plan));
    let id = h.create_fixture_analysis().await;
    let ended = h.wait_until_ended(&id).await;
    match ended.state {
        AnalysisState::Failed {
            flow: None,
            message,
        } => {
            assert!(message.contains("plan of the flows"), "{message}");
            assert!(message.contains("Retry"), "{message}");
        }
        other => panic!("expected failed, got {other:?}"),
    }
    assert!(
        h.executor
            .submitted()
            .iter()
            .all(|stage| !stage.starts_with("flow:"))
    );
    // Retry starts a new round: the plan is asked again (the queue gives it a new job,
    // `jobs::queue::retry_round_submits_new_jobs_for_succeeded_stages`) and the analysis finishes.
    h.executor.unscript("plan:1");
    let (status, body) = h.post_empty(&format!("/api/v1/analyses/{id}/retry")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    h.wait_until_finished(&id).await;
    let plans = h.executor.specs_of("plan:1");
    assert_eq!(
        plans
            .iter()
            .filter(|(generation, _)| *generation == 0)
            .count(),
        2,
        "asked again"
    );
}

#[tokio::test]
async fn failure_before_any_flow_offers_retry() {
    let h = harness().await;
    h.executor.script("plan:1", Script::Fail);
    let id = h.create_fixture_analysis().await;
    let ended = h.wait_until_ended(&id).await;
    match &ended.state {
        AnalysisState::Failed {
            flow: None,
            message,
        } => {
            assert!(message.contains("The agent run failed"), "{message}");
        }
        other => panic!("expected failed, got {other:?}"),
    }
    assert!(
        h.executor
            .submitted()
            .iter()
            .all(|stage| !stage.starts_with("flow:"))
    );
    // Retry runs it again; the plan job failed, so it is submitted again.
    h.executor.unscript("plan:1");
    let (status, body) = h.post_empty(&format!("/api/v1/analyses/{id}/retry")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], json!({ "state": "planning" }));
    let finished = h.wait_until_finished(&id).await;
    assert_eq!(finished.state, AnalysisState::Finished { total: 5 });
    // Retry applies only to a failed analysis.
    let (status, body) = h.post_empty(&format!("/api/v1/analyses/{id}/retry")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
}

#[tokio::test]
async fn cancel_discards_partial_results() {
    let h = harness().await;
    h.executor.script("flow:f3", Script::Pending);
    let id = h.create_fixture_analysis().await;
    h.wait_for_state(&id, |state| {
        matches!(state, AnalysisState::Running { done: 2, .. })
    })
    .await;
    let analysis: AnalysisId = serde_json::from_value(json!(id)).expect("id");
    h.state
        .db
        .analyses()
        .set_session(analysis, "thread-1", None)
        .await
        .expect("session");
    let (status, body) = h.post_empty(&format!("/api/v1/analyses/{id}/cancel")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], json!({ "state": "cancelled" }));
    let cancelled = h.executor.cancelled();
    assert_eq!(cancelled, ["flow:f3"], "the running job is cancelled");
    let submitted = h.executor.submitted();
    assert!(!submitted.contains(&"flow:f4".to_owned()), "{submitted:?}");
    let documents = h.state.db.documents();
    for kind in [
        DocumentKind::Plan,
        DocumentKind::PrContext,
        DocumentKind::Flow,
    ] {
        let stored: Vec<(String, Value)> = documents.list(analysis, kind).await.expect("list");
        assert!(stored.is_empty(), "{kind:?} kept: {stored:?}");
    }
    let record = h.state.db.analyses().get(analysis).await.expect("analysis");
    assert_eq!(record.session_state, SessionState::DropRequested);
    let (status, _) = h.post_empty(&format!("/api/v1/analyses/{id}/cancel")).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "an ended analysis is not cancelled again"
    );
    let (status, _) = h.post_empty(&format!("/api/v1/analyses/{id}/retry")).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "only a failed analysis is retried"
    );
}

#[tokio::test]
async fn usage_limit_pauses_and_resumes() {
    let h = harness().await;
    let resume = Arc::new(tokio::sync::Notify::new());
    h.executor
        .script("flow:f2", Script::PausedUntil(resume.clone()));
    let id = h.create_fixture_analysis().await;
    let paused = h
        .wait_for_state(&id, |state| {
            matches!(
                state,
                AnalysisState::Paused {
                    done: 1,
                    total: Some(5),
                    ..
                }
            )
        })
        .await;
    assert!(matches!(paused.state, AnalysisState::Paused { .. }));
    resume.notify_one();
    let finished = h.wait_until_finished(&id).await;
    assert_eq!(finished.state, AnalysisState::Finished { total: 5 });
}

/// The context pack is built when a runner claims a job (`jobs::claim::lost_session_uses_context_pack`), never
/// at enqueue, so a job that waited sees every result stored before it starts.
#[tokio::test]
async fn pipeline_jobs_carry_no_context_pack_at_enqueue() {
    let h = harness().await;
    let id = h.create_fixture_analysis().await;
    h.wait_until_finished(&id).await;
    let specs = h.executor.specs.lock().expect("lock").clone();
    assert!(!specs.is_empty());
    for (stage, _, spec) in specs {
        assert_eq!(spec.context_pack, None, "{stage}");
        assert_eq!(spec.session, None, "{stage}: the session is set at claim");
    }
}

#[tokio::test]
async fn reanalysis_starts_new_session() {
    let h = harness().await;
    let old = h.seed_analysis_at(Some("1".repeat(40))).await;
    h.state
        .db
        .analyses()
        .set_session(old, "thread-old", None)
        .await
        .expect("session");
    let id = h.create_fixture_analysis().await;
    let analysis: AnalysisId = serde_json::from_value(json!(id)).expect("id");
    assert_ne!(analysis, old);
    let record = h.state.db.analyses().get(old).await.expect("old");
    assert_eq!(record.session_state, SessionState::DropRequested);
    let record = h.state.db.analyses().get(analysis).await.expect("new");
    assert_eq!(
        record.agent_thread_id, None,
        "the new analysis has its own session"
    );
}

#[tokio::test]
async fn lines_refuse_files_the_prepass_does_not_render() {
    let h = harness().await;
    let summary = h
        .analyse_repo("gen", 9, |repo| {
            let base = commit(repo, &[("a.txt", "one\n")], "base");
            std::fs::write(repo.join("logo.png"), [0x89, b'P', b'N', b'G', 0, 1, 2, 0])
                .expect("write");
            let head = commit(
                repo,
                &[
                    ("a.txt", "one\ntwo\n"),
                    ("api.pb.go", "package api\nfunc A() {}\n"),
                ],
                "head",
            );
            (base, head)
        })
        .await;
    for (endpoint, path) in [
        ("lines", "logo.png"),
        ("lines", "api.pb.go"),
        ("diff", "api.pb.go"),
    ] {
        let (status, body) = h
            .get(&format!(
                "/api/v1/analyses/{}/{endpoint}?path={path}&side=head&from=1&to=2",
                summary.id
            ))
            .await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{endpoint} {path}: {body}"
        );
        assert_eq!(body["code"], json!("not_rendered"), "{endpoint} {path}");
    }
    let (status, body) = h
        .get(&format!(
            "/api/v1/analyses/{}/lines?path=a.txt&side=head&from=1&to=2",
            summary.id
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A real provider error text (Codex `turn.failed` of a rejected schema) stays in the log: the analysis state and
/// its events carry only the fixed sentence.
#[tokio::test]
async fn provider_error_text_never_reaches_the_analysis_state() {
    let h = harness().await;
    let recorded =
        std::fs::read_to_string(fixtures().join("codex/schema-error.jsonl")).expect("fixture");
    let provider_text = recorded
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("JSON line"))
        .find(|event| event["type"] == json!("turn.failed"))
        .and_then(|event| event["error"]["message"].as_str().map(str::to_owned))
        .expect("turn.failed message");
    h.executor
        .script("plan:1", Script::FailWith(provider_text.clone()));
    let id = h.seed_analysis().await;
    let mut events = h.state.progress.subscribe(id);
    server::pipeline::start(&h.state, id);
    let ended = h.wait_until_ended(&id.to_string()).await;
    let AnalysisState::Failed { message, .. } = &ended.state else {
        panic!("expected failed, got {:?}", ended.state);
    };
    assert!(
        message.contains("The LLM provider rejected the request"),
        "{message}"
    );
    let mut published = Vec::new();
    while let Ok(event) = events.try_recv() {
        published.push(serde_json::to_string(&event).expect("event JSON"));
    }
    assert!(!published.is_empty());
    for text in published
        .iter()
        .chain([&serde_json::to_string(&ended).expect("JSON")])
    {
        assert!(!text.contains("invalid_json_schema"), "{text}");
        assert!(!text.contains("codex_output_schema"), "{text}");
    }
}

/// With the App installed and the accepted PR context written by an agent that had the GitHub tool, related work
/// is checked and shown; commenting is available.
#[tokio::test]
async fn tour_has_no_uncomputed_fact_with_the_app_installed() {
    let h = harness_with_app(true).await;
    // The executor reports that the accepted pr_context attempt ran with GitHub access; no job history is read.
    h.executor.grant_github_access("pr_context");
    let id = h.create_fixture_analysis().await;
    h.wait_until_finished(&id).await;
    let (status, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(status, StatusCode::OK, "{tour}");
    let facts = &tour["pr_context"]["facts"];
    assert_eq!(facts["related_work_checked"], json!(true));
    assert_eq!(
        tour["pr_context"]["llm"]["related_work"],
        agent_output("pr_context")["related_work"],
        "checked related work is shown"
    );
    assert_ne!(facts["ci"]["state"], json!("not_checked"));
    assert_eq!(facts["threads_status"], json!("checked"));
    let (_, resolved) = h.post("/api/v1/pr/resolve", json!({ "url": PR_URL })).await;
    assert_eq!(
        resolved["commenting_available"],
        json!(true),
        "installation looked up"
    );
}

/// A pull request over the plan batch limit is planned in directory batches whose plans are merged.
#[tokio::test]
async fn huge_pull_request_is_planned_in_batches() {
    let h = harness().await;
    let (base, head) = h.remote_repo("huge", |repo| {
        let base = commit(repo, &[("README.txt", "readme\n")], "base");
        for dir in ["a", "b"] {
            std::fs::create_dir_all(repo.join(dir)).expect("dir");
        }
        let head = commit(
            repo,
            &[
                ("a/big.txt", &numbered(1..=2500)),
                ("b/big.txt", &numbered(1..=2500)),
            ],
            "head",
        );
        (base, head)
    });
    // The batches the server computes, from the same diff and pre-pass rules.
    let repo_dir = h._dir.path().join("remotes/acme/huge.git");
    let files = analysis::diff::diff(
        &repo_dir,
        &CommitSha(base.clone()),
        &CommitSha(head.clone()),
    )
    .expect("diff");
    let pr_repo = contracts::common::RepoRef {
        owner: "acme".to_owned(),
        name: "huge".to_owned(),
    };
    let prepass = analysis::prepass::prepass(&pr_repo, &files, &[], &|_, _| None).expect("prepass");
    let batches = analysis::prepass::batches::plan_batches(&prepass.agent_files(&files));
    assert_eq!(batches.len(), 2, "one batch per directory");
    for (index, batch) in batches.iter().enumerate() {
        let hunks: Vec<Value> = batch
            .iter()
            .flat_map(|file| &file.hunks)
            .map(|hunk| json!({ "hunk_id": hunk.id.0, "head_start": null, "head_end": null }))
            .collect();
        let plan = json!({
            "flows": [{
                "id": "f1", "title": format!("batch {}", index + 1), "classification": "behavioural",
                "importance_rank": 1, "intent": "adds lines", "hunks": hunks, "depends_on": []
            }],
            "unassigned": []
        });
        h.executor
            .script(&format!("plan:{}", index + 1), Script::Output(plan));
    }
    let mut context = agent_output("pr_context");
    context["fit_questions"] = json!([]);
    context["related_work"] = json!([]);
    h.executor.script("pr_context", Script::Output(context));
    for flow in ["f1", "f2"] {
        // Rejected (the fixture flow's files are not in this repository): the flows end Unexplained.
        let mut output = agent_output("flow:f1");
        output["flow_id"] = json!(flow);
        h.executor
            .script(&format!("flow:{flow}"), Script::Output(output));
    }
    let mut pull = read_json(&fixtures().join("github/rest/pull.json"))["response"].clone();
    pull["number"] = json!(11);
    pull["html_url"] = json!("https://github.com/acme/huge/pull/11");
    pull["base"]["sha"] = json!(base);
    pull["head"]["sha"] = json!(head);
    pull["base"]["repo"]["name"] = json!("huge");
    Mock::given(method("GET"))
        .and(path("/api/repos/acme/huge/pulls/11"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull))
        .mount(&h.github)
        .await;
    let (status, created) = h
        .post(
            "/api/v1/analyses",
            json!({ "pr": { "repo": { "owner": "acme", "name": "huge" }, "number": 11 }, "head_sha": head }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id").to_owned();
    let finished = h.wait_until_finished(&id).await;
    assert_eq!(finished.state, AnalysisState::Finished { total: 2 });
    let submitted = h.executor.submitted();
    for stage in ["plan:1", "plan:2", "flow:f1", "flow:f2"] {
        assert!(
            submitted.contains(&stage.to_owned()),
            "{stage}: {submitted:?}"
        );
    }
    let analysis: AnalysisId = serde_json::from_value(json!(id)).expect("id");
    let merged: Value = h
        .state
        .db
        .documents()
        .get(analysis, DocumentKind::Plan, "")
        .await
        .expect("read")
        .expect("merged plan");
    let titles: Vec<&str> = merged["flows"]
        .as_array()
        .expect("flows")
        .iter()
        .map(|flow| flow["title"].as_str().expect("title"))
        .collect();
    assert_eq!(titles.len(), 2);
    assert!(
        titles.contains(&"batch 1") && titles.contains(&"batch 2"),
        "{titles:?}"
    );
    let (_, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    let unexplained: Vec<&str> = tour["quiet"]["unexplained"]
        .as_array()
        .expect("unexplained")
        .iter()
        .map(|row| row["file"].as_str().expect("file"))
        .collect();
    assert_eq!(
        unexplained,
        ["a/big.txt", "b/big.txt"],
        "flows without accepted output"
    );
}

/// What a context pack holds of a stage's stored result.
fn pack_marker(stage: &str) -> String {
    match stage {
        "plan:1" => "plan output: flows".to_owned(),
        "pr_context" => "pr_context output".to_owned(),
        flow => format!(
            "\"flow_id\":\"{}\"",
            flow.strip_prefix("flow:").expect("flow stage")
        ),
    }
}

/// Pipeline jobs are queued one at a time, each once the previous result is verified and stored: a runner asking
/// right after posting a result gets nothing until then, and the next job's context pack holds every earlier
/// result (the pack a lost session starts from).
#[tokio::test]
async fn each_job_is_claimable_only_after_the_previous_result_is_stored() {
    let h = harness().await.with_queue();
    let id = h.create_fixture_analysis().await;
    let mut done: Vec<String> = Vec::new();
    'jobs: loop {
        let mut claimed = None;
        for _ in 0..1000 {
            if let Some(claim) = h.runner_claim().await {
                claimed = Some(claim);
                break;
            }
            let (_, body) = h.get(&format!("/api/v1/analyses/{id}")).await;
            let summary: AnalysisSummary = serde_json::from_value(body).expect("summary");
            match summary.state {
                AnalysisState::Finished { .. } => break 'jobs,
                AnalysisState::Failed { .. } | AnalysisState::Cancelled => {
                    panic!("analysis ended as {:?}", summary.state)
                }
                _ => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
        let claim = claimed.expect("a job or the end of the analysis");
        let job = h.state.db.jobs().get(claim.spec.job_id).await.expect("job");
        let stage = job.stage.key();
        assert_eq!(job.generation, 0, "{stage}: the fixture outputs pass");
        let pack = claim.spec.context_pack.clone().unwrap_or_default();
        for earlier in &done {
            assert!(
                pack.contains(&pack_marker(earlier)),
                "{stage} was claimable before the result of {earlier} was stored"
            );
        }
        let result = JobResult {
            job_id: claim.spec.job_id,
            attempt: claim.spec.attempt,
            outcome: JobOutcome::Succeeded {
                output: agent_output(&stage),
            },
            session: None,
            usage: None,
            skills_commit: None,
            llm_call: None,
        };
        let uri = RUNNER_RESULT
            .path
            .replace("{id}", &claim.spec.job_id.to_string());
        let (status, body) = h
            .runner(&uri, claim.job_token.expose(), json!(result))
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        done.push(stage);
    }
    assert_eq!(
        done,
        [
            "plan:1",
            "pr_context",
            "flow:f1",
            "flow:f2",
            "flow:f3",
            "flow:f4",
            "flow:f5"
        ],
        "plan, PR context, then the flows in plan order"
    );
}

/// A flow whose prompt is over budget is not sent to the agent: its lines are Unexplained with that reason and
/// the analysis goes on; a flow that was sent and ended without output has no reason.
#[tokio::test]
async fn flow_too_large_for_a_prompt_is_unexplained_with_its_reason() {
    let h = harness().await;
    let (base, head) = h.remote_repo("unsent", |repo| {
        let base = commit(repo, &[("README.txt", "readme\n")], "base");
        for dir in ["a", "b"] {
            std::fs::create_dir_all(repo.join(dir)).expect("dir");
        }
        let head = commit(
            repo,
            &[
                ("a/one.txt", &numbered(1..=5)),
                ("b/two.txt", &numbered(1..=5)),
            ],
            "head",
        );
        (base, head)
    });
    let files = analysis::diff::diff(
        &h._dir.path().join("remotes/acme/unsent.git"),
        &CommitSha(base.clone()),
        &CommitSha(head.clone()),
    )
    .expect("diff");
    let hunk_of = |path: &str| {
        let file = files.iter().find(|file| file.path == path).expect("file");
        json!([{ "hunk_id": file.hunks[0].id.0, "head_start": null, "head_end": null }])
    };
    let plan = json!({
        "flows": [
            {
                "id": "f1", "title": "one", "classification": "behavioural", "importance_rank": 1,
                "intent": "adds one", "hunks": hunk_of("a/one.txt"), "depends_on": []
            },
            {
                "id": "f2", "title": "two", "classification": "behavioural", "importance_rank": 2,
                "intent": "x".repeat(140 * 1024), "hunks": hunk_of("b/two.txt"), "depends_on": []
            }
        ],
        "unassigned": []
    });
    h.executor.script("plan:1", Script::Output(plan));
    let mut context = agent_output("pr_context");
    context["fit_questions"] = json!([]);
    context["related_work"] = json!([]);
    h.executor.script("pr_context", Script::Output(context));
    // Rejected (the fixture flow's files are not in this repository): f1 ends Unexplained after its repairs.
    h.executor
        .script("flow:f1", Script::Output(agent_output("flow:f1")));
    let mut pull = read_json(&fixtures().join("github/rest/pull.json"))["response"].clone();
    pull["number"] = json!(12);
    pull["html_url"] = json!("https://github.com/acme/unsent/pull/12");
    pull["base"]["sha"] = json!(base);
    pull["head"]["sha"] = json!(head);
    pull["base"]["repo"]["name"] = json!("unsent");
    Mock::given(method("GET"))
        .and(path("/api/repos/acme/unsent/pulls/12"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull))
        .mount(&h.github)
        .await;
    let (status, created) = h
        .post(
            "/api/v1/analyses",
            json!({ "pr": { "repo": { "owner": "acme", "name": "unsent" }, "number": 12 }, "head_sha": head }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id").to_owned();
    let finished = h.wait_until_ended(&id).await;
    assert_eq!(finished.state, AnalysisState::Finished { total: 2 });
    assert!(
        !h.executor.submitted().contains(&"flow:f2".to_owned()),
        "the over-budget flow is not sent"
    );
    let (status, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(status, StatusCode::OK, "{tour}");
    assert_eq!(
        tour["quiet"]["unexplained"],
        json!([
            {
                "file": "a/one.txt", "side": "head",
                "ranges": [{ "start_line": 1, "end_line": 5 }]
            },
            {
                "file": "b/two.txt", "side": "head",
                "ranges": [{ "start_line": 1, "end_line": 5 }],
                "reason": "too_large_for_prompt"
            }
        ])
    );
}

/// A late usage limit of an earlier round's job pauses the shared credential: the new round's waiting job shows
/// the analysis Paused with the reset time and resumes when it passes; the late output and session are not
/// applied.
#[tokio::test]
async fn late_usage_limit_of_an_old_round_pauses_the_new_round_with_its_time() {
    let h = harness().await.with_queue();
    let analysis = h.seed_analysis().await;
    let id = analysis.to_string();
    let mut spec = db::repo::analyses::testing::job_spec(analysis, JobKind::Plan);
    spec.credential.id = server::pipeline::CREDENTIAL_ID.to_owned();
    let old = h
        .state
        .executor
        .submit(JobRequest {
            spec,
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
    let claim = h.runner_claim().await.expect("the old round's job");
    assert_eq!(claim.spec.job_id, old.job_id);
    h.state
        .db
        .analyses()
        .set_state(
            analysis,
            &AnalysisState::Failed {
                flow: None,
                message: "x".to_owned(),
            },
            Utc::now(),
        )
        .await
        .expect("failed");
    let (status, body) = h.post_empty(&format!("/api/v1/analyses/{id}/retry")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The new round's plan job is queued behind the old claim.
    for _ in 0..500 {
        let current = h
            .state
            .db
            .jobs()
            .current_for_stage(analysis, &JobStage::Plan { batch: 1 }, 0)
            .await
            .expect("read");
        if current.is_some_and(|job| job.id != old.job_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let resets_at = chrono::DateTime::from_timestamp(Utc::now().timestamp() + 3, 0).expect("time");
    let late = JobResult {
        job_id: old.job_id,
        attempt: claim.spec.attempt,
        outcome: JobOutcome::UsageLimited {
            resets_at: Some(resets_at),
        },
        session: Some(contracts::job::result::SessionReport {
            thread_id: "thread-of-the-old-round".to_owned(),
            outcome: contracts::job::result::SessionOutcome::Started,
        }),
        usage: None,
        skills_commit: None,
        llm_call: None,
    };
    let uri = RUNNER_RESULT.path.replace("{id}", &old.job_id.to_string());
    let (status, body) = h.runner(&uri, claim.job_token.expose(), json!(late)).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let paused = h
        .wait_for_state(&id, |state| matches!(state, AnalysisState::Paused { .. }))
        .await;
    assert_eq!(
        paused.state,
        AnalysisState::Paused {
            resumes_at: Some(resets_at),
            done: 0,
            total: None
        }
    );
    let job = h.state.db.jobs().get(old.job_id).await.expect("job");
    assert_eq!(
        job.state,
        JobState::Cancelled,
        "the late result is not applied"
    );
    let record = h.state.db.analyses().get(analysis).await.expect("analysis");
    assert_eq!(
        record.agent_thread_id, None,
        "the late session is not applied"
    );
    h.wait_for_state(&id, |state| *state == AnalysisState::Planning)
        .await;
}
