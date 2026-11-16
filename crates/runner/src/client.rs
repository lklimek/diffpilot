//! HTTP client of the runner protocol `/runner/v1` (`docs/contracts/runner-protocol.md`). Talks to the
//! server's runner listener directly: never through a proxy, never following redirects.

use std::time::Duration;

use contracts::ids::JobId;
use contracts::job::access::Secret;
use contracts::job::protocol::{ClaimRequest, ClaimResponse, EventBatch, EventsAck, OutputReady};
use contracts::job::result::JobResult;
use contracts::routes::{RUNNER_CLAIM, RUNNER_EVENTS, RUNNER_OUTPUT, RUNNER_RESULT, Route};
use reqwest::StatusCode;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// A protocol request failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// No reply (connection, timeout); worth retrying.
    #[error("the server could not be reached")]
    Transport(#[source] reqwest::Error),
    /// The server failed (5xx); worth retrying.
    #[error("the server failed with status {0}")]
    Server(StatusCode),
    /// The server rejected the request (4xx); retrying the same request does not help. 409 means the attempt
    /// is stale or no longer the runner's.
    #[error("the server rejected the request with status {0}")]
    Rejected(StatusCode),
    /// The reply body was not the expected JSON.
    #[error("the server's reply could not be read")]
    InvalidReply(#[source] reqwest::Error),
    /// The HTTP client could not be built.
    #[error("the HTTP client could not be created")]
    Build(#[source] reqwest::Error),
}

impl ClientError {
    /// Whether the same request may succeed when sent again.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Transport(_) | Self::Server(_))
    }
}

/// Client of one server's runner listener.
#[derive(Debug, Clone)]
pub struct ServerClient {
    http: reqwest::Client,
    base: String,
    runner_secret: Secret,
}

impl ServerClient {
    /// A client for the runner listener at `base` (e.g. `http://server:3001`), with `request_timeout` per request.
    pub fn new(
        base: &str,
        runner_secret: Secret,
        request_timeout: Duration,
    ) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(request_timeout)
            .timeout(request_timeout)
            .build()
            .map_err(ClientError::Build)?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            runner_secret,
        })
    }

    /// `POST /runner/v1/claim` with the runner secret.
    pub async fn claim(&self, request: &ClaimRequest) -> Result<ClaimResponse, ClientError> {
        let response = self
            .post(&RUNNER_CLAIM, None, &self.runner_secret, request)
            .await?;
        response.json().await.map_err(ClientError::InvalidReply)
    }

    /// `POST /runner/v1/jobs/{id}/events` with the job token; an empty batch is a heartbeat.
    pub async fn events(
        &self,
        job: JobId,
        token: &Secret,
        batch: &EventBatch,
    ) -> Result<EventsAck, ClientError> {
        self.post_json(&RUNNER_EVENTS, Some(job), token, batch)
            .await
    }

    /// `POST /runner/v1/jobs/{id}/output` with the job token.
    pub async fn output(
        &self,
        job: JobId,
        token: &Secret,
        output: &OutputReady,
    ) -> Result<EventsAck, ClientError> {
        self.post_json(&RUNNER_OUTPUT, Some(job), token, output)
            .await
    }

    /// `POST /runner/v1/jobs/{id}/result` with the job token; any 2xx is success.
    pub async fn result(
        &self,
        job: JobId,
        token: &Secret,
        result: &JobResult,
    ) -> Result<(), ClientError> {
        self.post(&RUNNER_RESULT, Some(job), token, result)
            .await
            .map(drop)
    }

    async fn post_json<B: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        route: &Route,
        job: Option<JobId>,
        token: &Secret,
        body: &B,
    ) -> Result<R, ClientError> {
        let response = self.post(route, job, token, body).await?;
        response.json().await.map_err(ClientError::InvalidReply)
    }

    async fn post<B: Serialize + ?Sized>(
        &self,
        route: &Route,
        job: Option<JobId>,
        token: &Secret,
        body: &B,
    ) -> Result<reqwest::Response, ClientError> {
        let path = match job {
            Some(job) => route.path.replace("{id}", &job.to_string()),
            None => route.path.to_owned(),
        };
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .bearer_auth(token.expose())
            .json(body)
            .send()
            .await
            .map_err(ClientError::Transport)?;
        let status = response.status();
        if status.is_success() {
            Ok(response)
        } else if status.is_server_error() {
            Err(ClientError::Server(status))
        } else {
            Err(ClientError::Rejected(status))
        }
    }
}
