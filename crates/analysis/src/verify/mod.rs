//! Verification of agent output against the deterministic facts (brief §5 Pipeline 6, ADR-008): schema-valid
//! output is checked for ranges, hunk coverage, ids, graph consistency, fact ⇒ evidence, move claims and thread
//! aliases; issues feed the repair prompt. Bodies filled in by WP3-VERIFY.

use std::collections::BTreeMap;
use std::path::Path;

use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::agent::plan::{PlanOutput, PlannedFlow};
use contracts::agent::pr_context::PrContextOutput;
use contracts::api::tour::{FlowGraph, MoveFact};
use contracts::common::{FileRanges, LineSide};
use contracts::ids::{CommitSha, FlowId, StepId};

use crate::diff::DiffIndex;
use crate::moves::MovesReport;

/// Line facts of the files the agent output refers to, on both sides (base for deleted or emptied files,
/// removed blocks and base-side evidence).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineFacts {
    files: BTreeMap<(LineSide, String), FileFacts>,
}

/// Line facts of one file on one side.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileFacts {
    /// Number of lines.
    pub line_count: u32,
    /// Blank lines (trimmed from step range edges).
    pub blank_lines: Vec<u32>,
}

impl LineFacts {
    /// Reads the facts of `files` (side, path) at `base` or `head` from the repository at `repo` (read-only).
    pub fn load(
        repo: &Path,
        base: &CommitSha,
        head: &CommitSha,
        files: &[(LineSide, String)],
    ) -> Result<Self, VerifyError> {
        let _ = (repo, base, head, files);
        Err(VerifyError::NotImplemented {
            operation: "verify::LineFacts::load",
        })
    }

    /// Adds or replaces the facts of `path` on `side`.
    pub fn insert(&mut self, side: LineSide, path: &str, facts: FileFacts) {
        self.files.insert((side, path.to_owned()), facts);
    }

    /// The facts of `path` on `side`, if loaded (absent: the file does not exist on that side).
    pub fn get(&self, side: LineSide, path: &str) -> Option<&FileFacts> {
        self.files.get(&(side, path.to_owned()))
    }
}

/// A rejected detail of agent output; `message` is written for the repair prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyIssue {
    /// What is wrong.
    pub kind: VerifyIssueKind,
    /// Plain description with the location.
    pub message: String,
}

/// Kind of verification issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VerifyIssueKind {
    /// A hunk id not in the pre-pass.
    UnknownHunk,
    /// A hunk sub-range outside its hunk.
    RangeOutsideHunk,
    /// A changed line assigned to no flow and not listed as unassigned.
    LineUnassigned,
    /// A changed line assigned twice.
    LineAssignedTwice,
    /// A line number outside its file.
    LineOutOfRange,
    /// A changed line of the flow outside every step.
    ChangedLineUncovered,
    /// Two flows or steps share an id.
    DuplicateId,
    /// A reference to a step or flow that does not exist.
    UnknownReference,
    /// An edge whose end is not a step.
    DanglingEdge,
    /// A `fact` without evidence.
    FactWithoutEvidence,
    /// An annotation with an empty `was` or `is` (the highlight tooltip needs both).
    BehaviourMissing,
    /// `text` null on a consequence, question or note, or set on a behaviour annotation.
    RemarkMismatch,
    /// A base-side step that is not `deleted_site`, or base lines outside the file's hunks.
    BaseSideMisused,
    /// A thread alias that was not listed.
    UnknownThread,
}

/// Verification could not run or rejected the output.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// The output has issues; repair with their messages.
    #[error("the agent output has {} issue(s)", .0.len())]
    Rejected(Vec<VerifyIssue>),
    /// Reading head files failed.
    #[error("could not read the head files")]
    Io(#[from] std::io::Error),
    /// Declared but not implemented yet.
    #[error("the analysis operation {operation} is not implemented yet")]
    NotImplemented {
        /// The operation.
        operation: &'static str,
    },
}

