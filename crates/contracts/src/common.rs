//! Shared value types and the closed enums of brief §3.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A GitHub repository.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
pub struct RepoRef {
    /// Owner (user or organisation) login.
    pub owner: String,
    /// Repository name.
    pub name: String,
}

/// A pull request of a repository.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrRef {
    /// The repository.
    pub repo: RepoRef,
    /// The pull request number.
    pub number: u32,
}

/// Lines `start_line..=end_line`, 1-based, at the head commit unless stated otherwise.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
pub struct LineRange {
    /// First line, 1-based.
    pub start_line: u32,
    /// Last line, inclusive.
    pub end_line: u32,
}

/// A line range of one file.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
pub struct FileRange {
    /// Repository-relative path.
    pub file: String,
    /// The lines.
    pub range: LineRange,
}

/// Several line ranges of one file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FileRanges {
    /// Repository-relative path.
    pub file: String,
    /// The lines, ascending and disjoint.
    pub ranges: Vec<LineRange>,
}

/// A line the agent actually read and cites as evidence.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
pub struct Evidence {
    /// Repository-relative path at the head commit.
    pub file: String,
    /// 1-based line number at the head commit.
    pub line: u32,
}

/// Whether a statement was read in the code or inferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// Read in the code; needs at least one evidence line.
    Fact,
    /// Not directly read; a conclusion.
    Inference,
}

/// Risk at system scale, rated by the LLM (no numbers, brief §1).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum RiskBand {
    /// Small or contained effect.
    Low,
    /// Noticeable in some situations or by some users.
    Medium,
    /// A production user or operator would clearly notice.
    High,
}

/// Classification of a flow (brief §3, closed list).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    /// Changes observable behaviour.
    Behavioural,
    /// Changes a public API or contract.
    ApiContract,
    /// Adds new code.
    NewCode,
    /// Claimed refactor without behaviour change.
    RefactorPure,
    /// Moves code.
    Move,
    /// Renames identifiers.
    Rename,
    /// Test code.
    Test,
    /// Documentation and comments.
    DocsComments,
    /// Generated or mechanical changes.
    GeneratedMechanical,
    /// Configuration and build.
    ConfigBuild,
    /// Dependency changes.
    Dependency,
    /// Anything else.
    Other,
}

/// Kind of a step (fragment) of a flow (brief §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum FragmentKind {
    /// Logic changed in place.
    Modified,
    /// New code.
    Added,
    /// Block moved from elsewhere.
    Moved,
    /// Identifier rename only.
    Renamed,
    /// Code removed here.
    DeletedSite,
    /// Unchanged code connecting the flow.
    Context,
}

/// Kind of a flow-graph node, derived from the step kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// The step's code is new.
    Added,
    /// The step's code changed.
    Changed,
    /// Unchanged context.
    Unchanged,
}

impl From<FragmentKind> for NodeKind {
    fn from(kind: FragmentKind) -> Self {
        match kind {
            FragmentKind::Added => Self::Added,
            FragmentKind::Context => Self::Unchanged,
            FragmentKind::Modified
            | FragmentKind::Moved
            | FragmentKind::Renamed
            | FragmentKind::DeletedSite => Self::Changed,
        }
    }
}

/// Type of an annotation inside a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationType {
    /// Behaviour change, was → is.
    Behaviour,
    /// A consequence of the change.
    Consequence,
    /// A question for the reviewer to check (also LLM suspicions).
    Question,
    /// Any other remark.
    Note,
}

/// Whether an issue was introduced by the pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Introduced by this pull request.
    Introduced,
    /// Existed before this pull request.
    PreExisting,
    /// Cannot be told from what was read.
    Unclear,
}

/// Merge impact of an assessed step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Blocking {
    /// Should be resolved before this pull request merges.
    BlocksThisPr,
    /// Can be handled in a follow-up.
    FollowUp,
}

/// Kind of related work in the PR context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RelatedWorkKind {
    /// Duplicates this change.
    Duplicate,
    /// A competing solution to the same problem.
    CompetingSolution,
    /// A pull request stacked on or under this one.
    StackedPr,
    /// An issue linked to this change.
    LinkedIssue,
    /// A matching change in a companion repository.
    CompanionRepository,
}

/// Topic of a fit question.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum FitTopic {
    /// Right layer or repository?
    LayerOrRepository,
    /// Oversized for the problem?
    SizeForProblem,
    /// Duplicates an existing mechanism?
    DuplicateMechanism,
    /// Another fit question.
    Other,
}

/// LLM status of an existing GitHub review thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ThreadLlmStatus {
    /// The thread still applies to the current code.
    StillValid,
    /// The thread refers to code that no longer exists here.
    Stale,
    /// The current code already addresses the thread.
    AlreadyAddressed,
}

/// A deterministic check that a badge names (brief §1, §3). No badge means LLM analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceBadge {
    /// Moved block equal to its original after whitespace normalisation.
    TextIdentical,
    /// Recognised by a path rule.
    ClassifiedByPath,
    /// Recognised by a line rule.
    ClassifiedByLineRule,
}

/// Section of material that is not analysed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum NotAnalysedCategory {
    /// Tests.
    Tests,
    /// Docs and comments.
    DocsComments,
    /// Mechanical and generated material, lockfiles included.
    GeneratedMechanical,
}

/// Side of a diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum DiffSide {
    /// Base (old) side, e.g. a deleted line.
    Left,
    /// Head (new) side.
    Right,
}

/// Language of LLM-written texts.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum LlmLanguage {
    /// English (default).
    #[default]
    En,
    /// Polish.
    Pl,
}
