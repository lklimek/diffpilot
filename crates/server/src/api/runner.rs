//! Runner protocol: claim, events, output, result (`docs/contracts/runner-protocol.md`). Served only on the runner
//! listener; `claim` needs the runner secret, the job routes need that job's token for its current attempt.

use std::borrow::Cow;

use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::post;
use axum::{Json, Router};
use chrono::Utc;
use contracts::api::error::ApiErrorCode;
use contracts::ids::JobId;
use contracts::job::event::JobEvent;
use contracts::job::protocol::{ClaimRequest, ClaimResponse, EventBatch, EventsAck, OutputReady};
use contracts::job::result::{JobOutcome, JobResult, LlmCallOutcome, LlmCallStatus};
use contracts::job::state::JobState;
use contracts::routes::{RUNNER_CLAIM, RUNNER_EVENTS, RUNNER_OUTPUT, RUNNER_RESULT};
use db::error::DbError;
use db::repo::jobs::{Completion, OutputRecorded};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::jobs::claim::{self, ClaimError, hashes_match, lease_until, token_hash};
use crate::jobs::github_tokens::{self, TokenStoreError};
use crate::pipeline::context::StoredPacks;
use crate::state::AppState;

/// Body caps (threat model T7b, T12).
const CLAIM_BODY_LIMIT: usize = 64 * 1024;
const EVENTS_BODY_LIMIT: usize = 1024 * 1024;
const OUTPUT_BODY_LIMIT: usize = 4 * 1024 * 1024;

/// Longest event summary kept (`runner-protocol.md` Events).
const SUMMARY_MAX_CHARS: usize = 200;

/// Replacement for a known secret value found in runner data.
const REDACTED: &str = "[redacted]";

/// A runner request failed.
#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    /// Missing or wrong runner secret or job token, or the job's lease is over.
    #[error("the runner credentials are missing or not valid")]
    Unauthenticated,
    /// The body is not the expected JSON. Only the category and position are kept: the deserialiser's message
    /// quotes the body, which may hold a secret.
    #[error("the request body is not valid ({kind}, line {line}, column {column})")]
    Body {
        /// Category of the rejection.
        kind: &'static str,
        /// Line of the error, 0 when unknown.
        line: usize,
        /// Column of the error, 0 when unknown.
        column: usize,
    },
    /// The job id in the path is not valid.
    #[error("the job id is not valid")]
    Path(#[from] PathRejection),
    /// The result names another job than the path.
    #[error("the result belongs to another job")]
    JobMismatch,
    /// The claim failed.
    #[error("the claim failed")]
    Claim(#[from] ClaimError),
    /// The queue rejected or failed the request.
    #[error("the job queue rejected the request")]
    Db(#[from] DbError),
    /// The attempt's installation token could not be read for scrubbing.
    #[error("the job's installation token could not be read")]
    InstallationToken(#[from] TokenStoreError),
}

impl ApiErrorSource for RunnerError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Unauthenticated => ApiErrorCode::Unauthenticated,
            Self::Body { .. } | Self::Path(_) | Self::JobMismatch => ApiErrorCode::InvalidRequest,
            Self::Claim(ClaimError::Db(error)) | Self::Db(error) => error.code(),
            Self::Claim(
                ClaimError::GithubAccess(_)
                | ClaimError::TokenRejected(_)
                | ClaimError::GithubTimeout,
            ) => ApiErrorCode::GithubUnavailable,
            Self::Claim(
                ClaimError::Random(_)
                | ClaimError::StoreToken(_)
                | ClaimError::GithubTask(_)
                | ClaimError::ContextPack(_),
            )
            | Self::InstallationToken(_) => ApiErrorCode::Internal,
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Unauthenticated => Cow::Borrowed("The runner credentials are not valid."),
            Self::Body { .. } | Self::Path(_) => Cow::Borrowed("The request is not valid."),
            Self::JobMismatch => Cow::Borrowed("The result belongs to another job."),
            Self::Claim(ClaimError::Db(error)) | Self::Db(error) => error.public_message(),
            Self::Claim(
                ClaimError::GithubAccess(_)
                | ClaimError::TokenRejected(_)
                | ClaimError::GithubTimeout,
            ) => Cow::Borrowed("GitHub did not answer as expected. Try again later."),
            Self::Claim(
                ClaimError::Random(_)
                | ClaimError::StoreToken(_)
                | ClaimError::GithubTask(_)
                | ClaimError::ContextPack(_),
            )
            | Self::InstallationToken(_) => {
                Cow::Borrowed("Something went wrong on the server. Try again later.")
            }
        }
    }
}

impl From<JsonRejection> for RunnerError {
    fn from(rejection: JsonRejection) -> Self {
        let kind = match &rejection {
            JsonRejection::JsonDataError(_) => "unexpected shape",
            JsonRejection::JsonSyntaxError(_) => "syntax error",
            JsonRejection::MissingJsonContentType(_) => "missing JSON content type",
            JsonRejection::BytesRejection(_) => "unreadable or too large",
            _ => "rejected",
        };
        let (line, column) = json_position(&rejection.body_text());
        Self::Body { kind, line, column }
    }
}

/// Line and column from serde_json's trailing "at line L column C"; (0, 0) when absent.
fn json_position(text: &str) -> (usize, usize) {
    text.rsplit_once(" at line ")
        .and_then(|(_, rest)| {
            let (line, column) = rest.split_once(" column ")?;
            Some((line.trim().parse().ok()?, column.trim().parse().ok()?))
        })
        .unwrap_or((0, 0))
}

type RunnerResult<T> = Result<T, ApiFailure<RunnerError>>;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            RUNNER_CLAIM.path,
            post(claim_job).layer(DefaultBodyLimit::max(CLAIM_BODY_LIMIT)),
        )
        .route(
            RUNNER_EVENTS.path,
            post(post_events).layer(DefaultBodyLimit::max(EVENTS_BODY_LIMIT)),
        )
        .route(
            RUNNER_OUTPUT.path,
            post(post_output).layer(DefaultBodyLimit::max(OUTPUT_BODY_LIMIT)),
        )
        .route(
            RUNNER_RESULT.path,
            post(post_result).layer(DefaultBodyLimit::max(OUTPUT_BODY_LIMIT)),
        )
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn authorize_runner(state: &AppState, headers: &HeaderMap) -> Result<(), RunnerError> {
    let presented = bearer(headers).ok_or(RunnerError::Unauthenticated)?;
    let expected = token_hash(state.config.runner_secret.expose_secret());
    if hashes_match(&expected, &token_hash(presented)) {
        Ok(())
    } else {
        Err(RunnerError::Unauthenticated)
    }
}

