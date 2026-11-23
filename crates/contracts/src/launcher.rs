//! Runner-to-launcher protocol; names, images, mounts and limits are derived by the launcher.

use crate::{job::access::McpHttpEndpoint, runtime::ContainerId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One correlated launcher request on its private unix socket.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LauncherRequest {
    /// Caller-generated correlation id.
    pub id: String,
    /// Operation, without Docker configuration or host paths.
    pub action: LauncherAction,
}

/// Fixed-template container operations; files never pass through this protocol.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum LauncherAction {
    /// Create if absent and start from the pinned template (CS1/CS2).
    Ensure {
        /// Worker identity.
        container: ContainerId,
        /// Configured MemCan endpoint for CS1 only; rejected for CS2.
        memcan: Option<McpHttpEndpoint>,
    },
    /// Stop the worker and end all its turns.
    Stop {
        /// Worker identity.
        container: ContainerId,
    },
    /// Remove the worker; upstream credentials remain linked.
    Remove {
        /// Worker identity.
        container: ContainerId,
    },
    /// Run the one-shot CS3 fetch for this CS1 worker; rejected for CS2.
    Fetch {
        /// PR worker identity.
        container: ContainerId,
    },
    /// Relay proxy stdout only; stderr stays in launcher logs.
    Attach {
        /// Worker identity.
        container: ContainerId,
    },
    /// List managed workers for reconciliation.
    List,
}

/// Managed worker identity and current lifecycle state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ts_rs::TS)]
pub struct WorkerStatus {
    /// Worker identity derived from managed labels.
    pub container: ContainerId,
    /// Whether it is currently running.
    pub running: bool,
}

/// Correlated response; attach upgrades the connection to a raw relay after acknowledgement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LauncherResponse {
    /// Original request id.
    pub id: String,
    /// Managed workers (list), otherwise empty.
    pub workers: Vec<WorkerStatus>,
}
