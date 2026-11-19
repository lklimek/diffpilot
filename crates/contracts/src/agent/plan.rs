//! Output of the `plan` job: the changed hunks partitioned into flows.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::Classification;
use crate::ids::{FlowId, HunkId};

/// The flows of the pull request and the hunks left unassigned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanOutput {
    /// Flows; every listed hunk line belongs to exactly one flow or to `unassigned`.
    pub flows: Vec<PlannedFlow>,
    /// Hunks or hunk sub-ranges that belong to no flow, each with a reason.
    pub unassigned: Vec<UnassignedHunk>,
}

/// One flow: a logical change following control flow across files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlannedFlow {
    /// Flow id: f1, f2, ….
    pub id: FlowId,
    /// Short title.
    pub title: String,
    /// Classification.
    pub classification: Classification,
    /// Reading priority: 1 = read first; unique per flow.
    pub importance_rank: u32,
    /// What the flow does, imperative, with the entry points.
    pub intent: String,
    /// The hunks of this flow, whole or as head-line sub-ranges.
    pub hunks: Vec<HunkSelection>,
    /// Ids of flows this flow depends on.
    pub depends_on: Vec<FlowId>,
}

/// A whole hunk (`head_start` and `head_end` null) or a head-line sub-range of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HunkSelection {
    /// Hunk id from the hunk list.
    pub hunk_id: HunkId,
    /// First head line of the sub-range, or null for the whole hunk.
    pub head_start: Option<u32>,
    /// Last head line of the sub-range, or null for the whole hunk.
    pub head_end: Option<u32>,
}

/// A hunk or sub-range that belongs to no flow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnassignedHunk {
    /// Hunk id from the hunk list.
    pub hunk_id: HunkId,
    /// First head line of the sub-range, or null for the whole hunk.
    pub head_start: Option<u32>,
    /// Last head line of the sub-range, or null for the whole hunk.
    pub head_end: Option<u32>,
    /// Why it belongs to no flow.
    pub reason: String,
}
