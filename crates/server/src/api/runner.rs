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
use contracts::job::result::{JobFailureKind, JobOutcome, JobResult};
use contracts::job::state::JobState;
use contracts::routes::{RUNNER_CLAIM, RUNNER_EVENTS, RUNNER_OUTPUT, RUNNER_RESULT};
use db::error::DbError;
use db::repo::jobs::{Completion, JobRecord, OutputRecorded};
use secrecy::ExposeSecret;
use serde_json::Value;

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::jobs::claim::{self, ClaimError, hashes_match, lease_until, token_hash};
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
    /// The body is not the expected JSON.
    #[error("the request body is not valid")]
    Body(#[from] JsonRejection),
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
}

impl ApiErrorSource for RunnerError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Unauthenticated => ApiErrorCode::Unauthenticated,
            Self::Body(_) | Self::Path(_) | Self::JobMismatch => ApiErrorCode::InvalidRequest,
            Self::Claim(ClaimError::Db(error)) | Self::Db(error) => error.code(),
            Self::Claim(ClaimError::Random(_)) => ApiErrorCode::Internal,
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Unauthenticated => Cow::Borrowed("The runner credentials are not valid."),
            Self::Body(_) | Self::Path(_) => Cow::Borrowed("The request is not valid."),
            Self::JobMismatch => Cow::Borrowed("The result belongs to another job."),
            Self::Claim(ClaimError::Db(error)) | Self::Db(error) => error.public_message(),
            Self::Claim(ClaimError::Random(_)) => {
                Cow::Borrowed("Something went wrong on the server. Try again later.")
            }
        }
    }
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

/// Checks the job token of the current claim; returns the job and the presented token (for scrubbing).
async fn authorize_job(
    state: &AppState,
    headers: &HeaderMap,
    job: JobId,
) -> Result<(JobRecord, String), RunnerError> {
    let presented = bearer(headers).ok_or(RunnerError::Unauthenticated)?;
    let stored = state
        .db
        .jobs()
        .token_hash(job)
        .await?
        .ok_or(RunnerError::Unauthenticated)?;
    if !hashes_match(&stored, &token_hash(presented)) {
        return Err(RunnerError::Unauthenticated);
    }
    let record = state.db.jobs().get(job).await?;
    let lease_over = record
        .lease_expires_at
        .is_some_and(|lease| lease < Utc::now());
    if lease_over && matches!(record.state, JobState::Claimed | JobState::Running) {
        return Err(RunnerError::Unauthenticated);
    }
    Ok((record, presented.to_owned()))
}

