//! Ask over the fixture pull request `acme/node-client#482`: the HTTP routes, the follower with a scripted
//! executor, and claim order and claim contents with the real queue over the runner routes.

use std::collections::HashMap;
use std::path::Path as FsPath;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::{DateTime, TimeDelta};
use contracts::api::analyses::AnalysisState;
use contracts::api::events::AnalysisEvent;
use contracts::api::pr::PrSummary;
use contracts::api::questions::{AskQuestion, AskState};
use contracts::ids::{AnalysisId, FlowId, JobId, QuestionId};
use contracts::job::protocol::{Claim, ClaimResponse};
use contracts::job::result::{JobFailureKind, JobOutcome, JobResult};
use contracts::job::spec::{JobPriority, JobSpec};
use contracts::job::state::JobState;
use contracts::routes::{ASK_QUESTION, LIST_QUESTIONS, RUNNER_CLAIM, RUNNER_RESULT};
use db::repo::analyses::NewAnalysis;
use db::repo::analyses::testing::{job_spec, seed_analysis};
use db::repo::documents::DocumentKind;
use db::repo::jobs::{JobStage, NewJob};
use db::repo::{DEFAULT_TENANT, Db};
use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use http_body_util::BodyExt;
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

use super::*;
use crate::api::{router, runner_router};
use crate::config::tests::test_config;
use crate::jobs::executor::{ExecutorError, JobExecutor, JobHandle, JobRequest, JobUpdate};
use crate::jobs::queue::QueueExecutor;
use crate::pipeline::{self, CREDENTIAL_ID};
use crate::test_support::logs::LogCapture;
use crate::test_support::session_cookie;

const WAIT: Duration = Duration::from_secs(20);
const QUESTION: &str = "Can broadcast_tx now send the same transaction twice?";

/// Answers jobs only when told to: every job's updates come from a channel the test feeds.
#[derive(Debug, Default)]
struct Scripted {
    submitted: Mutex<Vec<JobRequest>>,
    channels: Mutex<HashMap<JobId, Channel>>,
    cancelled: Mutex<Vec<JobId>>,
}

type Update = Result<JobUpdate, ExecutorError>;

#[derive(Debug)]
struct Channel {
    sender: mpsc::UnboundedSender<Update>,
    receiver: Option<mpsc::UnboundedReceiver<Update>>,
}

impl Channel {
    fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded();
        Self {
            sender,
            receiver: Some(receiver),
        }
    }
}

impl Scripted {
    fn send(&self, job: JobId, update: JobUpdate) {
        let mut channels = self.channels.lock().expect("lock");
        let channel = channels.entry(job).or_insert_with(Channel::new);
        channel.sender.unbounded_send(Ok(update)).expect("send");
    }

    fn close(&self, job: JobId) {
        if let Some(channel) = self.channels.lock().expect("lock").get(&job) {
            channel.sender.close_channel();
        }
    }

    fn requests(&self) -> Vec<JobRequest> {
        self.submitted.lock().expect("lock").clone()
    }
}

impl JobExecutor for Scripted {
    fn submit(&self, request: JobRequest) -> BoxFuture<'_, Result<JobHandle, ExecutorError>> {
        let job_id = request.spec.job_id;
        self.submitted.lock().expect("lock").push(request);
        Box::pin(async move { Ok(JobHandle { job_id }) })
    }

    fn events(&self, handle: &JobHandle) -> BoxStream<'static, Update> {
        let mut channels = self.channels.lock().expect("lock");
        let channel = channels.entry(handle.job_id).or_insert_with(Channel::new);
        // A second follower of the same job (a restart) gets a fresh channel.
        let receiver = channel.receiver.take().unwrap_or_else(|| {
            *channel = Channel::new();
            channel.receiver.take().expect("fresh receiver")
        });
        Box::pin(receiver)
    }

    fn cancel(&self, handle: &JobHandle) -> BoxFuture<'_, Result<(), ExecutorError>> {
        self.cancelled.lock().expect("lock").push(handle.job_id);
        Box::pin(async { Ok(()) })
    }

    fn cancel_analysis(&self, _: AnalysisId) -> BoxFuture<'_, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
}

struct Fixture {
    state: AppState,
    /// The finished analysis of the fixture pull request.
    analysis: AnalysisId,
    /// Another analysis (planning) of the tenant.
    other: AnalysisId,
    cookie: String,
    scripted: Arc<Scripted>,
    _dir: TempDir,
}

fn fixture_dir() -> std::path::PathBuf {
    FsPath::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482")
}

fn fixture_json(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixture_dir().join(path)).expect("fixture"))
        .expect("JSON")
}

/// The fixture analysis, finished, with its stored outputs and pre-pass; `queue` runs the real job queue,
/// otherwise the scripted executor.
async fn fixture(queue: bool) -> Fixture {
    fixture_with(queue, None).await
}