/// A request authorised by a job token, bound to the attempt that token was issued for.
#[derive(Debug)]
struct Authorized {
    job: JobId,
    attempt: u32,
    cancel_requested: bool,
    token: String,
}

impl Authorized {
    /// A body may only speak for the attempt the token was issued for; the write re-checks it in its transaction.
    fn bind(&self, attempt: u32) -> Result<(), RunnerError> {
        if attempt == self.attempt {
            Ok(())
        } else {
            Err(RunnerError::Db(DbError::Conflict))
        }
    }
}

/// Checks the job token against the current claim, read together with its attempt and lease.
async fn authorize_job(
    state: &AppState,
    headers: &HeaderMap,
    job: JobId,
) -> Result<Authorized, RunnerError> {
    let presented = bearer(headers).ok_or(RunnerError::Unauthenticated)?;
    let claim = state.db.jobs().authorization(job).await?;
    let stored = claim.token_hash.ok_or(RunnerError::Unauthenticated)?;
    if !hashes_match(&stored, &token_hash(presented)) {
        return Err(RunnerError::Unauthenticated);
    }
    let lease_over = claim
        .lease_expires_at
        .is_some_and(|lease| lease < Utc::now());
    if lease_over && matches!(claim.state, JobState::Claimed | JobState::Running) {
        return Err(RunnerError::Unauthenticated);
    }
    Ok(Authorized {
        job,
        attempt: claim.attempt,
        cancel_requested: claim.cancel_requested,
        token: presented.to_owned(),
    })
}

/// The attempt's installation token, which the server also knows and scrubs (T3b, T10).
async fn installation_token(
    state: &AppState,
    auth: &Authorized,
) -> Result<Option<SecretString>, RunnerError> {
    Ok(github_tokens::load(&state.db, &state.config, auth.job, auth.attempt).await?)
}

/// Known secret values that must never be stored from runner data (threat model T10).
fn known_secrets<'a>(
    state: &'a AppState,
    job_token: &'a str,
    installation: Option<&'a SecretString>,
) -> [&'a str; 5] {
    [
        job_token,
        installation.map_or("", |token| token.expose_secret()),
        state.config.runner_secret.expose_secret(),
        state.config.memcan.token.expose_secret(),
        state.config.github.client_secret.expose_secret(),
    ]
}

fn scrub(text: &str, secrets: &[&str]) -> String {
    secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(text.to_owned(), |text, secret| {
            text.replace(secret, REDACTED)
        })
}

