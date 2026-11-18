//! Verification of agent output against the deterministic facts (brief §5 Pipeline 6, ADR-008): schema-valid
//! output is checked for ranges, hunk coverage, ids, graph consistency, fact ⇒ evidence, move claims and thread
//! aliases; issues feed the repair prompt. [`analysis`] verifies a whole analysis and orders the Tour.

mod check_flow;
mod check_plan;
mod facts;
mod plain;
pub(crate) mod spans;
pub mod wording;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::agent::ask::AskOutput;
use contracts::agent::plan::{PlanOutput, PlannedFlow};
use contracts::agent::pr_context::PrContextOutput;
use contracts::api::tour::{FlowGraph, MoveFact};
use contracts::common::{Evidence, FileRanges, LineSide};
use contracts::ids::{CommitSha, FlowId, StepId};

pub use facts::MAX_FACTS_BYTES;

use crate::blob::BlobError;
use crate::coverage::{self, Coverage, CoverageError};
use crate::diff::DiffIndex;
use crate::moves::MovesReport;
use crate::order::{OrderInput, tour_order};

/// Line facts of the files the agent output refers to, on both sides (base for deleted or emptied files,
/// removed blocks and base-side evidence).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineFacts {
    files: BTreeMap<(LineSide, String), FileFacts>,
    /// Files of the pinned tree that were not read: larger than the per-file limit or over the budget.
    unread: BTreeMap<(LineSide, String), u64>,
}

/// Line facts of one file on one side.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileFacts {
    /// Number of lines.
    pub line_count: u32,
    /// Blank lines (trimmed from step range edges).
    pub blank_lines: Vec<u32>,
}

impl FileFacts {
    /// Whether `line` is blank (whitespace only); `blank_lines` is ascending.
    pub fn is_blank(&self, line: u32) -> bool {
        self.blank_lines.binary_search(&line).is_ok()
    }
}

impl LineFacts {
    /// Reads the facts of `files` (side, path) at `base` or `head` from the repository at `repo` (read-only).
    /// Paths that are not files of the pinned tree of their side (missing, a directory, absolute, with `.` or
    /// `..` components) are left out, so verification rejects references to them. Reads are bounded by
    /// [`crate::blob::MAX_BLOB_BYTES`] per file, [`MAX_FACTS_BYTES`] per load and [`crate::blob::READ_TIMEOUT`];
    /// files beyond the limits are recorded as unread.
    pub fn load(
        repo: &Path,
        base: &CommitSha,
        head: &CommitSha,
        files: &[(LineSide, String)],
    ) -> Result<Self, VerifyError> {
        facts::load(repo, base, head, files)
    }

    /// Adds or replaces the facts of `path` on `side`.
    pub fn insert(&mut self, side: LineSide, path: &str, facts: FileFacts) {
        self.files.insert((side, path.to_owned()), facts);
    }

    /// The facts of `path` on `side`, if loaded (absent: the file does not exist on that side or was not read).
    pub fn get(&self, side: LineSide, path: &str) -> Option<&FileFacts> {
        self.files.get(&(side, path.to_owned()))
    }

    /// Records that `path` on `side` is a file of `size` bytes that was not read (over the read limits).
    pub fn insert_unread(&mut self, side: LineSide, path: &str, size: u64) {
        self.unread.insert((side, path.to_owned()), size);
    }

