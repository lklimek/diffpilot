//! Runner ↔ server messages: claim, events, result (`docs/contracts/runner-protocol.md`).

use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    /// Pull-request keys whose session files the runner deleted since its last claim.
    pub dropped_sessions: Vec<i64>,
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
    /// Pull-request keys whose session files the runner must delete (explicit removal; ADR-004).
    pub drop_sessions: Vec<i64>,
}

/// A claimed job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Claim {
    /// Bearer token for this job's events and result; never given to the agent.
    pub job_token: Secret,
    /// Server-side lease end, informational: the runner fences by [`LeaseTiming`] from its own send times.
    pub lease_expires_at: Timestamp,
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
    /// The renewed server-side lease end, informational (the runner fences by [`LeaseTiming`]).
    pub lease_expires_at: Timestamp,
}

/// Body of `POST /runner/v1/jobs/{id}/output`: the final output as soon as the model turn ends, before
/// session archival. Sent at most once per attempt, scrubbed like a result; the result stays authoritative.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutputReady {
    /// The attempt the output belongs to; a stale attempt is rejected.
    pub attempt: u32,
    /// The agent's final structured output (parsed JSON; the server validates it as it would a result).
    pub output: Value,
}

/// Lease and self-fencing timing in seconds, shared by server and runner (`docs/contracts/runner-protocol.md`).
///
/// The server grants `lease_secs` from when it receives an accepted request. The runner measures its own
/// deadline from when it SENT the last request that was acked (send ≤ receive, so a delayed ack never extends
/// it): at `fence_after_send_secs` it sends SIGTERM, after `term_grace_secs` SIGKILL, then copies the
/// credential back within `kill_and_cleanup_secs`, leaving `margin_secs` before the lease can end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaseTiming {
    /// Lease granted per accepted events or output request.
    pub lease_secs: u32,
    /// Longest gap between two runner requests (empty batch = heartbeat).
    pub heartbeat_secs: u32,
    /// Local deadline after the send time of the last acked request.
    pub fence_after_send_secs: u32,
    /// SIGTERM to SIGKILL.
    pub term_grace_secs: u32,
    /// SIGKILL, reaping and credential copy-back.
    pub kill_and_cleanup_secs: u32,
    /// Slack left before the lease ends.
    pub margin_secs: u32,
    /// Backoff of `output` retries.
    pub output_retry_secs: [u32; 3],
}

impl LeaseTiming {
    /// The v1 timing.
    pub const V1: LeaseTiming = LeaseTiming {
        lease_secs: 60,
        heartbeat_secs: 15,
        fence_after_send_secs: 35,
        term_grace_secs: 10,
        kill_and_cleanup_secs: 5,
        margin_secs: 10,
        output_retry_secs: [1, 2, 4],
    };
}