/// [`fixture`] with GitHub App settings (`github`), so claims mint an installation token.
async fn fixture_with(queue: bool, github: Option<crate::config::GithubConfig>) -> Fixture {
    let dir = tempfile::tempdir().expect("temp dir");
    let built = Command::new(fixture_dir().join("repo/build-repo.sh"))
        .arg(dir.path().join("remotes/acme/node-client.git"))
        .output()
        .expect("build-repo.sh");
    assert!(built.status.success(), "{built:?}");
    let mut config = test_config();
    config.data_dir = dir.path().join("data");
    if let Some(github) = github {
        config.github = github;
    }
    config.github.clone_url =
        url::Url::parse(&format!("file://{}", dir.path().join("remotes").display())).expect("url");
    let db = Db::in_memory().await.expect("db");
    let other = seed_analysis(&db).await.expect("seed");
    let scripted = Arc::new(Scripted::default());
    let executor: Arc<dyn JobExecutor> = if queue {
        Arc::new(QueueExecutor::new(db.clone()))
    } else {
        scripted.clone()
    };
    let state = AppState::new(config, db, executor);
    let pr: PrSummary =
        serde_json::from_value(fixture_json("api-seed/default/pr-resolve.json")["pr"].clone())
            .expect("pr");
    let analysis = state
        .db
        .analyses()
        .create(
            NewAnalysis {
                id: AnalysisId::new_random(),
                tenant_id: DEFAULT_TENANT.to_owned(),
                pull_request_id: 1,
                base_sha: pr.base_sha,
                head_sha: pr.head_sha,
                language: Default::default(),
                created_by: "u1".to_owned(),
            },
            Utc::now(),
        )
        .await
        .expect("analysis")
        .id;
    let documents = state.db.documents();
    let now = Utc::now();
    documents
        .put(
            analysis,
            DocumentKind::Plan,
            "",
            &fixture_json("agent/plan.json"),
            now,
        )
        .await
        .expect("plan");
    documents
        .put(
            analysis,
            DocumentKind::PrContext,
            "",
            &fixture_json("agent/pr_context.json"),
            now,
        )
        .await
        .expect("pr context");
    for n in 1..=5 {
        let flow = fixture_json(&format!("agent/flow-{n}.json"));
        documents
            .put(analysis, DocumentKind::Flow, &format!("f{n}"), &flow, now)
            .await
            .expect("flow");
    }
    let summary = state
        .db
        .analyses()
        .summary(analysis)
        .await
        .expect("summary");
    let (mirror, files) = pipeline::pr_diff(&state, &summary).await.expect("diff");
    pipeline::prepass::ensure(&state, &summary, &mirror, &files)
        .await
        .expect("pre-pass");
    state
        .db
        .analyses()
        .set_state(analysis, &AnalysisState::Finished { total: 5 }, Utc::now())
        .await
        .expect("finished");
    let cookie = session_cookie(&state, DEFAULT_TENANT).await;
    Fixture {
        state,
        analysis,
        other,
        cookie,
        scripted,
        _dir: dir,
    }
}