    /// The size of `path` on `side` if it is a file that was not read.
    pub fn unread_size(&self, side: LineSide, path: &str) -> Option<u64> {
        self.unread.get(&(side, path.to_owned())).copied()
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
    /// A behaviour annotation with an empty `was` or `is`.
    BehaviourMissing,
    /// `text` null on a consequence, question or note, or set on a behaviour annotation.
    RemarkMismatch,
    /// A base-side step that is not `deleted_site`, or base lines outside the file's hunks.
    BaseSideMisused,
    /// A thread alias that was not listed.
    UnknownThread,
    /// A path that is not a file of the pinned tree on its side.
    UnknownFile,
    /// A required text that is empty or whitespace only.
    EmptyText,
    /// A move claim that contradicts the step kind or the deterministic moved block at the step's lines.
    MoveClaimMismatch,
    /// A listed thread alias without a status.
    ThreadStatusMissing,
    /// A file too large to verify (over the per-file read limit or the read budget of one output).
    FileTooLarge,
    /// An impact reference on changed lines; impact references name code outside the diff.
    ImpactInsideDiff,
    /// Markup or a link other than `https://github.com/…` in text shown as plain text (an Ask answer).
    MarkupOrLink,
    /// Reviewer-facing text describes missing agent capabilities instead of the change.
    AgentLimitation,
    /// A risk assessment has no concrete correction or question to resolve.
    AssessmentActionMissing,
    /// A step has no assessment from which to derive the flow rating.
    AssessmentMissing,
    /// A flow output does not match the job schema; the pipeline requests repair before parsing it.
    SchemaMismatch,
    /// A stacked relationship asserted without a server-supplied deterministic fact.
    UnconfirmedStack,
}

impl VerifyIssueKind {
    /// Stable snake_case code for the repair prompt and logs.
    pub fn code(self) -> &'static str {
        match self {
            Self::UnknownHunk => "unknown_hunk",
            Self::RangeOutsideHunk => "range_outside_hunk",
            Self::LineUnassigned => "line_unassigned",
            Self::LineAssignedTwice => "line_assigned_twice",
            Self::LineOutOfRange => "line_out_of_range",
            Self::ChangedLineUncovered => "changed_line_uncovered",
            Self::DuplicateId => "duplicate_id",
            Self::UnknownReference => "unknown_reference",
            Self::DanglingEdge => "dangling_edge",
            Self::FactWithoutEvidence => "fact_without_evidence",
            Self::BehaviourMissing => "behaviour_missing",
            Self::RemarkMismatch => "remark_mismatch",
            Self::BaseSideMisused => "base_side_misused",
            Self::UnknownThread => "unknown_thread",
            Self::UnknownFile => "unknown_file",
            Self::EmptyText => "empty_text",
            Self::MoveClaimMismatch => "move_claim_mismatch",
            Self::ThreadStatusMissing => "thread_status_missing",
            Self::FileTooLarge => "file_too_large",
            Self::ImpactInsideDiff => "impact_inside_diff",
            Self::MarkupOrLink => "markup_or_link",
            Self::AgentLimitation => "agent_limitation",
            Self::SchemaMismatch => "schema_mismatch",
            Self::AssessmentMissing => "assessment_missing",
            Self::AssessmentActionMissing => "assessment_action_missing",
            Self::UnconfirmedStack => "unconfirmed_stack",
        }
    }
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
    /// The files of the analysed commits could not be read.
    #[error("the files of the analysed commits could not be read")]
    Read(#[from] BlobError),
    /// The coverage inputs do not match the diff.
    #[error("the coverage of the changed lines could not be computed")]
    Coverage(#[from] CoverageError),
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
    /// Changed lines of each flow: head lines and the removed base lines that go with them.
    pub flow_lines: BTreeMap<FlowId, Vec<FileRanges>>,
    /// Changed lines the plan left unassigned.
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
    /// The flow's planned changed lines, all inside its steps (blank lines need no step).
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

/// Verifies a `plan` output against the hunks listed in the plan prompt (`diff`, quiet hunks left out). Hunks
/// resolve through `diff` to their own file and side (two files may share line numbers; a renamed file has a
/// base and a head path).
pub fn plan(output: &PlanOutput, diff: &DiffIndex<'_>) -> Result<VerifiedPlan, VerifyError> {
    check_plan::plan(output, diff)
}

/// [`plan`] for a plan accepted and stored earlier, read again: lines it leaves in no flow and not in
/// `unassigned` (today's rules require more than the rules it was written for) are Unexplained, not an issue.
pub fn stored_plan(output: &PlanOutput, diff: &DiffIndex<'_>) -> Result<VerifiedPlan, VerifyError> {
    check_plan::stored_plan(output, diff)
}

/// Verifies a `pr_context` output (required texts; evidence lines inside their file on their side).
pub fn pr_context(output: &PrContextOutput, facts: &LineFacts) -> Result<(), VerifyError> {
    let mut issues = Issues::default();
    issues.text("problem", &output.problem);
    issues.text("why_needed", &output.why_needed);
    issues.text("what_it_gives", &output.what_it_gives);
    for (index, work) in output.related_work.iter().enumerate() {
        // TODO: accept stacked relationships only after the server supplies verified base/head facts.
        if work.kind == contracts::common::RelatedWorkKind::StackedPr {
            issues.push(VerifyIssueKind::UnconfirmedStack,
                format!("related_work[{index}]: no deterministic stacked relationship was supplied; omit this item or use an evidenced non-stacked kind"));
        }
        issues.text(&format!("related_work[{index}].reference"), &work.reference);
        issues.text(&format!("related_work[{index}].overlap"), &work.overlap);
    }
    for (index, question) in output.fit_questions.iter().enumerate() {
        let at = format!("fit_questions[{index}]");
        issues.text(&format!("{at}.question"), &question.question);
        issues.evidence(facts, &at, &question.evidence);
    }
    issues.finish(())
}

/// Verifies an `ask` output: a non-blank answer shown as plain text (no markup, no link except
/// `https://github.com/…`) and evidence lines inside their file on their side.
pub fn ask(output: &AskOutput, facts: &LineFacts) -> Result<(), VerifyError> {
    let mut issues = Issues::default();
    issues.text("answer", &output.answer);
    if let Some(found) = plain::check(&output.answer) {
        issues.push(
            VerifyIssueKind::MarkupOrLink,
            format!("answer contains {}; write plain text", found.describe()),
        );
    }
    issues.evidence(facts, "answer", &output.evidence);
    issues.finish(())
}

/// The (side, path) pairs whose [`LineFacts`] verifying an `ask` output needs.
pub fn ask_files(output: &AskOutput) -> Vec<(LineSide, String)> {
    let files: BTreeSet<_> = output.evidence.iter().map(evidence_file).collect();
    files.into_iter().collect()
}

/// Verifies an `analyse_flow` output.
pub fn flow(
    output: &FlowAnalysisOutput,
    context: FlowContext<'_>,
) -> Result<VerifiedFlow, VerifyError> {
    check_flow::flow(output, context)
}

/// The (side, path) pairs whose [`LineFacts`] verifying `output` needs: every location of the output and the
/// files of the flow's planned lines (`flow_lines`, for blank changed lines).
pub fn flow_files(
    output: &FlowAnalysisOutput,
    flow_lines: &[FileRanges],
) -> Vec<(LineSide, String)> {
    let mut files = BTreeSet::new();
    for step in &output.steps {
        files.insert((step.side, step.file.clone()));
        if let Some(from) = &step.moved_from {
            files.insert((LineSide::Base, from.file.clone()));
        }
        for annotation in &step.annotations {
            files.extend(annotation.evidence.iter().map(evidence_file));
        }
    }
    let impacts = output.impact_refs.iter();
    files.extend(impacts.map(|impact| (LineSide::Head, impact.file.clone())));
    for pattern in &output.pattern_repetitions {
        files.extend(
            pattern
                .sites
                .iter()
                .map(|site| (site.side, site.file.clone())),
        );
    }
    files.extend(
        flow_lines
            .iter()
            .map(|lines| (lines.side, lines.file.clone())),
    );
    files.into_iter().collect()
}

/// The (side, path) pairs whose [`LineFacts`] verifying a `pr_context` output needs.
pub fn pr_context_files(output: &PrContextOutput) -> Vec<(LineSide, String)> {
    let questions = output.fit_questions.iter();
    let files: BTreeSet<_> = questions
        .flat_map(|question| question.evidence.iter().map(evidence_file))
        .collect();
    files.into_iter().collect()
}

fn evidence_file(evidence: &Evidence) -> (LineSide, String) {
    (evidence.side, evidence.file.clone())
}

/// Reads the line facts of the given files (side, path), e.g. with [`LineFacts::load`].
pub type FactsReader<'a> = dyn Fn(&[(LineSide, String)]) -> Result<LineFacts, VerifyError> + 'a;

/// Everything verifying one analysis needs; plain inputs, IO only through `facts`.
#[derive(Clone, Copy)]
pub struct AnalysisInput<'a> {
    /// The whole diff of the pull request.
    pub diff: &'a DiffIndex<'a>,
    /// The hunks listed in the plan prompt (the diff without quiet hunks).
    pub listed: &'a DiffIndex<'a>,
    /// Changed lines the pre-pass took out of the agent's hunks: Not analysed items and identical moves.
    pub quiet: &'a [FileRanges],
    /// Reads the line facts of the given files (usually [`LineFacts::load`]); called once per output with that
    /// output's files ([`pr_context_files`], [`flow_files`]), so each output keeps its own read budget.
    pub facts: &'a FactsReader<'a>,
    /// Deterministic moves.
    pub moves: &'a MovesReport,
    /// The `plan` output.
    pub plan: &'a PlanOutput,
    /// The `pr_context` output.
    pub pr_context: &'a PrContextOutput,
    /// The current `analyse_flow` output of each analysed flow (one per flow; a repaired output replaces the
    /// rejected one).
    pub flows: &'a [FlowAnalysisOutput],
    /// Thread aliases listed in each flow's prompt; a flow without an entry lists none.
    pub thread_aliases: &'a BTreeMap<FlowId, Vec<String>>,
    /// The outputs were accepted earlier and are read again: lines today's rules add that the plan or a flow does
    /// not cover are Unexplained, not a rejection. A new output is checked with `false`.
    pub stored: bool,
}

impl std::fmt::Debug for AnalysisInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnalysisInput")
            .field("plan", self.plan)
            .field("pr_context", self.pr_context)
            .field("flows", &self.flows.len())
            .finish_non_exhaustive()
    }
}

