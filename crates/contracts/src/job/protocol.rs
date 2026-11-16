//! Runner ↔ server messages: claim, events, result (`docs/contracts/runner-protocol.md`).

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::AnalysisId;
use crate::job::access::Secret;
use crate::job::event::JobEvent;
use crate::job::spec::{JobSpec, LlmCredentialRef};

/// Body of `POST /runner/v1/claim`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClaimRequest {
    /// Stable name of the runner instance, for logs.
    pub runner_id: String,
    /// The LLM credentials the runner holds and whether each is usable.
    pub credentials: Vec<CredentialStatus>,
    /// Analyses whose session files the runner deleted since its last claim.
    pub dropped_sessions: Vec<AnalysisId>,
}

/// A credential held by the runner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CredentialStatus {
    /// The credential.
    pub credential: LlmCredentialRef,
    /// False once a refresh failed; the owner must sign in again.
    pub usable: bool,
}

/// Reply to a claim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClaimResponse {
    /// The claimed job, or none when nothing is runnable.
    pub claim: Option<Claim>,
    /// Analyses whose session files the runner must delete (cancelled or superseded; ADR-004).
    pub drop_sessions: Vec<AnalysisId>,
}

/// A claimed job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Claim {
    /// Bearer token for this job's events and result; never given to the agent.
    pub job_token: Secret,
    /// The lease ends unless renewed by posting events.
    pub lease_expires_at: DateTime<Utc>,
    /// The job, with `access` filled in.
    pub spec: JobSpec,
}

/// Body of `POST /runner/v1/jobs/{id}/events`; an empty batch is a heartbeat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventBatch {
    /// The attempt the events belong to; a stale attempt is rejected.
    pub attempt: u32,
    /// Events in sequence order.
    pub events: Vec<JobEvent>,
}

/// Reply to an event batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventsAck {
    /// The job should be cancelled: stop the agent and post a `cancelled` result.
    pub cancel_requested: bool,
    /// The renewed lease.
    pub lease_expires_at: DateTime<Utc>,
}
