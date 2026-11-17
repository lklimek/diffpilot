//! Output of the `analyse_flow` job: one flow walked step by step.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::{
    AnnotationType, Basis, Blocking, Evidence, FragmentKind, LineSide, Origin, RiskBand,
    ThreadLlmStatus,
};
use crate::ids::FlowId;

/// The analysis of one flow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FlowAnalysisOutput {
    /// Id of the analysed flow.
    pub flow_id: FlowId,
    /// The flow introduction.
    pub introduction: FlowIntroduction,
    /// Steps in reading order following control flow; every changed line of the flow lies inside a step.
    pub steps: Vec<AgentStep>,
    /// Graph edges between steps in execution order.
    pub edges: Vec<AgentEdge>,
    /// Code outside the diff that this change may affect.
    pub impact_refs: Vec<AgentImpactRef>,
    /// Patterns applied in several places, explained once.
    pub pattern_repetitions: Vec<AgentPatternRepetition>,
    /// Status of every listed GitHub thread located in this flow's steps.
    pub thread_statuses: Vec<AgentThreadStatus>,
}

/// Introduction of a flow (brief §3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FlowIntroduction {
    /// Problem before this pull request: the local trigger and what was wrong or missing.
    pub problem_before: String,
    /// The intended correction this pull request makes.
    pub change: String,
    /// A short concrete example of behaviour before and after.
    pub example_before_after: String,
}

/// One step: a line range of one file, normally at the head commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentStep {
    /// Step id, unique within this output; used by edges and references.
    pub id: String,
    /// Short title, also the diagram label.
    pub title: String,
    /// Repository-relative path on the step's side.
    pub file: String,
    /// head normally; base only for removed code with no head line to anchor to (deleted or emptied file,
    /// removed block); kind is then deleted_site and the lines are base lines.
    pub side: LineSide,
    /// First line of the step on its side.
    pub start_line: u32,
    /// Last line of the step on its side.
    pub end_line: u32,
    /// Enclosing symbol, or null.
    pub symbol: Option<String>,
    /// Step kind.
    pub kind: FragmentKind,
    /// Original location of a moved block, or null.
    pub moved_from: Option<MovedFrom>,
    /// Who reaches this step.
    pub called_by: String,
    /// What this step decides or changes.
    pub decides: String,
    /// Where execution goes next.
    pub next: String,
    /// Why this step or modification is needed.
    pub why: String,
    /// Annotations anchored to lines inside the step.
    pub annotations: Vec<AgentAnnotation>,
    /// Required assessment of the change, including low risk; nullable only when reading older stored output.
    #[schemars(with = "Assessment")]
    pub assessment: Option<Assessment>,
}

/// Original location of a moved block at the base commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct MovedFrom {
    /// Repository-relative path at the base commit.
    pub file: String,
    /// First base line.
    pub base_start: u32,
    /// Last base line.
    pub base_end: u32,
}

/// An annotation anchored to lines inside a step (same side as the step); the annotated range is highlighted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentAnnotation {
    /// Annotation type.
    #[serde(rename = "type")]
    pub kind: AnnotationType,
    /// fact needs evidence lines you read; anything else is inference.
    pub basis: Basis,
    /// First annotated line.
    pub start_line: u32,
    /// Last annotated line.
    pub end_line: u32,
    /// Behaviour of these lines before this pull request (hover tooltip; facts only, no risk or verdict).
    pub was: String,
    /// Behaviour of these lines after this pull request (hover tooltip; facts only, no risk or verdict).
    pub is: String,
    /// The consequence, question or note; null for a behaviour annotation (its content is was and is).
    pub text: Option<String>,
    /// Lines read that support the annotation.
    pub evidence: Vec<Evidence>,
}

/// Assessment of a step at system scale (brief §3); evidence comes before it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Assessment {
    /// What a production user or operator would notice.
    pub headline: String,
    /// Risk band from the severity skill; never its numeric scores.
    pub risk: RiskBand,
    /// Reason for the band, also for low.
    pub reason: String,
    /// What was read and that nothing was executed.
    pub basis_note: String,
    /// What was not checked.
    pub not_checked: String,
    /// Introduced by this pull request or pre-existing.
    pub origin: Origin,
    /// Whether an action is proposed before merge, as a follow-up, or neither.
    pub blocking: Blocking,
    /// The smallest correction, or null.
    pub smallest_correction: Option<String>,
}

/// A graph edge between two steps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentEdge {
    /// Source step id.
    pub from_step_id: String,
    /// Target step id.
    pub to_step_id: String,
    /// Label for decision outcomes, loop back-edges and error exits; null otherwise.
    pub label: Option<String>,
}

/// Code outside the diff that the change may affect; its location (head lines) is its evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentImpactRef {
    /// Id of the step whose change reaches this code.
    pub step_id: String,
    /// Repository-relative path at the head commit.
    pub file: String,
    /// First head line.
    pub start_line: u32,
    /// Last head line.
    pub end_line: u32,
    /// Enclosing symbol, or null.
    pub symbol: Option<String>,
    /// Why this code may be affected.
    pub reason: String,
    /// fact if read in the code, otherwise inference.
    pub basis: Basis,
}

/// A pattern applied in several places, explained once in a step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentPatternRepetition {
    /// The pattern.
    pub description: String,
    /// Id of the step that explains it.
    pub explained_in_step_id: String,
    /// The other places where it is applied.
    pub sites: Vec<AgentPatternSite>,
}

/// One place where a repeated pattern is applied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentPatternSite {
    /// Repository-relative path on that side.
    pub file: String,
    /// head normally; base for a removed site.
    pub side: LineSide,
    /// First line.
    pub start_line: u32,
    /// Last line.
    pub end_line: u32,
}

/// The status of a listed GitHub review thread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentThreadStatus {
    /// Thread alias from the thread list: t1, t2, ….
    pub thread_alias: String,
    /// Status against the current code.
    pub status: ThreadLlmStatus,
    /// One line on why.
    pub reason: String,
}