/// A verified plan with its hunk selections resolved to lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedPlan {
    /// The plan as produced by the agent.
    pub output: PlanOutput,
    /// Changed head lines of each flow.
    pub flow_lines: BTreeMap<FlowId, Vec<FileRanges>>,
    /// Changed head lines the plan left unassigned.
    pub unassigned: Vec<FileRanges>,
}

/// A wording rule hit in LLM text (logged as a warning, never a rejection; plan §12.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordingWarning {
    /// The banned phrase or pattern.
    pub phrase: String,
    /// Where it occurred (field path).
    pub location: String,
}

/// A verified flow analysis, ready to be projected into the Tour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedFlow {
    /// The flow.
    pub flow_id: FlowId,
    /// The agent output with step ranges trimmed of blank edge lines.
    pub output: FlowAnalysisOutput,
    /// Server step ids in step order (`<flow>.<n>`).
    pub step_ids: Vec<StepId>,
    /// Graph with nodes derived from the steps.
    pub graph: FlowGraph,
    /// Changed lines covered by the steps.
    pub covered: Vec<FileRanges>,
    /// Deterministic move facts per moved step.
    pub moves: Vec<(StepId, MoveFact)>,
    /// Wording warnings.
    pub warnings: Vec<WordingWarning>,
}

/// What verifying one flow needs besides its output.
#[derive(Clone, Copy, Debug)]
pub struct FlowContext<'a> {
    /// The flow as planned.
    pub planned: &'a PlannedFlow,
    /// The verified plan.
    pub plan: &'a VerifiedPlan,
    /// Line facts of both sides.
    pub facts: &'a LineFacts,
    /// The diff: hunk paths on both sides and removed lines → head deletion markers.
    pub diff: &'a DiffIndex<'a>,
    /// Deterministic moves.
    pub moves: &'a MovesReport,
    /// Aliases of the GitHub threads listed in the prompt (`t1`, …).
    pub thread_aliases: &'a [String],
}

/// Verifies a `plan` output against the pre-pass hunks. Hunks resolve through `diff` to their own file and
/// side (two files may share line numbers; a renamed file has a base and a head path).
pub fn plan(output: &PlanOutput, diff: &DiffIndex<'_>) -> Result<VerifiedPlan, VerifyError> {
    let _ = (output, diff);
    Err(VerifyError::NotImplemented {
        operation: "verify::plan",
    })
}

/// Verifies a `pr_context` output (evidence lines inside their file on their side).
pub fn pr_context(output: &PrContextOutput, facts: &LineFacts) -> Result<(), VerifyError> {
    let _ = (output, facts);
    Err(VerifyError::NotImplemented {
        operation: "verify::pr_context",
    })
}

/// Verifies an `analyse_flow` output.
pub fn flow(
    output: &FlowAnalysisOutput,
    context: FlowContext<'_>,
) -> Result<VerifiedFlow, VerifyError> {
    let _ = (output, context);
    Err(VerifyError::NotImplemented {
        operation: "verify::flow",
    })
}

#[cfg(test)]
mod tests {
    use contracts::common::LineSide;

    use super::*;

    #[test]
    fn facts_are_kept_per_side() {
        let mut facts = LineFacts::default();
        facts.insert(
            LineSide::Base,
            "src/rpc/old.rs",
            FileFacts {
                line_count: 40,
                blank_lines: vec![12],
            },
        );
        facts.insert(
            LineSide::Head,
            "src/rpc/old.rs",
            FileFacts {
                line_count: 0,
                blank_lines: vec![],
            },
        );
        assert_eq!(
            facts
                .get(LineSide::Base, "src/rpc/old.rs")
                .map(|f| f.line_count),
            Some(40)
        );
        assert_eq!(
            facts
                .get(LineSide::Head, "src/rpc/old.rs")
                .map(|f| f.line_count),
            Some(0),
            "an emptied file"
        );
        assert!(facts.get(LineSide::Head, "src/rpc/gone.rs").is_none());
    }
}
