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
use contracts::agent::JobKind;
use contracts::api::analyses::{AnalysisState, AnalysisSummary};
use contracts::api::events::AnalysisEvent;
use contracts::api::pr::PrSummary;
use contracts::ids::{AnalysisId, CommitSha, JobId};
use contracts::job::protocol::{Claim, ClaimResponse};
use contracts::job::result::{JobFailureKind, JobOutcome, JobResult};
use contracts::job::spec::{JobPriority, JobSpec};
use contracts::job::state::JobState;
use contracts::routes::{RUNNER_CLAIM, RUNNER_RESULT};
use contracts::time::Timestamp;
use db::repo::analyses::{NewAnalysis, SessionState};
use db::repo::documents::DocumentKind;
use db::repo::jobs::JobStage;
use db::repo::sessions::NewSession;
use db::repo::users::GithubProfile;
use db::repo::{DEFAULT_TENANT, Db};
use futures::future::BoxFuture;
use futures::stream::{self, BoxStream, StreamExt};
use http_body_util::BodyExt;
use jiff::SignedDuration;
use secrecy::SecretString;
use serde_json::{Value, json};
use server::api;
use server::auth::{lock_user, store_sign_in_tokens};
use server::config::Config;
use server::github::oauth::TokenGrant;
use server::jobs::executor::{ExecutorError, JobExecutor, JobHandle, JobRequest, JobUpdate};
use server::jobs::progress::ProgressHub;
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
    /// Fails with this typed cause.
    FailKind(JobFailureKind),
    Configuration,
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

