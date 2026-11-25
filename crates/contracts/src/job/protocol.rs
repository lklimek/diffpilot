//! Runner ↔ server messages: claim, events, result (`docs/contracts/runner-protocol.md`).

use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::job::access::Secret;
use crate::job::event::JobEvent;
use crate::job::spec::JobSpec;

/// Body of `POST /runner/v1/claim`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClaimRequest {
    /// Stable name of the runner instance, for logs.
    pub runner_id: String,
    /// False requests housekeeping and retirement only, without leasing a new job.
    #[serde(default = "accept_job_default")]
    pub accept_job: bool,
    /// PR containers and local keys successfully retired after the preceding claim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retired: Vec<i64>,
}

fn accept_job_default() -> bool {
    true
}

/// Reply to a claim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClaimResponse {
    /// Retired PRs: remove their containers/volumes and revoke only local keys.
    #[serde(default)]
    pub retire: Vec<i64>,
    /// The claimed job, or none when nothing is runnable.
    pub claim: Option<Claim>,
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
/// it): at `fence_after_send_secs` it cancels the turn, then stops the worker within
/// `term_grace_secs` + `kill_and_cleanup_secs`, leaving `margin_secs` before the lease can end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaseTiming {
    /// Lease granted per accepted events or output request.
    pub lease_secs: u32,
    /// Longest gap between two runner requests (empty batch = heartbeat).
    pub heartbeat_secs: u32,
    /// Local deadline after the send time of the last acked request.
    pub fence_after_send_secs: u32,
    /// Turn cancellation grace before stopping the worker.
    pub term_grace_secs: u32,
    /// Worker stop and per-job cleanup budget.
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