/// The agent output an issue or warning belongs to: the job to repair.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OutputRef {
    /// The `plan` output.
    Plan,
    /// The `pr_context` output.
    PrContext,
    /// The `analyse_flow` output of a flow.
    Flow(FlowId),
}

/// Structured repair feedback: one rejected output with its issues.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repair {
    /// The output to repair.
    pub output: OutputRef,
    /// What is wrong with it.
    pub issues: Vec<VerifyIssue>,
}

/// The verdict on one analysis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisVerdict {
    /// Rejected outputs with their issues; empty when every output was accepted.
    pub repairs: Vec<Repair>,
    /// Wording warnings of all outputs (to be logged; never a rejection).
    pub warnings: Vec<(OutputRef, WordingWarning)>,
    /// The Tour skeleton; `None` while the plan is rejected.
    pub tour: Option<TourSkeleton>,
    /// The move detection ran out of its work budget (`MovesReport::incomplete`): some moves have no fact. For
    /// the pipeline to record; never a rejection.
    pub moves_incomplete: bool,
}

/// The ordered Tour content built from the accepted outputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TourSkeleton {
    /// The verified plan.
    pub plan: VerifiedPlan,
    /// Accepted flows in tour order: behaviour by importance, LLM-classified refactors, LLM-classified tests.
    pub flows: Vec<VerifiedFlow>,
    /// Planned flows without an accepted output, in tour order; their lines are Unexplained.
    pub unanalysed: Vec<FlowId>,
    /// Coverage over the accepted flows and the quiet lines; Unexplained is the first quiet row.
    pub coverage: Coverage,
}