/// Scrubs strings and object keys; a key that collides after scrubbing gets the first free `#<n>` suffix, so no
/// value is lost.
fn scrub_value(value: &mut Value, secrets: &[&str]) {
    match value {
        Value::String(text) => *text = scrub(text, secrets),
        Value::Array(items) => items.iter_mut().for_each(|item| scrub_value(item, secrets)),
        Value::Object(map) => {
            for (key, mut item) in std::mem::take(map) {
                scrub_value(&mut item, secrets);
                let key = scrub(&key, secrets);
                let key = if map.contains_key(&key) {
                    (2usize..)
                        .map(|n| format!("{key}#{n}"))
                        .find(|candidate| !map.contains_key(candidate))
                        .unwrap_or(key)
                } else {
                    key
                };
                map.insert(key, item);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// Scrubs every text field of a result before it is stored (also the LLM status copied to the credential).
fn scrub_result(result: &mut JobResult, secrets: &[&str]) {
    let JobResult {
        job_id: _,
        attempt: _,
        outcome,
        session,
        usage: _,
        skills_commit,
        llm_call,
    } = result;
    match outcome {
        JobOutcome::Succeeded { output } => scrub_value(output, secrets),
        JobOutcome::Failed { kind: _, message } => *message = scrub(message, secrets),
        JobOutcome::Cancelled | JobOutcome::UsageLimited { resets_at: _ } => {}
    }
    if let Some(session) = session {
        session.thread_id = scrub(&session.thread_id, secrets);
    }
    if let Some(commit) = skills_commit {
        *commit = scrub(commit, secrets);
    }
    if let Some(LlmCallOutcome {
        at: _,
        status: LlmCallStatus::NotWorking { message },
    }) = llm_call
    {
        *message = scrub(message, secrets);
    }
}

fn scrub_event(mut event: JobEvent, secrets: &[&str]) -> JobEvent {
    event.summary = scrub(&event.summary, secrets)
        .chars()
        .take(SUMMARY_MAX_CHARS)
        .collect();
    event
}

async fn claim_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<ClaimRequest>, JsonRejection>,
) -> RunnerResult<Json<ClaimResponse>> {
    authorize_runner(&state, &headers)?;
    let Json(request) = body.map_err(RunnerError::from)?;
    let packs = StoredPacks {
        db: state.db.clone(),
    };
    let response = claim::claim(&state.db, &state.config, &packs, &request, Utc::now())
        .await
        .map_err(RunnerError::from)?;
    if let Some(claimed) = &response.claim {
        let analysis = claimed.spec.analysis_id;
        // The claim is done; a failed publish only delays the new stage until the next event or reconnect.
        if let Err(error) = state.progress.publish_analysis(&state.db, analysis).await {
            tracing::warn!(%analysis, ?error, "the stage of a claimed job could not be published to the progress stream");
        }
    }
    Ok(Json(response))
}

async fn post_events(
    State(state): State<AppState>,
    path: Result<Path<JobId>, PathRejection>,
    headers: HeaderMap,
    body: Result<Json<EventBatch>, JsonRejection>,
) -> RunnerResult<Json<EventsAck>> {
    let Path(job) = path.map_err(RunnerError::from)?;
    let auth = authorize_job(&state, &headers, job).await?;
    let Json(batch) = body.map_err(RunnerError::from)?;
    Ok(Json(apply_events(&state, &auth, batch).await?))
}

async fn apply_events(
    state: &AppState,
    auth: &Authorized,
    batch: EventBatch,
) -> Result<EventsAck, RunnerError> {
    auth.bind(batch.attempt)?;
    let installation = installation_token(state, auth).await?;
    let secrets = known_secrets(state, &auth.token, installation.as_ref());
    let events: Vec<JobEvent> = batch
        .events
        .into_iter()
        .map(|event| scrub_event(event, &secrets))
        .collect();
    let lease_expires_at = lease_until(Utc::now());
    let recorded = state
        .db
        .jobs()
        .record_events(auth.job, auth.attempt, &events, lease_expires_at)
        .await?;
    Ok(EventsAck {
        cancel_requested: recorded.cancel_requested,
        lease_expires_at,
    })
}

async fn post_output(
    State(state): State<AppState>,
    path: Result<Path<JobId>, PathRejection>,
    headers: HeaderMap,
    body: Result<Json<OutputReady>, JsonRejection>,
) -> RunnerResult<Json<EventsAck>> {
    let Path(job) = path.map_err(RunnerError::from)?;
    let auth = authorize_job(&state, &headers, job).await?;
    let Json(ready) = body.map_err(RunnerError::from)?;
    Ok(Json(apply_output(&state, &auth, ready).await?))
}

async fn apply_output(
    state: &AppState,
    auth: &Authorized,
    mut ready: OutputReady,
) -> Result<EventsAck, RunnerError> {
    auth.bind(ready.attempt)?;
    let installation = installation_token(state, auth).await?;
    scrub_value(
        &mut ready.output,
        &known_secrets(state, &auth.token, installation.as_ref()),
    );
    let lease_expires_at = lease_until(Utc::now());
    let recorded = state
        .db
        .jobs()
        .record_output(auth.job, auth.attempt, &ready.output, lease_expires_at)
        .await?;
    let cancel_requested = match recorded {
        OutputRecorded::Stored { cancel_requested } => cancel_requested,
        OutputRecorded::AlreadyStored => auth.cancel_requested,
    };
    Ok(EventsAck {
        cancel_requested,
        lease_expires_at,
    })
}

async fn post_result(
    State(state): State<AppState>,
    path: Result<Path<JobId>, PathRejection>,
    headers: HeaderMap,
    body: Result<Json<JobResult>, JsonRejection>,
) -> RunnerResult<StatusCode> {
    let Path(job) = path.map_err(RunnerError::from)?;
    let auth = authorize_job(&state, &headers, job).await?;
    let Json(result) = body.map_err(RunnerError::from)?;
    apply_result(&state, &auth, result).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn apply_result(
    state: &AppState,
    auth: &Authorized,
    mut result: JobResult,
) -> Result<(), RunnerError> {
    if result.job_id != auth.job {
        return Err(RunnerError::JobMismatch);
    }
    auth.bind(result.attempt)?;
    let installation = installation_token(state, auth).await?;
    scrub_result(
        &mut result,
        &known_secrets(state, &auth.token, installation.as_ref()),
    );
    let completion = state.db.jobs().complete(&result, Utc::now()).await?;
    if let Completion::Applied { state: job_state } = completion {
        tracing::info!(
            job = %auth.job,
            attempt = result.attempt,
            state = job_state.as_str(),
            "job result recorded"
        );
        // The provider's raw text (already scrubbed) is for operators: users see a plain sentence by its kind.
        if let Some(LlmCallOutcome {
            status: LlmCallStatus::NotWorking { message },
            ..
        }) = &result.llm_call
        {
            tracing::warn!(job = %auth.job, attempt = result.attempt, %message, "LLM call failed");
        }
        github_tokens::nudge();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use chrono::{DateTime, TimeDelta};
    use contracts::agent::JobKind;
    use contracts::api::analyses::{AnalysisState, StageFlow, StageKind};
    use contracts::api::events::AnalysisEvent;
    use contracts::ids::{AnalysisId, FlowId};
    use contracts::job::access::AgentAccess;
    use contracts::job::event::ActivityKind;
    use contracts::job::protocol::{CredentialStatus, LeaseTiming};
    use contracts::job::result::{JobFailureKind, SessionOutcome, SessionReport};
    use contracts::job::spec::{CredentialKind, JobPriority, LlmCredentialRef};
    use db::repo::analyses::SessionState;
    use db::repo::analyses::testing::{CREDENTIAL, job_spec, seed_analysis};
    use db::repo::documents::DocumentKind;
    use db::repo::jobs::{JobStage, NewJob};
    use http_body_util::BodyExt;
    use serde::Serialize;
    use serde_json::json;
    use tower::ServiceExt;

    use futures::StreamExt;

    use super::*;
    use crate::api::runner_router;
    use crate::jobs::executor::JobRequest;
    use crate::state::tests::test_state;
    use crate::test_support::logs::LogCapture;

    struct Harness {
        state: AppState,
        app: Router,
    }

    struct Claimed {
        job: JobId,
        token: String,
        lease_expires_at: DateTime<Utc>,
        spec_access: AgentAccess,
    }

    impl Harness {
        async fn new() -> Self {
            let state = test_state().await;
            Self {
                app: runner_router(state.clone()),
                state,
            }
        }

        fn secret(&self) -> String {
            self.state.config.runner_secret.expose_secret().to_owned()
        }

        async fn enqueue(&self, credential: &str) -> (AnalysisId, JobId) {
            let analysis = seed_analysis(&self.state.db).await.expect("seed");
            let mut spec = job_spec(analysis, JobKind::AnalyseFlow);
            spec.credential.id = credential.to_owned();
            let job = self
                .state
                .db
                .jobs()
                .enqueue(
                    NewJob {
                        spec,
                        priority: JobPriority::Batch,
                        stage: JobStage::Flow(FlowId("f1".to_owned())),
                        generation: 0,
                    },
                    Utc::now(),
                )
                .await
                .expect("enqueue");
            (analysis, job.id)
        }

        async fn send(
            &self,
            path: &str,
            bearer: Option<&str>,
            body: impl Serialize,
        ) -> (StatusCode, Value) {
            self.send_raw(path, bearer, serde_json::to_vec(&body).expect("json"))
                .await
        }

        async fn send_raw(
            &self,
            path: &str,
            bearer: Option<&str>,
            body: Vec<u8>,
        ) -> (StatusCode, Value) {
            let mut request = Request::post(path).header(header::CONTENT_TYPE, "application/json");
            if let Some(bearer) = bearer {
                request = request.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
            }
            let response = self
                .app
                .clone()
                .oneshot(request.body(Body::from(body)).expect("request"))
                .await
                .expect("infallible");
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

        async fn claim(&self, credential: &str) -> Claimed {
            let (status, body) = self
                .send(
                    RUNNER_CLAIM.path,
                    Some(&self.secret()),
                    claim_request(credential),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let response: ClaimResponse = serde_json::from_value(body).expect("claim response");
            let claim = response.claim.expect("a claimed job");
            Claimed {
                job: claim.spec.job_id,
                token: claim.job_token.expose().to_owned(),
                lease_expires_at: claim.lease_expires_at,
                spec_access: claim.spec.access,
            }
        }
    }

    fn claim_request(credential: &str) -> ClaimRequest {
        ClaimRequest {
            runner_id: "runner-1".to_owned(),
            credentials: vec![CredentialStatus {
                credential: LlmCredentialRef {
                    id: credential.to_owned(),
                    kind: CredentialKind::CodexChatgpt,
                },
                usable: true,
            }],
            dropped_sessions: Vec::new(),
        }
    }

    fn path(route: &str, job: JobId) -> String {
        route.replace("{id}", &job.to_string())
    }

    fn batch(attempt: u32, summary: &str) -> EventBatch {
        EventBatch {
            attempt,
            events: vec![JobEvent {
                seq: 1,
                at: Utc::now(),
                activity: ActivityKind::Command,
                summary: summary.to_owned(),
            }],
        }
    }

    fn succeeded(job: JobId, attempt: u32) -> JobResult {
        JobResult {
            job_id: job,
            attempt,
            outcome: JobOutcome::Succeeded {
                output: json!({"flows": []}),
            },
            session: None,
            usage: None,
            skills_commit: None,
            llm_call: None,
        }
    }

    #[tokio::test]
    async fn runner_routes_require_secret_or_own_job_token() {
        let harness = Harness::new().await;
        harness.enqueue(CREDENTIAL).await;
        harness.enqueue("second").await;
        let request = claim_request(CREDENTIAL);
        for bearer in [None, Some("wrong-secret"), Some("")] {
            let (status, body) = harness.send(RUNNER_CLAIM.path, bearer, &request).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{bearer:?}");
            assert_eq!(body["code"], json!("unauthenticated"));
        }
        let own = harness.claim(CREDENTIAL).await;
        let other = harness.claim("second").await;

        let routes: [(&str, Value); 3] = [
            (RUNNER_EVENTS.path, json!(batch(1, "x"))),
            (
                RUNNER_OUTPUT.path,
                json!(OutputReady {
                    attempt: 1,
                    output: json!({})
                }),
            ),
            (RUNNER_RESULT.path, json!(succeeded(own.job, 1))),
        ];
        for (route, body) in &routes {
            let target = path(route, own.job);
            let secret = harness.secret();
            for bearer in [
                None,
                Some("wrong-token"),
                Some(other.token.as_str()),
                Some(secret.as_str()),
            ] {
                let (status, _) = harness.send(&target, bearer, body).await;
                assert_eq!(status, StatusCode::UNAUTHORIZED, "{route} with {bearer:?}");
            }
        }
        for (route, body) in routes {
            let (status, reply) = harness
                .send(&path(route, own.job), Some(&own.token), body)
                .await;
            assert!(status.is_success(), "{route}: {status} {reply}");
        }
        let (status, _) = harness
            .send(
                &path(RUNNER_EVENTS.path, JobId::new_random()),
                Some(&own.token),
                batch(1, "x"),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// A claim publishes the analysis with its new stage; the stage starts at the claim and a reconnect
    /// snapshot reads the same.
    #[tokio::test]
    async fn claim_publishes_the_stage_it_starts() {
        let harness = Harness::new().await;
        let (analysis, _) = harness.enqueue(CREDENTIAL).await;
        let db = &harness.state.db;
        let plan: Value =
            serde_json::from_str(include_str!("../../../../fixtures/pr-482/agent/plan.json"))
                .expect("fixture plan");
        db.documents()
            .put(analysis, DocumentKind::Plan, "", &plan, Utc::now())
            .await
            .expect("plan");
        db.analyses()
            .set_state(
                analysis,
                &AnalysisState::Running {
                    done: 0,
                    total: 5,
                    stage: None,
                },
                Utc::now(),
            )
            .await
            .expect("running");
        let mut live = harness.state.progress.subscribe(analysis);
        let before = Utc::now();
        harness.claim(CREDENTIAL).await;
        let after = Utc::now();
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), live.recv())
            .await
            .expect("an event on claim")
            .expect("event");
        let AnalysisEvent::Analysis { summary } = event else {
            panic!("an analysis event: {event:?}");
        };
        let AnalysisState::Running {
            stage: Some(stage), ..
        } = &summary.state
        else {
            panic!("a running stage: {:?}", summary.state);
        };
        assert_eq!(stage.kind, StageKind::Flow);
        assert_eq!(
            stage.flow,
            Some(StageFlow {
                index: 1,
                title: "Send an RPC request with retry".to_owned(),
            })
        );
        let started = stage.started_at.expect("started");
        assert!(before <= started && started <= after, "{started}");
        let snapshot = db.analyses().summary(analysis).await.expect("summary");
        assert_eq!(
            snapshot, summary,
            "a reconnect snapshot shows the same stage"
        );
    }

    /// Submits a job like the driver does and follows its updates in the background.
    async fn submit_followed(
        harness: &Harness,
        analysis: AnalysisId,
        stage: JobStage,
        kind: JobKind,
    ) {
        let request = JobRequest {
            spec: job_spec(analysis, kind),
            priority: JobPriority::Batch,
            stage,
            generation: 0,
            pr_repo: contracts::common::RepoRef {
                owner: "acme".to_owned(),
                name: "node-client".to_owned(),
            },
        };
        let handle = harness
            .state
            .executor
            .submit(request)
            .await
            .expect("submit");
        let mut updates = harness.state.executor.events(&handle);
        tokio::spawn(async move { while updates.next().await.is_some() {} });
    }

    /// Waits for an analysis event whose stage (JSON, `null` when absent) passes `wanted`; returns its summary.
    async fn stage_where(
        live: &mut tokio::sync::broadcast::Receiver<AnalysisEvent>,
        wanted: impl Fn(&Value) -> bool,
    ) -> contracts::api::analyses::AnalysisSummary {
        let mut seen = Vec::new();
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(5), live.recv())
                .await
                .unwrap_or_else(|_| panic!("the stage was not published; seen {seen:?}"))
                .expect("event");
            if let AnalysisEvent::Analysis { summary } = event {
                let stage = serde_json::to_value(&summary.state).expect("json")["stage"].clone();
                if wanted(&stage) {
                    return summary;
                }
                seen.push(stage);
            }
        }
    }

    /// Waits for an analysis event whose stage is `expected`.
    async fn stage_event(
        live: &mut tokio::sync::broadcast::Receiver<AnalysisEvent>,
        expected: Value,
    ) -> contracts::api::analyses::AnalysisSummary {
        stage_where(live, |stage| *stage == expected).await
    }

    /// The stream follows every stage change, not only claims: a job's end, the next submit (waiting for the
    /// runner) and a lease expiry without a new claim; the last event equals the REST read.
    #[tokio::test]
    async fn stage_changes_are_published_when_jobs_end_are_submitted_or_expire() {
        let harness = Harness::new().await;
        let db = &harness.state.db;
        let analysis = seed_analysis(db).await.expect("seed");
        let plan: Value =
            serde_json::from_str(include_str!("../../../../fixtures/pr-482/agent/plan.json"))
                .expect("fixture plan");
        db.documents()
            .put(analysis, DocumentKind::Plan, "", &plan, Utc::now())
            .await
            .expect("plan");
        db.analyses()
            .set_state(
                analysis,
                &AnalysisState::Running {
                    done: 0,
                    total: 5,
                    stage: None,
                },
                Utc::now(),
            )
            .await
            .expect("running");
        let mut live = harness.state.progress.subscribe(analysis);
        submit_followed(&harness, analysis, JobStage::PrContext, JobKind::PrContext).await;
        stage_event(&mut live, json!({"kind": "pr_context", "started_at": null})).await;
        let claimed = harness.claim(CREDENTIAL).await;
        let target = path(RUNNER_RESULT.path, claimed.job);
        let (status, body) = harness
            .send(&target, Some(&claimed.token), succeeded(claimed.job, 1))
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        stage_event(&mut live, Value::Null).await;
        let flow_1 = JobStage::Flow(FlowId("f1".to_owned()));
        submit_followed(&harness, analysis, flow_1, JobKind::AnalyseFlow).await;
        let waiting = json!({"kind": "flow", "flow": {"index": 1, "title": "Send an RPC request with retry"},
            "started_at": null});
        stage_event(&mut live, waiting.clone()).await;
        // A claim whose lease ends at once: the follower requeues the job, no new claim follows.
        let now = Utc::now();
        db.jobs()
            .claim(db::repo::jobs::ClaimParams {
                credential_ids: &[CREDENTIAL.to_owned()],
                job_token_hash: b"hash",
                lease_until: now + TimeDelta::milliseconds(600),
                now,
            })
            .await
            .expect("claim")
            .expect("claimed");
        stage_where(&mut live, |stage| {
            stage["kind"] == "flow" && stage["started_at"].is_string()
        })
        .await;
        let last = stage_event(&mut live, waiting).await;
        assert_eq!(
            last,
            db.analyses().summary(analysis).await.expect("summary"),
            "the last event equals the REST read"
        );
    }

    #[tokio::test]
    async fn expired_token_is_rejected() {
        let harness = Harness::new().await;
        harness.enqueue(CREDENTIAL).await;
        let claimed = harness.claim(CREDENTIAL).await;
        // The lease passes without a heartbeat; the token is refused even before the sweep requeues the job.
        harness
            .state
            .db
            .jobs()
            .record_events(claimed.job, 1, &[], Utc::now() - TimeDelta::seconds(1))
            .await
            .expect("lease in the past");
        let target = path(RUNNER_EVENTS.path, claimed.job);
        let (status, _) = harness
            .send(&target, Some(&claimed.token), batch(1, "x"))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // The next claim's sweep requeues the job and clears the token for good.
        let reclaimed = harness.claim(CREDENTIAL).await;
        assert_eq!(reclaimed.job, claimed.job);
        let (status, _) = harness
            .send(&target, Some(&claimed.token), batch(1, "x"))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, body) = harness
            .send(&target, Some(&reclaimed.token), batch(2, "x"))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    #[tokio::test]
    async fn stale_attempt_is_a_conflict() {
        let harness = Harness::new().await;
        harness.enqueue(CREDENTIAL).await;
        let claimed = harness.claim(CREDENTIAL).await;
        let (status, body) = harness
            .send(
                &path(RUNNER_EVENTS.path, claimed.job),
                Some(&claimed.token),
                batch(2, "x"),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["code"], json!("conflict"));
        let (status, _) = harness
            .send(
                &path(RUNNER_RESULT.path, claimed.job),
                Some(&claimed.token),
                succeeded(claimed.job, 0),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn output_rejected_for_stale_attempt() {
        let harness = Harness::new().await;
        harness.enqueue(CREDENTIAL).await;
        let claimed = harness.claim(CREDENTIAL).await;
        let target = path(RUNNER_OUTPUT.path, claimed.job);
        for attempt in [0, 2] {
            let (status, _) = harness
                .send(
                    &target,
                    Some(&claimed.token),
                    OutputReady {
                        attempt,
                        output: json!({"a": 1}),
                    },
                )
                .await;
            assert_eq!(status, StatusCode::CONFLICT, "attempt {attempt}");
        }
        let job = harness.state.db.jobs().get(claimed.job).await.expect("get");
        assert_eq!(job.ready_output, None);
        let (status, ack) = harness
            .send(
                &target,
                Some(&claimed.token),
                OutputReady {
                    attempt: 1,
                    output: json!({"a": 1}),
                },
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ack["cancel_requested"], json!(false));
        let (status, _) = harness
            .send(
                &target,
                Some(&claimed.token),
                OutputReady {
                    attempt: 1,
                    output: json!({"a": 2}),
                },
            )
            .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "a different output for the same attempt"
        );
    }

    #[tokio::test]
    async fn result_posted_twice_is_stored_once() {
        let harness = Harness::new().await;
        harness.enqueue(CREDENTIAL).await;
        let claimed = harness.claim(CREDENTIAL).await;
        let target = path(RUNNER_RESULT.path, claimed.job);
        let result = succeeded(claimed.job, 1);
        for _ in 0..2 {
            let (status, body) = harness.send(&target, Some(&claimed.token), &result).await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        }
        let job = harness.state.db.jobs().get(claimed.job).await.expect("get");
        assert_eq!(job.state, JobState::Succeeded);
        assert_eq!(job.result, Some(result));
        let (status, _) = harness
            .send(
                &target,
                Some(&claimed.token),
                succeeded(JobId::new_random(), 1),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "result for another job");
    }

    #[tokio::test]
    async fn claim_uses_lease_timing_v1_and_sends_memcan_without_github_when_not_installed() {
        let harness = Harness::new().await;
        let (_, job) = harness.enqueue(CREDENTIAL).await;
        let before = Utc::now();
        let claimed = harness.claim(CREDENTIAL).await;
        let after = Utc::now();
        let lease = TimeDelta::seconds(i64::from(LeaseTiming::V1.lease_secs));
        assert!(claimed.lease_expires_at >= before + lease - TimeDelta::milliseconds(1));
        assert!(claimed.lease_expires_at <= after + lease);
        assert_eq!(claimed.job, job);
        assert!(claimed.spec_access.github.is_none(), "no App installation");
        let memcan = claimed.spec_access.memcan.expect("MemCan endpoint");
        assert_eq!(memcan.url, harness.state.config.memcan.url.to_string());
        let stored = harness.state.db.jobs().token_hash(job).await.expect("hash");
        assert_eq!(
            stored,
            Some(token_hash(&claimed.token)),
            "only the hash is stored"
        );
        let (status, body) = harness
            .send(
                RUNNER_CLAIM.path,
                Some(&harness.secret()),
                claim_request(CREDENTIAL),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["claim"],
            Value::Null,
            "one job per credential at a time"
        );
    }

    #[tokio::test]
    async fn result_and_output_size_capped() {
        let harness = Harness::new().await;
        harness.enqueue(CREDENTIAL).await;
        let claimed = harness.claim(CREDENTIAL).await;
        let huge = "x".repeat(OUTPUT_BODY_LIMIT);
        let output = serde_json::to_vec(&OutputReady {
            attempt: 1,
            output: json!({"text": huge}),
        })
        .expect("json");
        let (status, _) = harness
            .send_raw(
                &path(RUNNER_OUTPUT.path, claimed.job),
                Some(&claimed.token),
                output,
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let mut result = succeeded(claimed.job, 1);
        result.outcome = JobOutcome::Succeeded {
            output: json!({"text": huge}),
        };
        let (status, _) = harness
            .send(
                &path(RUNNER_RESULT.path, claimed.job),
                Some(&claimed.token),
                result,
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let job = harness.state.db.jobs().get(claimed.job).await.expect("get");
        assert_eq!((job.ready_output, job.result), (None, None));
    }

    #[tokio::test]
    async fn result_records_the_last_llm_call_and_usage_limit_pauses_the_credential() {
        let harness = Harness::new().await;
        harness.enqueue(CREDENTIAL).await;
        let claimed = harness.claim(CREDENTIAL).await;
        let call = LlmCallOutcome {
            at: Utc::now(),
            status: LlmCallStatus::NotWorking {
                message: "You have hit your usage limit.".to_owned(),
            },
        };
        let resets_at = Utc::now() + TimeDelta::hours(2);
        let mut result = succeeded(claimed.job, 1);
        result.outcome = JobOutcome::UsageLimited {
            resets_at: Some(resets_at),
        };
        result.llm_call = Some(call.clone());
        let (status, _) = harness
            .send(
                &path(RUNNER_RESULT.path, claimed.job),
                Some(&claimed.token),
                result,
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let credentials = harness.state.db.llm_credentials();
        let last = credentials.last_call("default").await.expect("last call");
        assert_eq!(last.map(|outcome| outcome.status), Some(call.status));
        let credential = credentials.get(CREDENTIAL).await.expect("credential");
        assert!(
            credential
                .paused_until
                .is_some_and(|until| until > Utc::now())
        );
        let job = harness.state.db.jobs().get(claimed.job).await.expect("get");
        assert_eq!(job.state, JobState::Paused);
    }

    #[tokio::test]
    async fn server_scrubs_known_secrets_from_events_outputs_and_results() {
        let harness = Harness::new().await;
        harness.enqueue(CREDENTIAL).await;
        let claimed = harness.claim(CREDENTIAL).await;
        let memcan = harness.state.config.memcan.token.expose_secret().to_owned();
        let leaked = format!(
            "token {} memcan {memcan} {}",
            claimed.token,
            "y".repeat(300)
        );
        let (status, _) = harness
            .send(
                &path(RUNNER_EVENTS.path, claimed.job),
                Some(&claimed.token),
                batch(1, &leaked),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = harness
            .send(
                &path(RUNNER_OUTPUT.path, claimed.job),
                Some(&claimed.token),
                OutputReady {
                    attempt: 1,
                    output: json!({"answer": [leaked.clone()]}),
                },
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let mut result = succeeded(claimed.job, 1);
        result.outcome = JobOutcome::Failed {
            kind: JobFailureKind::ProviderFailed,
            message: leaked.clone(),
        };
        let (status, _) = harness
            .send(
                &path(RUNNER_RESULT.path, claimed.job),
                Some(&claimed.token),
                result,
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let events = harness
            .state
            .db
            .jobs()
            .events(claimed.job, 0)
            .await
            .expect("events");
        let job = harness.state.db.jobs().get(claimed.job).await.expect("get");
        let stored = format!("{events:?} {:?} {:?}", job.ready_output, job.result);
        for secret in [claimed.token.as_str(), memcan.as_str()] {
            assert!(!stored.contains(secret), "secret stored: {stored}");
        }
        assert!(stored.contains(REDACTED));
        assert_eq!(events[0].summary.chars().count(), SUMMARY_MAX_CHARS);
    }

    /// The attempt's installation token, which the server holds encrypted, is masked in events, output keys and
    /// values, and result text (T3b, T10).
    #[tokio::test]
    async fn server_scrubs_installation_token_from_events_outputs_and_results() {
        use crate::github::app_token::tests::{
            MINTED_TOKEN, github_config, minted, mount_installation, mount_mint, write_key,
        };

        let dir = tempfile::tempdir().expect("temp dir");
        let github = wiremock::MockServer::start().await;
        mount_installation(&github).await;
        mount_mint(&github, minted(Utc::now()), 1).await;
        let mut state = test_state().await;
        let mut config = crate::config::tests::test_config();
        config.github = github_config(&github, write_key(dir.path()));
        state.config = std::sync::Arc::new(config);
        let harness = Harness {
            app: runner_router(state.clone()),
            state,
        };
        harness.enqueue(CREDENTIAL).await;
        let claimed = harness.claim(CREDENTIAL).await;
        assert!(claimed.spec_access.github.is_some(), "a token was minted");
        let leaked = format!("gh {MINTED_TOKEN} end");
        let (status, _) = harness
            .send(
                &path(RUNNER_EVENTS.path, claimed.job),
                Some(&claimed.token),
                batch(1, &leaked),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = harness
            .send(
                &path(RUNNER_OUTPUT.path, claimed.job),
                Some(&claimed.token),
                OutputReady {
                    attempt: 1,
                    output: json!({ MINTED_TOKEN: [leaked.clone()] }),
                },
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let mut result = succeeded(claimed.job, 1);
        result.outcome = JobOutcome::Failed {
            kind: JobFailureKind::ProviderFailed,
            message: leaked.clone(),
        };
        let (status, _) = harness
            .send(
                &path(RUNNER_RESULT.path, claimed.job),
                Some(&claimed.token),
                result,
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let events = harness
            .state
            .db
            .jobs()
            .events(claimed.job, 0)
            .await
            .expect("events");
        let job = harness.state.db.jobs().get(claimed.job).await.expect("get");
        for (place, stored) in [
            ("event", format!("{events:?}")),
            ("output", format!("{:?}", job.ready_output)),
            ("result", format!("{:?}", job.result)),
        ] {
            assert!(!stored.contains(MINTED_TOKEN), "{place}: {stored}");
            assert!(stored.contains(REDACTED), "{place}: {stored}");
        }
    }

    #[tokio::test]
    async fn server_scrubs_secrets_from_result_metadata_keys_and_llm_status() {
        let harness = Harness::new().await;
        harness.enqueue(CREDENTIAL).await;
        let claimed = harness.claim(CREDENTIAL).await;
        let memcan = harness.state.config.memcan.token.expose_secret().to_owned();
        let token = claimed.token.clone();
        let (status, _) = harness
            .send(
                &path(RUNNER_OUTPUT.path, claimed.job),
                Some(&token),
                OutputReady {
                    attempt: 1,
                    output: json!({ token.clone(): {format!("k{memcan}"): 1} }),
                },
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let mut result = succeeded(claimed.job, 1);
        result.outcome = JobOutcome::Succeeded {
            output: json!({ token.clone(): [format!("v{memcan}")] }),
        };
        result.session = Some(SessionReport {
            thread_id: format!("thread-{token}"),
            outcome: SessionOutcome::Started,
        });
        result.skills_commit = Some(memcan.clone());
        result.llm_call = Some(LlmCallOutcome {
            at: Utc::now(),
            status: LlmCallStatus::NotWorking {
                message: format!("refresh failed for {token}"),
            },
        });
        let (status, _) = harness
            .send(&path(RUNNER_RESULT.path, claimed.job), Some(&token), result)
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let job = harness.state.db.jobs().get(claimed.job).await.expect("get");
        let credential = harness
            .state
            .db
            .llm_credentials()
            .get(CREDENTIAL)
            .await
            .expect("credential");
        let stored = format!(
            "{:?} {:?} {:?}",
            job.ready_output, job.result, credential.last_call
        );
        for secret in [token.as_str(), memcan.as_str()] {
            assert!(!stored.contains(secret), "secret stored: {stored}");
        }
        assert!(credential.last_call.is_some(), "the status row was written");
    }

    #[tokio::test]
    async fn result_records_the_session_of_the_analysis() {
        let harness = Harness::new().await;
        let (analysis, _) = harness.enqueue(CREDENTIAL).await;
        let claimed = harness.claim(CREDENTIAL).await;
        let mut result = succeeded(claimed.job, 1);
        result.session = Some(SessionReport {
            thread_id: "thread-1".to_owned(),
            outcome: SessionOutcome::Started,
        });
        result.skills_commit = Some("abc".to_owned());
        let (status, _) = harness
            .send(
                &path(RUNNER_RESULT.path, claimed.job),
                Some(&claimed.token),
                result,
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let record = harness
            .state
            .db
            .analyses()
            .get(analysis)
            .await
            .expect("analysis");
        assert_eq!(record.agent_thread_id.as_deref(), Some("thread-1"));
        assert_eq!(record.session_state, SessionState::Active);
        assert_eq!(record.skills_commit.as_deref(), Some("abc"));
    }

    /// A repeated result of an earlier job never restores its older session.
    #[tokio::test]
    async fn repeated_result_does_not_restore_an_older_session() {
        let harness = Harness::new().await;
        let (analysis, _) = harness.enqueue(CREDENTIAL).await;
        let claimed = harness.claim(CREDENTIAL).await;
        let mut result = succeeded(claimed.job, 1);
        result.session = Some(SessionReport {
            thread_id: "S1".to_owned(),
            outcome: SessionOutcome::Started,
        });
        let uri = path(RUNNER_RESULT.path, claimed.job);
        let (status, _) = harness
            .send(&uri, Some(&claimed.token), result.clone())
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        // The next job of the analysis started a new session after a loss.
        harness
            .state
            .db
            .analyses()
            .set_session(analysis, "S2", None)
            .await
            .expect("session");
        let (status, _) = harness.send(&uri, Some(&claimed.token), result).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "a repeat is idempotent");
        let record = harness
            .state
            .db
            .analyses()
            .get(analysis)
            .await
            .expect("analysis");
        assert_eq!(record.agent_thread_id.as_deref(), Some("S2"));
    }

    #[tokio::test]
    async fn result_after_a_cancel_request_does_not_record_its_session() {
        let harness = Harness::new().await;
        let (analysis, _) = harness.enqueue(CREDENTIAL).await;
        let claimed = harness.claim(CREDENTIAL).await;
        harness
            .state
            .db
            .jobs()
            .request_cancel(claimed.job, Utc::now())
            .await
            .expect("cancel");
        let mut result = succeeded(claimed.job, 1);
        result.session = Some(SessionReport {
            thread_id: "S3".to_owned(),
            outcome: SessionOutcome::Started,
        });
        let (status, _) = harness
            .send(
                &path(RUNNER_RESULT.path, claimed.job),
                Some(&claimed.token),
                result,
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let record = harness
            .state
            .db
            .analyses()
            .get(analysis)
            .await
            .expect("analysis");
        assert_eq!(
            record.agent_thread_id, None,
            "cancel wins: the session is not kept"
        );
    }

    #[tokio::test]
    async fn cancelled_result_does_not_record_its_session() {
        let harness = Harness::new().await;
        let (analysis, _) = harness.enqueue(CREDENTIAL).await;
        let claimed = harness.claim(CREDENTIAL).await;
        let mut result = succeeded(claimed.job, 1);
        result.outcome = JobOutcome::Cancelled;
        result.session = Some(SessionReport {
            thread_id: "thread-1".to_owned(),
            outcome: SessionOutcome::Started,
        });
        let (status, _) = harness
            .send(
                &path(RUNNER_RESULT.path, claimed.job),
                Some(&claimed.token),
                result,
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let record = harness
            .state
            .db
            .analyses()
            .get(analysis)
            .await
            .expect("analysis");
        assert_eq!(record.agent_thread_id, None);
    }

    #[test]
    fn scrubbed_object_keys_stay_unique() {
        let mut value = json!({"Sa": 1, "[redacted]a": 2, "x": {"S": "S"}});
        scrub_value(&mut value, &["S"]);
        let map = value.as_object().expect("object");
        assert_eq!(map.len(), 3, "{value}");
        assert!(!value.to_string().contains('S'), "{value}");
        assert!(map.contains_key("[redacted]a") && map.contains_key("[redacted]a#2"));
        let mut kept: Vec<i64> = ["[redacted]a", "[redacted]a#2"]
            .iter()
            .filter_map(|key| map[*key].as_i64())
            .collect();
        kept.sort_unstable();
        assert_eq!(kept, [1, 2], "no value is lost on a collision");
    }

    #[tokio::test]
    async fn old_token_cannot_write_a_reclaimed_attempt() {
        let harness = Harness::new().await;
        harness.enqueue(CREDENTIAL).await;
        let first = harness.claim(CREDENTIAL).await;
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", first.token).parse().expect("header"),
        );
        // Authorise with the first attempt's token, then the lease ends and the job is claimed again.
        let auth = authorize_job(&harness.state, &headers, first.job)
            .await
            .expect("authorised for attempt 1");
        harness
            .state
            .db
            .jobs()
            .expire_leases(Utc::now() + TimeDelta::seconds(61))
            .await
            .expect("expire");
        let second = harness.claim(CREDENTIAL).await;
        assert_eq!(second.job, first.job);
        for attempt in [1, 2] {
            let events = apply_events(&harness.state, &auth, batch(attempt, "old")).await;
            assert!(
                matches!(events, Err(RunnerError::Db(DbError::Conflict))),
                "{events:?}"
            );
            let output = apply_output(
                &harness.state,
                &auth,
                OutputReady {
                    attempt,
                    output: json!({"old": true}),
                },
            )
            .await;
            assert!(
                matches!(output, Err(RunnerError::Db(DbError::Conflict))),
                "{output:?}"
            );
            let result = apply_result(&harness.state, &auth, succeeded(first.job, attempt)).await;
            assert!(
                matches!(result, Err(RunnerError::Db(DbError::Conflict))),
                "{result:?}"
            );
        }
        let job = harness.state.db.jobs().get(first.job).await.expect("get");
        assert_eq!((job.attempt, job.state), (2, JobState::Claimed));
        assert_eq!((job.ready_output, job.result), (None, None));
        assert!(
            harness
                .state
                .db
                .jobs()
                .events(first.job, 0)
                .await
                .expect("events")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn invalid_runner_body_is_not_logged() {
        let capture = LogCapture::start();
        let harness = Harness::new().await;
        harness.enqueue(CREDENTIAL).await;
        let claimed = harness.claim(CREDENTIAL).await;
        let canary = "canary-secret-7f3a";
        let bodies = [
            format!(
                r#"{{"attempt":1,"events":[{{"seq":1,"at":"2026-11-16T12:00:00Z","activity":"{canary}","summary":"s"}}]}}"#
            ),
            format!(r#"{{"attempt":1,"events":[{{"{canary}":1}}]}}"#),
            format!(r#"{{"attempt":1,"events":["{canary}"#),
        ];
        for body in bodies {
            let (status, reply) = harness
                .send_raw(
                    &path(RUNNER_EVENTS.path, claimed.job),
                    Some(&claimed.token),
                    body.into_bytes(),
                )
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(!reply.to_string().contains(canary));
        }
        let logs = capture.text();
        assert!(logs.contains("API request rejected"), "{logs}");
        assert!(!logs.contains(canary), "{logs}");
        assert!(logs.contains("line"), "position is kept: {logs}");
    }

    #[tokio::test]
    async fn public_listener_rejects_runner_routes() {
        let harness = Harness::new().await;
        let (_, job) = harness.enqueue(CREDENTIAL).await;
        let public = crate::api::router(harness.state.clone());
        let request = Request::post(RUNNER_CLAIM.path)
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", harness.secret()),
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&claim_request(CREDENTIAL)).expect("json"),
            ))
            .expect("request");
        let response = public.oneshot(request).await.expect("infallible");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let stored = harness.state.db.jobs().get(job).await.expect("get");
        assert_eq!(stored.state, JobState::Queued, "nothing was claimed");
    }
}
