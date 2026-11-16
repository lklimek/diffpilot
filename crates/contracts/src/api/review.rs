//! The reviewer's progress: ticks and the last viewed block.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::NotAnalysedCategory;
use crate::ids::{FlowId, StepId};

/// Review state of one user for one analysis; a flow is reviewed when all its steps are ticked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReviewState {
    /// Ticked steps.
    pub ticked_steps: Vec<StepId>,
    /// Last viewed block; null before the first visit (starts at the PR context).
    pub last_viewed: Option<BlockRef>,
}

/// A block of the Tour sequence or a menu row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "block", rename_all = "snake_case")]
pub enum BlockRef {
    /// The PR context.
    PrContext,
    /// A flow introduction.
    FlowIntro {
        /// The flow.
        flow_id: FlowId,
    },
    /// A step.
    Step {
        /// The step.
        step_id: StepId,
    },
    /// A quiet row.
    Quiet {
        /// The row.
        row: QuietRowRef,
    },
    /// The pending-review list.
    PendingReview,
}

/// A quiet row of the menu.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "row", rename_all = "snake_case")]
pub enum QuietRowRef {
    /// Unexplained lines.
    Unexplained,
    /// Moves with identical text.
    Moves,
    /// A Not analysed section.
    NotAnalysed {
        /// The section.
        category: NotAnalysedCategory,
    },
}