async fn json_of(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

impl Fixture {
    async fn ask_as(
        &self,
        analysis: AnalysisId,
        step: &str,
        text: &str,
        cookie: &str,
        csrf: Option<&str>,
    ) -> (StatusCode, Value) {
        let path = ASK_QUESTION
            .path
            .replace("{id}", &analysis.to_string())
            .replace("{step_id}", step);
        let mut request = Request::post(path)
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(csrf) = csrf {
            request = request.header("x-csrf-token", csrf);
        }
        let request = request
            .body(Body::from(json!({ "text": text }).to_string()))
            .expect("request");
        json_of(
            router(self.state.clone())
                .oneshot(request)
                .await
                .expect("infallible"),
        )
        .await
    }

    async fn ask(&self, step: &str, text: &str) -> (StatusCode, Value) {
        self.ask_as(self.analysis, step, text, &self.cookie, Some("csrf"))
            .await
    }

    /// Asks `text` on `step` and returns the stored question's id.
    async fn asked(&self, step: &str, text: &str) -> QuestionId {
        let (status, body) = self.ask(step, text).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        serde_json::from_value(body["id"].clone()).expect("question id")
    }

    async fn list(&self) -> (StatusCode, Value) {
        let request = Request::get(
            LIST_QUESTIONS
                .path
                .replace("{id}", &self.analysis.to_string()),
        )
        .header(header::COOKIE, &self.cookie)
        .body(Body::empty())
        .expect("request");
        json_of(
            router(self.state.clone())
                .oneshot(request)
                .await
                .expect("infallible"),
        )
        .await
    }

    async fn question(&self, id: QuestionId) -> AskQuestion {
        self.state
            .db
            .questions()
            .get(id, Utc::now())
            .await
            .expect("question")
            .question
    }

    async fn wait_for(&self, id: QuestionId, done: impl Fn(&AskQuestion) -> bool) -> AskQuestion {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let question = self.question(id).await;
            if done(&question) {
                return question;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "question did not reach the expected state: {question:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// The scripted job of `question`, once submitted.
    async fn scripted_job(&self, question: QuestionId) -> JobRequest {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let found = self
                .scripted
                .requests()
                .into_iter()
                .find(|request| request.stage == JobStage::Ask(question));
            if let Some(request) = found {
                return request;
            }
            assert!(tokio::time::Instant::now() < deadline, "no job submitted");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// The question's queued job id, once submitted (real queue).
    async fn queued_job(&self, question: QuestionId) -> JobId {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let record = self
                .state
                .db
                .questions()
                .get(question, Utc::now())
                .await
                .expect("question");
            if let Some(job) = record.job_id {
                return job;
            }
            assert!(tokio::time::Instant::now() < deadline, "no job submitted");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn runner(&self, uri: &str, bearer: &str, body: Value) -> (StatusCode, Value) {
        let request = Request::post(uri)
            .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("request");
        json_of(
            runner_router(self.state.clone())
                .oneshot(request)
                .await
                .expect("infallible"),
        )
        .await
    }

    /// One runner claim for the pipeline credential.
    async fn claim(&self) -> Option<Claim> {
        let request = json!({
            "runner_id": "runner-1",
            "credentials": [{
                "credential": { "id": CREDENTIAL_ID, "kind": "codex_chatgpt" },
                "usable": true,
            }],
            "dropped_sessions": [],
        });
        let secret = self.state.config.runner_secret.expose_secret().to_owned();
        let (status, body) = self.runner(RUNNER_CLAIM.path, &secret, request).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        serde_json::from_value::<ClaimResponse>(body)
            .expect("claim response")
            .claim
    }

    async fn post_result(&self, claim: &Claim, outcome: JobOutcome) {
        let result = JobResult {
            job_id: claim.spec.job_id,
            attempt: claim.spec.attempt,
            outcome,
            session: None,
            usage: None,
            skills_commit: None,
            llm_call: None,
        };
        self.post_job_result(claim, result).await;
    }

    async fn post_job_result(&self, claim: &Claim, result: JobResult) {
        let uri = RUNNER_RESULT
            .path
            .replace("{id}", &claim.spec.job_id.to_string());
        let token = claim.job_token.expose().to_owned();
        let (status, body) = self
            .runner(&uri, &token, serde_json::to_value(&result).expect("result"))
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }

    /// A batch job of `analysis` on the pipeline credential.
    async fn batch_job(&self, analysis: AnalysisId, flow: &str) -> JobId {
        let mut spec = job_spec(analysis, contracts::agent::JobKind::AnalyseFlow);
        spec.credential.id = CREDENTIAL_ID.to_owned();
        self.state
            .db
            .jobs()
            .enqueue(
                NewJob {
                    spec,
                    priority: JobPriority::Batch,
                    stage: JobStage::Flow(FlowId(flow.to_owned())),
                    generation: 0,
                },
                Utc::now(),
            )
            .await
            .expect("enqueue")
            .id
    }
}

fn answer(text: &str, evidence: Value) -> Value {
    json!({ "answer": text, "evidence": evidence })
}

fn good_answer() -> Value {
    answer(
        "mempool::broadcast_tx accepts each transaction once; does the node reject a repeated submit?",
        json!([{ "file": "src/mempool/mod.rs", "side": "head", "line": 126 }]),
    )
}

fn succeeded(job: JobId, output: Value) -> JobUpdate {
    JobUpdate::Finished {
        result: JobResult {
            job_id: job,
            attempt: 1,
            outcome: JobOutcome::Succeeded { output },
            session: None,
            usage: None,
            skills_commit: None,
            llm_call: None,
        },
        github_access: false,
    }
}

fn finished(job: JobId, outcome: JobOutcome) -> JobUpdate {
    JobUpdate::Finished {
        result: JobResult {
            job_id: job,
            attempt: 1,
            outcome,
            session: None,
            usage: None,
            skills_commit: None,
            llm_call: None,
        },
        github_access: false,
    }
}

fn event(seq: u32) -> JobUpdate {
    JobUpdate::Event(contracts::job::event::JobEvent {
        seq,
        at: Utc::now(),
        activity: contracts::job::event::ActivityKind::AgentMessage,
        summary: "reading".to_owned(),
    })
}

// --- Acceptance 1: routes -------------------------------------------------------------------------------

#[tokio::test]
async fn ask_question_is_accepted_pending_and_listed_after_reload() {
    let f = fixture(false).await;
    let (status, body) = f.ask("f1.5", &format!("  {QUESTION}\n")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["state"], "pending");
    assert_eq!(body["step_id"], "f1.5");
    assert_eq!(body["text"], QUESTION, "trimmed, otherwise as given");
    assert_eq!(body["answer"], Value::Null);
    assert!(body.get("resumes_at").is_none());
    let (status, listed) = f.list().await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed.as_array().map(Vec::len), Some(1));
    assert_eq!(listed[0]["id"], body["id"]);
    let id: QuestionId = serde_json::from_value(body["id"].clone()).expect("id");
    let record = f
        .state
        .db
        .questions()
        .get(id, Utc::now())
        .await
        .expect("stored");
    assert_eq!(record.user_id, "u1");
}

#[tokio::test]
async fn ask_rejects_steps_outside_the_tour_and_aliases() {
    let f = fixture(false).await;
    for step in ["f1.01", "f1.0", "f1.11", "f9.1", "f1", "x"] {
        let (status, body) = f.ask(step, QUESTION).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{step}: {body}");
        assert_eq!(body["code"], "not_found");
    }
    let (_, listed) = f.list().await;
    assert_eq!(listed, json!([]), "nothing stored");
}

#[tokio::test]
async fn ask_needs_a_finished_analysis_and_csrf_and_the_tenant() {
    let f = fixture(false).await;
    // The other analysis is planning.
    let (status, body) = f
        .ask_as(f.other, "f1.1", QUESTION, &f.cookie, Some("csrf"))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "analysis_not_finished");
    let (status, body) = f
        .ask_as(f.analysis, "f1.1", QUESTION, &f.cookie, None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "csrf_rejected");
    // A session of another tenant sees no analysis: not_found before any work.
    db::repo::analyses::testing::seed_tenant(&f.state.db, "other")
        .await
        .expect("tenant");
    let foreign = session_cookie(&f.state, "other").await;
    let (status, body) = f
        .ask_as(f.analysis, "f1.1", QUESTION, &foreign, Some("csrf"))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (_, listed) = f.list().await;
    assert_eq!(listed, json!([]), "nothing stored");
    assert!(f.scripted.requests().is_empty(), "no job submitted");
}

#[tokio::test]
async fn ask_rejects_blank_and_too_long_questions() {
    let f = fixture(false).await;
    let (status, body) = f.ask("f1.1", " \n ").await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::BAD_REQUEST, &json!("invalid_request"))
    );
    let long = "x".repeat(AskRequest::MAX_TEXT_CHARS + 1);
    let (status, body) = f.ask("f1.1", &long).await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::BAD_REQUEST, &json!("invalid_request"))
    );
    let fits = "é".repeat(AskRequest::MAX_TEXT_CHARS);
    let (status, body) = f.ask("f1.1", &fits).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "characters, not bytes: {body}"
    );
}

