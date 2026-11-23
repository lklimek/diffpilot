//! Provider-neutral container, immutable thread and first-Ask routing contracts.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::agent::JobKind;
use crate::ids::UserId;

/// A worker's identity; each identity owns at most one running job.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum ContainerId {
    /// Persistent PR worker (CS1).
    Pr {
        /// Database PR key.
        #[ts(type = "number")]
        pull_request_id: i64,
        /// User paying for this worker's turns.
        payer: UserId,
    },
    /// Ephemeral answer-only worker (CS2).
    Judge {
        /// Dashboard user paying for the turn.
        payer: UserId,
    },
}

/// Fixed workspace shape; paths never come from model output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Workspace {
    /// CS1 flat workspace, including the checkout and checker context.
    Pr,
    /// CS2 tmpfs workspace, with filesystem-only runner setup and cleanup.
    Judge,
}

impl Workspace {
    /// The PR shape (CS1).
    pub const PR: Self = Self::Pr;
    /// Fixed thread cwd, above the untrusted checkout.
    pub const fn cwd(self) -> &'static str {
        match self {
            Self::Pr => "/work",
            Self::Judge => "/tmp",
        }
    }
    /// Tagged flow hand-in path, restored here for repairs.
    pub const fn flow_output(self) -> Option<&'static str> {
        match self {
            Self::Pr => Some("/work/out/flow.md"),
            Self::Judge => None,
        }
    }
    /// Whether runner setup may execute commands; CS2 uses only filesystem RPCs.
    pub const fn allows_commands(self) -> bool {
        matches!(self, Self::Pr)
    }
    /// PR data hand-in directory.
    pub const PR_DATA: &'static str = "/work/pr-data";
    /// Flow checker context file.
    pub const FLOW_CONTEXT: &'static str = "/work/flow-context.json";
    /// Head checkout.
    pub const REPO: &'static str = "/work/repo";
    /// Review report hand-in path.
    pub const REPORT: &'static str = "/work/out/report.json";
}

/// Immutable thread policy; resend its full configuration on start, fork and resume.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ThreadKind {
    /// Analysis, including flow repairs.
    Analytic,
    /// Private Ask with read-only MemCan.
    Ask,
    /// Review pass or private review Ask, without MemCan.
    Verify,
    /// Strict JSON answer, without model tools.
    Judge,
}

impl ThreadKind {
    /// Fixed policy for a job kind.
    pub const fn for_job(kind: JobKind) -> Self {
        match kind {
            JobKind::Plan | JobKind::PrContext | JobKind::AnalyseFlow => Self::Analytic,
            JobKind::Ask => Self::Ask,
            JobKind::VerifyComments => Self::Verify,
            JobKind::JudgeWork => Self::Judge,
        }
    }
    /// Whether read-only MemCan tools may be enabled.
    pub const fn memcan_read(self) -> bool {
        matches!(self, Self::Analytic | Self::Ask)
    }
    /// Whether MemCan write tools may be enabled.
    pub const fn memcan_write(self) -> bool {
        matches!(self, Self::Analytic)
    }
    /// Whether any model tool item is acceptable.
    pub const fn allows_model_tools(self) -> bool {
        !matches!(self, Self::Judge)
    }
}

/// How a private working thread was seeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum SessionSeed {
    /// Fork of the same payer's analytic thread.
    Fork,
    /// Private context pack, including recovery after volume loss.
    ContextPack,
}

/// Durable first-Ask preflight state; pending is never interpreted as completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PreflightState {
    /// Confirmed fresh parent context at or below 80k, or a context-pack seed.
    NotNeeded,
    /// Required, unknown, interrupted or not yet acknowledged.
    Pending,
    /// Standalone compaction succeeded with turn/completed.
    Done,
}

/// First-Ask preflight bound to the working thread's seed and analytic generation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AskPreflight {
    /// Working-thread seed.
    pub seed: SessionSeed,
    /// Analytic generation at seeding.
    #[ts(type = "number")]
    pub generation: i64,
    /// Confirmed preflight state.
    pub state: PreflightState,
}

/// Last-context estimate, separate from cumulative usage and billable job deltas.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LastContext {
    /// Input and output context tokens reported for the last completed turn.
    #[ts(type = "number")]
    pub tokens: u64,
    /// Thread generation this estimate describes; missing/stale usage needs recovery.
    #[ts(type = "number")]
    pub generation: i64,
}