/// Verifies every output of one analysis and orders the Tour.
///
/// The plan is checked against `listed`, each flow output against its planned flow, the PR context on its
/// own. Rejected outputs come back as [`Repair`]s (the pipeline repairs each at most twice). A flow without
/// an accepted output is `unanalysed`; its changed lines, like the plan's `unassigned` lines, are
/// Unexplained, so every changed line is in exactly one flow, a quiet item or Unexplained.
///
/// # Errors
/// [`VerifyError::Coverage`] when `quiet` names a file that is not in the diff.
pub fn analysis(input: AnalysisInput<'_>) -> Result<AnalysisVerdict, VerifyError> {
    let mut repairs = Vec::new();
    let mut warnings: Vec<(OutputRef, WordingWarning)> = Vec::new();
    let mut warn = |output: OutputRef, found: Vec<WordingWarning>| {
        warnings.extend(found.into_iter().map(|warning| (output.clone(), warning)));
    };
    warn(OutputRef::Plan, wording::plan(input.plan));
    warn(OutputRef::PrContext, wording::pr_context(input.pr_context));
    let checked = if input.stored {
        check_plan::stored_plan(input.plan, input.listed)
    } else {
        plan(input.plan, input.listed)
    };
    let verified_plan = match rejected(checked)? {
        Ok(verified) => Some(verified),
        Err(issues) => {
            repairs.push(Repair {
                output: OutputRef::Plan,
                issues,
            });
            None
        }
    };
    let context_facts = (input.facts)(&pr_context_files(input.pr_context))?;
    if let Err(issues) = rejected(pr_context(input.pr_context, &context_facts))? {
        repairs.push(Repair {
            output: OutputRef::PrContext,
            issues,
        });
    }
    let Some(verified_plan) = verified_plan else {
        return Ok(AnalysisVerdict {
            repairs,
            warnings,
            tour: None,
            moves_incomplete: input.moves.incomplete,
        });
    };
    let mut accepted: BTreeMap<FlowId, VerifiedFlow> = BTreeMap::new();
    for output in input.flows {
        let id = &output.flow_id;
        let planned = input.plan.flows.iter().find(|flow| &flow.id == id);
        let result = match planned {
            _ if accepted.contains_key(id) => Err(vec![issue(
                VerifyIssueKind::DuplicateId,
                format!("flow_id: a second output for flow {id}"),
            )]),
            None => Err(vec![issue(
                VerifyIssueKind::UnknownReference,
                format!("flow_id: {id} is not a flow of the plan"),
            )]),
            Some(planned) => {
                let aliases = input.thread_aliases.get(id).map_or(&[][..], Vec::as_slice);
                let flow_lines = verified_plan
                    .flow_lines
                    .get(id)
                    .map_or(&[][..], Vec::as_slice);
                let facts = (input.facts)(&flow_files(output, flow_lines))?;
                let context = FlowContext {
                    planned,
                    plan: &verified_plan,
                    facts: &facts,
                    diff: input.diff,
                    moves: input.moves,
                    thread_aliases: aliases,
                };
                let missing = if input.stored {
                    check_flow::uncovered_lines(output, context)
                } else {
                    BTreeMap::new()
                };
                if missing.is_empty() {
                    rejected(flow(output, context))?
                } else {
                    let reduced = without_lines(&verified_plan, id, &missing);
                    rejected(flow(
                        output,
                        FlowContext {
                            plan: &reduced,
                            ..context
                        },
                    ))?
                }
            }
        };
        match result {
            Ok(verified) => {
                warn(OutputRef::Flow(id.clone()), verified.warnings.clone());
                accepted.insert(id.clone(), verified);
            }
            Err(issues) => repairs.push(Repair {
                output: OutputRef::Flow(id.clone()),
                issues,
            }),
        }
    }
    let planned: Vec<OrderInput> = input.plan.flows.iter().map(OrderInput::from).collect();
    let mut flows = Vec::with_capacity(accepted.len());
    let mut unanalysed = Vec::new();
    for id in tour_order(&planned) {
        match accepted.remove(&id) {
            Some(verified) => flows.push(verified),
            None => unanalysed.push(id),
        }
    }
    let coverage = coverage::check(input.diff.files(), input.quiet, &flows)?;
    Ok(AnalysisVerdict {
        repairs,
        warnings,
        tour: Some(TourSkeleton {
            plan: verified_plan,
            flows,
            unanalysed,
            coverage,
        }),
        moves_incomplete: input.moves.incomplete,
    })
}