// --- Acceptance 6: limits -------------------------------------------------------------------------------

#[tokio::test]
async fn ask_limits_questions_per_user_and_hour() {
    let f = fixture(false).await;
    for _ in 0..MAX_QUESTIONS_PER_USER_PER_HOUR {
        let stored = f
            .state
            .db
            .questions()
            .create(
                NewQuestion {
                    id: QuestionId::new_random(),
                    analysis_id: f.analysis,
                    step_id: StepId("f1.1".to_owned()),
                    user_id: "u1".to_owned(),
                    text: "earlier".to_owned(),
                },
                QuestionLimits {
                    per_analysis: u32::MAX,
                    per_user: u32::MAX,
                    user_window_start: Utc::now(),
                },
                Utc::now(),
            )
            .await
            .expect("create");
        assert!(matches!(stored, Asked::Stored(_)));
    }
    let (status, body) = f.ask("f1.1", QUESTION).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("in the last hour")),
        "{body}"
    );
}

// --- Acceptance 2: the job --------------------------------------------------------------------------------

#[tokio::test]
async fn ask_job_goes_through_the_executor_as_an_interactive_ask_job() {
    let f = fixture(false).await;
    let id = f.asked("f1.5", QUESTION).await;
    let request = f.scripted_job(id).await;
    assert_eq!(request.priority, JobPriority::Interactive);
    assert_eq!(request.generation, 0);
    let spec: &JobSpec = &request.spec;
    assert_eq!(spec.kind, contracts::agent::JobKind::Ask);
    assert_eq!(spec.analysis_id, f.analysis);
    assert!(
        spec.prompt.contains(QUESTION),
        "the question is in the prompt"
    );
    assert!(spec.prompt.contains("f1.5"));
    assert_eq!(spec.context_pack, None, "built at claim");
    assert_eq!(spec.skills, ["severity"], "shared rules require severity");
    assert_eq!(spec.session, None, "set at claim");
    assert_eq!(spec.credential.id, CREDENTIAL_ID);
    assert_eq!(spec.access, contracts::job::access::AgentAccess::default());
    assert_eq!(
        f.state
            .db
            .questions()
            .get(id, Utc::now())
            .await
            .expect("q")
            .job_id,
        Some(spec.job_id)
    );
}

#[tokio::test]
async fn ask_answer_stored_and_streamed() {
    let f = fixture(false).await;
    let mut events = f.state.progress.subscribe(f.analysis);
    let id = f.asked("f1.5", QUESTION).await;
    let job = f.scripted_job(id).await.spec.job_id;
    f.scripted.send(job, event(1));
    f.wait_for(id, |q| q.state == AskState::Answering).await;
    f.scripted.send(job, JobUpdate::OutputReady(good_answer()));
    let ready = f.wait_for(id, |q| q.state == AskState::Answered).await;
    // A compaction event after the ready answer keeps it shown.
    f.scripted.send(job, event(2));
    f.scripted.send(job, succeeded(job, good_answer()));
    f.scripted.close(job);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let stored = f.question(id).await;
    assert_eq!(stored.state, AskState::Answered);
    assert_eq!(stored.answer, ready.answer);
    let answer = stored.answer.expect("answer");
    assert!(answer.answer.starts_with("mempool::broadcast_tx"));
    let mut states = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let AnalysisEvent::Question { question } = event {
            assert_eq!(question.id, id);
            states.push(question.state);
        }
    }
    assert_eq!(
        states,
        [
            AskState::Pending,
            AskState::Answering,
            AskState::Answered,
            AskState::Answered
        ]
    );
    // Survives a reload: the list and the SSE snapshot read it back.
    let (_, listed) = f.list().await;
    assert_eq!(listed[0]["state"], "answered");
    assert_eq!(listed[0]["answer"]["answer"], answer.answer);
}