/// A flow fixture as the tagged file a runner would return for a claimed flow job.
fn tagged_flow_output(spec: &contracts::job::spec::JobSpec, stage: &str) -> Value {
    let context: analysis::tagged::Context = serde_json::from_value(
        spec.flow_file
            .clone()
            .expect("flow job carries checker context")
            .context,
    )
    .expect("flow context");
    let document = serde_json::from_value(agent_output(stage)).expect("flow fixture");
    Value::String(analysis::tagged::encode(&document, &context.catalog()).expect("flow fixture"))
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
        self.analyses.lock().expect("lock").insert(
            job_id,
            request.spec.target.analysis_id().expect("analysis job"),
        );
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
        let mut outcome = match script {
            None => JobOutcome::Succeeded {
                output: agent_output(&stage),
            },
            Some(Script::Output(output)) => JobOutcome::Succeeded { output },
            Some(Script::Fail) => JobOutcome::Failed {
                kind: JobFailureKind::ProviderFailed,
                message: "the provider exited with an error".to_owned(),
            },
            Some(Script::Configuration) => JobOutcome::Failed {
                kind: JobFailureKind::Configuration,
                message: "private diagnostic: missing severity".to_owned(),
            },
            Some(Script::FailWith(message)) => JobOutcome::Failed {
                kind: JobFailureKind::ProviderFailed,
                message,
            },
            Some(Script::FailKind(kind)) => JobOutcome::Failed {
                kind,
                message: "private diagnostic of the runner".to_owned(),
            },
            Some(Script::Pending) => return stream::pending().boxed(),
            Some(Script::PausedUntil(notify)) => {
                paused = Some(notify);
                JobOutcome::Succeeded {
                    output: agent_output(&stage),
                }
            }
        };
        if stage.starts_with("flow:") {
            let spec = self
                .specs_of(&stage)
                .into_iter()
                .find(|(g, _)| *g == generation)
                .expect("flow fixture")
                .1;
            let context: analysis::tagged::Context =
                serde_json::from_value(spec.flow_file.expect("flow fixture").context)
                    .expect("flow fixture");
            if let JobOutcome::Succeeded { output } = &mut outcome
                && !output.is_string()
            {
                *output = match serde_json::from_value::<
                    contracts::agent::analyse_flow::FlowAnalysisOutput,
                >(output.clone())
                {
                    Ok(document) => Value::String(
                        analysis::tagged::encode(&document, &context.catalog())
                            .expect("flow fixture"),
                    ),
                    Err(_) => Value::String(output.to_string()), // deliberately malformed script
                };
            }
        }
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
        ("SR_ALLOWED_LOGINS", "alice".to_owned()),
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
    // Server warnings and errors go to the test's captured output: a failed test shows why the server failed.
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("warn")
        .try_init();
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

    let statuses = read_json(&fixtures().join("github/rest/commit-status.json"));
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/repos/acme/node-client/commits/{head}/status"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(&statuses["response"]))
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
    let now = Timestamp::now();
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
                expires_at: now + SignedDuration::from_hours(24),
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

    /// [`Harness::create_fixture_analysis_on_request`] with background analysis switched on at once: every flow
    /// is analysed without a request.
    async fn create_fixture_analysis(&self) -> String {
        let id = self.create_fixture_analysis_on_request().await;
        self.set_background(&id, true).await;
        id
    }

    /// Switches background analysis of `id` on or off.
    async fn set_background(&self, id: &str, enabled: bool) {
        let response = self
            .send(
                "PUT",
                &format!("/api/v1/analyses/{id}/background"),
                Auth::SessionCsrf,
                Some(json!({ "enabled": enabled })),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// Resolves the fixture PR and creates its analysis through the API, as a user does (flows on request);
    /// returns the analysis id.
    async fn create_fixture_analysis_on_request(&self) -> String {
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
        let state = AppState::with_queue(
            config(self._dir.path(), &self.github, None),
            self.state.db.clone(),
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

    /// Polls for the next claim; panics once the analysis has ended or nothing is claimable in time.
    async fn claim_next(&self, id: &str) -> Claim {
        for _ in 0..500 {
            if let Some(claim) = self.runner_claim().await {
                return claim;
            }
            let (_, body) = self.get(&format!("/api/v1/analyses/{id}")).await;
            assert!(
                !matches!(
                    body["state"]["state"].as_str(),
                    Some("failed" | "cancelled")
                ),
                "the analysis ended while a job was expected: {}",
                body["state"]
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no job became claimable in time");
    }

    /// The stage key and generation of a claimed job.
    async fn stage_of(&self, claim: &Claim) -> (String, u32) {
        let job = self
            .state
            .db
            .jobs()
            .get(claim.spec.job_id)
            .await
            .expect("job");
        (job.stage.key(), job.generation)
    }

    /// Posts the claimed attempt's result as the runner does.
    async fn complete(&self, claim: &Claim, outcome: JobOutcome) {
        let result = JobResult {
            job_id: claim.spec.job_id,
            attempt: claim.spec.attempt,
            outcome,
            session: None,
            usage: None,
            skills_commit: None,
            llm_call: None,
        };
        let uri = RUNNER_RESULT
            .path
            .replace("{id}", &claim.spec.job_id.to_string());
        let (status, body) = self
            .runner(&uri, claim.job_token.expose(), json!(result))
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }

    /// Runs plan and PR context through the runner routes with their fixture outputs: the review is open.
    async fn open_review(&self, id: &str) {
        for stage in ["plan:1", "pr_context"] {
            let claim = self.claim_next(id).await;
            assert_eq!(self.stage_of(&claim).await.0, stage);
            self.complete(
                &claim,
                JobOutcome::Succeeded {
                    output: agent_output(stage),
                },
            )
            .await;
        }
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

    /// Switches background analysis of a stored analysis on: its driver analyses every flow without a request.
    async fn background_on(&self, id: AnalysisId) {
        self.state
            .db
            .flow_work()
            .set_background(id, true, Timestamp::now())
            .await
            .expect("background on");
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
            .upsert(DEFAULT_TENANT, repo.id, &pr, Timestamp::now())
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
                Timestamp::now(),
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
            .upsert(DEFAULT_TENANT, repo.id, &pr, Timestamp::now())
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
                Timestamp::now(),
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
    assert_eq!(created["background_analysis"], json!(false));
    let id = created["id"].as_str().expect("id").to_owned();
    h.set_background(&id, true).await;

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
    tour["analysis"]["progress"]["started_at"] = seed["analysis"]["created_at"].clone();
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
    h.background_on(id).await;
    let documents = h.state.db.documents();
    let now = Timestamp::now();
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
async fn restart_keeps_demand_and_background_off_with_partial_results() {
    let h = harness().await;
    let id = h.seed_analysis().await;
    let now = Timestamp::now();
    for (kind, key, output) in [
        (DocumentKind::Plan, "", "plan:1"),
        (DocumentKind::PrContext, "", "pr_context"),
        (DocumentKind::Flow, "f1", "flow:f1"),
        (DocumentKind::Flow, "f3", "flow:f3"),
    ] {
        h.state
            .db
            .documents()
            .put(id, kind, key, &agent_output(output), now)
            .await
            .unwrap();
    }
    h.state
        .db
        .flow_work()
        .set_background(id, false, now)
        .await
        .unwrap();
    h.state
        .db
        .flow_work()
        .request(id, &contracts::ids::FlowId("f4".into()), now)
        .await
        .unwrap();
    assert_eq!(
        server::pipeline::resume_unfinished(&h.state).await.unwrap(),
        1
    );
    for _ in 0..200 {
        if h.state
            .db
            .documents()
            .get::<Value>(id, DocumentKind::Flow, "f4")
            .await
            .unwrap()
            .is_some()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (status, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(status, StatusCode::OK, "{tour}");
    assert_eq!(tour["analysis"]["background_analysis"], false);
    assert_eq!(
        tour["flows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["analysis_status"]["state"] == "analysed")
            .count(),
        3
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.executor.submitted(), ["flow:f4"]);
    let (status, body) = json_of(
        h.send(
            "PUT",
            &format!("/api/v1/analyses/{id}/background"),
            Auth::SessionCsrf,
            Some(json!({"enabled":true})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    h.wait_until_finished(&id.to_string()).await;
    assert_eq!(
        *h.executor.submitted.lock().unwrap(),
        ["flow:f4", "flow:f2", "flow:f5"]
    );
}

/// The analysis's one MemCan profile lookup reaches every prompt (fenced as data) and the Tour's facts.
#[tokio::test]
async fn memcan_profile_lookup_reaches_every_prompt_and_the_tour() {
    let h = harness().await;
    let id = h.seed_analysis().await;
    h.background_on(id).await;
    let profile = "MemCan-profile-CANARY: payments service; risk lives in src/ledger.";
    h.state
        .db
        .documents()
        .put(
            id,
            DocumentKind::MemcanProfile,
            "",
            &json!({"memcan": "checked", "text": profile}),
            Timestamp::now(),
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
        if seed["path"] == "src/config.rs" && seed["side"] == "head" {
            assert_eq!(
                lines["deleted"][0]["replacement"],
                json!({"start_line":23,"end_line":23})
            );
        }
        if seed["path"] == "src/rpc/client.rs" && seed["side"] == "head" {
            assert!(
                lines["context_only"]
                    .as_array()
                    .expect("context lines")
                    .contains(&json!(89))
            );
        }
        // `let http = …` re-indented into the new `loop` (base 101, head 95): ordinary unchanged code, not hidden.
        if seed["path"] == "src/rpc/client.rs" {
            let (line, side) = if seed["side"] == "head" {
                (95, "head")
            } else {
                (101, "base")
            };
            for field in ["changed", "context_only"] {
                assert!(
                    !lines[field].as_array().expect(field).contains(&json!(line)),
                    "{side} {line} in {field}"
                );
            }
            for block in lines["deleted"].as_array().expect("blocks") {
                let start = block["base_start"].as_u64().expect("start");
                let total = block["total_lines"].as_u64().expect("total");
                assert!(
                    !(start..start + total).contains(&101),
                    "base 101 shown as removed"
                );
            }
        }
        // The view hides exactly the lines the stored pre-pass covers by a line rule (one decision).
        let prepass: analysis::prepass::Prepass = h
            .state
            .db
            .documents()
            .get(id, DocumentKind::Prepass, "")
            .await
            .expect("read pre-pass")
            .expect("stored pre-pass");
        let side: contracts::common::LineSide =
            serde_json::from_value(seed["side"].clone()).expect("side");
        let path = seed["path"].as_str().expect("path");
        assert_eq!(
            lines["context_only"],
            json!(prepass.view_context(side, path)),
            "{uri}: route and pre-pass agree"
        );
        // The historical seed still checks every original field; new metadata is asserted above.
        lines.as_object_mut().expect("lines").remove("context_only");
        for block in lines["deleted"].as_array_mut().expect("blocks") {
            block.as_object_mut().expect("block").remove("context_only");
            block.as_object_mut().expect("block").remove("replacement");
        }
        assert_eq!(lines, seed, "{}", seed_path.display());
    }
}

#[tokio::test]
async fn a_pre_pass_stored_in_an_older_shape_is_computed_again() {
    let h = harness().await;
    let id = h.seed_analysis().await;
    let uri = format!("/api/v1/analyses/{id}/lines?path=src/rpc/client.rs&side=head&from=1&to=224");
    let (status, fresh) = h.get(&uri).await;
    assert_eq!(status, StatusCode::OK, "{fresh}");
    let documents = h.state.db.documents();
    let mut old: Value = documents
        .get(id, DocumentKind::Prepass, "")
        .await
        .expect("read")
        .expect("stored");
    // The shape of 1525f6a: no quiet moves or whitespace pairs, identical moves as a hunk disposition.
    let object = old.as_object_mut().expect("object");
    object.remove("quiet_moves");
    object.remove("whitespace");
    old["hunks"][0]["disposition"] = json!({"kind": "identical_move"});
    documents
        .put(id, DocumentKind::Prepass, "", &old, Timestamp::now())
        .await
        .expect("store the old shape");
    let (status, again) = h.get(&uri).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again, fresh, "computed again from the stored diff");
    let stored: Option<analysis::prepass::Prepass> = documents
        .get(id, DocumentKind::Prepass, "")
        .await
        .expect("the new shape is stored");
    assert!(stored.is_some());
}

/// The fixture plan as an earlier build wrote it: h19/h20 (an in-file move, quiet then) in no flow and not in
/// `unassigned`.
fn plan_of_an_earlier_build() -> Value {
    let mut plan = agent_output("plan:1");
    plan["unassigned"]
        .as_array_mut()
        .expect("unassigned")
        .retain(|hunk| !["h19", "h20"].contains(&hunk["hunk_id"].as_str().unwrap_or_default()));
    plan
}

/// Whether `tour`'s Unexplained lists `start..=end` of src/rpc/client.rs on `side`.
fn unexplained_has(tour: &Value, side: &str, start: u64, end: u64) -> bool {
    tour["quiet"]["unexplained"]
        .as_array()
        .expect("unexplained")
        .iter()
        .any(|item| {
            item["file"] == "src/rpc/client.rs"
                && item["side"] == side
                && item["ranges"].as_array().expect("ranges").iter().any(|r| {
                    r["start_line"].as_u64() <= Some(start) && r["end_line"].as_u64() >= Some(end)
                })
        })
}

/// The moved code of h19/h20 without its context-only doc comments (head 112, 145; base 173, 206).
const H19_H20: [(&str, u64, u64); 2] = [("head", 113, 144), ("base", 174, 205)];

#[tokio::test]
async fn stored_plan_and_flow_missing_newly_mandatory_lines_leave_them_unexplained() {
    let h = harness().await;
    let id = h.seed_analysis().await;
    h.background_on(id).await;
    server::pipeline::resume_unfinished(&h.state)
        .await
        .expect("resume");
    h.wait_until_finished(&id.to_string()).await;
    let documents = h.state.db.documents();
    documents
        .put(
            id,
            DocumentKind::Plan,
            "",
            &plan_of_an_earlier_build(),
            Timestamp::now(),
        )
        .await
        .expect("store the old plan");
    // Flow f1 as if its step s9 (head 30-43: changed lines 35, 42 and removed base 41) had not been required.
    let mut flow: Value = agent_output("flow:f1");
    flow["steps"]
        .as_array_mut()
        .expect("steps")
        .retain(|step| step["id"] != "s9");
    flow["edges"]
        .as_array_mut()
        .expect("edges")
        .retain(|edge| edge["from_step_id"] != "s9" && edge["to_step_id"] != "s9");
    documents
        .put(id, DocumentKind::Flow, "f1", &flow, Timestamp::now())
        .await
        .expect("store the narrower flow");
    let (status, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(status, StatusCode::OK, "{tour}");
    let flows = tour["flows"].as_array().expect("flows");
    assert_eq!(flows.len(), 5);
    for flow in flows {
        assert_eq!(
            flow["analysis_status"]["state"], "analysed",
            "{}",
            flow["id"]
        );
        assert!(
            !flow["steps"].as_array().expect("steps").is_empty(),
            "{}",
            flow["id"]
        );
    }
    let f1 = flows.iter().find(|flow| flow["id"] == "f1").expect("f1");
    assert_eq!(
        f1["steps"].as_array().map(Vec::len),
        Some(9),
        "f1 keeps its other steps"
    );
    for (side, start, end) in
        H19_H20
            .into_iter()
            .chain([("head", 35, 35), ("head", 42, 42), ("base", 41, 41)])
    {
        assert!(
            unexplained_has(&tour, side, start, end),
            "{side} {start}-{end} in Unexplained: {}",
            tour["quiet"]["unexplained"]
        );
    }
}

#[tokio::test]
async fn resume_reads_a_stored_plan_as_stored() {
    let h = harness().await;
    let id = h.seed_analysis().await;
    h.background_on(id).await;
    let documents = h.state.db.documents();
    documents
        .put(
            id,
            DocumentKind::Plan,
            "",
            &plan_of_an_earlier_build(),
            Timestamp::now(),
        )
        .await
        .expect("old plan");
    documents
        .put(
            id,
            DocumentKind::PrContext,
            "",
            &agent_output("pr_context"),
            Timestamp::now(),
        )
        .await
        .expect("pr context");
    server::pipeline::resume_unfinished(&h.state)
        .await
        .expect("resume");
    let finished = h.wait_until_finished(&id.to_string()).await;
    assert_eq!(finished.state, AnalysisState::Finished { total: 5 });
    let (status, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(status, StatusCode::OK, "{tour}");
    for (side, start, end) in H19_H20 {
        assert!(
            unexplained_has(&tour, side, start, end),
            "{side} {start}-{end}"
        );
    }
}

#[tokio::test]
async fn retry_reads_a_stored_plan_as_stored() {
    let h = harness().await;
    h.executor.script("pr_context", Script::Fail);
    let id = h.create_fixture_analysis().await;
    let ended = h.wait_until_ended(&id).await;
    assert!(
        matches!(ended.state, AnalysisState::Failed { .. }),
        "{ended:?}"
    );
    let analysis: AnalysisId = serde_json::from_value(json!(id)).expect("id");
    h.state
        .db
        .documents()
        .put(
            analysis,
            DocumentKind::Plan,
            "",
            &plan_of_an_earlier_build(),
            Timestamp::now(),
        )
        .await
        .expect("old plan");
    h.executor.unscript("pr_context");
    let (status, body) = h.post_empty(&format!("/api/v1/analyses/{id}/retry")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let finished = h.wait_until_finished(&id).await;
    assert_eq!(finished.state, AnalysisState::Finished { total: 5 });
}

#[tokio::test]
async fn a_stored_flow_failing_another_check_is_failed_and_can_be_analysed_again() {
    let h = harness().await;
    let id = h.seed_analysis().await;
    h.background_on(id).await;
    server::pipeline::resume_unfinished(&h.state)
        .await
        .expect("resume");
    h.wait_until_finished(&id.to_string()).await;
    let mut flow: Value = agent_output("flow:f1");
    flow["edges"]
        .as_array_mut()
        .expect("edges")
        .push(json!({"from_step_id": "s1", "to_step_id": "s99", "label": null}));
    h.state
        .db
        .documents()
        .put(id, DocumentKind::Flow, "f1", &flow, Timestamp::now())
        .await
        .expect("store a flow that fails a check");
    let (status, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(status, StatusCode::OK, "{tour}");
    let f1 = tour["flows"]
        .as_array()
        .expect("flows")
        .iter()
        .find(|flow| flow["id"] == "f1")
        .expect("f1");
    assert_eq!(f1["analysis_status"]["state"], "failed", "{f1}");
    let message = f1["analysis_status"]["message"].as_str().expect("message");
    assert!(
        !message.contains('_'),
        "no problem code in user text: {message}"
    );
    assert!(message.ends_with("Analyse this flow again."), "{message}");
    assert!(f1["steps"].as_array().expect("steps").is_empty());
    let planned = f1["planned"]["ranges"].as_array().expect("planned ranges");
    assert!(
        planned
            .iter()
            .any(|r| r["file"] == "src/rpc/client.rs" && r["side"] == "head"),
        "its lines are listed: {planned:?}"
    );

    // An accepted stored flow of the finished analysis: the request changes nothing.
    let (status, body) = h
        .post(
            &format!("/api/v1/analyses/{id}/flows/f2/request"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"]["state"], "finished", "{body}");
    // The rejected one is discarded and analysed again: the finished analysis runs again, then finishes.
    let (status, body) = h
        .post(
            &format!("/api/v1/analyses/{id}/flows/f1/request"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_ne!(body["state"]["state"], "finished", "{body}");
    let finished = h.wait_until_finished(&id.to_string()).await;
    assert_eq!(finished.state, AnalysisState::Finished { total: 5 });
    let runs = h.executor.specs_of("flow:f1");
    assert_eq!(
        runs.iter()
            .map(|(generation, _)| *generation)
            .collect::<Vec<_>>(),
        [0, 0],
        "a new first run, never a repair of the rejected output"
    );
    let (status, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(status, StatusCode::OK, "{tour}");
    let f1 = tour["flows"]
        .as_array()
        .expect("flows")
        .iter()
        .find(|flow| flow["id"] == "f1")
        .expect("f1");
    assert_eq!(f1["analysis_status"]["state"], "analysed", "{f1}");
    assert!(!f1["steps"].as_array().expect("steps").is_empty());
}

/// Cancel of a finished analysis that runs again for one flow ends that run only: the analysis is finished
/// again with its results, and the flow says how to analyse it again.
#[tokio::test]
async fn cancelling_the_rerun_of_a_finished_analysis_keeps_its_results() {
    let h = harness().await;
    let id = h.seed_analysis().await;
    h.background_on(id).await;
    server::pipeline::resume_unfinished(&h.state)
        .await
        .expect("resume");
    let first = h.wait_until_finished(&id.to_string()).await;
    let mut flow: Value = agent_output("flow:f1");
    flow["edges"]
        .as_array_mut()
        .expect("edges")
        .push(json!({"from_step_id": "s1", "to_step_id": "s99", "label": null}));
    h.state
        .db
        .documents()
        .put(id, DocumentKind::Flow, "f1", &flow, Timestamp::now())
        .await
        .expect("store a flow that fails a check");
    h.executor.script("flow:f1", Script::Pending);
    let (status, body) = h
        .post(
            &format!("/api/v1/analyses/{id}/flows/f1/request"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["finished_at"], Value::Null, "{body}");
    let (status, body) = h.post_empty(&format!("/api/v1/analyses/{id}/cancel")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let summary: AnalysisSummary = serde_json::from_value(body).expect("summary");
    assert_eq!(summary.state, AnalysisState::Finished { total: 5 });
    assert_eq!(summary.finished_at, first.finished_at);
    let flows = h
        .state
        .db
        .documents()
        .list::<Value>(id, DocumentKind::Flow)
        .await
        .expect("flows");
    assert_eq!(flows.len(), 4, "the other results stay");
    let (status, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(status, StatusCode::OK, "{tour}");
    let f1 = tour["flows"]
        .as_array()
        .expect("flows")
        .iter()
        .find(|flow| flow["id"] == "f1")
        .expect("f1");
    assert_eq!(f1["analysis_status"]["state"], "failed", "{f1}");
    assert!(
        f1["analysis_status"]["message"]
            .as_str()
            .is_some_and(|m| m.ends_with("Analyse this flow again.")),
        "{f1}"
    );
}

/// "Analyse all remaining flows" also runs again a stored flow that today's checks reject.
#[tokio::test]
async fn analysing_all_remaining_flows_reruns_a_rejected_stored_flow() {
    let h = harness().await;
    let id = h.create_fixture_analysis_on_request().await;
    h.wait_for_state(&id, |state| {
        matches!(state, AnalysisState::Running { idle: true, .. })
    })
    .await;
    let (status, body) = h
        .post(
            &format!("/api/v1/analyses/{id}/flows/f1/request"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    h.wait_for_state(&id, |state| {
        matches!(
            state,
            AnalysisState::Running {
                done: 1,
                idle: true,
                ..
            }
        )
    })
    .await;
    let analysis: AnalysisId = serde_json::from_value(json!(id)).expect("id");
    let mut flow: Value = agent_output("flow:f1");
    flow["edges"]
        .as_array_mut()
        .expect("edges")
        .push(json!({"from_step_id": "s1", "to_step_id": "s99", "label": null}));
    h.state
        .db
        .documents()
        .put(analysis, DocumentKind::Flow, "f1", &flow, Timestamp::now())
        .await
        .expect("store a flow that fails a check");
    h.set_background(&id, true).await;
    let finished = h.wait_until_finished(&id).await;
    assert_eq!(finished.state, AnalysisState::Finished { total: 5 });
    let runs = h.executor.specs_of("flow:f1");
    assert_eq!(
        runs.iter()
            .map(|(generation, _)| *generation)
            .collect::<Vec<_>>(),
        [0, 0],
        "a new first run, never a repair of the rejected output"
    );
    let (_, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    let f1 = tour["flows"]
        .as_array()
        .expect("flows")
        .iter()
        .find(|flow| flow["id"] == "f1")
        .expect("f1");
    assert_eq!(f1["analysis_status"]["state"], "analysed", "{f1}");
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
    // Line 95 only moved into the new `loop` (whitespace only): unchanged code.
    assert_eq!(body["changed"], json!([96, 97, 98, 99]));
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
        ("POST", format!("/api/v1/analyses/{id}/pause")),
        ("POST", format!("/api/v1/analyses/{id}/resume")),
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
        format!("/api/v1/analyses/{id}/pause"),
        format!("/api/v1/analyses/{id}/resume"),
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
    let now = Timestamp::now();
    h.state
        .db
        .sessions()
        .create(
            &NewSession {
                token_hash: &sha256(b"other-tenant-session"),
                tenant_id: "other",
                user_id: &h.user_id,
                csrf_token: CSRF,
                expires_at: now + SignedDuration::from_hours(24),
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
        ("POST", format!("/api/v1/analyses/{id}/pause")),
        ("POST", format!("/api/v1/analyses/{id}/resume")),
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
    assert!(
        matches!(summary.state, AnalysisState::Planning { .. }),
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
        json!({ "after_line": 3, "base_start": 5004, "lines": [], "total_lines": 1, "context_only": [] })
    );
}

/// Waits until `stage` was submitted `runs` times and the driver of analysis `id` ended (nothing can follow),
/// then checks that no further run was submitted.
async fn settled_runs(h: &Harness, id: &str, stage: &str, runs: usize) {
    for _ in 0..500 {
        if h.executor.specs_of(stage).len() >= runs {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    wait_parked(analysis_id(id)).await;
    assert_eq!(h.executor.specs_of(stage).len(), runs, "runs of {stage}");
}

/// Waits until no driver task works on the analysis (nothing is admitted).
async fn wait_parked(analysis: AnalysisId) {
    for _ in 0..250 {
        if !server::pipeline::is_driven(analysis) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the driver did not park");
}

fn analysis_id(id: &str) -> AnalysisId {
    serde_json::from_value(json!(id)).expect("analysis id")
}

async fn wait_for_failed_flow(h: &Harness, id: &str, flow: &str) -> Value {
    for _ in 0..300 {
        let (status, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
        if status == StatusCode::OK
            && tour["flows"].as_array().is_some_and(|flows| {
                flows
                    .iter()
                    .any(|f| f["id"] == flow && f["analysis_status"]["state"] == "failed")
            })
        {
            return tour;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("flow {flow} did not fail in time");
}

#[tokio::test]
async fn failed_job_keeps_partial_review_and_can_be_requested_again() {
    let h = harness().await;
    h.executor.script("flow:f2", Script::Fail);
    let id = h.create_fixture_analysis().await;
    let tour = wait_for_failed_flow(&h, &id, "f2").await;
    assert_eq!(tour["analysis"]["review_ready"], true);
    assert_eq!(tour["analysis"]["state"]["state"], "running");
    h.executor
        .script("flow:f2", Script::Output(agent_output("flow:f2")));
    let (status, body) = h
        .post(
            &format!("/api/v1/analyses/{id}/flows/f2/request"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        h.wait_until_finished(&id).await.state,
        AnalysisState::Finished { total: 5 }
    );
}

/// A failed flow's ranges stay explicitly unassessed in its planned flow.
#[tokio::test]
async fn schema_invalid_flow_exhausts_repairs_and_stays_explicitly_failed() {
    let h = harness().await;
    h.executor
        .script("flow:f2", Script::Output(json!({ "flow_id": "f2" })));
    let id = h.create_fixture_analysis().await;
    // Three runs (two repairs) per turn; the first turn and two automatic retries.
    settled_runs(&h, &id, "flow:f2", 9).await;
    let tour = wait_for_failed_flow(&h, &id, "f2").await;
    let flow = tour["flows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "f2")
        .unwrap();
    assert!(flow["steps"].as_array().unwrap().is_empty());
    assert!(!flow["planned"]["ranges"].as_array().unwrap().is_empty());
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
}

#[tokio::test]
async fn restart_with_failing_preparation_cancels_live_jobs() {
    let h = harness().await;
    // The head commit is not on the remote: preparing the analysis (fetch) fails at restart.
    let id = h.seed_analysis_at(Some("f".repeat(40))).await;
    let progress = ProgressHub::default();
    let queue: Arc<QueueExecutor> =
        Arc::new(QueueExecutor::new(h.state.db.clone(), progress.clone()));
    let state = AppState {
        progress,
        ..AppState::new(
            config(h._dir.path(), &h.github, None),
            h.state.db.clone(),
            queue.clone(),
        )
    };
    let pending = queue
        .submit(JobRequest {
            spec: db::repo::analyses::testing::job_spec(id, JobKind::Plan),
            priority: JobPriority::Batch,
            stage: JobStage::Plan { batch: 1 },
            generation: 0,
            pr_repo: Some(contracts::common::RepoRef {
                owner: "acme".to_owned(),
                name: "node-client".to_owned(),
            }),
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
            reason: None,
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
    assert_eq!(facts["ci"]["total"], json!(runs + 1));
    assert_eq!(facts["ci"]["statuses"]["pending"], 1);
    assert_eq!(facts["ci"]["check_runs"]["skipped"], 1);
    assert_eq!(facts["ci"]["check_runs"]["neutral"], 1);
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
async fn missing_step_assessments_trigger_repair_and_restore_flow_risk() {
    for absent in [false, true] {
        let h = harness().await;
        let mut output = agent_output("flow:f2");
        if absent {
            output["steps"][0]
                .as_object_mut()
                .unwrap()
                .remove("assessment");
        } else {
            output["steps"][0]["assessment"] = Value::Null;
        }
        h.executor.script("flow:f2#0", Script::Output(output));
        let id = h.create_fixture_analysis().await;
        let finished = h.wait_until_finished(&id).await;
        assert_eq!(finished.state, AnalysisState::Finished { total: 5 });
        assert!(h.executor.submitted().contains(&"flow:f2#1".to_owned()));
        let (_, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
        for flow in tour["flows"].as_array().unwrap() {
            assert!(flow["llm"]["risk"].is_string());
            assert!(flow["steps"].as_array().unwrap().iter().all(|step| {
                let assessment = &step["llm"]["assessment"];
                assessment["risk"].is_string()
                    && assessment["reason"].is_string()
                    && (assessment["risk"] == "low" || assessment["basis_note"].is_string())
            }));
        }
    }
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
    let restored = runs[1]
        .1
        .flow_file
        .as_ref()
        .unwrap()
        .previous
        .as_ref()
        .unwrap();
    assert!(restored.starts_with("# flow f2"));
    assert!(restored.contains("9990-9999"));
    assert!(runs[1].1.prompt.contains("editing that file"));

    assert!(
        runs[1].1.prompt.contains("repair 1 of 2"),
        "{}",
        runs[1].1.prompt
    );
    let (_, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(tour["flows"].as_array().expect("flows").len(), 5);
}

#[tokio::test]
async fn exhausted_flow_repairs_leave_a_retryable_flow_in_partial_review() {
    let h = harness().await;
    h.executor
        .script("flow:f2", Script::Output(rejected_flow("flow:f2")));
    let id = h.create_fixture_analysis().await;
    let tour = wait_for_failed_flow(&h, &id, "f2").await;
    let submitted = h.executor.submitted();
    for run in ["flow:f2", "flow:f2#1", "flow:f2#2"] {
        assert!(submitted.contains(&run.to_owned()), "{submitted:?}");
    }
    assert!(!submitted.contains(&"flow:f2#3".to_owned()));
    assert_eq!(tour["flows"].as_array().unwrap().len(), 5);
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
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
            reason: None,
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
            reason: None,
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
    // The MemCan profile was not checked (unreachable here): the Retry looks it up again.
    let analysis: AnalysisId = serde_json::from_value(json!(id)).expect("id");
    let stale = json!({"memcan": "not_checked", "round": "before the retry"});
    h.state
        .db
        .documents()
        .put(
            analysis,
            DocumentKind::MemcanProfile,
            "",
            &stale,
            Timestamp::now(),
        )
        .await
        .expect("lookup");
    // Retry runs it again; the plan job failed, so it is submitted again.
    h.executor.unscript("plan:1");
    let (status, body) = h.post_empty(&format!("/api/v1/analyses/{id}/retry")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], json!({ "state": "planning" }));
    let finished = h.wait_until_finished(&id).await;
    assert_eq!(finished.state, AnalysisState::Finished { total: 5 });
    let lookup: Option<Value> = h
        .state
        .db
        .documents()
        .get(analysis, DocumentKind::MemcanProfile, "")
        .await
        .expect("lookup");
    assert_eq!(
        lookup,
        Some(json!({"memcan": "not_checked"})),
        "looked up again"
    );
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
    assert_eq!(record.session_state, SessionState::Active);
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
        assert_eq!(
            spec.skills,
            ["severity"],
            "{stage}: prompts require severity"
        );
        assert_eq!(spec.session, None, "{stage}: the session is set at claim");
    }
}

#[tokio::test]
async fn reanalysis_keeps_the_pull_request_session() {
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
    assert_eq!(record.session_state, SessionState::Active);
    let record = h.state.db.analyses().get(analysis).await.expect("new");
    assert_eq!(
        record.agent_thread_id.as_deref(),
        Some("thread-old"),
        "the new analysis shares the PR session"
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
    assert!(
        facts["related_work_links"].is_array(),
        "structured links must accompany related work"
    );
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

/// Creates the analysis of a pull request over the plan batch limit (two directory batches): each batch's plan,
/// one flow over all its hunks, is scripted by `plan_script(batch, plan)`; both flows' outputs are rejected.
async fn huge_analysis(h: &Harness, mut plan_script: impl FnMut(usize, Value) -> Script) -> String {
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
            .script(&format!("plan:{}", index + 1), plan_script(index + 1, plan));
    }
    let mut context = agent_output("pr_context");
    context["fit_questions"] = json!([]);
    context["related_work"] = json!([]);
    h.executor.script("pr_context", Script::Output(context));
    for flow in ["f1", "f2"] {
        // Rejected (the fixture flow's files are not in this repository): the flows remain explicitly failed.
        let mut output = agent_output("flow:f1");
        output["flow_id"] = json!(flow);
        h.executor
            .script(&format!("flow:{flow}"), Script::Output(output));
    }
    let mut pull = read_json(&fixtures().join("github/rest/pull.json"))["response"].clone();
    pull["additions"] = json!(26081);
    pull["deletions"] = json!(1000);
    pull["changed_files"] = json!(371);
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
    let (status, resolved) = h
        .post(
            "/api/v1/pr/resolve",
            json!({"url": "https://github.com/acme/huge/pull/11"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        resolved["size_warning"],
        json!({"changed_lines": 27081, "changed_files": 371, "estimate": null})
    );
    assert_eq!(
        resolved["size"],
        json!({"changed_lines": 27081, "changed_files": 371})
    );
    let (status, created) = h
        .post(
            "/api/v1/analyses",
            json!({ "pr": { "repo": { "owner": "acme", "name": "huge" }, "number": 11 }, "head_sha": head }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().expect("id").to_owned()
}

/// A pull request over the plan batch limit is planned in directory batches whose plans are merged.
#[tokio::test]
async fn huge_pull_request_is_planned_in_batches() {
    let h = harness().await;
    let id = huge_analysis(&h, |_, plan| Script::Output(plan)).await;
    h.set_background(&id, true).await;
    // Each flow fails for good after its first turn and two automatic retries (three runs each).
    settled_runs(&h, &id, "flow:f1", 9).await;
    settled_runs(&h, &id, "flow:f2", 9).await;
    wait_for_failed_flow(&h, &id, "f1").await;
    wait_for_failed_flow(&h, &id, "f2").await;
    let (_, summary) = h.get(&format!("/api/v1/analyses/{id}")).await;
    let summary: AnalysisSummary = serde_json::from_value(summary).unwrap();
    assert!(matches!(
        summary.state,
        AnalysisState::Running {
            done: 0,
            total: 2,
            ..
        }
    ));
    let progress = summary.progress.as_ref().expect("whole-run progress");
    assert_eq!(progress.planning.done, 2);
    assert_eq!(progress.planning.total, Some(2));
    assert_ne!(progress.percent, Some(100));
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
        .unwrap()
        .iter()
        .map(|rows| rows["file"].as_str().unwrap())
        .collect();
    assert_eq!(
        unexplained,
        ["a/big.txt", "b/big.txt"],
        "the lines of failed flows stay listed as not explained"
    );
    let pending: Vec<&str> = tour["flows"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|f| {
            assert_eq!(f["analysis_status"]["state"], "failed");
            f["planned"]["ranges"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["file"].as_str().unwrap())
        })
        .collect();
    assert_eq!(pending, ["a/big.txt", "b/big.txt"]);
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
}

/// An analysis planning in batches when the server was upgraded: Retry merges the stored batch plans, whose
/// lines today's rules may find missing; they are read as stored (those lines Unexplained), never failed for good.
#[tokio::test]
async fn retry_reads_stored_batch_plans_as_stored() {
    let h = harness().await;
    let mut planned = Vec::new();
    let id = huge_analysis(&h, |batch, plan| {
        planned.push(plan.clone());
        if batch == 2 {
            Script::Fail
        } else {
            Script::Output(plan)
        }
    })
    .await;
    let ended = h.wait_until_ended(&id).await;
    assert!(
        matches!(ended.state, AnalysisState::Failed { .. }),
        "{ended:?}"
    );
    // Batch 2's plan of an earlier build: it leaves lines 2001-2500 of b/big.txt in no flow and not unassigned.
    let mut earlier = planned[1].clone();
    let selection = &mut earlier["flows"][0]["hunks"][0];
    selection["head_start"] = json!(1);
    selection["head_end"] = json!(2000);
    let analysis: AnalysisId = serde_json::from_value(json!(id)).expect("id");
    h.state
        .db
        .documents()
        .put(
            analysis,
            DocumentKind::Plan,
            "2",
            &earlier,
            Timestamp::now(),
        )
        .await
        .expect("stored batch plan");
    h.executor.unscript("plan:2");
    let (status, body) = h.post_empty(&format!("/api/v1/analyses/{id}/retry")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let summary = h
        .wait_for_state(&id, |state| {
            matches!(
                state,
                AnalysisState::Failed { .. } | AnalysisState::Running { idle: true, .. }
            )
        })
        .await;
    assert!(
        matches!(summary.state, AnalysisState::Running { .. }),
        "{:?}",
        summary.state
    );
    assert_eq!(h.executor.specs_of("plan:2").len(), 1, "not planned again");
    let (status, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(status, StatusCode::OK, "{tour}");
    let unexplained = tour["quiet"]["unexplained"]
        .as_array()
        .expect("unexplained");
    assert!(
        unexplained.iter().any(|rows| rows["file"] == "b/big.txt"
            && rows["ranges"]
                .as_array()
                .expect("ranges")
                .iter()
                .any(|r| r["start_line"] == 2001 && r["end_line"] == 2500)),
        "{unexplained:?}"
    );
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
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
                output: if let Some(file) = &claim.spec.flow_file {
                    let context: analysis::tagged::Context =
                        serde_json::from_value(file.context.clone()).unwrap();
                    let doc = serde_json::from_value(agent_output(&stage)).unwrap();
                    json!(analysis::tagged::encode(&doc, &context.catalog()).unwrap())
                } else {
                    agent_output(&stage)
                },
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
async fn flow_too_large_for_a_prompt_stays_failed_with_its_reason() {
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
    h.set_background(&id, true).await;
    let tour = wait_for_failed_flow(&h, &id, "f2").await;
    assert!(!h.executor.submitted().contains(&"flow:f2".to_owned()));
    let flow = tour["flows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "f2")
        .unwrap();
    assert!(
        flow["analysis_status"]["message"]
            .as_str()
            .unwrap()
            .contains("too large")
    );
    assert_eq!(flow["planned"]["ranges"][0]["file"], "b/two.txt");
    // Both flows failed for good: their lines stay listed as not explained, the unsent ones with the reason.
    settled_runs(&h, &id, "flow:f1", 9).await;
    let tour = wait_for_failed_flow(&h, &id, "f1").await;
    let lines = json!([{ "start_line": 1, "end_line": 5 }]);
    assert_eq!(
        tour["quiet"]["unexplained"],
        json!([
            { "file": "a/one.txt", "side": "head", "ranges": lines },
            { "file": "b/two.txt", "side": "head", "ranges": lines, "reason": "too_large_for_prompt" }
        ])
    );
    // The flow that was sent used its automatic retries (a job round each); the unsent one used neither.
    for (flow, expected) in [("f1", (2, 2)), ("f2", (2, 0))] {
        let flow = contracts::ids::FlowId(flow.to_owned());
        let retries = db::repo::flow_work::testing::retries(&h.state.db, analysis_id(&id), &flow)
            .await
            .expect("retries");
        assert_eq!(retries, expected, "{flow:?}");
    }
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
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
            pr_repo: Some(contracts::common::RepoRef {
                owner: "acme".to_owned(),
                name: "node-client".to_owned(),
            }),
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
                reason: None,
                flow: None,
                message: "x".to_owned(),
            },
            Timestamp::now(),
        )
        .await
        .expect("failed");
    let (status, body) = h.post_empty(&format!("/api/v1/analyses/{id}/retry")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The new round's plan job is queued behind the old claim.
    let mut queued = false;
    for _ in 0..500 {
        let current = h
            .state
            .db
            .jobs()
            .current_for_stage(analysis, &JobStage::Plan { batch: 1 }, 0)
            .await
            .expect("read");
        if current.is_some_and(|job| job.id != old.job_id) {
            queued = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(queued, "the new round's plan job was not queued within 5 s");
    let resets_at = Timestamp::from_second(Timestamp::now().as_second() + 3).expect("time");
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
    assert!(
        matches!(
            paused.state,
            AnalysisState::Paused {
                resumes_at: Some(at),
                done: 0,
                total: None,
                ..
            } if at == resets_at
        ),
        "{:?}",
        paused.state
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
    h.wait_for_state(&id, |state| matches!(state, AnalysisState::Planning { .. }))
        .await;
}

#[tokio::test]
async fn configuration_failure_survives_storage_and_has_no_retry_message() {
    let h = harness().await;
    h.executor.script("plan:1", Script::Configuration);
    let id = h.create_fixture_analysis().await;
    h.wait_until_ended(&id).await;
    let (status, body) = h.get(&format!("/api/v1/analyses/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["state"]["reason"], "configuration");
    assert_eq!(
        body["state"]["message"],
        "The server is misconfigured: a required agent skill is missing. Ask the administrator."
    );
}

#[tokio::test]
async fn oversized_mechanical_file_is_not_renderable() {
    let h = harness().await;
    let summary = h
        .analyse_repo("huge-mechanical", 11, |repo| {
            let base = commit(repo, &[("Cargo.lock", "version = 3\n")], "base");
            let huge = "# padding\n".repeat(1024 * 1024);
            let head = commit(repo, &[("Cargo.lock", &huge)], "large lockfile");
            (base, head)
        })
        .await;
    let (mirror, files) = server::pipeline::pr_diff(&h.state, &summary).await.unwrap();
    let prepass = server::pipeline::prepass::ensure_for_read(&h.state, &summary, &mirror, &files)
        .await
        .unwrap();
    let items = prepass.not_analysed_with_binaries();
    let head = items
        .iter()
        .flat_map(|section| &section.items)
        .find(|item| item.file == "Cargo.lock" && item.side == contracts::common::LineSide::Head)
        .unwrap();
    assert_eq!(head.renderable, Some(false));
}

#[tokio::test]
async fn user_pause_keeps_the_running_result_and_next_job_uses_the_new_model() {
    let h = harness().await.with_queue();
    let id = h.create_fixture_analysis().await;
    let mut first = None;
    for _ in 0..200 {
        first = h.runner_claim().await;
        if first.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let first = first.unwrap();
    assert_eq!(first.spec.model.model, "gpt-6.1-sol");
    let pause = format!("/api/v1/analyses/{id}/pause");
    let (status, _) = json_of(h.send("POST", &pause, Auth::Session, None).await).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, paused) = h.post(&pause, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    assert_eq!(paused["state"]["reason"], "user");
    assert!(paused["state"]["paused_by"].is_string());
    let result = JobResult {
        job_id: first.spec.job_id,
        attempt: first.spec.attempt,
        outcome: JobOutcome::Succeeded {
            output: agent_output("plan:1"),
        },
        session: None,
        usage: None,
        skills_commit: None,
        llm_call: None,
    };
    let uri = RUNNER_RESULT
        .path
        .replace("{id}", &first.spec.job_id.to_string());
    assert_eq!(
        h.runner(&uri, first.job_token.expose(), json!(result))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    let analysis: AnalysisId = serde_json::from_value(json!(id)).unwrap();
    assert!(
        h.state
            .db
            .documents()
            .get::<Value>(analysis, DocumentKind::Plan, "")
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        h.state
            .db
            .jobs()
            .list_for_analysis(analysis, &[])
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(h.runner_claim().await.is_none());
    assert_eq!(
        h.get(&format!("/api/v1/analyses/{id}/tour")).await.0,
        StatusCode::CONFLICT
    );
    h.state
        .db
        .settings()
        .set_llm_selection(
            &contracts::job::spec::ModelChoice {
                model: "gpt-6-astra".into(),
                reasoning_effort: contracts::job::spec::ReasoningEffort::Low,
            },
            "alice",
            Timestamp::now(),
        )
        .await
        .unwrap();
    let resume = format!("/api/v1/analyses/{id}/resume");
    assert_eq!(
        json_of(h.send("POST", &resume, Auth::Session, None).await)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let mut moved = read_json(&fixtures().join("github/rest/pull.json"))["response"].clone();
    moved["head"]["sha"] = json!("f".repeat(40));
    Mock::given(method("GET"))
        .and(path("/api/repos/acme/node-client/pulls/482"))
        .respond_with(ResponseTemplate::new(200).set_body_json(moved))
        .with_priority(1)
        .up_to_n_times(1)
        .mount(&h.github)
        .await;
    let (status, refusal) = h.post(&resume, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refusal}");
    assert_eq!(refusal["code"], "conflict");
    assert_eq!(
        refusal["message"],
        "The pull request changed meanwhile. Open it again."
    );
    assert!(h.runner_claim().await.is_none());
    let (status, resumed) = h.post(&resume, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    let mut next = None;
    for _ in 0..200 {
        next = h.runner_claim().await;
        if next.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let next = next.unwrap();
    assert_eq!(next.spec.kind, JobKind::PrContext);
    assert_eq!(next.spec.model.model, "gpt-6-astra");
    assert_eq!(
        next.spec.model.reasoning_effort,
        contracts::job::spec::ReasoningEffort::Low
    );
    let (_, summary) = h.get(&format!("/api/v1/analyses/{id}")).await;
    assert_eq!(
        summary["models_used"],
        json!([
            {"model":"gpt-6.1-sol","reasoning_effort":"high"},
            {"model":"gpt-6-astra","reasoning_effort":"low"}
        ])
    );
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
}

#[tokio::test]
async fn restart_leaves_user_paused_analysis_untouched_even_without_a_remote() {
    let h = harness().await.with_queue();
    let id = h
        .seed_unprepared("missing", &"1".repeat(40), &"2".repeat(40))
        .await;
    h.state
        .db
        .analyses()
        .pause_by_user(
            id,
            "alice",
            Timestamp::now() - SignedDuration::from_hours(24 * 3),
        )
        .await
        .unwrap();
    server::pipeline::resume_unfinished(&h.state).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let summary = h.state.db.analyses().summary(id).await.unwrap();
    assert!(
        matches!(
            summary.state,
            AnalysisState::Paused {
                reason: Some(contracts::api::analyses::PauseReason::User),
                ..
            }
        ),
        "{:?}",
        summary.state
    );
    assert!(
        h.state
            .db
            .jobs()
            .list_for_analysis(id, &[])
            .await
            .unwrap()
            .is_empty()
    );
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
}

#[tokio::test]
async fn lazy_review_opens_after_context_before_any_flow() {
    let h = harness().await.with_queue();
    let id = h.create_fixture_analysis().await;
    for stage in ["plan:1", "pr_context"] {
        let mut claim = None;
        for _ in 0..200 {
            claim = h.runner_claim().await;
            if claim.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let claim = claim.expect("job");
        let result = JobResult {
            job_id: claim.spec.job_id,
            attempt: claim.spec.attempt,
            outcome: JobOutcome::Succeeded {
                output: agent_output(stage),
            },
            session: None,
            usage: None,
            skills_commit: None,
            llm_call: None,
        };
        let uri = RUNNER_RESULT
            .path
            .replace("{id}", &claim.spec.job_id.to_string());
        assert_eq!(
            h.runner(&uri, claim.job_token.expose(), json!(result))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
    }
    for _ in 0..200 {
        let (_, summary) = h.get(&format!("/api/v1/analyses/{id}")).await;
        if summary["review_ready"] == true {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (status, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(status, StatusCode::OK, "{tour}");
    assert_eq!(tour["analysis"]["review_ready"], true);
    assert_eq!(tour["flows"].as_array().unwrap().len(), 5);
    assert!(
        tour["flows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["steps"].as_array().unwrap().is_empty())
    );
    assert!(
        !tour["flows"][0]["planned"]["ranges"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let background = format!("/api/v1/analyses/{id}/background");
    assert_eq!(
        json_of(
            h.send(
                "PUT",
                &background,
                Auth::Session,
                Some(json!({"enabled":false}))
            )
            .await
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        json_of(
            h.send(
                "PUT",
                &background,
                Auth::SessionCsrf,
                Some(json!({"enabled":false}))
            )
            .await
        )
        .await
        .0,
        StatusCode::OK
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.runner_claim().await.is_none());
    let request = format!("/api/v1/analyses/{id}/flows/f4/request");
    assert_eq!(
        json_of(h.send("POST", &request, Auth::Session, None).await)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(h.post(&request, json!({})).await.0, StatusCode::OK);
    let mut claim = None;
    for _ in 0..200 {
        claim = h.runner_claim().await;
        if claim.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let claim = claim.unwrap();
    let job = h.state.db.jobs().get(claim.spec.job_id).await.unwrap();
    assert_eq!(
        job.stage,
        JobStage::Flow(contracts::ids::FlowId("f4".into()))
    );
    // A new demand cannot interrupt f4; it is the next job after its accepted result.
    assert_eq!(
        h.post(
            &format!("/api/v1/analyses/{id}/flows/f2/request"),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(h.runner_claim().await.is_none());
    let result = JobResult {
        job_id: claim.spec.job_id,
        attempt: claim.spec.attempt,
        outcome: JobOutcome::Succeeded {
            output: tagged_flow_output(&claim.spec, "flow:f4"),
        },
        session: None,
        usage: None,
        skills_commit: None,
        llm_call: None,
    };
    let uri = RUNNER_RESULT
        .path
        .replace("{id}", &claim.spec.job_id.to_string());
    assert_eq!(
        h.runner(&uri, claim.job_token.expose(), json!(result))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    for _ in 0..200 {
        if h.state
            .db
            .documents()
            .get::<Value>(
                serde_json::from_value(json!(id)).unwrap(),
                DocumentKind::Flow,
                "f4",
            )
            .await
            .unwrap()
            .is_some()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (_, partial) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    let f4 = partial["flows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "f4")
        .unwrap();
    assert_eq!(f4["analysis_status"]["state"], "analysed");
    assert!(!f4["steps"].as_array().unwrap().is_empty());
    let mut next = None;
    for _ in 0..200 {
        next = h.runner_claim().await;
        if next.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let next = next.unwrap();
    assert_eq!(
        h.state.db.jobs().get(next.spec.job_id).await.unwrap().stage,
        JobStage::Flow(contracts::ids::FlowId("f2".into()))
    );
    // Pause keeps accepted steps usable for private Ask after the active flow finishes.
    assert_eq!(
        h.post(&format!("/api/v1/analyses/{id}/pause"), json!({}))
            .await
            .0,
        StatusCode::OK
    );
    let result = JobResult {
        job_id: next.spec.job_id,
        attempt: next.spec.attempt,
        outcome: JobOutcome::Succeeded {
            output: tagged_flow_output(&next.spec, "flow:f2"),
        },
        session: None,
        usage: None,
        skills_commit: None,
        llm_call: None,
    };
    let uri = RUNNER_RESULT
        .path
        .replace("{id}", &next.spec.job_id.to_string());
    assert_eq!(
        h.runner(&uri, next.job_token.expose(), json!(result))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let step = f4["steps"][0]["id"].as_str().unwrap();
    let (status, question) = h
        .post(
            &format!("/api/v1/analyses/{id}/steps/{step}/questions"),
            json!({"text":"What changed here?"}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{question}");
    let mut ask = None;
    for _ in 0..200 {
        ask = h.runner_claim().await;
        if ask.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        ask.expect("Ask is runnable while user-paused").spec.kind,
        JobKind::Ask
    );
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
}

/// The removed `test` classification fails the plan's schema check: the plan is repaired like any rejected
/// output (never a failed analysis), and no flow is planned from the rejected answer.
#[tokio::test]
async fn test_classification_is_repaired_before_flow_jobs() {
    let h = harness().await;
    let mut output = agent_output("plan:1");
    output["flows"][0]["classification"] = json!("test");
    output["flows"][0]["title"] = json!("Test retry helpers");
    h.executor.script("plan:1#0", Script::Output(output));
    let id = h.create_fixture_analysis().await;
    let ended = h.wait_until_ended(&id).await;
    assert_eq!(ended.state, AnalysisState::Finished { total: 5 });
    let runs = h.executor.specs_of("plan:1");
    assert_eq!(runs.len(), 2);
    assert!(runs[1].1.prompt.contains("/flows/0/classification"));
    let order: Vec<(String, u32)> = h
        .executor
        .specs
        .lock()
        .expect("lock")
        .iter()
        .map(|(stage, generation, _)| (stage.clone(), *generation))
        .collect();
    let repaired = order
        .iter()
        .position(|run| *run == ("plan:1".to_owned(), 1))
        .expect("the repair run");
    let first_flow = order
        .iter()
        .position(|(stage, _)| stage.starts_with("flow:"))
        .expect("a flow run");
    assert!(
        repaired < first_flow,
        "no flow before the repaired plan: {order:?}"
    );
    let (_, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    let flows = tour["flows"].as_array().expect("flows");
    assert!(
        flows
            .iter()
            .all(|flow| flow["llm"]["title"] != "Test retry helpers")
    );
}

/// A repair run refused because the demand changed is not a failure: the demanded flow runs next, and the
/// rejected flow is repaired afterwards from its stored output (no second first run).
#[tokio::test]
async fn repair_run_superseded_by_a_flow_request_waits_for_its_turn() {
    let h = harness().await.with_queue();
    let id = h.create_fixture_analysis().await;
    h.open_review(&id).await;
    let first = h.claim_next(&id).await;
    assert_eq!(h.stage_of(&first).await, ("flow:f1".to_owned(), 0));
    let (status, body) = h
        .post(
            &format!("/api/v1/analyses/{id}/flows/f3/request"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    h.complete(
        &first,
        JobOutcome::Succeeded {
            output: json!("not a flow file"),
        },
    )
    .await;
    let demanded = h.claim_next(&id).await;
    assert_eq!(h.stage_of(&demanded).await, ("flow:f3".to_owned(), 0));
    h.complete(
        &demanded,
        JobOutcome::Succeeded {
            output: tagged_flow_output(&demanded.spec, "flow:f3"),
        },
    )
    .await;
    let repair = h.claim_next(&id).await;
    assert_eq!(h.stage_of(&repair).await, ("flow:f1".to_owned(), 1));
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
}

/// Background analysis switched off while a flow runs: its rejected output is not repaired until work is
/// admitted again, and the analysis stays usable meanwhile.
#[tokio::test]
async fn repair_run_superseded_by_background_off_resumes_with_background_on() {
    let h = harness().await.with_queue();
    let id = h.create_fixture_analysis().await;
    h.open_review(&id).await;
    let first = h.claim_next(&id).await;
    assert_eq!(h.stage_of(&first).await, ("flow:f1".to_owned(), 0));
    let background = format!("/api/v1/analyses/{id}/background");
    let switch = |enabled: bool| {
        h.send(
            "PUT",
            &background,
            Auth::SessionCsrf,
            Some(json!({ "enabled": enabled })),
        )
    };
    assert_eq!(switch(false).await.status(), StatusCode::OK);
    h.complete(
        &first,
        JobOutcome::Succeeded {
            output: json!("not a flow file"),
        },
    )
    .await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(h.runner_claim().await.is_none());
    let (_, summary) = h.get(&format!("/api/v1/analyses/{id}")).await;
    assert_eq!(summary["state"]["state"], "running", "{summary}");
    assert_eq!(switch(true).await.status(), StatusCode::OK);
    let repair = h.claim_next(&id).await;
    assert_eq!(h.stage_of(&repair).await, ("flow:f1".to_owned(), 1));
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
}

/// A failed flow is retried automatically twice, after the flows not analysed yet; then it stays failed with
/// its cause, the driver ends, and the user's Retry still runs it.
#[tokio::test]
async fn failed_flow_is_retried_automatically_twice_then_waits_for_the_user() {
    let h = harness().await;
    h.executor.script("flow:f2", Script::Fail);
    let id = h.create_fixture_analysis().await;
    settled_runs(&h, &id, "flow:f2", 3).await;
    let order: Vec<String> = h
        .executor
        .specs
        .lock()
        .expect("lock")
        .iter()
        .map(|(stage, _, _)| stage.clone())
        .collect();
    assert_eq!(
        order,
        [
            "plan:1",
            "pr_context",
            "flow:f1",
            "flow:f2",
            "flow:f3",
            "flow:f4",
            "flow:f5",
            "flow:f2",
            "flow:f2"
        ]
    );
    let tour = wait_for_failed_flow(&h, &id, "f2").await;
    assert_eq!(tour["analysis"]["state"]["state"], "running");
    assert_eq!(tour["analysis"]["state"]["done"], 4);
    wait_parked(analysis_id(&id)).await;
    assert_eq!(
        server::pipeline::resume_unfinished(&h.state)
            .await
            .expect("resume"),
        0,
        "an analysis without admitted work is not prepared again at start"
    );
    h.executor.unscript("flow:f2");
    let (status, body) = h
        .post(
            &format!("/api/v1/analyses/{id}/flows/f2/request"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        h.wait_until_finished(&id).await.state,
        AnalysisState::Finished { total: 5 }
    );
}

/// With background analysis off and nothing requested the driver ends and the analysis stays `running`
/// without a stage; a flow request starts the work again.
#[tokio::test]
async fn analysis_without_admitted_work_parks_and_a_flow_request_restarts_it() {
    let h = harness().await.with_queue();
    let id = h.create_fixture_analysis().await;
    h.open_review(&id).await;
    let first = h.claim_next(&id).await;
    assert_eq!(h.stage_of(&first).await, ("flow:f1".to_owned(), 0));
    let background = format!("/api/v1/analyses/{id}/background");
    let off = h
        .send(
            "PUT",
            &background,
            Auth::SessionCsrf,
            Some(json!({ "enabled": false })),
        )
        .await;
    assert_eq!(off.status(), StatusCode::OK);
    h.complete(
        &first,
        JobOutcome::Succeeded {
            output: tagged_flow_output(&first.spec, "flow:f1"),
        },
    )
    .await;
    wait_parked(analysis_id(&id)).await;
    let (_, summary) = h.get(&format!("/api/v1/analyses/{id}")).await;
    assert_eq!(summary["state"]["state"], "running", "{summary}");
    assert_eq!(summary["state"]["done"], 1, "{summary}");
    assert_eq!(summary["state"]["stage"], Value::Null, "{summary}");
    assert!(h.runner_claim().await.is_none());
    assert_eq!(
        server::pipeline::resume_unfinished(&h.state)
            .await
            .expect("resume"),
        0
    );
    let (status, body) = h
        .post(
            &format!("/api/v1/analyses/{id}/flows/f4/request"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let next = h.claim_next(&id).await;
    assert_eq!(h.stage_of(&next).await, ("flow:f4".to_owned(), 0));
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
}

/// After the user changes the language the lookup still shows the pull request's analysis with its language;
/// an analysis in the new language is started only on purpose, and the lookup then prefers it.
#[tokio::test]
async fn lookup_shows_an_analysis_made_in_another_language() {
    let h = harness().await;
    let first = h.create_fixture_analysis_on_request().await;
    wait_parked(analysis_id(&first)).await;
    let changed = h
        .send(
            "PUT",
            "/api/v1/settings/language",
            Auth::SessionCsrf,
            Some(json!({ "language": "pl" })),
        )
        .await;
    assert!(changed.status().is_success(), "{}", changed.status());
    let (status, resolved) = h.post("/api/v1/pr/resolve", json!({ "url": PR_URL })).await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert_eq!(resolved["analysis"]["id"], json!(first), "{resolved}");
    assert_eq!(resolved["analysis"]["language"], "en");
    assert_eq!(resolved["head_moved"], false);
    let second = h.create_fixture_analysis_on_request().await;
    assert_ne!(second, first, "another language is a new analysis");
    let (_, resolved) = h.post("/api/v1/pr/resolve", json!({ "url": PR_URL })).await;
    assert_eq!(resolved["analysis"]["id"], json!(second), "{resolved}");
    assert_eq!(resolved["analysis"]["language"], "pl");
    for id in [first, second] {
        h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
            .await;
    }
}

/// The lookup carries the size of any pull request, not only of a large one, and makes no time claim without
/// history.
#[tokio::test]
async fn resolve_carries_the_size_of_any_pull_request() {
    let h = harness().await;
    let (status, resolved) = h.post("/api/v1/pr/resolve", json!({ "url": PR_URL })).await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert_eq!(
        resolved["size"],
        json!({ "changed_lines": 238, "changed_files": 13 })
    );
    assert_eq!(resolved["size_warning"], Value::Null);
    assert_eq!(resolved.get("planning_estimate"), None);
}

/// Waits for an `analysis` event of a running analysis whose `idle` is as given; returns its state.
async fn idle_event(
    events: &mut tokio::sync::broadcast::Receiver<AnalysisEvent>,
    idle: bool,
) -> Value {
    let wait = async {
        loop {
            if let AnalysisEvent::Analysis { summary } = events.recv().await.expect("event") {
                let state = serde_json::to_value(&summary.state).expect("state");
                if state["state"] == "running" && state["idle"] == idle {
                    return state;
                }
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .expect("no analysis event with the expected idle value")
}

/// Nothing is analysed until the user asks: a new analysis plans, describes the PR context and waits (review
/// ready, idle, no flow job); reading the Tour requests nothing; a requested flow runs and the analysis waits
/// again; background analysis on runs the rest. Each change of `idle` is an `analysis` event.
#[tokio::test]
async fn new_analysis_waits_after_plan_and_context_until_flows_are_requested() {
    let h = harness().await.with_queue();
    let id = h.create_fixture_analysis_on_request().await;
    let analysis = analysis_id(&id);
    let mut events = h.state.progress.subscribe(analysis);
    h.open_review(&id).await;
    let state = idle_event(&mut events, true).await;
    assert_eq!((&state["done"], &state["total"]), (&json!(0), &json!(5)));
    wait_parked(analysis).await;
    let (_, summary) = h.get(&format!("/api/v1/analyses/{id}")).await;
    assert_eq!(summary["state"]["idle"], true, "{summary}");
    assert_eq!(summary["state"]["stage"], Value::Null, "{summary}");
    assert_eq!(summary["review_ready"], true, "{summary}");
    assert_eq!(summary["background_analysis"], false, "{summary}");
    assert_eq!(summary["progress"]["estimate"], Value::Null, "{summary}");
    assert_eq!(summary["progress"]["percent"], Value::Null, "{summary}");
    // Reading requests nothing.
    let (status, tour) = h.get(&format!("/api/v1/analyses/{id}/tour")).await;
    assert_eq!(status, StatusCode::OK, "{tour}");
    let flows = tour["flows"].as_array().expect("flows");
    assert_eq!(flows.len(), 5);
    for flow in flows {
        assert_eq!(flow["analysis_status"]["state"], "not_analysed", "{flow}");
        assert!(
            !flow["planned"]["ranges"]
                .as_array()
                .expect("ranges")
                .is_empty(),
            "the planned lines give the flow's size: {flow}"
        );
    }
    assert!(
        h.runner_claim().await.is_none(),
        "no flow job without a request"
    );
    // A restart leaves it waiting.
    assert_eq!(
        server::pipeline::resume_unfinished(&h.state)
            .await
            .expect("resume"),
        0
    );
    let (status, requested) = h
        .post(
            &format!("/api/v1/analyses/{id}/flows/f3/request"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{requested}");
    assert_eq!(requested["state"]["idle"], false, "{requested}");
    idle_event(&mut events, false).await;
    let claim = h.claim_next(&id).await;
    assert_eq!(h.stage_of(&claim).await, ("flow:f3".to_owned(), 0));
    h.complete(
        &claim,
        JobOutcome::Succeeded {
            output: tagged_flow_output(&claim.spec, "flow:f3"),
        },
    )
    .await;
    let state = idle_event(&mut events, true).await;
    assert_eq!(state["done"], 1);
    wait_parked(analysis).await;
    assert!(
        h.runner_claim().await.is_none(),
        "only the requested flow ran"
    );
    h.set_background(&id, true).await;
    idle_event(&mut events, false).await;
    for _ in 0..4 {
        let claim = h.claim_next(&id).await;
        let (stage, _) = h.stage_of(&claim).await;
        h.complete(
            &claim,
            JobOutcome::Succeeded {
                output: tagged_flow_output(&claim.spec, &stage),
            },
        )
        .await;
    }
    assert_eq!(
        h.wait_until_finished(&id).await.state,
        AnalysisState::Finished { total: 5 }
    );
}

/// With background analysis off a requested flow that fails is retried automatically twice and then waits for
/// the user; no other flow runs.
#[tokio::test]
async fn requested_flow_that_fails_is_retried_automatically_and_nothing_else_runs() {
    let h = harness().await;
    h.executor
        .script("flow:f2", Script::FailKind(JobFailureKind::Timeout));
    let id = h.create_fixture_analysis_on_request().await;
    wait_parked(analysis_id(&id)).await;
    let (status, body) = h
        .post(
            &format!("/api/v1/analyses/{id}/flows/f2/request"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    settled_runs(&h, &id, "flow:f2", 3).await;
    let flows: Vec<String> = h
        .executor
        .submitted()
        .into_iter()
        .filter(|stage| stage.starts_with("flow:"))
        .collect();
    assert_eq!(flows, ["flow:f2", "flow:f2", "flow:f2"]);
    let tour = wait_for_failed_flow(&h, &id, "f2").await;
    let flow = tour["flows"]
        .as_array()
        .expect("flows")
        .iter()
        .find(|f| f["id"] == "f2")
        .expect("f2");
    assert_eq!(
        flow["analysis_status"]["message"],
        "The flow analysis failed. The LLM call took too long and was stopped. Retry this flow."
    );
    assert_eq!(tour["analysis"]["state"]["idle"], true);
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
}

/// A flow request stored while no driver ran (the server stopped before starting one) is picked up at start.
#[tokio::test]
async fn restart_starts_a_driver_for_a_stored_flow_request() {
    let h = harness().await.with_queue();
    let id = h.create_fixture_analysis().await;
    h.open_review(&id).await;
    let first = h.claim_next(&id).await;
    let off = h
        .send(
            "PUT",
            &format!("/api/v1/analyses/{id}/background"),
            Auth::SessionCsrf,
            Some(json!({ "enabled": false })),
        )
        .await;
    assert_eq!(off.status(), StatusCode::OK);
    h.complete(
        &first,
        JobOutcome::Succeeded {
            output: tagged_flow_output(&first.spec, "flow:f1"),
        },
    )
    .await;
    wait_parked(analysis_id(&id)).await;
    h.state
        .db
        .flow_work()
        .request(
            analysis_id(&id),
            &contracts::ids::FlowId("f4".to_owned()),
            Timestamp::now(),
        )
        .await
        .expect("request");
    assert!(h.runner_claim().await.is_none(), "no driver, no job");
    assert_eq!(
        server::pipeline::resume_unfinished(&h.state)
            .await
            .expect("resume"),
        1
    );
    let next = h.claim_next(&id).await;
    assert_eq!(h.stage_of(&next).await, ("flow:f4".to_owned(), 0));
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
}

#[tokio::test]
async fn failed_flow_job_keeps_its_cause() {
    let h = harness().await;
    h.executor
        .script("flow:f2", Script::FailKind(JobFailureKind::Timeout));
    let id = h.create_fixture_analysis().await;
    settled_runs(&h, &id, "flow:f2", 3).await;
    let tour = wait_for_failed_flow(&h, &id, "f2").await;
    let flow = tour["flows"]
        .as_array()
        .expect("flows")
        .iter()
        .find(|f| f["id"] == "f2")
        .expect("f2");
    assert_eq!(
        flow["analysis_status"]["message"],
        "The flow analysis failed. The LLM call took too long and was stopped. Retry this flow."
    );
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
}

/// While an automatic retry of a failed flow is left, its message says so instead of asking for a Retry.
#[tokio::test]
async fn failed_flow_with_an_automatic_retry_left_says_so() {
    let h = harness().await.with_queue();
    let id = h.create_fixture_analysis().await;
    h.open_review(&id).await;
    let first = h.claim_next(&id).await;
    assert_eq!(h.stage_of(&first).await, ("flow:f1".to_owned(), 0));
    h.complete(
        &first,
        JobOutcome::Failed {
            kind: JobFailureKind::Timeout,
            message: "the runtime cap was reached".to_owned(),
        },
    )
    .await;
    let tour = wait_for_failed_flow(&h, &id, "f1").await;
    let flow = tour["flows"]
        .as_array()
        .expect("flows")
        .iter()
        .find(|f| f["id"] == "f1")
        .expect("f1");
    assert_eq!(
        flow["analysis_status"]["message"],
        "The flow analysis failed. The LLM call took too long and was stopped. It will be tried again automatically."
    );
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
}

/// A flow retry cannot help when the credential is unusable: the analysis stops with the sign-in sentence.
#[tokio::test]
async fn unusable_credential_in_a_flow_job_stops_the_analysis_with_the_sign_in_sentence() {
    let h = harness().await;
    h.executor.script(
        "flow:f2",
        Script::FailKind(JobFailureKind::CredentialUnusable),
    );
    let id = h.create_fixture_analysis().await;
    let failed = h
        .wait_for_state(&id, |state| matches!(state, AnalysisState::Failed { .. }))
        .await;
    let AnalysisState::Failed { flow, message, .. } = failed.state else {
        unreachable!("waited for failed");
    };
    assert_eq!(flow.map(|flow| flow.id.0), Some("f2".to_owned()));
    assert!(
        message.contains("Sign-in to the LLM provider is needed"),
        "{message}"
    );
    assert_eq!(
        h.executor.specs_of("flow:f2").len(),
        1,
        "no automatic retry"
    );
}

/// The Tour's flow status comes from the documents its content was built from: a flow stored after that read is
/// not called analysed with its placeholder content.
#[tokio::test]
async fn tour_of_an_earlier_read_does_not_call_a_flow_stored_since_analysed() {
    let h = harness().await.with_queue();
    let id = h.create_fixture_analysis().await;
    let analysis = analysis_id(&id);
    h.open_review(&id).await;
    let first = h.claim_next(&id).await;
    assert_eq!(h.stage_of(&first).await, ("flow:f1".to_owned(), 0));
    let read = server::pipeline::context::StoredResults::load(&h.state.db, analysis)
        .await
        .expect("results");
    h.complete(
        &first,
        JobOutcome::Succeeded {
            output: tagged_flow_output(&first.spec, "flow:f1"),
        },
    )
    .await;
    for _ in 0..200 {
        let stored: Option<Value> = h
            .state
            .db
            .documents()
            .get(analysis, DocumentKind::Flow, "f1")
            .await
            .expect("read");
        if stored.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let summary = h
        .state
        .db
        .analyses()
        .summary(analysis)
        .await
        .expect("summary");
    let tour = server::pipeline::tour::load_from(&h.state, &summary, read)
        .await
        .expect("tour");
    let flow = tour
        .flows
        .iter()
        .find(|flow| flow.id.0 == "f1")
        .expect("f1");
    assert!(flow.steps.is_empty(), "built from the earlier read");
    assert_ne!(
        flow.analysis_status.as_ref().map(|status| status.state),
        Some(contracts::api::tour::FlowAnalysisState::Analysed)
    );
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
}

/// A flow job that succeeded before a restart, its verified document not stored yet, is recovered at start
/// without a new agent call, also when background analysis was switched off meanwhile.
#[tokio::test]
async fn restart_recovers_a_succeeded_flow_job_with_background_off() {
    let h = harness().await.with_queue();
    let id = h.create_fixture_analysis().await;
    let analysis = analysis_id(&id);
    h.open_review(&id).await;
    let flow = JobStage::Flow(contracts::ids::FlowId("f1".to_owned()));
    let jobs = h.state.db.jobs();
    for _ in 0..300 {
        let queued = jobs
            .list_for_analysis(analysis, &[JobState::Queued])
            .await
            .expect("jobs");
        if queued.iter().any(|job| job.stage == flow) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Background off withdraws the queued first flow: no driver works on the analysis afterwards.
    let background = format!("/api/v1/analyses/{id}/background");
    let off = h
        .send(
            "PUT",
            &background,
            Auth::SessionCsrf,
            Some(json!({ "enabled": false })),
        )
        .await;
    assert_eq!(off.status(), StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(700)).await;
    let withdrawn = jobs
        .list_for_analysis(analysis, &[JobState::Cancelled])
        .await
        .expect("jobs")
        .into_iter()
        .find(|job| job.stage == flow)
        .expect("the withdrawn flow job");
    // The state a crash leaves: the flow started by background work, the switch turned off after the claim,
    // the job succeeded, its document not stored.
    let now = Timestamp::now();
    h.state
        .db
        .flow_work()
        .set_background(analysis, true, now)
        .await
        .expect("on");
    let mut spec = withdrawn.spec.clone();
    spec.job_id = JobId::new_random();
    h.state
        .executor
        .submit(JobRequest {
            spec,
            priority: JobPriority::Batch,
            stage: flow.clone(),
            generation: 0,
            pr_repo: Some(contracts::common::RepoRef {
                owner: "acme".to_owned(),
                name: "node-client".to_owned(),
            }),
        })
        .await
        .expect("submit");
    let claim = h.claim_next(&id).await;
    assert_eq!(h.stage_of(&claim).await, ("flow:f1".to_owned(), 0));
    h.state
        .db
        .flow_work()
        .set_background(analysis, false, now)
        .await
        .expect("off");
    h.complete(
        &claim,
        JobOutcome::Succeeded {
            output: tagged_flow_output(&claim.spec, "flow:f1"),
        },
    )
    .await;
    let known = jobs
        .list_for_analysis(analysis, &[])
        .await
        .expect("jobs")
        .len();

    server::pipeline::resume_unfinished(&h.state)
        .await
        .expect("resume");
    let mut stored = None::<Value>;
    for _ in 0..300 {
        stored = h
            .state
            .db
            .documents()
            .get(analysis, DocumentKind::Flow, "f1")
            .await
            .expect("read");
        if stored.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        stored.is_some(),
        "the succeeded job's result is stored at start"
    );
    assert_eq!(
        jobs.list_for_analysis(analysis, &[])
            .await
            .expect("jobs")
            .len(),
        known,
        "no new agent call"
    );
    h.post(&format!("/api/v1/analyses/{id}/cancel"), json!({}))
        .await;
}

/// A plan output that does not match its schema gets a repair run like any other rejected output.
#[tokio::test]
async fn schema_invalid_plan_output_is_repaired() {
    let h = harness().await;
    h.executor.script("plan:1#0", Script::Output(json!({})));
    let id = h.create_fixture_analysis().await;
    assert_eq!(
        h.wait_until_finished(&id).await.state,
        AnalysisState::Finished { total: 5 }
    );
    let runs = h.executor.specs_of("plan:1");
    assert_eq!(runs.len(), 2);
    assert!(
        runs[1].1.prompt.contains("schema_mismatch"),
        "{}",
        runs[1].1.prompt
    );
}