/// Known secret values that must never be stored from runner data (threat model T10).
fn known_secrets<'a>(state: &'a AppState, job_token: &'a str) -> [&'a str; 3] {
    [
        job_token,
        state.config.runner_secret.expose_secret(),
        state.config.memcan.token.expose_secret(),
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

fn scrub_value(value: &mut Value, secrets: &[&str]) {
    match value {
        Value::String(text) => *text = scrub(text, secrets),
        Value::Array(items) => items.iter_mut().for_each(|item| scrub_value(item, secrets)),
        Value::Object(map) => map.values_mut().for_each(|item| scrub_value(item, secrets)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
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
    let response = claim::claim(&state.db, &request, Utc::now())
        .await
        .map_err(RunnerError::from)?;
    Ok(Json(response))
}

async fn post_events(
    State(state): State<AppState>,
    path: Result<Path<JobId>, PathRejection>,
    headers: HeaderMap,
    body: Result<Json<EventBatch>, JsonRejection>,
) -> RunnerResult<Json<EventsAck>> {
    let Path(job) = path.map_err(RunnerError::from)?;
    let (_, token) = authorize_job(&state, &headers, job).await?;
    let Json(batch) = body.map_err(RunnerError::from)?;
    let secrets = known_secrets(&state, &token);
    let events: Vec<JobEvent> = batch
        .events
        .into_iter()
        .map(|event| scrub_event(event, &secrets))
        .collect();
    let lease_expires_at = lease_until(Utc::now());
    let recorded = state
        .db
        .jobs()
        .record_events(job, batch.attempt, &events, lease_expires_at)
        .await
        .map_err(RunnerError::from)?;
    Ok(Json(EventsAck {
        cancel_requested: recorded.cancel_requested,
        lease_expires_at,
    }))
}

async fn post_output(
    State(state): State<AppState>,
    path: Result<Path<JobId>, PathRejection>,
    headers: HeaderMap,
    body: Result<Json<OutputReady>, JsonRejection>,
) -> RunnerResult<Json<EventsAck>> {
    let Path(job) = path.map_err(RunnerError::from)?;
    let (record, token) = authorize_job(&state, &headers, job).await?;
    let Json(mut ready) = body.map_err(RunnerError::from)?;
    scrub_value(&mut ready.output, &known_secrets(&state, &token));
    let lease_expires_at = lease_until(Utc::now());
    let recorded = state
        .db
        .jobs()
        .record_output(job, ready.attempt, &ready.output, lease_expires_at)
        .await
        .map_err(RunnerError::from)?;
    let cancel_requested = match recorded {
        OutputRecorded::Stored { cancel_requested } => cancel_requested,
        OutputRecorded::AlreadyStored => record.cancel_requested,
    };
    Ok(Json(EventsAck {
        cancel_requested,
        lease_expires_at,
    }))
}

async fn post_result(
    State(state): State<AppState>,
    path: Result<Path<JobId>, PathRejection>,
    headers: HeaderMap,
    body: Result<Json<JobResult>, JsonRejection>,
) -> RunnerResult<StatusCode> {
    let Path(job) = path.map_err(RunnerError::from)?;
    let (record, token) = authorize_job(&state, &headers, job).await?;
    let Json(mut result) = body.map_err(RunnerError::from)?;
    if result.job_id != job {
        return Err(RunnerError::JobMismatch.into());
    }
    let secrets = known_secrets(&state, &token);
    match &mut result.outcome {
        JobOutcome::Succeeded { output } => scrub_value(output, &secrets),
        JobOutcome::Failed { message, .. } => *message = scrub(message, &secrets),
        JobOutcome::Cancelled | JobOutcome::UsageLimited { .. } => {}
    }
    let now = Utc::now();
    let completion = state
        .db
        .jobs()
        .complete(&result, now)
        .await
        .map_err(RunnerError::from)?;
    if let Completion::Applied { state: job_state } = completion {
        tracing::info!(%job, attempt = result.attempt, state = job_state.as_str(), "job result recorded");
        record_credential_outcome(&state, &record, &result, job_state).await;
        // TODO(WP3-APPTOKEN): revoke the attempt's installation token (`Jobs::github_tokens_to_revoke`).
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Updates the LLM status and the credential's pause or usability from an applied result. The result is already
/// stored, so a failure here is logged rather than turned into an error the runner would retry.
async fn record_credential_outcome(
    state: &AppState,
    record: &JobRecord,
    result: &JobResult,
    job_state: JobState,
) {
    let credentials = state.db.llm_credentials();
    let credential = record.spec.credential.id.as_str();
    if let Some(call) = &result.llm_call
        && let Err(error) = credentials.record_call(credential, call).await
    {
        tracing::warn!(%error, credential, "could not record the LLM call outcome; Settings shows an older status");
    }
    if job_state == JobState::Paused {
        let paused = match state.db.jobs().get(record.id).await {
            Ok(job) => job.resumes_at,
            Err(error) => {
                tracing::warn!(%error, job = %record.id, "could not read the pause end of a usage-limited job");
                None
            }
        };
        if let Some(until) = paused
            && let Err(error) = credentials.pause(credential, until).await
        {
            tracing::warn!(%error, credential, "could not pause the LLM credential after a usage limit");
        }
    }
    if let JobOutcome::Failed {
        kind: JobFailureKind::CredentialUnusable,
        ..
    } = result.outcome
        && let Err(error) = credentials.mark_unusable(credential, Utc::now()).await
    {
        tracing::warn!(%error, credential, "could not mark the LLM credential unusable");
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use chrono::{DateTime, TimeDelta};
    use contracts::agent::JobKind;
    use contracts::ids::{AnalysisId, FlowId};
    use contracts::job::access::AgentAccess;
    use contracts::job::event::ActivityKind;
    use contracts::job::protocol::{CredentialStatus, LeaseTiming};
    use contracts::job::result::{LlmCallOutcome, LlmCallStatus};
    use contracts::job::spec::{CredentialKind, JobPriority, LlmCredentialRef};
    use db::repo::analyses::testing::{CREDENTIAL, job_spec, seed_analysis};
    use db::repo::jobs::{JobStage, NewJob};
    use http_body_util::BodyExt;
    use serde::Serialize;
    use serde_json::json;
    use tower::ServiceExt;

    use super::*;
    use crate::api::runner_router;
    use crate::state::tests::test_state;

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
    async fn claim_uses_lease_timing_v1_and_sends_no_credentials_in_the_slice() {
        let harness = Harness::new().await;
        let (_, job) = harness.enqueue(CREDENTIAL).await;
        let before = Utc::now();
        let claimed = harness.claim(CREDENTIAL).await;
        let after = Utc::now();
        let lease = TimeDelta::seconds(i64::from(LeaseTiming::V1.lease_secs));
        assert!(claimed.lease_expires_at >= before + lease - TimeDelta::milliseconds(1));
        assert!(claimed.lease_expires_at <= after + lease);
        assert_eq!(claimed.job, job);
        assert_eq!(claimed.spec_access, AgentAccess::default());
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
