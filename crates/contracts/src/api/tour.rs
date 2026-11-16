//! The Tour: PR context, flows with steps, quiet rows and GitHub threads of a finished analysis.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::agent::analyse_flow::{AgentAnnotation, Assessment, FlowIntroduction, MovedFrom};
use crate::agent::pr_context::PrContextOutput;
use crate::api::analyses::AnalysisSummary;
use crate::common::{
    AnnotationType, Basis, Classification, Evidence, FileRange, FileRanges, FragmentKind,
    LineRange, LineSide, NodeKind, NotAnalysedCategory, ProvenanceBadge, RiskBand, ThreadLlmStatus,
};
use crate::ids::{AnnotationId, FlowId, StepId};

/// Response of `GET /api/v1/analyses/{id}/tour`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Tour {
    /// The analysis.
    pub analysis: AnalysisSummary,
    /// The PR context block.
    pub pr_context: PrContextBlock,
    /// Flows in tour order: behaviour by importance, LLM-classified refactors, LLM-classified tests.
    pub flows: Vec<Flow>,
    /// Quiet rows, always after the flows.
    pub quiet: QuietRows,
    /// Every GitHub review thread of the pull request.
    pub threads: Vec<GithubThread>,
}

/// The PR context: deterministic facts and LLM content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrContextBlock {
    /// Deterministic facts.
    pub facts: PrContextFacts,
    /// LLM analysis.
    pub llm: PrContextOutput,
}

/// Deterministic facts of the PR context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrContextFacts {
    /// CI status of the head commit.
    pub ci: CiStatus,
    /// Counters from the pre-pass.
    pub counters: Counters,
    /// Where the project profile came from.
    pub project_profile: ProjectProfileStatus,
    /// Paths in the pull request diff that look like agent instruction files (AGENTS.md, CLAUDE.md, `.agents/`,
    /// `.codex/`, `.claude/`, `.cursor*`, `.github/copilot-instructions.md`); shown as a notice, empty = none.
    pub agent_instruction_files: Vec<String>,
    /// False when related work could not be looked up (the GitHub App is not installed on the repository);
    /// the related-work list then reads "not checked".
    pub related_work_checked: bool,
    /// Number of flows.
    pub flow_count: u32,
    /// Number of steps over all flows.
    pub step_count: u32,
}

/// CI status of the head commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CiStatus {
    /// Overall state.
    pub state: CiState,
    /// Number of checks.
    pub total: u32,
    /// Failing checks.
    pub failing: Vec<CiCheck>,
}

/// Overall CI state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum CiState {
    /// Every check passed.
    Passing,
    /// At least one check failed.
    Failing,
    /// Checks still running.
    Pending,
    /// No checks.
    None,
}

/// A failing check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CiCheck {
    /// Check name.
    pub name: String,
    /// Link to the check, if any.
    pub url: Option<String>,
    /// Names of failing tests, when the check reports them.
    pub failing_tests: Vec<String>,
}

/// Counters over material that is not analysed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Counters {
    /// Lines removed from tests.
    pub lines_removed_from_tests: u32,
    /// Names of changed dependencies.
    pub dependencies_changed: Vec<String>,
}

/// Sources of the project profile; empty means no profile (ratings are component-level).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ProjectProfileStatus {
    /// Where the profile came from.
    pub sources: Vec<ProfileSource>,
}

/// A source of the project profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ProfileSource {
    /// `.claudius/project-profile.md` on the base ref.
    RepositoryFile,
    /// MemCan memory `type: project-profile`.
    Memcan,
}

/// One flow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Flow {
    /// Flow id.
    pub id: FlowId,
    /// 1-based position in the tour.
    pub position: u32,
    /// Ordering group and menu label, derived from the classification.
    pub group: FlowGroup,
    /// Deterministic facts.
    pub facts: FlowFacts,
    /// LLM analysis.
    pub llm: FlowLlm,
    /// Flow graph; nodes derived by the server from the steps.
    pub graph: FlowGraph,
    /// Steps in order.
    pub steps: Vec<Step>,
}

/// Ordering group of a flow (brief §4.3 flow order).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum FlowGroup {
    /// Ordinary flow, ordered by importance.
    Behaviour,
    /// Labelled "LLM-classified refactor"; never collapsed.
    LlmClassifiedRefactor,
    /// Labelled "LLM-classified tests"; never hidden.
    LlmClassifiedTests,
}

impl From<Classification> for FlowGroup {
    fn from(classification: Classification) -> Self {
        match classification {
            Classification::RefactorPure | Classification::Rename => Self::LlmClassifiedRefactor,
            Classification::Test => Self::LlmClassifiedTests,
            _ => Self::Behaviour,
        }
    }
}