#[tokio::test]
async fn invalid_answers_fail_the_question_and_are_never_shown() {
    let f = fixture(false).await;
    let unfit = [
        json!({ "answer": "missing evidence" }),
        answer("Is <img src=x onerror=alert(1)> safe?", json!([])),
        answer("See https://evil.example/x for details?", json!([])),
        answer("This retry has no risk at all.", json!([])),
        answer(&"x".repeat(MAX_ANSWER_CHARS + 1), json!([])),
        answer(
            "Is the file there?",
            json!([{ "file": "src/no-such-file.rs", "side": "head", "line": 1 }]),
        ),
    ];
    for output in unfit {
        let id = f.asked("f1.5", QUESTION).await;
        let job = f.scripted_job(id).await.spec.job_id;
        f.scripted.send(job, JobUpdate::OutputReady(output.clone()));
        f.scripted.send(job, succeeded(job, output.clone()));
        let failed = f.wait_for(id, |q| q.state == AskState::Failed).await;
        assert_eq!(failed.answer, None, "{output}");
        assert_eq!(
            failed.failure.as_deref(),
            Some(job::UNFIT_ANSWER),
            "{output}"
        );
    }
}

/// The real failure path: the runner's result carries the provider's text (with a URL and the attempt's
/// installation token) in the outcome and in `llm_call`. The operator log may carry it scrubbed, at warn; no log
/// line carries a secret or the question, and no response, event or stored question carries the provider's text.
#[tokio::test]
async fn failed_ask_shows_a_plain_sentence_and_logs_no_secret_or_question() {
    use contracts::job::result::{LlmCallOutcome, LlmCallStatus};

    use crate::github::app_token::tests::{
        MINTED_TOKEN, github_config, minted, mount_installation, mount_mint, write_key,
    };

    let capture = LogCapture::start();
    let keys = tempfile::tempdir().expect("temp dir");
    let github = wiremock::MockServer::start().await;
    mount_installation(&github).await;
    mount_mint(&github, minted(Utc::now()), 1).await;
    let f = fixture_with(true, Some(github_config(&github, write_key(keys.path())))).await;
    let mut events = f.state.progress.subscribe(f.analysis);
    let marker = "question-text-marker-7f3a";
    let id = f.asked("f1.5", &format!("{QUESTION} {marker}")).await;
    f.queued_job(id).await;
    let claim = f.claim().await.expect("the ask job");
    assert!(
        claim.spec.access.github.is_some(),
        "an installation token was minted"
    );
    let provider_url = "https://api.provider.example/v1/responses?trace=4411";
    let provider_text =
        format!("401 Unauthorized from {provider_url}: token {MINTED_TOKEN} rejected");
    f.post_job_result(
        &claim,
        JobResult {
            job_id: claim.spec.job_id,
            attempt: claim.spec.attempt,
            outcome: JobOutcome::Failed {
                kind: JobFailureKind::ProviderFailed,
                message: provider_text.clone(),
            },
            session: None,
            usage: None,
            skills_commit: None,
            llm_call: Some(LlmCallOutcome {
                at: Utc::now(),
                status: LlmCallStatus::NotWorking {
                    message: provider_text.clone(),
                },
            }),
        },
    )
    .await;
    let failed = f.wait_for(id, |q| q.state == AskState::Failed).await;
    assert!(
        failed
            .failure
            .as_deref()
            .is_some_and(|failure| failure.ends_with("Ask again.")),
        "{failed:?}"
    );

    let stored = format!(
        "{:?}",
        f.state.db.questions().get(id, Utc::now()).await.expect("q")
    );
    let (_, listed) = f.list().await;
    let mut streamed = String::new();
    while let Ok(event) = events.try_recv() {
        streamed.push_str(&serde_json::to_string(&event).expect("event"));
    }
    assert!(streamed.contains("\"failed\""), "{streamed}");
    for (place, text) in [
        ("stored", stored),
        ("list", listed.to_string()),
        ("sse", streamed),
    ] {
        for provider in [
            "api.provider.example",
            "401 Unauthorized",
            "ghs_",
            "[redacted]",
        ] {
            assert!(!text.contains(provider), "{place} shows {provider}: {text}");
        }
    }

    let logs = capture.text();
    assert!(
        logs.contains("LLM call failed") && logs.contains(provider_url),
        "the operator log keeps the scrubbed provider text:\n{logs}"
    );
    let config = &f.state.config;
    for secret in [
        MINTED_TOKEN,
        claim.job_token.expose(),
        config.runner_secret.expose_secret(),
        config.memcan.token.expose_secret(),
        config.github.client_secret.expose_secret(),
        marker,
    ] {
        assert!(!logs.contains(secret), "{secret} logged:\n{logs}");
    }
}

// --- Acceptance 4: states ---------------------------------------------------------------------------------

#[tokio::test]
async fn requeued_attempt_returns_a_ready_answer_to_answering() {
    let f = fixture(false).await;
    let id = f.asked("f1.5", QUESTION).await;
    let job = f.scripted_job(id).await.spec.job_id;
    f.scripted.send(job, JobUpdate::OutputReady(good_answer()));
    f.wait_for(id, |q| q.state == AskState::Answered).await;
    f.scripted.send(job, JobUpdate::Requeued { attempt: 2 });
    let again = f.wait_for(id, |q| q.state == AskState::Answering).await;
    assert_eq!(again.answer, None);
    f.scripted.send(job, finished(job, JobOutcome::Cancelled));
    let cancelled = f.wait_for(id, |q| q.state == AskState::Failed).await;
    assert_eq!(cancelled.failure.as_deref(), Some(job::SUPERSEDED));
}

