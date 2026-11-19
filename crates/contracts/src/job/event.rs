//! Normalised job events posted by the runner (activity log and liveness; not shown as a feed).

use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One normalised agent activity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobEvent {
    /// Sequence number within the attempt, from 1.
    pub seq: u32,
    /// When the runner saw it.
    pub at: Timestamp,
    /// What kind of activity.
    pub activity: ActivityKind,
    /// Short plain summary, at most 200 characters; never raw command output; known secret values scrubbed.
    pub summary: String,
}

/// Kind of agent activity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    /// A shell command.
    Command,
    /// An MCP tool call.
    McpToolCall,
    /// A message of the agent.
    AgentMessage,
    /// A reasoning summary.
    Reasoning,
}