/// `plan` with the `missing` lines taken out of flow `id`'s lines.
fn without_lines(
    plan: &VerifiedPlan,
    id: &FlowId,
    missing: &BTreeMap<(LineSide, String), Vec<spans::Span>>,
) -> VerifiedPlan {
    let mut reduced = plan.clone();
    for ranges in reduced.flow_lines.get_mut(id).into_iter().flatten() {
        if let Some(gone) = missing.get(&(ranges.side, ranges.file.clone())) {
            let kept = spans::subtract(&spans::from_ranges(&ranges.ranges), gone);
            ranges.ranges = spans::to_ranges(&kept);
        }
    }
    reduced
}

/// Splits a verification result into accepted or rejected-with-issues; other errors pass through.
fn rejected<T>(result: Result<T, VerifyError>) -> Result<Result<T, Vec<VerifyIssue>>, VerifyError> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(VerifyError::Rejected(issues)) => Ok(Err(issues)),
        Err(other) => Err(other),
    }
}

fn issue(kind: VerifyIssueKind, message: String) -> VerifyIssue {
    VerifyIssue { kind, message }
}

/// Collects the issues of one output.
#[derive(Debug, Default)]
struct Issues(Vec<VerifyIssue>);

impl Issues {
    fn push(&mut self, kind: VerifyIssueKind, message: String) {
        self.0.push(issue(kind, message));
    }