#[tokio::test]
async fn usage_limit_pauses_the_question_with_its_reset_time() {
    let f = fixture(true).await;
    let id = f.asked("f1.5", QUESTION).await;
    f.queued_job(id).await;
    let claim = f.claim().await.expect("the ask job");
    let resets_at = DateTime::from_timestamp(Utc::now().timestamp() + 3600, 0).expect("time");
    f.post_result(
        &claim,
        JobOutcome::UsageLimited {
            resets_at: Some(resets_at),
        },
    )
    .await;
    let paused = f
        .wait_for(id, |q| {
            q.state == AskState::Pending && q.resumes_at.is_some()
        })
        .await;
    assert_eq!(paused.resumes_at, Some(resets_at));
    let (_, listed) = f.list().await;
    assert_eq!(
        listed[0]["resumes_at"],
        serde_json::to_value(resets_at).expect("time")
    );
}

#[tokio::test]
async fn superseded_analysis_cancels_its_open_questions_and_refuses_new_ones() {
    let f = fixture(true).await;
    let id = f.asked("f1.5", QUESTION).await;
    let job = f.queued_job(id).await;
    // What a re-analysis on a moved head does to the old analysis (`pipeline::ingest::create`).
    f.state
        .db
        .analyses()
        .request_session_drop(f.analysis)
        .await
        .expect("drop");
    f.state
        .executor
        .cancel_analysis(f.analysis)
        .await
        .expect("cancel");
    let failed = f.wait_for(id, |q| q.state == AskState::Failed).await;
    assert_eq!(failed.failure.as_deref(), Some(job::SUPERSEDED));
    assert_eq!(
        f.state.db.jobs().get(job).await.expect("job").state,
        JobState::Cancelled
    );
    let (status, body) = f.ask("f1.5", QUESTION).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
}

#[tokio::test]
async fn restart_follows_open_questions_to_their_end() {
    let f = fixture(false).await;
    // Stopped after storing, before submitting.
    let unsent = match f
        .state
        .db
        .questions()
        .create(
            NewQuestion {
                id: QuestionId::new_random(),
                analysis_id: f.analysis,
                step_id: StepId("f2.1".to_owned()),
                user_id: "u1".to_owned(),
                text: QUESTION.to_owned(),
            },
            QuestionLimits {
                per_analysis: 10,
                per_user: 10,
                user_window_start: Utc::now(),
            },
            Utc::now(),
        )
        .await
        .expect("create")
    {
        Asked::Stored(record) => record.question.id,
        other => panic!("{other:?}"),
    };
    // Stopped while the agent was answering: a job whose updates stream ends without a result.
    let running = f.asked("f1.5", QUESTION).await;
    let job = f.scripted_job(running).await.spec.job_id;
    f.scripted.send(job, event(1));
    f.wait_for(running, |q| q.state == AskState::Answering)
        .await;

    assert!(resume_unanswered(&f.state).await.expect("resume") >= 1);
    let resent = f.scripted_job(unsent).await.spec.job_id;
    f.scripted.send(resent, succeeded(resent, good_answer()));
    f.wait_for(unsent, |q| q.state == AskState::Answered).await;
    // The follower of the running question is still there; the job's stream ends without a result.
    f.scripted.close(job);
    let failed = f.wait_for(running, |q| q.state == AskState::Failed).await;
    assert_eq!(failed.failure.as_deref(), Some(job::SERVER_ERROR));
}

impl Fixture {
    /// The state a stopped server left: a question on its claimed `ask` job, and no follower.
    async fn stopped_on_a_claimed_job(&self) -> (QuestionId, Claim) {
        let Asked::Stored(record) = self
            .state
            .db
            .questions()
            .create(
                NewQuestion {
                    id: QuestionId::new_random(),
                    analysis_id: self.analysis,
                    step_id: StepId("f1.5".to_owned()),
                    user_id: "u1".to_owned(),
                    text: QUESTION.to_owned(),
                },
                QuestionLimits {
                    per_analysis: 10,
                    per_user: 10,
                    user_window_start: Utc::now(),
                },
                Utc::now(),
            )
            .await
            .expect("create")
        else {
            panic!("stored");
        };
        let id = record.question.id;
        let mut spec = job_spec(self.analysis, contracts::agent::JobKind::Ask);
        spec.credential.id = CREDENTIAL_ID.to_owned();
        let handle = self
            .state
            .executor
            .submit(JobRequest {
                spec,
                priority: JobPriority::Interactive,
                stage: JobStage::Ask(id),
                generation: 0,
                pr_repo: contracts::common::RepoRef {
                    owner: "acme".to_owned(),
                    name: "node-client".to_owned(),
                },
            })
            .await
            .expect("submit");
        self.state
            .db
            .questions()
            .set_job(id, handle.job_id)
            .await
            .expect("link");
        let claim = self.claim().await.expect("the ask job");
        assert_eq!(claim.spec.job_id, handle.job_id);
        (id, claim)
    }

    /// Stores what the follower stores for a ready output that passed the checks.
    async fn provisionally_answered(&self, id: QuestionId, output: Value) {
        let output = serde_json::from_value(output).expect("ask output");
        self.state
            .db
            .questions()
            .update(id, &QuestionUpdate::Provisional(output), Utc::now())
            .await
            .expect("provisional answer");
    }
}

