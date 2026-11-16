//! The job state machine.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// State of a job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Waiting for a runner.
    Queued,
    /// Claimed, no event yet.
    Claimed,
    /// The runner reported progress.
    Running,
    /// Finished with output.
    Succeeded,
    /// Failed for good.
    Failed,
    /// Cancelled.
    Cancelled,
    /// Waiting for the usage limit to reset.
    Paused,
}

/// An event that moves a job between states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JobTransition {
    /// A runner claimed the job.
    Claim,
    /// The runner posted its first events.
    Start,
    /// The runner posted a `succeeded` result.
    Succeed,
    /// The runner posted a `failed` result.
    Fail,
    /// Cancelled before a runner had it, or the runner posted `cancelled`.
    Cancel,
    /// The runner posted `usage_limited`.
    Pause,
    /// The usage limit reset.
    Resume,
    /// The lease expired without a result; cancellation, if requested, wins over requeue and failure.
    LeaseExpired {
        /// No attempt left.
        exhausted: bool,
        /// Cancellation had been requested.
        cancel_requested: bool,
    },
}

/// A transition not allowed from the current state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("job transition {transition:?} is not allowed from state {from:?}")]
pub struct InvalidTransition {
    /// The current state.
    pub from: JobState,
    /// The rejected transition.
    pub transition: JobTransition,
}

impl JobState {
    /// Every state.
    pub const ALL: [JobState; 7] = [
        Self::Queued,
        Self::Claimed,
        Self::Running,
        Self::Succeeded,
        Self::Failed,
        Self::Cancelled,
        Self::Paused,
    ];

    /// The snake_case name used on the wire and in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Paused => "paused",
        }
    }

    /// True for states a job never leaves.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    /// The state after `transition`, or an error if it is not allowed here.
    pub fn transition(self, transition: JobTransition) -> Result<JobState, InvalidTransition> {
        use JobState::{Cancelled, Claimed, Failed, Paused, Queued, Running, Succeeded};
        use JobTransition::{Cancel, Claim, Fail, LeaseExpired, Pause, Resume, Start, Succeed};
        let next = match (self, transition) {
            (Queued, Claim) => Claimed,
            (Claimed, Start) => Running,
            (Claimed | Running, Succeed) => Succeeded,
            (
                Claimed | Running,
                LeaseExpired {
                    cancel_requested: true,
                    ..
                },
            ) => Cancelled,
            (Claimed | Running, Fail)
            | (
                Claimed | Running,
                LeaseExpired {
                    exhausted: true, ..
                },
            ) => Failed,
            (
                Claimed | Running,
                LeaseExpired {
                    exhausted: false, ..
                },
            )
            | (Paused, Resume) => Queued,
            (Claimed | Running, Pause) => Paused,
            (Queued | Claimed | Running | Paused, Cancel) => Cancelled,
            (from, transition) => return Err(InvalidTransition { from, transition }),
        };
        Ok(next)
    }
}