    /// A required text must not be blank.
    fn text(&mut self, at: &str, text: &str) {
        self.reviewer_text(at, text);
        if text.trim().is_empty() {
            self.push(VerifyIssueKind::EmptyText, format!("{at} is empty"));
        }
    }

    fn reviewer_text(&mut self, at: &str, text: &str) {
        if wording::agent_limitation(text) {
            self.push(VerifyIssueKind::AgentLimitation,
                format!("{at}: describe the change and evidence, not missing agent tools, skills or instructions; do not claim checks you could not perform"));
        }
    }

    /// `path` must be a file on `side` and `lines` lines of it; true when they are.
    fn location(
        &mut self,
        facts: &LineFacts,
        at: &str,
        side: LineSide,
        path: &str,
        (start, end): (u32, u32),
    ) -> bool {
        let side_name = side_name(side);
        let Some(file) = facts.get(side, path) else {
            if let Some(size) = facts.unread_size(side, path) {
                let limit = crate::blob::MAX_BLOB_BYTES;
                let message = format!(
                    "{at}: {path} on {side_name} has {size} bytes, more than verification reads ({limit} bytes per \
                     file, {MAX_FACTS_BYTES} per output); refer to fewer or smaller files"
                );
                self.push(VerifyIssueKind::FileTooLarge, message);
            } else {
                let message = format!("{at}: {path} is not a file of the {side_name} commit");
                self.push(VerifyIssueKind::UnknownFile, message);
            }
            return false;
        };
        if start == 0 || start > end || end > file.line_count {
            let count = file.line_count;
            self.push(
                VerifyIssueKind::LineOutOfRange,
                format!(
                    "{at}: lines {start}-{end} are not lines of {path} on {side_name} (1-{count})"
                ),
            );
            return false;
        }
        true
    }

    /// Every evidence line must be a line of its file on its side.
    fn evidence(&mut self, facts: &LineFacts, at: &str, evidence: &[Evidence]) {
        for (index, item) in evidence.iter().enumerate() {
            let at = format!("{at}.evidence[{index}]");
            self.location(facts, &at, item.side, &item.file, (item.line, item.line));
        }
    }

    fn finish<T>(self, value: T) -> Result<T, VerifyError> {
        if self.0.is_empty() {
            Ok(value)
        } else {
            Err(VerifyError::Rejected(self.0))
        }
    }
}

fn side_name(side: LineSide) -> &'static str {
    match side {
        LineSide::Head => "head",
        LineSide::Base => "base",
    }
}

/// Spans per (side, path) as `FileRanges`, in key order.
fn to_file_ranges(lines: BTreeMap<(LineSide, String), Vec<spans::Span>>) -> Vec<FileRanges> {
    lines
        .into_iter()
        .map(|((side, file), lines)| FileRanges {
            file,
            side,
            ranges: spans::to_ranges(&spans::merge(lines)),
        })
        .collect()
}