#[tokio::test]
async fn restart_with_the_real_queue_follows_the_running_job_without_resubmitting() {
    let f = fixture(true).await;
    let (id, claim) = f.stopped_on_a_claimed_job().await;
    assert_eq!(resume_unanswered(&f.state).await.expect("resume"), 1);
    f.post_result(
        &claim,
        JobOutcome::Succeeded {
            output: good_answer(),
        },
    )
    .await;
    f.wait_for(id, |q| q.state == AskState::Answered).await;
    let jobs = f
        .state
        .db
        .jobs()
        .list_for_analysis(f.analysis, &[])
        .await
        .expect("jobs");
    assert_eq!(
        jobs.len(),
        1,
        "the running job is followed, not submitted again"
    );
}

#[tokio::test]
async fn restart_after_the_job_result_replaces_a_provisional_answer_with_the_final_one() {
    let f = fixture(true).await;
    let (id, claim) = f.stopped_on_a_claimed_job().await;
    f.provisionally_answered(id, answer("Is the ready answer final?", json!([])))
        .await;
    // The result arrived; the server stopped before the follower applied it.
    f.post_result(
        &claim,
        JobOutcome::Succeeded {
            output: good_answer(),
        },
    )
    .await;
    resume_unanswered(&f.state).await.expect("resume");
    let expected = serde_json::from_value(good_answer()).expect("output");
    let done = f
        .wait_for(id, |q| q.answer.as_ref() == Some(&expected))
        .await;
    assert_eq!(done.state, AskState::Answered);
    let record = f.state.db.questions().get(id, Utc::now()).await.expect("q");
    assert!(record.answered_at.is_some(), "final");
}

#[tokio::test]
async fn restart_after_a_cancelled_result_withdraws_a_provisional_answer() {
    let f = fixture(true).await;
    let (id, claim) = f.stopped_on_a_claimed_job().await;
    f.provisionally_answered(id, good_answer()).await;
    f.post_result(&claim, JobOutcome::Cancelled).await;
    resume_unanswered(&f.state).await.expect("resume");
    let failed = f.wait_for(id, |q| q.state == AskState::Failed).await;
    assert_eq!(failed.answer, None);
    assert_eq!(failed.failure.as_deref(), Some(job::SUPERSEDED));
    let (_, listed) = f.list().await;
    assert_eq!(listed[0]["answer"], Value::Null, "withdrawn from the list");
}

#[tokio::test]
async fn restart_after_the_lease_expired_withdraws_the_provisional_answer_of_the_lost_attempt() {
    let f = fixture(true).await;
    let (id, claim) = f.stopped_on_a_claimed_job().await;
    f.provisionally_answered(id, good_answer()).await;
    // While the server was down the attempt's lease passed: the job is queued again, its ready output dropped.
    crate::jobs::queue::expire_leases(&f.state.db, Utc::now() + TimeDelta::hours(1))
        .await
        .expect("expire");
    assert_eq!(
        f.state
            .db
            .jobs()
            .get(claim.spec.job_id)
            .await
            .expect("job")
            .state,
        JobState::Queued
    );
    resume_unanswered(&f.state).await.expect("resume");
    let withdrawn = f.wait_for(id, |q| q.answer.is_none()).await;
    assert_ne!(withdrawn.state, AskState::Answered, "{withdrawn:?}");
    // The next attempt answers again.
    let claim = f.claim().await.expect("the next attempt");
    f.post_result(
        &claim,
        JobOutcome::Succeeded {
            output: good_answer(),
        },
    )
    .await;
    f.wait_for(id, |q| q.state == AskState::Answered).await;
    let record = f.state.db.questions().get(id, Utc::now()).await.expect("q");
    assert!(record.answered_at.is_some(), "final");
}