/// Deterministic facts of a flow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FlowFacts {
    /// Files touched by the flow's steps.
    pub files: Vec<String>,
    /// Number of steps.
    pub step_count: u32,
}

/// LLM content of a flow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FlowLlm {
    /// Title.
    pub title: String,
    /// Classification.
    pub classification: Classification,
    /// Highest band of the step assessments; null when no step is assessed.
    pub risk: Option<RiskBand>,
    /// Flow introduction.
    pub introduction: FlowIntroduction,
    /// Flows this flow depends on.
    pub depends_on: Vec<FlowId>,
    /// Patterns explained once, applied elsewhere.
    pub pattern_repetitions: Vec<PatternRepetition>,
}

/// A flow graph: nodes are steps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FlowGraph {
    /// One node per step.
    pub nodes: Vec<GraphNode>,
    /// Edges in execution order.
    pub edges: Vec<GraphEdge>,
}

/// A graph node, the diagram twin of one step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct GraphNode {
    /// The step.
    pub step_id: StepId,
    /// Step number.
    pub number: u32,
    /// Node kind.
    pub kind: NodeKind,
    /// Label (the step title).
    pub label: String,
}

/// A graph edge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct GraphEdge {
    /// Source step.
    pub from: StepId,
    /// Target step.
    pub to: StepId,
    /// Branch label (decision outcome, loop back-edge, error exit).
    pub label: Option<String>,
}

/// One step of a flow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Step {
    /// Step id.
    pub id: StepId,
    /// 1-based number within the flow.
    pub number: u32,
    /// Deterministic facts.
    pub facts: StepFacts,
    /// LLM analysis.
    pub llm: StepLlm,
}

/// Deterministic facts of a step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct StepFacts {
    /// Repository-relative path on the step's side.
    pub file: String,
    /// `head` normally; `base` for removed code without a head location (deleted or emptied file).
    pub side: LineSide,
    /// Step lines on that side.
    pub range: LineRange,
    /// GitHub link to the lines at the head commit (base commit for a base-side step).
    pub github_url: String,
    /// Changed lines inside the step: added lines of a head step, removed lines of a base step.
    pub changed_lines: Vec<LineRange>,
    /// Move check result, for steps whose code was moved.
    pub moved: Option<MoveFact>,
    /// Callers and callees from the tech plugin; null in v1 ("n/a — needs a language plugin").
    pub call_hierarchy: Option<CallHierarchy>,
    /// Ids of the GitHub threads located in this step.
    pub thread_ids: Vec<String>,
}

/// Result of the deterministic move check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct MoveFact {
    /// The original block at the base commit.
    pub from: FileRange,
    /// `text_identical` when no line differs after whitespace normalisation, else null.
    pub badge: Option<ProvenanceBadge>,
    /// Number of lines that differ from the original.
    pub differing_lines: u32,
}

/// Callers and callees of a step symbol.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CallHierarchy {
    /// Callers.
    pub callers: Vec<CallSite>,
    /// Callees.
    pub callees: Vec<CallSite>,
}

/// A caller or callee.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CallSite {
    /// Symbol.
    pub symbol: String,
    /// Repository-relative path.
    pub file: String,
    /// Line.
    pub line: u32,
    /// GitHub link.
    pub github_url: String,
}

/// LLM content of a step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct StepLlm {
    /// Title.
    pub title: String,
    /// Step kind.
    pub kind: FragmentKind,
    /// Enclosing symbol.
    pub symbol: Option<String>,
    /// Who reaches this step.
    pub called_by: String,
    /// What it decides or changes.
    pub decides: String,
    /// Where execution goes next.
    pub next: String,
    /// Why it is needed.
    pub why: String,
    /// Annotations.
    pub annotations: Vec<Annotation>,
    /// Assessment, collapsed by default; null when the LLM gave none.
    pub assessment: Option<Assessment>,
    /// Original location the LLM claims for a moved block.
    pub moved_from: Option<MovedFrom>,
    /// Code outside the diff this step may affect, shown inline as real code.
    pub impact: Vec<ImpactRef>,
}

