//! Common gateway admin DTOs; real credentials, rotation generations and markers stay gateway-only.

use crate::{
    ids::UserId,
    job::{
        access::Secret,
        result::{LlmCallOutcome, Usage},
    },
    runtime::ContainerId,
    time::Timestamp,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// ChatGPT-plan connection state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum CredentialState {
    /// A staged connection attempt, preserving an existing usable connection.
    Linking,
    /// A usable credential is held by the gateway.
    Linked,
    /// Explicit user action is required; the gateway responds with HTTP 403.
    RelinkNeeded,
}

/// Safe relink reason; never contains a token, upstream response or account identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RelinkReason {
    /// The upstream definitively rejected the credential.
    Terminal,
    /// Exchange or persistence outcome is unknown; the generation must never be replayed.
    Quarantined,
}

/// The only supported credential mode; no paid fallback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMode {
    /// Gateway-run ChatGPT-plan device linking.
    #[default]
    ChatgptPlan,
}

/// Starts device linking without accepting upstream secrets or endpoints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct LinkCredential {
    /// ChatGPT-plan linking only.
    pub mode: CredentialMode,
}

/// Public device-code challenge; upstream tokens never cross this boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DeviceLink {
    /// Short code the owner enters at the provider.
    pub user_code: String,
    /// Fixed provider verification page.
    pub verification_url: String,
    /// Deadline of the staged attempt.
    pub expires_at: Timestamp,
}

/// Non-secret status shared by gateway, server and Settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CredentialStatus {
    /// Only ChatGPT-plan linking is supported.
    pub mode: CredentialMode,
    /// Common connection state.
    pub state: CredentialState,
    /// Present only when relink is needed.
    pub relink_reason: Option<RelinkReason>,
    /// Human-readable connection label.
    pub label: Option<String>,
    /// Provider plan label, when supplied.
    pub plan: Option<String>,
    /// Payer pause after a usage limit.
    pub paused_until: Option<Timestamp>,
    /// Most recent model call.
    pub last_call: Option<LlmCallOutcome>,
}

/// Explicit user-authorized upstream unlink; distinct from retiring a local virtual key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct UnlinkCredential {
    /// User whose credential and local keys are unlinked.
    pub user: UserId,
}

/// Reasons to revoke a local virtual key; none authorizes upstream revoke.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum KeyRevokeReason {
    /// The job ended or the next turn rotates the key.
    Rotation,
    /// Worker stopped idle.
    IdleStop,
    /// Worker was removed or reconciled as missing.
    Retirement,
}

/// Mint a fresh, expiring key and revoke the previous key for this container.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct MintKey {
    /// Worker identity and payer.
    pub container: ContainerId,
}

/// A local virtual key returned only to the runner, for its tmpfs file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MintedKey {
    /// Gateway-local opaque key id.
    pub id: String,
    /// Virtual key, never a real upstream credential.
    pub key: Secret,
    /// Bounded lifetime (at most 90 minutes).
    pub expires_at: Timestamp,
}

/// Revoke local keys without logging out or revoking an upstream credential.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RevokeKeys {
    /// Worker whose keys become unusable.
    pub container: ContainerId,
    /// Local-only revocation reason.
    pub reason: KeyRevokeReason,
}

/// Payer usage, summed from request or turn deltas without fork inheritance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PayerUsage {
    /// Payer.
    pub user: UserId,
    /// Billable token counts, separate from last-context estimates.
    pub usage: Usage,
    /// Reset time if usage is limited.
    pub resets_at: Option<Timestamp>,
}