#[tokio::test]
async fn restart_shows_again_a_provisional_answer_whose_attempt_still_runs() {
    let f = fixture(true).await;
    let (id, claim) = f.stopped_on_a_claimed_job().await;
    // The runner reported the ready output and the follower stored it; then the server stopped.
    let uri = contracts::routes::RUNNER_OUTPUT
        .path
        .replace("{id}", &claim.spec.job_id.to_string());
    let ready = contracts::job::protocol::OutputReady {
        attempt: claim.spec.attempt,
        output: good_answer(),
    };
    let token = claim.job_token.expose().to_owned();
    let (status, body) = f
        .runner(&uri, &token, serde_json::to_value(&ready).expect("ready"))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    f.provisionally_answered(id, good_answer()).await;
    resume_unanswered(&f.state).await.expect("resume");
    let shown = f
        .wait_for(id, |q| q.state == AskState::Answered && q.answer.is_some())
        .await;
    assert_eq!(
        shown.answer,
        Some(serde_json::from_value(good_answer()).expect("output"))
    );
    let record = f.state.db.questions().get(id, Utc::now()).await.expect("q");
    assert_eq!(
        record.answered_at, None,
        "still provisional before the result"
    );
    f.post_result(
        &claim,
        JobOutcome::Succeeded {
            output: good_answer(),
        },
    )
    .await;
    let deadline = tokio::time::Instant::now() + WAIT;
    while f
        .state
        .db
        .questions()
        .get(id, Utc::now())
        .await
        .expect("q")
        .answered_at
        .is_none()
    {
        assert!(tokio::time::Instant::now() < deadline, "never final");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// --- Acceptance 3: claim order, one ask at a time ----------------------------------------------------------

#[tokio::test]
async fn ask_claimed_before_batch() {
    let f = fixture(true).await;
    let batch = f.batch_job(f.other, "f1").await;
    let id = f.asked("f1.5", QUESTION).await;
    let ask_job = f.queued_job(id).await;
    let claim = f.claim().await.expect("a job");
    assert_eq!(
        claim.spec.job_id, ask_job,
        "the ask job overtakes the older batch job"
    );
    f.post_result(
        &claim,
        JobOutcome::Succeeded {
            output: good_answer(),
        },
    )
    .await;
    f.wait_for(id, |q| q.state == AskState::Answered).await;
    let next = f.claim().await.expect("the batch job");
    assert_eq!(next.spec.job_id, batch);
}

#[tokio::test]
async fn ask_never_preempts_a_running_job_and_questions_of_one_analysis_run_one_at_a_time() {
    let f = fixture(true).await;
    let batch = f.batch_job(f.other, "f1").await;
    let later_batch = f.batch_job(f.other, "f2").await;
    let running = f.claim().await.expect("the batch job");
    assert_eq!(running.spec.job_id, batch);
    // Jobs queue in submission order; each follower submits on its own, so the second is asked once the first
    // job is queued.
    let first = f.asked("f1.5", "first?").await;
    let first_job = f.queued_job(first).await;
    let second = f.asked("f1.5", "second?").await;
    let second_job = f.queued_job(second).await;
    assert_eq!(
        f.claim().await.map(|claim| claim.spec.job_id),
        None,
        "no pre-emption"
    );
    f.post_result(&running, JobOutcome::Cancelled).await;
    let claim = f.claim().await.expect("an ask job");
    assert_eq!(
        claim.spec.job_id, first_job,
        "ask before the next batch job, oldest first"
    );
    assert_eq!(
        f.state.db.jobs().get(second_job).await.expect("job").state,
        JobState::Queued,
        "the second question waits queued"
    );
    f.post_result(
        &claim,
        JobOutcome::Succeeded {
            output: good_answer(),
        },
    )
    .await;
    let claim = f.claim().await.expect("the second ask");
    assert_eq!(claim.spec.job_id, second_job);
    assert!(f.state.db.jobs().get(later_batch).await.expect("job").state == JobState::Queued);
}

/// After a lost session the runner starts a new one with the context pack and the prompt: the stored project
/// profile must be in that input, as data.
#[tokio::test]
async fn ask_after_a_lost_session_still_has_the_project_profile() {
    let f = fixture(true).await;
    let marker = "PROFILE-ONLY-MARKER-7193: the ledger is the risky part";
    f.state
        .db
        .documents()
        .put(
            f.analysis,
            DocumentKind::MemcanProfile,
            "",
            &json!({"memcan": "checked", "text": marker}),
            Utc::now(),
        )
        .await
        .expect("profile lookup");
    let question = f.asked("f1.5", QUESTION).await;
    f.queued_job(question).await;
    let claim = f.claim().await.expect("ask");
    let input = format!(
        "{}\n{}",
        claim.spec.context_pack.as_deref().unwrap_or_default(),
        claim.spec.prompt
    );
    let fenced = input
        .split("<<<DATA ")
        .find(|block| block.contains("project profile: MemCan memory"))
        .unwrap_or_default();
    assert!(fenced.contains(marker), "{input}");
}

#[tokio::test]
async fn ask_resumes_analysis_session_with_a_context_pack_of_the_step() {
    let f = fixture(true).await;
    f.state
        .db
        .analyses()
        .set_session(f.analysis, "thread-482", None)
        .await
        .expect("session");
    let earlier = f.asked("f1.5", "Is the retry bounded?").await;
    f.queued_job(earlier).await;
    let claim = f.claim().await.expect("first ask");
    let earlier_answer = answer("Is max_attempts read from the config?", json!([]));
    f.post_result(
        &claim,
        JobOutcome::Succeeded {
            output: earlier_answer,
        },
    )
    .await;
    f.wait_for(earlier, |q| q.state == AskState::Answered).await;
    let other_step = f.asked("f2.1", "Unrelated?").await;
    f.queued_job(other_step).await;
    let claim = f.claim().await.expect("second ask");
    f.post_result(
        &claim,
        JobOutcome::Succeeded {
            output: good_answer(),
        },
    )
    .await;
    f.wait_for(other_step, |q| q.state == AskState::Answered)
        .await;

    let third = f.asked("f1.5", QUESTION).await;
    f.queued_job(third).await;
    let claim = f.claim().await.expect("third ask");
    let session = claim.spec.session.as_ref().expect("the analysis's session");
    assert_eq!(session.thread_id, "thread-482");
    let pack = claim
        .spec
        .context_pack
        .as_deref()
        .expect("pack for a lost session");
    assert!(pack.contains("asked step f1.5 (full analysis)"), "{pack}");
    assert!(
        pack.contains("Is the retry bounded?"),
        "earlier question of the step"
    );
    assert!(
        pack.contains("Is max_attempts read from the config?"),
        "its answer"
    );
    assert!(
        !pack.contains("Unrelated?"),
        "questions of other steps are left out"
    );
    assert!(claim.spec.access.memcan.is_some(), "MemCan access at claim");
    assert_eq!(
        claim.spec.access.github, None,
        "no App installed in the test config"
    );
}
