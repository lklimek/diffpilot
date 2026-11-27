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

impl RepoRef {
    /// Whether this is a canonical GitHub issue URL in this repository.
    pub fn contains_issue_url(&self, reference: &str) -> bool {
        let Some(path) = reference.strip_prefix("https://github.com/") else {
            return false;
        };
        let mut parts = path.splitn(4, '/');
        let (Some(owner), Some(name), Some("issues"), Some(number)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        owner.eq_ignore_ascii_case(&self.owner)
            && name.eq_ignore_ascii_case(&self.name)
            && !number.starts_with('0')
            && number.bytes().all(|byte| byte.is_ascii_digit())
            && number.parse::<u64>().is_ok()
    }
}

/// A pull request of a repository.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrRef {
    /// The repository.
    pub repo: RepoRef,
    /// The pull request number.
    pub number: u32,
}

/// Commit a line number refers to. Head is the common case (final code); base only for code that no longer
/// exists at the head commit (a deleted file, an emptied file, removed lines without a head line to anchor to).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
    TS,
)]
#[serde(rename_all = "snake_case")]
pub enum LineSide {
    /// Lines of the head commit.
    #[default]
    Head,
    /// Lines of the base commit (removed code).
    Base,
}

/// Lines `start_line..=end_line`, 1-based, on the side given by the enclosing type.
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
    /// Repository-relative path on that side.
    pub file: String,
    /// Commit the lines refer to.
    pub side: LineSide,
    /// The lines.
    pub range: LineRange,
}

/// Several line ranges of one file on one side.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FileRanges {
    /// Repository-relative path on that side.
    pub file: String,
    /// Commit the lines refer to.
    pub side: LineSide,
    /// The lines, ascending and disjoint.
    pub ranges: Vec<LineRange>,
}

/// A line the agent actually read and cites as evidence.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
pub struct Evidence {
    /// Repository-relative path on that side.
    pub file: String,
    /// head for the checked-out code; base only for removed code read from the base commit.
    pub side: LineSide,
    /// 1-based line number on that side.
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationType {
    /// Behaviour change, was → is.
    Behaviour,
    /// A consequence of the change.
    Consequence,
    /// A risk the LLM states as a concrete scenario (brief §1 addendum 2026-11-18).
    Flag,
    /// Any other remark.
    Note,
}

// Hand-written so that `question`, stored before flags, reads as `flag` (ts-rs does not support `alias`).
impl<'de> Deserialize<'de> for AnnotationType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const NAMES: &[&str] = &["behaviour", "consequence", "flag", "note"];
        let name = String::deserialize(deserializer)?;
        match name.as_str() {
            "behaviour" => Ok(Self::Behaviour),
            "consequence" => Ok(Self::Consequence),
            "flag" | "question" => Ok(Self::Flag),
            "note" => Ok(Self::Note),
            other => Err(serde::de::Error::unknown_variant(other, NAMES)),
        }
    }
}

/// How strongly the LLM flags a risk: its own judgement, not checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum FlagLevel {
    /// A likely defect or damage if the scenario is reachable.
    Red,
    /// Worth a look: plausible but weaker, or of lower cost.
    Yellow,
    /// A flag stored before flags had levels (a former question to check); never in new agent output.
    Unknown,
}

/// The scenario of a flag; its title (the risk class, "Potential …") is the annotation's `text`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Flag {
    /// red or yellow in agent output; unknown only on a flag stored before levels.
    pub level: FlagLevel,
    /// Ordered steps: who acts, the order of events, the outcome and its cost.
    pub scenario: Vec<String>,
    /// One line: what was not reproduced or run.
    pub not_checked: String,
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
    /// No blocking action or follow-up proposed; this is not an assurance of safety.
    NoActionProposed,
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
    /// Git treats the file as binary (content check); it is not shown or analysed.
    BinaryContent,
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
    /// Russian.
    Ru,
}