/// An annotation anchored to lines of a step (same side as the step); its range is a highlight.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Annotation {
    /// Annotation id.
    pub id: AnnotationId,
    /// Type.
    #[serde(rename = "type")]
    pub kind: AnnotationType,
    /// Fact or inference.
    pub basis: Basis,
    /// Annotated lines.
    pub range: LineRange,
    /// Behaviour before the pull request; the hover tooltip shows only was → is.
    pub was: String,
    /// Behaviour after the pull request.
    pub is: String,
    /// The consequence, question or note; null for a behaviour annotation.
    pub text: Option<String>,
    /// Evidence lines.
    pub evidence: Vec<Evidence>,
}

impl Annotation {
    /// Projects a verified agent annotation; `id` is the server id `<step>.a<n>`.
    pub fn from_agent(id: AnnotationId, agent: AgentAnnotation) -> Self {
        Self {
            id,
            kind: agent.kind,
            basis: agent.basis,
            range: LineRange {
                start_line: agent.start_line,
                end_line: agent.end_line,
            },
            was: agent.was,
            is: agent.is,
            text: agent.text,
            evidence: agent.evidence,
        }
    }
}

/// Code outside the diff that a step may affect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ImpactRef {
    /// Repository-relative path at the head commit.
    pub file: String,
    /// Lines.
    pub range: LineRange,
    /// Enclosing symbol.
    pub symbol: Option<String>,
    /// Why it may be affected.
    pub reason: String,
    /// Fact or inference.
    pub basis: Basis,
    /// GitHub link.
    pub github_url: String,
}

/// A pattern explained once and applied in several places ("same pattern × N").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PatternRepetition {
    /// The pattern.
    pub description: String,
    /// The step that explains it.
    pub explained_in: StepId,
    /// The other places.
    pub sites: Vec<PatternSite>,
}

/// One place where a repeated pattern is applied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PatternSite {
    /// Repository-relative path on that side.
    pub file: String,
    /// Commit the lines refer to.
    pub side: LineSide,
    /// Lines.
    pub range: LineRange,
    /// The step containing these lines, if any.
    pub step_id: Option<StepId>,
    /// GitHub link.
    pub github_url: String,
}

/// Quiet rows: never ranked above a flow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct QuietRows {
    /// Changed lines no flow explains ("Unexplained — read these yourself"); listed first. Removed lines
    /// without a head location are base-side entries.
    pub unexplained: Vec<FileRanges>,
    /// Moves whose text is deterministically identical.
    pub moves: Vec<IdenticalMove>,
    /// Material recognised by path or line rules.
    pub not_analysed: Vec<NotAnalysedSection>,
}

/// A moved block whose text is identical after whitespace normalisation. The UI says "Body text identical"
/// and nothing more: identical text does not mean unchanged behaviour, and no call-site flow is linked in v1
/// (ADR-008).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct IdenticalMove {
    /// Original block at the base commit.
    pub from: FileRange,
    /// New block at the head commit.
    pub to: FileRange,
    /// Always `text_identical`.
    pub badge: ProvenanceBadge,
}

/// A Not analysed section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct NotAnalysedSection {
    /// Section.
    pub category: NotAnalysedCategory,
    /// Number of files in the section.
    pub file_count: u32,
    /// Items.
    pub items: Vec<NotAnalysedItem>,
}

/// One file (or line ranges of it) recognised by a rule; one item per file and side.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct NotAnalysedItem {
    /// Repository-relative path on that side.
    pub file: String,
    /// `base` for a deleted file or removed lines, else `head`.
    pub side: LineSide,
    /// Matched changed lines on that side; null for the whole file.
    pub ranges: Option<Vec<LineRange>>,
    /// How it was recognised.
    pub badge: ProvenanceBadge,
    /// The rule (path glob or line regex).
    pub rule: String,
}

/// A GitHub review thread of the pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct GithubThread {
    /// GitHub node id.
    pub id: String,
    /// Repository-relative path.
    pub path: String,
    /// Last line; null for file-level threads.
    pub line: Option<u32>,
    /// First line of a multi-line thread.
    pub start_line: Option<u32>,
    /// Diff side (GitHub LEFT = base, RIGHT = head); null for file-level threads.
    pub side: Option<LineSide>,
    /// Author login of the first comment.
    pub author: String,
    /// Start of the first comment.
    pub excerpt: String,
    /// Number of comments.
    pub comment_count: u32,
    /// Link to the thread.
    pub url: String,
    /// Resolved on GitHub.
    pub is_resolved: bool,
    /// Outdated on GitHub.
    pub is_outdated: bool,
    /// The step the thread is located in, if any.
    pub step_id: Option<StepId>,
    /// LLM status for threads located in a flow's steps; null otherwise.
    pub llm_status: Option<ThreadLlmStatus>,
    /// LLM reason for the status.
    pub llm_reason: Option<String>,
}
