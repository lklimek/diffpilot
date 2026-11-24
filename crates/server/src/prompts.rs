//! Agent prompts per job kind: the templates of `prompts/` filled from typed inputs by pure functions (no IO).
//!
//! Every prompt starts with `Job: <kind>[ <flow>].` (the fake provider reads the first flow id), names the pull
//! request and head commit, and carries the rules block of `docs/contracts/agent-output.md`. Text from the
//! repository, the pull request, GitHub, MemCan or earlier agent output enters only as a data block whose fence id
//! never occurs in its content, so the content cannot end its block early.

use std::collections::{BTreeMap, BTreeSet};

use analysis::diff::{FileDiff, FileStatus, Hunk};
use analysis::moves::MovedBlock;
use analysis::prepass::{Disposition, NotRenderedReason, Prepass};
use analysis::verify::VerifyIssue;
use contracts::agent::analyse_flow::{
    AgentEdge, AgentImpactRef, AgentPatternRepetition, AgentStep, AgentThreadStatus,
    FlowAnalysisOutput, FlowIntroduction,
};
use contracts::agent::plan::{PlanOutput, PlannedFlow};
use contracts::agent::pr_context::PrContextOutput;
use contracts::agent::verify_comments::VerifyItem;
use contracts::api::deps::{DependencyStatus, DependencyView, RepositoryKind, TypeDefinition};
use contracts::api::files::HunkPart;
use contracts::api::pr::PrSummary;
use contracts::api::questions::AskState;
use contracts::api::tour::GithubThread;
use contracts::common::{Classification, FragmentKind, LineRange, LineSide, LlmLanguage};
use contracts::ids::{FlowId, HunkId, StepId};
use serde::Serialize;
use sha2::{Digest, Sha256};

const HEADER: &str = include_str!("../../../prompts/header.md");
const RULES: &str = include_str!("../../../prompts/rules.md");
const PARTS: &str = include_str!("../../../prompts/parts.md");
const PLAN: &str = include_str!("../../../prompts/plan.md");
const PR_CONTEXT: &str = include_str!("../../../prompts/pr_context.md");
const ANALYSE_FLOW: &str = include_str!("../../../prompts/analyse_flow.md");
const ASK: &str = include_str!("../../../prompts/ask.md");
const ASK_LINES: &str = include_str!("../../../prompts/ask_lines.md");
const ASK_FLOW: &str = include_str!("../../../prompts/ask_flow.md");
const ASK_PR: &str = include_str!("../../../prompts/ask_pr.md");
const REPAIR: &str = include_str!("../../../prompts/repair.md");
const CONTEXT_PACK: &str = include_str!("../../../prompts/context_pack.md");

/// Skills required by the shared prompt rules, including repair and Ask jobs.
pub fn required_skills() -> Vec<String> {
    contracts::agent::ANALYSIS_SKILLS
        .iter()
        .map(|skill| (*skill).to_owned())
        .collect()
}

/// Owner default for sub-agents: off (ADR-002; threat model T5, `[agents] enabled = false`).
pub const SUB_AGENTS_DEFAULT: bool = false;
/// Largest `plan`, `pr_context`, `analyse_flow` or `ask` prompt in bytes (about 32k tokens); a repair prompt
/// may add its quoted output. Untrusted texts are clipped to fixed caps, so only a hunk listing can exceed it:
/// the pipeline then plans in directory batches ([`PlanBatch`]).
pub const PROMPT_BUDGET_BYTES: usize = 128 * 1024;
/// Largest rejected output quoted in a repair prompt, in bytes.
pub const MAX_REJECTED_BYTES: usize = 160 * 1024;
/// Largest context pack in bytes, kept by omitting entries; the asked step of an `ask` pack is never shortened,
/// so only an asked step too large for the pack is an error.
pub const CONTEXT_PACK_BUDGET_BYTES: usize = 160 * 1024;
/// Repair runs allowed per output (brief §5 Pipeline 6).
pub const MAX_REPAIRS: u32 = 2;
/// Calibration examples quoted, at most.
pub const MAX_CALIBRATION_EXAMPLES: usize = 8;
/// Characters quoted per calibration example, at most.
pub const MAX_CALIBRATION_CHARS: usize = 1000;
/// GitHub threads listed in one `analyse_flow` prompt, at most ([`listed_thread_aliases`]).
pub const MAX_THREADS: usize = 50;

const MAX_TITLE_CHARS: usize = 300;
/// Most bytes of the per-crate dependency entries in a `pr_context` prompt.
const MAX_DEPENDENCY_BYTES: usize = 16 * 1024;
/// Most characters of a PR-controlled name or path in a dependency entry.
const MAX_DEPENDENCY_NAME_CHARS: usize = 80;
/// Most characters of a path in a dependency entry (cut in the middle; a line number is never cut).
const MAX_DEPENDENCY_PATH_CHARS: usize = 300;
/// Most facts of one crate in its dependency entry; the rest are counted.
const MAX_PROMPT_FACTS: usize = 15;
/// Most used crates of one crate in its dependency entry; the rest are counted.
const MAX_PROMPT_USES: usize = 20;
/// Most controlled dependency repositories the agent is given to look in (ADR-005 addendum).
const MAX_LOOKUP_REPOS: usize = 5;
/// Most crate names listed per lookup repository.
const MAX_LOOKUP_CRATES: usize = 10;
/// Longest PR description quoted in a prompt, in characters.
pub(crate) const MAX_DESCRIPTION_CHARS: usize = 16_000;
/// Longest project profile quoted in a prompt, in characters.
pub(crate) const MAX_PROFILE_CHARS: usize = 16_000;
const MAX_EXCERPT_CHARS: usize = 2_000;
const MAX_QUESTION_CHARS: usize = 4_000;
/// Classic diff quoted in an `ask` prompt about lines, at most.
const MAX_ASK_DIFF_BYTES: usize = 32 * 1024;
/// Line owners and overlapping steps quoted in an `ask` prompt about lines, at most (each).
const MAX_ASK_FACTS_BYTES: usize = 16 * 1024;
/// A flow quoted in an `ask` prompt about it, and the plan's flows and the PR context in one about the whole PR, at
/// most (each).
const MAX_ASK_CONTEXT_BYTES: usize = 32 * 1024;
/// Earlier questions of a thread quoted in a follow-up's prompt, at most; the first question always stays.
const MAX_ASK_THREAD_BYTES: usize = 32 * 1024;
/// Latest follow-ups quoted after a thread's first question, at most.
const MAX_THREAD_FOLLOW_UPS: usize = 6;
/// Characters quoted per earlier answer of a thread, at most.
const MAX_THREAD_ANSWER_CHARS: usize = 4_000;
const MAX_ISSUE_CHARS: usize = 1_000;
const MAX_LISTED_PATHS: usize = 50;
const MAX_LISTED_MOVES: usize = 100;
const PACK_PLAN_BYTES: usize = 16 * 1024;
const PACK_PR_CONTEXT_BYTES: usize = 16 * 1024;
const PACK_FLOWS_BYTES: usize = 48 * 1024;
const PACK_ASK_FLOW_BYTES: usize = 48 * 1024;
const PACK_ANSWERS_BYTES: usize = 16 * 1024;
/// Bound on a pack section's bytes besides its content: fences, label and an omission or clipping note.
const PACK_SECTION_OVERHEAD: usize = 512;

/// A prompt could not be rendered.
#[derive(Debug, thiserror::Error)]
pub enum PromptError {
    /// The prompt exceeds its budget (a plan over too many hunks: plan in directory batches).
    #[error("the {kind} prompt has {bytes} bytes, over its budget of {budget}")]
    OverBudget {
        /// Job kind.
        kind: &'static str,
        /// Rendered size.
        bytes: usize,
        /// The budget.
        budget: usize,
    },
    /// A repair attempt outside `1..=MAX_REPAIRS`.
    #[error("repair attempt {attempt} is not allowed (at most {MAX_REPAIRS})")]
    RepairAttempt {
        /// The attempt.
        attempt: u32,
    },
    /// A repair without issues.
    #[error("a repair prompt needs at least one issue")]
    NoIssues,
    /// A template names a placeholder or part that does not exist.
    #[error("a prompt template names the unknown placeholder {name}")]
    Template {
        /// The name.
        name: String,
    },
    /// The asked step id is not `<flow>.<n>` of a step of the given flow analysis.
    #[error("step {step_id} is not a step of the given flow analysis")]
    UnknownStep {
        /// The step id.
        step_id: String,
    },
    /// Stored agent output could not be serialised.
    #[error("agent output could not be serialised for the prompt")]
    Json(#[from] serde_json::Error),
}

/// What every job's prompt needs.
#[derive(Clone, Copy, Debug)]
pub struct JobContext<'a> {
    /// The pull request as analysed (its base is the analysis base).
    pub pr: &'a PrSummary,
    /// Language of the LLM texts.
    pub language: LlmLanguage,
    /// Whether the agent may delegate to sub-agents ([`SUB_AGENTS_DEFAULT`]).
    pub sub_agents: bool,
    /// The analysis's deterministic pre-pass.
    pub prepass: &'a Prepass,
}

/// The project profile sources; the repository file wins on conflict (brief §2).
#[derive(Clone, Copy, Debug, Default)]
pub struct Profile<'a> {
    /// `.claudius/project-profile.md` at base (`Prepass::profile_file`).
    pub file: Option<&'a str>,
    /// MemCan memory `type: project-profile`.
    pub memcan: Option<&'a str>,
}

/// One batch of a huge pull request's plan (ADR-008: plan jobs over directory batches).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanBatch {
    /// 1-based batch number.
    pub number: u32,
    /// Number of batches.
    pub total: u32,
}

/// Input of the `plan` prompt.
#[derive(Clone, Copy, Debug)]
pub struct PlanInput<'a> {
    /// Common context.
    pub job: JobContext<'a>,
    /// The diff (of this batch); hunks the pre-pass took out are not listed.
    pub files: &'a [FileDiff],
    /// Project profile.
    pub profile: Profile<'a>,
    /// The batch, for a huge pull request.
    pub batch: Option<PlanBatch>,
}

/// Input of the `pr_context` prompt.
#[derive(Clone, Copy, Debug)]
pub struct PrContextInput<'a> {
    /// Common context.
    pub job: JobContext<'a>,
    /// The pull request description, if the server has it.
    pub description: Option<&'a str>,
    /// Project profile.
    pub profile: Profile<'a>,
    /// The Dependencies view of the head (ADR-021), if computed.
    pub dependencies: Option<&'a DependencyView>,
}

/// A GitHub review thread listed for a flow under its alias (`t1`, …).
#[derive(Clone, Copy, Debug)]
pub struct PromptThread<'a> {
    /// Alias the agent answers with in `thread_statuses`.
    pub alias: &'a str,
    /// The thread.
    pub thread: &'a GithubThread,
}

/// Direction of a calibration example.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalibrationDirection {
    /// A risk band judged too high.
    Overrated,
    /// A risk band judged too low.
    Underrated,
}

/// A severity calibration example from MemCan (untrusted text).
#[derive(Clone, Copy, Debug)]
pub struct Calibration<'a> {
    /// Direction.
    pub direction: CalibrationDirection,
    /// Memory text.
    pub text: &'a str,
}

/// Input of the `analyse_flow` prompt.
#[derive(Clone, Copy, Debug)]
pub struct FlowInput<'a> {
    /// Common context.
    pub job: JobContext<'a>,
    /// The diff.
    pub files: &'a [FileDiff],
    /// The flow as planned.
    pub flow: &'a PlannedFlow,
    /// Project profile.
    pub profile: Profile<'a>,
    /// GitHub threads located in the flow, with their aliases.
    pub threads: &'a [PromptThread<'a>],
    /// Calibration examples; `None` when MemCan could not be read (not checked).
    pub calibration: Option<&'a [Calibration<'a>]>,
}

/// Input of the `ask` prompt.
#[derive(Clone, Copy, Debug)]
pub struct AskInput<'a> {
    /// Common context.
    pub job: JobContext<'a>,
    /// Server id of the step.
    pub step_id: &'a StepId,
    /// The step from its flow analysis.
    pub step: &'a AgentStep,
    /// Lines of the step the question is about; `None` for the whole step.
    pub lines: Option<LineRange>,
    /// The reviewer's question.
    pub question: &'a str,
    /// Earlier questions of its thread, oldest first; empty for a first question.
    pub thread: &'a [ThreadQuestion<'a>],
    /// Project profile.
    pub profile: Profile<'a>,
}

/// An earlier question of a follow-up's thread with its answer, if any.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct ThreadQuestion<'a> {
    /// The reviewer's question.
    pub question: &'a str,
    /// Its state when the follow-up was submitted.
    pub state: AskState,
    /// The answer text, once answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<&'a str>,
}

/// Input of the `ask` prompt about an analysed flow as a whole.
#[derive(Clone, Copy, Debug)]
pub struct AskFlowInput<'a> {
    /// Common context.
    pub job: JobContext<'a>,
    /// The flow's analysis.
    pub flow: &'a FlowAnalysisOutput,
    /// The reviewer's question.
    pub question: &'a str,
    /// Earlier questions of its thread, oldest first.
    pub thread: &'a [ThreadQuestion<'a>],
    /// Project profile.
    pub profile: Profile<'a>,
}

/// Input of the `ask` prompt about the whole pull request.
#[derive(Clone, Copy, Debug)]
pub struct AskPrInput<'a> {
    /// Common context.
    pub job: JobContext<'a>,
    /// The stored plan.
    pub plan: Option<&'a PlanOutput>,
    /// The stored PR context.
    pub pr_context: Option<&'a PrContextOutput>,
    /// The review comment the question is about (T-L4); `None` for a question about the whole PR.
    pub item: Option<&'a VerifyItem>,
    /// The reviewer's question.
    pub question: &'a str,
    /// Earlier questions of its thread, oldest first.
    pub thread: &'a [ThreadQuestion<'a>],
    /// Project profile.
    pub profile: Profile<'a>,
}

/// Lines a question is about without a single step (Files view): a range of a file, or the whole file.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct AskedLines<'a> {
    /// Repository-relative path on `side`.
    pub path: &'a str,
    /// Commit the lines refer to.
    pub side: LineSide,
    /// The lines; `None` for the whole file.
    #[serde(flatten)]
    pub range: Option<LineRange>,
}

/// A step in summary, as every prompt lists steps (title, place, kind).
#[derive(Clone, Copy, Debug, Serialize)]
pub struct StepSummary<'a> {
    /// The step's title.
    pub title: &'a str,
    /// Repository-relative path on `side`.
    pub file: &'a str,
    /// Side of the step's lines.
    pub side: LineSide,
    /// First line.
    pub start_line: u32,
    /// Last line.
    pub end_line: u32,
    /// Step kind.
    pub kind: FragmentKind,
}

impl<'a> StepSummary<'a> {
    fn of(step: &'a AgentStep) -> Self {
        Self {
            title: &step.title,
            file: &step.file,
            side: step.side,
            start_line: step.start_line,
            end_line: step.end_line,
            kind: step.kind,
        }
    }
}

/// A step of the analysis the asked lines touch, in summary, with its server id and flow.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct TouchedStep<'a> {
    /// Server id of the step.
    pub step_id: &'a StepId,
    /// Its flow.
    pub flow_id: &'a FlowId,
    /// The flow's title.
    pub flow_title: &'a str,
    /// The step.
    #[serde(flatten)]
    pub step: StepSummary<'a>,
}

/// Input of the `ask` prompt about lines without a single step.
#[derive(Clone, Copy, Debug)]
pub struct AskLinesInput<'a> {
    /// Common context.
    pub job: JobContext<'a>,
    /// The lines.
    pub lines: AskedLines<'a>,
    /// Their classic diff as text, or why it is not included.
    pub diff: &'a str,
    /// Owners of their changed lines (server facts).
    pub facts: &'a [HunkPart],
    /// Steps overlapping them.
    pub steps: &'a [TouchedStep<'a>],
    /// The reviewer's question.
    pub question: &'a str,
    /// Earlier questions of its thread, oldest first.
    pub thread: &'a [ThreadQuestion<'a>],
    /// Project profile.
    pub profile: Profile<'a>,
}

/// The job whose output is repaired, with its original input.
#[derive(Clone, Copy, Debug)]
pub enum RepairOf<'a> {
    /// A `plan` output.
    Plan(&'a PlanInput<'a>),
    /// A `pr_context` output.
    PrContext(&'a PrContextInput<'a>),
    /// An `analyse_flow` output.
    Flow(&'a FlowInput<'a>),
}

/// Input of a repair prompt (same job kind and output schema as the repaired job).
#[derive(Clone, Copy, Debug)]
pub struct RepairInput<'a> {
    /// The repaired job.
    pub of: RepairOf<'a>,
    /// Repair attempt, `1..=MAX_REPAIRS`.
    pub attempt: u32,
    /// Every issue verify found (`analysis::verify`).
    pub issues: &'a [VerifyIssue],
    /// The rejected output.
    pub rejected: &'a serde_json::Value,
}

/// An earlier private question of this PR with its answer.
#[derive(Clone, Copy, Debug)]
pub struct AnsweredQuestion<'a> {
    /// The question.
    pub question: &'a str,
    /// The LLM answer.
    pub answer: &'a str,
}

/// What an `ask` job's context pack adds.
#[derive(Clone, Copy, Debug)]
pub struct AskContext<'a> {
    /// What the question is about.
    pub asked: Asked<'a>,
    /// Earlier questions of this pull request, oldest first.
    pub earlier: &'a [AnsweredQuestion<'a>],
}

/// What an asked question is about, for its context pack.
#[derive(Clone, Copy, Debug)]
pub enum Asked<'a> {
    /// A step, given in full with the rest of its flow analysis.
    Step {
        /// The step asked about.
        step_id: &'a StepId,
        /// The flow analysis containing the step.
        flow: &'a FlowAnalysisOutput,
    },
    /// An analysed flow as a whole, given in full.
    Flow {
        /// The flow analysis.
        flow: &'a FlowAnalysisOutput,
    },
    /// The whole pull request: the pack's plan, PR context and flows are the context.
    Pr,
    /// Lines without a single step, given with the steps overlapping them in full.
    Lines {
        /// The lines.
        lines: AskedLines<'a>,
        /// The overlapping steps with their server ids.
        steps: &'a [(StepId, &'a AgentStep)],
    },
}

/// Input of the context pack that seeds a new session after a lost one (brief §5 Agent sessions).
#[derive(Clone, Copy, Debug)]
pub struct ContextPackInput<'a> {
    /// The pull request.
    pub pr: &'a PrSummary,
    /// Stored plan.
    pub plan: Option<&'a PlanOutput>,
    /// Stored PR context.
    pub pr_context: Option<&'a PrContextOutput>,
    /// Stored flow analyses.
    pub flows: &'a [FlowAnalysisOutput],
    /// For `ask` jobs.
    pub ask: Option<AskContext<'a>>,
}

#[derive(Serialize)]
struct FlowEntry<'a> {
    id: &'a FlowId,
    title: &'a str,
    classification: Classification,
    importance_rank: u32,
    intent: &'a str,
    depends_on: &'a [FlowId],
}

impl<'a> FlowEntry<'a> {
    fn of(flow: &'a PlannedFlow) -> Self {
        Self {
            id: &flow.id,
            title: &flow.title,
            classification: flow.classification,
            importance_rank: flow.importance_rank,
            intent: &flow.intent,
            depends_on: &flow.depends_on,
        }
    }
}

#[derive(Serialize)]
struct StepEntry<'a> {
    id: &'a str,
    #[serde(flatten)]
    step: StepSummary<'a>,
}

impl<'a> StepEntry<'a> {
    fn of(id: &'a str, step: &'a AgentStep) -> Self {
        Self {
            id,
            step: StepSummary::of(step),
        }
    }
}

/// A step in full, with its server id.
#[derive(Serialize)]
struct AskedStep<'a> {
    step_id: &'a str,
    #[serde(flatten)]
    step: &'a AgentStep,
}

/// The rest of a flow analysis besides its steps.
#[derive(Serialize)]
struct FlowRest<'a> {
    edges: &'a [AgentEdge],
    impact_refs: &'a [AgentImpactRef],
    pattern_repetitions: &'a [AgentPatternRepetition],
    thread_statuses: &'a [AgentThreadStatus],
}

#[derive(Serialize)]
struct AnalysedFlowEntry<'a> {
    flow_id: &'a FlowId,
    introduction: &'a FlowIntroduction,
    steps: Vec<StepEntry<'a>>,
}

#[derive(Serialize)]
struct ThreadEntry<'a> {
    alias: &'a str,
    path: &'a str,
    start_line: Option<u32>,
    line: Option<u32>,
    side: Option<LineSide>,
    author: &'a str,
    comments: u32,
    resolved: bool,
    outdated: bool,
    first_comment_start: &'a str,
    clipped_characters: usize,
}

#[derive(Serialize)]
struct AnswerEntry<'a> {
    question: &'a str,
    answer: &'a str,
}

/// The `plan` prompt: the hunks given to the agent, listed and never inlined; over budget is an error.
pub fn plan(input: &PlanInput<'_>) -> Result<String, PromptError> {
    let prompt = frame("plan", input.job, &plan_task(input)?)?;
    within_budget("plan", prompt, PROMPT_BUDGET_BYTES)
}

/// The `pr_context` prompt.
pub fn pr_context(input: &PrContextInput<'_>) -> Result<String, PromptError> {
    let prompt = frame("pr_context", input.job, &pr_context_task(input)?)?;
    within_budget("pr_context", prompt, PROMPT_BUDGET_BYTES)
}

/// The `analyse_flow` prompt of one planned flow.
pub fn analyse_flow(input: &FlowInput<'_>) -> Result<String, PromptError> {
    let job = format!("analyse_flow {}", plain(&input.flow.id.0));
    let prompt = frame(&job, input.job, &flow_task(input)?)?;
    within_budget("analyse_flow", prompt, PROMPT_BUDGET_BYTES)
}

/// The `ask` prompt.
pub fn ask(input: &AskInput<'_>) -> Result<String, PromptError> {
    let step = serde_json::to_string(&AskedStep {
        step_id: &input.step_id.0,
        step: input.step,
    })?;
    let question = clipped_data("question", input.question, MAX_QUESTION_CHARS)?;
    let mut about = plain(&input.step_id.0);
    if let Some(lines) = input.lines {
        about.push_str(&format!(
            " (its lines {}-{})",
            lines.start_line, lines.end_line
        ));
    }
    let task = fill(
        ASK,
        &[
            ("step_id", &about),
            ("ask_rules", part("ask_rules")?),
            ("lenses", part("lenses")?),
            ("profile", &profile(input.profile)?),
            ("step", &data("step", &step)),
            ("question", &question),
            ("thread", &thread_block(input.thread)?),
        ],
    )?;
    let prompt = frame("ask", input.job, &task)?;
    within_budget("ask", prompt, PROMPT_BUDGET_BYTES)
}

/// The review comment a question is about with an introduction, or nothing for a question about the whole PR.
fn subject_block(item: Option<&VerifyItem>) -> Result<String, PromptError> {
    let Some(item) = item else {
        return Ok(String::new());
    };
    let comment = serde_json::to_string(item)?;
    Ok(format!(
        "{}\n{}",
        part("comment_item")?,
        json_block("review comment", &comment, MAX_ASK_CONTEXT_BYTES)?
    ))
}

/// The `ask` prompt about lines without a single step (Files view): the lines, their classic diff, the server's
/// owners of their changed lines and the overlapping steps in summary; no other flow analysis.
pub fn ask_lines(input: &AskLinesInput<'_>) -> Result<String, PromptError> {
    let lines = &input.lines;
    let place = match lines.range {
        Some(range) => format!(
            "lines {}-{} of the file named in the lines block, on {}",
            range.start_line,
            range.end_line,
            side_label(lines.side)
        ),
        None => format!(
            "the whole file named in the lines block, on {}",
            side_label(lines.side)
        ),
    };
    let question = clipped_data("question", input.question, MAX_QUESTION_CHARS)?;
    let task = fill(
        ASK_LINES,
        &[
            ("place", &place),
            ("ask_rules", part("ask_rules")?),
            ("lenses", part("lenses")?),
            ("profile", &profile(input.profile)?),
            ("lines", &data("lines", &serde_json::to_string(lines)?)),
            (
                "diff",
                &json_block("classic diff", input.diff, MAX_ASK_DIFF_BYTES)?,
            ),
            (
                "facts",
                &json_block(
                    "owners of the changed lines",
                    &serde_json::to_string(input.facts)?,
                    MAX_ASK_FACTS_BYTES,
                )?,
            ),
            (
                "steps",
                &lines_block(
                    "overlapping steps",
                    input
                        .steps
                        .iter()
                        .map(serde_json::to_string)
                        .collect::<Result<_, _>>()?,
                    MAX_ASK_FACTS_BYTES,
                )?,
            ),
            ("question", &question),
            ("thread", &thread_block(input.thread)?),
        ],
    )?;
    let prompt = frame("ask", input.job, &task)?;
    within_budget("ask", prompt, PROMPT_BUDGET_BYTES)
}

/// The `ask` prompt about an analysed flow as a whole: its introduction and its steps in summary, with server ids.
pub fn ask_flow(input: &AskFlowInput<'_>) -> Result<String, PromptError> {
    let flow = input.flow;
    let ids: Vec<String> = (1..=flow.steps.len())
        .map(|number| format!("{}.{number}", flow.flow_id.0))
        .collect();
    let entry = AnalysedFlowEntry {
        flow_id: &flow.flow_id,
        introduction: &flow.introduction,
        steps: flow
            .steps
            .iter()
            .zip(&ids)
            .map(|(step, id)| StepEntry::of(id, step))
            .collect(),
    };
    let question = clipped_data("question", input.question, MAX_QUESTION_CHARS)?;
    let task = fill(
        ASK_FLOW,
        &[
            ("flow_id", &plain(&flow.flow_id.0)),
            ("ask_rules", part("ask_rules")?),
            ("lenses", part("lenses")?),
            ("profile", &profile(input.profile)?),
            (
                "flow",
                &json_block(
                    "flow",
                    &serde_json::to_string(&entry)?,
                    MAX_ASK_CONTEXT_BYTES,
                )?,
            ),
            ("question", &question),
            ("thread", &thread_block(input.thread)?),
        ],
    )?;
    let prompt = frame("ask", input.job, &task)?;
    within_budget("ask", prompt, PROMPT_BUDGET_BYTES)
}

/// The `ask` prompt about the whole pull request: the plan's flows and the PR context, each bounded.
pub fn ask_pr(input: &AskPrInput<'_>) -> Result<String, PromptError> {
    let flows = input
        .plan
        .map(|plan| {
            plan.flows
                .iter()
                .map(|flow| serde_json::to_string(&FlowEntry::of(flow)))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let context = input
        .pr_context
        .map(serde_json::to_string)
        .transpose()?
        .unwrap_or_default();
    let question = clipped_data("question", input.question, MAX_QUESTION_CHARS)?;
    let task = fill(
        ASK_PR,
        &[
            ("ask_rules", part("ask_rules")?),
            ("lenses", part("lenses")?),
            ("profile", &profile(input.profile)?),
            (
                "flows",
                &lines_block("planned flows", flows, MAX_ASK_CONTEXT_BYTES)?,
            ),
            (
                "context",
                &json_block("PR context", &context, MAX_ASK_CONTEXT_BYTES)?,
            ),
            ("question", &question),
            ("thread", &thread_block(input.thread)?),
            ("subject", &subject_block(input.item)?),
        ],
    )?;
    let prompt = frame("ask", input.job, &task)?;
    within_budget("ask", prompt, PROMPT_BUDGET_BYTES)
}

/// A comment Ask without an analysis uses the same answer rules and data fences, with no MemCan profile.
pub fn ask_comment(
    pr: &PrSummary,
    language: LlmLanguage,
    item: &VerifyItem,
    question: &str,
    thread: &[ThreadQuestion<'_>],
) -> Result<String, PromptError> {
    let task = fill(
        ASK_PR,
        &[
            ("ask_rules", part("ask_rules")?),
            ("lenses", part("lenses")?),
            (
                "profile",
                &profile(Profile {
                    file: None,
                    memcan: None,
                })?,
            ),
            (
                "flows",
                &lines_block("planned flows", Vec::new(), MAX_ASK_CONTEXT_BYTES)?,
            ),
            (
                "context",
                &json_block("PR context", "", MAX_ASK_CONTEXT_BYTES)?,
            ),
            (
                "question",
                &clipped_data("question", question, MAX_QUESTION_CHARS)?,
            ),
            ("thread", &thread_block(thread)?),
            ("subject", &subject_block(Some(item))?),
        ],
    )?;
    let prompt = frame_inner("ask", pr, language, SUB_AGENTS_DEFAULT, None, &task)?;
    within_budget("ask", prompt, PROMPT_BUDGET_BYTES)
}

/// Private recovery pack: public PR/comment context and only the payer's earlier final answers.
pub fn comment_context(
    pr: &PrSummary,
    item: &VerifyItem,
    earlier: &[(String, contracts::agent::ask::AskOutput)],
) -> Result<String, PromptError> {
    let mut pack = format!(
        "Context pack for {}/{}#{} at head {}\n",
        pr.repo.owner, pr.repo.name, pr.number, pr.head_sha.0
    );
    pack.push_str(&clipped_data(
        "pull request",
        &serde_json::to_string(pr)?,
        MAX_ASK_CONTEXT_BYTES,
    )?);
    pack.push_str(&subject_block(Some(item))?);
    pack.push_str(&json_block(
        "your earlier answers",
        &serde_json::to_string(earlier)?,
        MAX_ASK_CONTEXT_BYTES,
    )?);
    within_budget("private context pack", pack, CONTEXT_PACK_BUDGET_BYTES)
}

/// The earlier questions of a follow-up's thread with an introduction, or nothing for a first question: the first
/// question, then the latest follow-ups that fit [`MAX_ASK_THREAD_BYTES`] (at most [`MAX_THREAD_FOLLOW_UPS`]), with a
/// note on the omitted ones. Each entry is clipped to at most half the budget, so the first always fits; a clipped
/// entry gives `clipped_chars` and the block the usual clipped note.
fn thread_block(thread: &[ThreadQuestion<'_>]) -> Result<String, PromptError> {
    let entries = thread
        .iter()
        .map(thread_entry)
        .collect::<Result<Vec<_>, _>>()?;
    let Some(((first, first_clipped), rest)) = entries.split_first() else {
        return Ok(String::new());
    };
    let mut used = first.len();
    let mut clipped = *first_clipped;
    let mut latest = Vec::new();
    for (entry, entry_clipped) in rest.iter().rev().take(MAX_THREAD_FOLLOW_UPS) {
        if used + entry.len() > MAX_ASK_THREAD_BYTES {
            break;
        }
        used += entry.len();
        clipped += entry_clipped;
        latest.push(entry.as_str());
    }
    let omitted = rest.len() - latest.len();
    let mut content = format!("{first}\n");
    for entry in latest.iter().rev() {
        content.push_str(entry);
        content.push('\n');
    }
    let mut out = format!("{}\n", part("thread")?);
    out.push_str(&data("earlier questions of this thread", &content));
    out.push_str(&clipped_note(clipped)?);
    if omitted > 0 {
        out.push_str(&omitted_note(omitted)?);
    }
    Ok(out)
}

/// One entry of a thread block as a JSON line and the characters clipped from it: the question and the answer cut
/// to their limits, then the answer shortened until the line fits half of [`MAX_ASK_THREAD_BYTES`].
fn thread_entry(asked: &ThreadQuestion<'_>) -> Result<(String, usize), PromptError> {
    #[derive(Serialize)]
    struct Entry<'e> {
        question: &'e str,
        state: AskState,
        #[serde(skip_serializing_if = "Option::is_none")]
        answer: Option<&'e str>,
        #[serde(skip_serializing_if = "is_zero")]
        clipped_chars: usize,
    }
    fn is_zero(chars: &usize) -> bool {
        *chars == 0
    }
    let (question, question_clipped) = clip(asked.question, MAX_QUESTION_CHARS);
    let answer = asked.answer.unwrap_or_default();
    let mut keep = MAX_THREAD_ANSWER_CHARS;
    loop {
        let (shown, answer_clipped) = clip(answer, keep);
        let clipped_chars = question_clipped + answer_clipped;
        let line = serde_json::to_string(&Entry {
            question: &question,
            state: asked.state,
            answer: asked.answer.map(|_| shown.as_str()),
            clipped_chars,
        })?;
        if line.len() <= MAX_ASK_THREAD_BYTES / 2 || keep == 0 {
            return Ok((line, clipped_chars));
        }
        keep /= 2;
    }
}

fn side_label(side: LineSide) -> &'static str {
    match side {
        LineSide::Head => "the head commit",
        LineSide::Base => "the base commit (removed code)",
    }
}

/// The repair prompt: exactly the verify issues, the whole rejected output and the original task. A rejected
/// output over [`MAX_REJECTED_BYTES`] is not repairable ([`PromptError::OverBudget`] of kind `repair`).
pub fn repair(input: &RepairInput<'_>) -> Result<String, PromptError> {
    if input.attempt == 0 || input.attempt > MAX_REPAIRS {
        return Err(PromptError::RepairAttempt {
            attempt: input.attempt,
        });
    }
    if input.issues.is_empty() {
        return Err(PromptError::NoIssues);
    }
    let (kind, job, context, task) = match input.of {
        RepairOf::Plan(plan) => ("plan", "plan".to_owned(), plan.job, plan_task(plan)?),
        RepairOf::PrContext(context) => (
            "pr_context",
            "pr_context".to_owned(),
            context.job,
            pr_context_task(context)?,
        ),
        RepairOf::Flow(flow) => (
            "analyse_flow",
            format!("analyse_flow {}", plain(&flow.flow.id.0)),
            flow.job,
            flow_task(flow)?,
        ),
    };
    let issues: Vec<String> = input
        .issues
        .iter()
        .map(|issue| {
            let (message, _) = clip(&issue.message, MAX_ISSUE_CHARS);
            format!("{}: {}", issue.kind.code(), one_line(&message))
        })
        .collect();
    let rejected = if matches!(input.of, RepairOf::Flow(_)) {
        "Your file is still available for this session: the server restored its rejected contents to out/flow.md in this job. Fix the listed problems by editing that file; keep the rest unchanged. Run sr-check flow out/flow.md again.".to_owned()
    } else {
        input.rejected.to_string()
    };
    if rejected.len() > MAX_REJECTED_BYTES {
        return Err(PromptError::OverBudget {
            kind: "repair",
            bytes: rejected.len(),
            budget: MAX_REJECTED_BYTES,
        });
    }
    let rejected = data("rejected output", &rejected);
    let repair_template = if matches!(input.of, RepairOf::Flow(_)) {
        REPAIR.replace(
            "Return the complete corrected output in the same schema.",
            "Edit out/flow.md; do not rewrite the complete document.",
        )
    } else {
        REPAIR.to_owned()
    };
    let body = fill(
        &repair_template,
        &[
            ("attempt", &input.attempt.to_string()),
            ("max", &MAX_REPAIRS.to_string()),
            (
                "issues",
                &data("issues found by the checks", &issues.join("\n")),
            ),
            ("rejected", &rejected),
            ("task", &task),
        ],
    )?;
    let prompt = frame(&format!("repair {job}"), context, &body)?;
    within_budget(kind, prompt, PROMPT_BUDGET_BYTES + MAX_REJECTED_BYTES)
}

/// The context pack: stored results of the analysis as data, within [`CONTEXT_PACK_BUDGET_BYTES`]. The asked step
/// of an `ask` pack is reserved first and never shortened; the other sections share what is left, each up to its
/// own share, in order: rest of the asked flow, earlier questions, PR context, plan, analysed flows.
pub fn context_pack(input: &ContextPackInput<'_>) -> Result<String, PromptError> {
    let pr = input.pr;
    let (repo, number, head) = (repo(pr), pr.number.to_string(), plain(&pr.head_sha.0));
    let pack = |blocks: &str| {
        fill(
            CONTEXT_PACK,
            &[
                ("repo", &repo),
                ("number", &number),
                ("head", &head),
                ("blocks", blocks),
            ],
        )
    };
    let mut blocks = String::new();
    let mut sections: Vec<(String, Vec<String>, usize, bool)> = Vec::new();
    if let Some(ask) = &input.ask {
        let asked = match ask.asked {
            Asked::Step { step_id, flow } => Some(asked_flow(step_id, flow)?),
            Asked::Flow { flow } => Some(asked_whole_flow(flow)?),
            Asked::Lines { lines, steps } => Some(asked_lines(lines, steps)?),
            Asked::Pr => None,
        };
        if let Some((asked, label, rest)) = asked {
            blocks.push_str(&asked);
            sections.push((label, rest, PACK_ASK_FLOW_BYTES, true));
        }
        let earlier = ask
            .earlier
            .iter()
            .map(|pair| {
                serde_json::to_string(&AnswerEntry {
                    question: pair.question,
                    answer: pair.answer,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let label = "earlier private questions of this pull request".to_owned();
        sections.push((label, earlier, PACK_ANSWERS_BYTES, true));
    }
    if let Some(context) = input.pr_context {
        let context = vec![serde_json::to_string(context)?];
        sections.push((
            "pr_context output".to_owned(),
            context,
            PACK_PR_CONTEXT_BYTES,
            false,
        ));
    }
    if let Some(plan) = input.plan {
        let flows = plan
            .flows
            .iter()
            .map(|flow| serde_json::to_string(&FlowEntry::of(flow)))
            .collect::<Result<Vec<_>, _>>()?;
        sections.push((
            "plan output: flows".to_owned(),
            flows,
            PACK_PLAN_BYTES,
            true,
        ));
    }
    if !input.flows.is_empty() {
        let flows = input
            .flows
            .iter()
            .map(|flow| {
                serde_json::to_string(&AnalysedFlowEntry {
                    flow_id: &flow.flow_id,
                    introduction: &flow.introduction,
                    steps: flow
                        .steps
                        .iter()
                        .map(|step| StepEntry::of(&step.id, step))
                        .collect(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let label = "analysed flows (summary)".to_owned();
        sections.push((label, flows, PACK_FLOWS_BYTES, true));
    }
    // The asked step must fit with the frame and every section's fences and notes; else nothing can be shortened.
    let reserved = pack(&blocks)?.len() + sections.len() * PACK_SECTION_OVERHEAD;
    let mut remaining =
        CONTEXT_PACK_BUDGET_BYTES
            .checked_sub(reserved)
            .ok_or(PromptError::OverBudget {
                kind: "context pack",
                bytes: reserved,
                budget: CONTEXT_PACK_BUDGET_BYTES,
            })?;
    for (label, entries, share, lines) in sections {
        let max = share.min(remaining);
        let section = if lines {
            lines_block(&label, entries, max)?
        } else {
            json_block(&label, &entries.concat(), max)?
        };
        remaining = remaining.saturating_sub(section.len().saturating_sub(PACK_SECTION_OVERHEAD));
        blocks.push_str(&section);
    }
    within_budget("context pack", pack(&blocks)?, CONTEXT_PACK_BUDGET_BYTES)
}

/// The asked lines (a data block), and the label and entries of the steps overlapping them, in full.
fn asked_lines(
    lines: AskedLines<'_>,
    steps: &[(StepId, &AgentStep)],
) -> Result<(String, String, Vec<String>), PromptError> {
    let asked = data("asked lines", &serde_json::to_string(&lines)?);
    let entries = steps
        .iter()
        .map(|(step_id, step)| {
            serde_json::to_string(&AskedStep {
                step_id: &step_id.0,
                step,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let label = "steps overlapping the asked lines (full analysis)".to_owned();
    Ok((asked, label, entries))
}

/// The asked flow's introduction (a data block), and the label and entries of its steps in full and the rest of its
/// analysis.
fn asked_whole_flow(
    flow: &FlowAnalysisOutput,
) -> Result<(String, String, Vec<String>), PromptError> {
    let flow_id = plain(&flow.flow_id.0);
    let introduction = serde_json::to_string(&AnalysedFlowEntry {
        flow_id: &flow.flow_id,
        introduction: &flow.introduction,
        steps: Vec::new(),
    })?;
    let asked = data(
        &format!("asked flow {flow_id} (introduction)"),
        &introduction,
    );
    let mut entries = Vec::with_capacity(flow.steps.len() + 1);
    for (at, step) in flow.steps.iter().enumerate() {
        let step_id = format!("{}.{}", flow.flow_id.0, at + 1);
        entries.push(serde_json::to_string(&AskedStep {
            step_id: &step_id,
            step,
        })?);
    }
    entries.push(serde_json::to_string(&FlowRest {
        edges: &flow.edges,
        impact_refs: &flow.impact_refs,
        pattern_repetitions: &flow.pattern_repetitions,
        thread_statuses: &flow.thread_statuses,
    })?);
    let label = format!("steps and rest of the asked flow {flow_id}");
    Ok((asked, label, entries))
}

/// The asked step in full (a data block), and the label and entries of the rest of its flow analysis: introduction,
/// the other steps in full, then edges, impact references, pattern repetitions and thread statuses.
fn asked_flow(
    step_id: &StepId,
    flow: &FlowAnalysisOutput,
) -> Result<(String, String, Vec<String>), PromptError> {
    let unknown = || PromptError::UnknownStep {
        step_id: step_id.0.clone(),
    };
    let (flow_id, number) = step_id.0.rsplit_once('.').ok_or_else(unknown)?;
    let index = number
        .parse::<usize>()
        .ok()
        .and_then(|number| number.checked_sub(1))
        .filter(|index| flow_id == flow.flow_id.0 && *index < flow.steps.len())
        .ok_or_else(unknown)?;
    let asked = serde_json::to_string(&AskedStep {
        step_id: &step_id.0,
        step: &flow.steps[index],
    })?;
    let step_id = plain(&step_id.0);
    let asked = data(&format!("asked step {step_id} (full analysis)"), &asked);
    let mut entries = vec![serde_json::to_string(&AnalysedFlowEntry {
        flow_id: &flow.flow_id,
        introduction: &flow.introduction,
        steps: Vec::new(),
    })?];
    for (at, step) in flow.steps.iter().enumerate().filter(|(at, _)| *at != index) {
        let step_id = format!("{}.{}", flow.flow_id.0, at + 1);
        entries.push(serde_json::to_string(&AskedStep {
            step_id: &step_id,
            step,
        })?);
    }
    entries.push(serde_json::to_string(&FlowRest {
        edges: &flow.edges,
        impact_refs: &flow.impact_refs,
        pattern_repetitions: &flow.pattern_repetitions,
        thread_statuses: &flow.thread_statuses,
    })?);
    let label = format!("rest of the flow analysis containing step {step_id}");
    Ok((asked, label, entries))
}

fn plan_task(input: &PlanInput<'_>) -> Result<String, PromptError> {
    let prepass = input.job.prepass;
    let dispositions: BTreeMap<&HunkId, Disposition> = prepass
        .hunks
        .iter()
        .map(|hunk| (&hunk.id, hunk.disposition))
        .collect();
    let context = prepass.context();
    let context_only = prepass.context_only_hunks(input.files);
    let mut listing = String::new();
    for file in input.files {
        // A hunk the pre-pass does not know is listed: only a deterministic rule takes material out.
        let hunks: Vec<&Hunk> = file
            .hunks
            .iter()
            .filter(|hunk| {
                dispositions
                    .get(&hunk.id)
                    .is_none_or(|kind| *kind == Disposition::Agent)
                    && !context_only.contains(&hunk.id)
            })
            .collect();
        if hunks.is_empty() {
            continue;
        }
        listing.push_str(&file_label(file));
        listing.push('\n');
        for hunk in hunks {
            listing.push_str(&format!(
                "  {} {}{}\n",
                plain(&hunk.id.0),
                spans(hunk),
                context_note(file, hunk, &context)
            ));
        }
    }
    let batch = match input.batch {
        Some(batch) => {
            let number = batch.number.to_string();
            let total = batch.total.to_string();
            fill(
                part("plan_batch")?,
                &[("number", &number), ("total", &total)],
            )?
        }
        None => String::new(),
    };
    fill(
        PLAN,
        &[
            ("batch", &batch),
            ("profile", &profile(input.profile)?),
            ("moves", &moves(prepass, prepass.moves.moved.iter())?),
            ("hunks", &data("hunks", &listing)),
        ],
    )
}

fn pr_context_task(input: &PrContextInput<'_>) -> Result<String, PromptError> {
    let pr = input.job.pr;
    let mut text = format!("{}\n", part("pr_text")?);
    text.push_str(&clipped_data(
        "pull request title",
        &pr.title,
        MAX_TITLE_CHARS,
    )?);
    if let Some(description) = input.description {
        let label = "pull request description";
        text.push_str(&clipped_data(label, description, MAX_DESCRIPTION_CHARS)?);
    }
    let prepass = input.job.prepass;
    let related: Vec<String> = prepass
        .related_repos
        .iter()
        .map(|repo| format!("{}/{}", repo.owner, repo.name))
        .collect();
    let not_checked = &prepass.counters.dependency_files_not_checked;
    let dependency_files = path_list(
        "dependency_files_not_checked",
        "dependency files not checked",
        not_checked,
    )?;
    fill(
        PR_CONTEXT,
        &[
            ("lenses", part("lenses")?),
            ("profile", &profile(input.profile)?),
            ("pr_text", &text),
            (
                "related_repos",
                &path_list("related_repos", "related repositories", &related)?,
            ),
            ("dependency_files", &dependency_files),
            ("dependencies", &dependencies(input.dependencies)?),
        ],
    )
}

/// The changed crates of `view` as one-line JSON entries, and the controlled dependency repositories to look in;
/// empty without a Cargo map. PR-controlled names are clipped; entries, facts and repositories are bounded.
fn dependencies(view: Option<&DependencyView>) -> Result<String, PromptError> {
    let Some(view) = view.filter(|view| view.status == DependencyStatus::Available) else {
        return Ok(String::new());
    };
    let short = |text: &str| -> String {
        let (shown, omitted) = clip(text, MAX_DEPENDENCY_NAME_CHARS);
        if omitted == 0 {
            shown
        } else {
            format!("{shown}…")
        }
    };
    // A cited line keeps its number: only the path is bounded, cut in its middle.
    let line = |at: &contracts::api::deps::HeadLine| {
        format!(
            "{}:{}",
            middle_clipped(&at.file, MAX_DEPENDENCY_PATH_CHARS),
            at.line
        )
    };
    let controlled: Vec<_> = view.repositories.iter().filter(|r| r.controlled).collect();
    let mut lookups = Vec::new();
    for repository in controlled.iter().filter(|r| r.kind != RepositoryKind::This) {
        let Some(repo) = &repository.repo else {
            continue;
        };
        let mut names: Vec<String> = repository
            .crates
            .iter()
            .take(MAX_LOOKUP_CRATES)
            .map(|c| short(&c.name))
            .collect();
        if repository.crates.len() > MAX_LOOKUP_CRATES {
            names.push(format!(
                "+{} more",
                repository.crates.len() - MAX_LOOKUP_CRATES
            ));
        }
        lookups.push(format!(
            "{}/{} (crates: {})",
            short(&repo.owner),
            short(&repo.name),
            names.join(", ")
        ));
    }
    let mut entries = Vec::new();
    for krate in view.repositories.iter().flat_map(|r| &r.crates) {
        let Some(changes) = &krate.changes else {
            continue;
        };
        let mut uses = Vec::new();
        let mut uncontrolled = 0;
        for repository in &view.repositories {
            for other in repository
                .crates
                .iter()
                .filter(|c| c.used_by.contains(&krate.name))
            {
                if !repository.controlled {
                    uncontrolled += 1;
                    continue;
                }
                let from = match (&repository.kind, &repository.repo) {
                    (RepositoryKind::This, _) => "this repository".to_owned(),
                    (_, Some(repo)) => short(&format!("{}/{}", repo.owner, repo.name)),
                    (_, None) => "other".to_owned(),
                };
                uses.push(serde_json::json!({"crate": short(&other.name), "from": from}));
            }
        }
        let uses_not_listed = uses.len().saturating_sub(MAX_PROMPT_USES);
        uses.truncate(MAX_PROMPT_USES);
        let facts: Vec<serde_json::Value> = changes
            .facts
            .iter()
            .take(MAX_PROMPT_FACTS)
            .map(|fact| {
                let definition = match &fact.definition {
                    TypeDefinition::Located { at } => line(at),
                    TypeDefinition::NotLocated => "not located".to_owned(),
                    TypeDefinition::NotSearched => "not searched".to_owned(),
                    TypeDefinition::OtherRepository => "in another repository".to_owned(),
                };
                serde_json::json!({
                    "at": line(&fact.at),
                    "kind": fact.kind,
                    "type": short(&fact.written),
                    "type_crate": short(&fact.type_crate),
                    "definition": definition,
                })
            })
            .collect();
        let facts_not_listed = u64::from(changes.facts_hidden)
            + u64::from(changes.other_facts)
            + (changes.facts.len() - facts.len()) as u64;
        let flows: Vec<&str> = changes.flows.iter().map(|f| f.id.0.as_str()).collect();
        let notes: Vec<String> = changes.notes.iter().map(|n| short(&n.text)).collect();
        let entry = serde_json::json!({
            "crate": short(&krate.name),
            "manifest": krate
                .manifest
                .as_deref()
                .map(|path| middle_clipped(path, MAX_DEPENDENCY_PATH_CHARS)),
            "changed_lines": changes.changed_lines,
            "flows": flows,
            "uses": uses,
            "other_dependencies": u64::from(changes.other_dependencies)
                + uncontrolled
                + uses_not_listed as u64,
            "facts": facts,
            "facts_not_listed": facts_not_listed,
            "notes": notes,
        });
        entries.push(entry.to_string());
    }
    if entries.is_empty() {
        return Ok(String::new());
    }
    let mut out = format!("{}\n", part("dependencies")?);
    out.push_str(&lines_block("dependencies", entries, MAX_DEPENDENCY_BYTES)?);
    out.push_str(&path_list(
        "dependency_repos",
        "dependency repositories",
        &lookups[..lookups.len().min(MAX_LOOKUP_REPOS)],
    )?);
    if lookups.len() > MAX_LOOKUP_REPOS {
        out.push_str(&omitted_note(lookups.len() - MAX_LOOKUP_REPOS)?);
    }
    Ok(out)
}

fn flow_task(input: &FlowInput<'_>) -> Result<String, PromptError> {
    let flow = input.flow;
    let context = input.job.prepass.context();
    let context_only = input.job.prepass.context_only_hunks(input.files);
    let mut hunks = String::new();
    let mut paths: Vec<&str> = Vec::new();
    for selection in &flow.hunks {
        let found = input.files.iter().find_map(|file| {
            let hunk = file
                .hunks
                .iter()
                .find(|hunk| hunk.id == selection.hunk_id)?;
            Some((file, hunk))
        });
        let Some((file, hunk)) = found else {
            hunks.push_str(&format!(
                "{} (not in the diff)\n",
                plain(&selection.hunk_id.0)
            ));
            continue;
        };
        if context_only.contains(&hunk.id) {
            continue;
        }
        paths.extend(file.old_path.as_deref());
        paths.push(&file.path);
        let id = plain(&hunk.id.0);
        hunks.push_str(&format!("{id} {} {}", file_label(file), spans(hunk)));
        if let (Some(start), Some(end)) = (selection.head_start, selection.head_end) {
            hunks.push_str(&format!(", only head lines {start}-{end}"));
        }
        hunks.push_str(&context_note(file, hunk, &context));
        hunks.push('\n');
    }
    let moved = input.job.prepass.moves.moved.iter().filter(|block| {
        paths.contains(&block.from.file.as_str()) || paths.contains(&block.to.file.as_str())
    });
    let planned = serde_json::to_string(&FlowEntry::of(flow))?;
    fill(
        ANALYSE_FLOW,
        &[
            ("flow_id", &plain(&flow.id.0)),
            ("lenses", part("lenses")?),
            ("flow", &data("planned flow", &planned)),
            ("hunks", &data("flow hunks", &hunks)),
            ("moves", &moves(input.job.prepass, moved)?),
            ("profile", &profile(input.profile)?),
            ("threads", &threads(input.threads)?),
            ("calibration", &calibration(input.calibration)?),
        ],
    )
}

/// Rules 4 (words never written), 5 (read-only), 6 (untrusted data) and 9 (GitHub tools) of the shared rules block
/// as bullets, for a read-only job with its own template (`verify_comments`), so those rules have one source.
pub(crate) fn read_only_rules() -> Result<String, PromptError> {
    let rules = fill(
        RULES,
        &[
            ("language", ""),
            ("sub_agents", ""),
            ("file_permission", ""),
            ("output_instruction", ""),
            ("defect_form", ""),
        ],
    )?;
    Ok(rules
        .lines()
        .filter_map(|line| {
            ["4. ", "5. ", "6. ", "9. "]
                .iter()
                .find_map(|number| line.strip_prefix(number))
        })
        .map(|rule| format!("- {}", rule.replace("  ", " ")))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Header, rules block and pre-pass notices, then the task.
fn frame(job: &str, context: JobContext<'_>, task: &str) -> Result<String, PromptError> {
    frame_inner(
        job,
        context.pr,
        context.language,
        context.sub_agents,
        Some(context.prepass),
        task,
    )
}

fn frame_inner(
    job: &str,
    pr: &PrSummary,
    language: LlmLanguage,
    sub_agents: bool,
    prepass: Option<&Prepass>,
    task: &str,
) -> Result<String, PromptError> {
    let header = fill(
        HEADER,
        &[
            ("job", job),
            ("repo", &repo(pr)),
            ("number", &pr.number.to_string()),
            ("base", &plain(&pr.base_sha.0)),
            ("head", &plain(&pr.head_sha.0)),
        ],
    )?;
    let language = match language {
        LlmLanguage::En => part("language_en")?,
        LlmLanguage::Pl => part("language_pl")?,
        LlmLanguage::Ru => part("language_ru")?,
    };
    let sub_agents = if sub_agents {
        part("sub_agents_on")?
    } else {
        part("sub_agents_off")?
    };
    let file_job = job.contains("analyse_flow");
    let output_instruction = if file_job {
        "Write the tagged flow document to out/flow.md in the job directory. Run `sr-check format` for the grammar and `sr-check catalog out/flow.md` for file/hunk ids. Run `sr-check flow out/flow.md`; edit the file to fix every problem, rerun until ok, then finish with one line: Flow file ready. Never return the document in the final message."
    } else {
        "Output only the JSON object of the output schema."
    };
    // Ask and flow analysis state risks as scenarios (brief §1 addenda 2026-11-18).
    let defect_form = if job == "ask" {
        "A suspected defect is an LLM statement of risk: a short title naming the risk class, prefixed \"Potential\", a numbered scenario (who acts, the order of events, the outcome and its cost) and one closing line saying what was not reproduced or checked."
    } else if file_job {
        "A suspected defect is a `flag` annotation: a concrete risk scenario, never a question or a verdict."
    } else {
        "Suspected defects are phrased as questions to check, each ending with \"?\"."
    };
    let file_permission = if file_job {
        "You may write/edit out/flow.md and run the installed sr-check checker; these are the only write/execute exceptions."
    } else {
        ""
    };
    let rules = fill(
        RULES,
        &[
            ("language", language),
            ("sub_agents", sub_agents),
            ("file_permission", file_permission),
            ("output_instruction", output_instruction),
            ("defect_form", defect_form),
        ],
    )?;
    let Some(prepass) = prepass else {
        return Ok(format!("{header}\n{rules}\n{}\n", task.trim_end()));
    };
    let mut sides: BTreeMap<&str, BTreeSet<LineSide>> = BTreeMap::new();
    let generated = prepass
        .not_rendered
        .iter()
        .filter(|item| item.reason == NotRenderedReason::Generated);
    for item in generated {
        sides.entry(&item.file).or_default().insert(item.side);
    }
    let generated: Vec<String> = sides
        .into_iter()
        .map(|(file, sides)| {
            let versions = match (
                sides.contains(&LineSide::Base),
                sides.contains(&LineSide::Head),
            ) {
                (true, true) => "base and head versions",
                (true, false) => "base version",
                _ => "head version",
            };
            format!("{file} ({versions})")
        })
        .collect();
    let instruction_files = &prepass.agent_instruction_files;
    let label = "agent instruction files";
    let mut notices = path_list("instruction_files", label, instruction_files)?;
    notices.push_str(&path_list(
        "generated_files",
        "generated file versions",
        &generated,
    )?);
    Ok(format!("{header}\n{rules}\n{notices}{}\n", task.trim_end()))
}

fn within_budget(kind: &'static str, prompt: String, budget: usize) -> Result<String, PromptError> {
    if prompt.len() > budget {
        return Err(PromptError::OverBudget {
            kind,
            bytes: prompt.len(),
            budget,
        });
    }
    Ok(prompt)
}

fn profile(profile: Profile<'_>) -> Result<String, PromptError> {
    if profile.file.is_none() && profile.memcan.is_none() {
        return Ok(format!("{}\n", part("profile_none")?));
    }
    let mut out = format!("{}\n", part("profile")?);
    if let Some(file) = profile.file {
        let label = "project profile: repository file .claudius/project-profile.md at base (wins)";
        out.push_str(&clipped_data(label, file, MAX_PROFILE_CHARS)?);
    }
    if let Some(memory) = profile.memcan {
        let label = "project profile: MemCan memory";
        out.push_str(&clipped_data(label, memory, MAX_PROFILE_CHARS)?);
    }
    Ok(out)
}

fn moves<'a>(
    prepass: &Prepass,
    blocks: impl Iterator<Item = &'a MovedBlock>,
) -> Result<String, PromptError> {
    let lines: Vec<String> = blocks
        .map(|block| {
            let differ = if !block.identical {
                format!(
                    "text differs in {} head and {} base lines",
                    block.differing_head_lines.len(),
                    block.differing_base_lines.len()
                )
            } else if prepass.is_quiet_move(block) {
                "text identical, moved into another file: context only".to_owned()
            } else {
                "text identical".to_owned()
            };
            format!(
                "base {} {}-{} -> head {} {}-{}: {differ}",
                block.from.file,
                block.from.range.start_line,
                block.from.range.end_line,
                block.to.file,
                block.to.range.start_line,
                block.to.range.end_line,
            )
        })
        .collect();
    let mut out = if lines.is_empty() {
        String::new()
    } else {
        let list = listed("moved blocks", &lines, MAX_LISTED_MOVES)?;
        format!("{}\n{list}", part("moves")?)
    };
    if prepass.moves.incomplete {
        out.push_str(part("moves_incomplete")?);
        out.push('\n');
    }
    Ok(out)
}

/// Aliases of the threads an `analyse_flow` prompt lists: the first [`MAX_THREADS`] of `threads`, in order. Pass
/// exactly these to verify (`FlowContext::thread_aliases`): later threads are omitted from the prompt.
pub fn listed_thread_aliases(threads: &[PromptThread<'_>]) -> Vec<String> {
    threads
        .iter()
        .take(MAX_THREADS)
        .map(|thread| thread.alias.to_owned())
        .collect()
}

/// The thread section: the first [`MAX_THREADS`] threads (see [`listed_thread_aliases`]) and a note on the rest.
fn threads(threads: &[PromptThread<'_>]) -> Result<String, PromptError> {
    if threads.is_empty() {
        return Ok(format!("{}\n", part("threads_none")?));
    }
    let mut out = format!("{}\n", part("threads")?);
    for item in threads.iter().take(MAX_THREADS) {
        let thread = item.thread;
        let (excerpt, omitted) = clip(&thread.excerpt, MAX_EXCERPT_CHARS);
        let entry = serde_json::to_string(&ThreadEntry {
            alias: item.alias,
            path: &thread.path,
            start_line: thread.start_line,
            line: thread.line,
            side: thread.side,
            author: &thread.author,
            comments: thread.comment_count,
            resolved: thread.is_resolved,
            outdated: thread.is_outdated,
            first_comment_start: &excerpt,
            clipped_characters: omitted,
        })?;
        out.push_str(&data(
            &format!("review thread {}", plain(item.alias)),
            &entry,
        ));
    }
    if threads.len() > MAX_THREADS {
        out.push_str(&omitted_note(threads.len() - MAX_THREADS)?);
    }
    Ok(out)
}

fn calibration(examples: Option<&[Calibration<'_>]>) -> Result<String, PromptError> {
    let Some(examples) = examples else {
        return Ok(format!("{}\n", part("calibration_not_checked")?));
    };
    if examples.is_empty() {
        return Ok(format!("{}\n", part("calibration_none")?));
    }
    let mut out = format!("{}\n", part("calibration")?);
    for (index, example) in examples.iter().take(MAX_CALIBRATION_EXAMPLES).enumerate() {
        let direction = match example.direction {
            CalibrationDirection::Overrated => "overrated",
            CalibrationDirection::Underrated => "underrated",
        };
        let label = format!("calibration example {} ({direction})", index + 1);
        out.push_str(&clipped_data(&label, example.text, MAX_CALIBRATION_CHARS)?);
    }
    if examples.len() > MAX_CALIBRATION_EXAMPLES {
        out.push_str(&omitted_note(examples.len() - MAX_CALIBRATION_EXAMPLES)?);
    }
    Ok(out)
}

/// The intro part and a data block of `items`, or nothing when there are none.
fn path_list(intro: &str, label: &str, items: &[String]) -> Result<String, PromptError> {
    if items.is_empty() {
        return Ok(String::new());
    }
    let list = listed(label, items, MAX_LISTED_PATHS)?;
    Ok(format!("{}\n{list}", part(intro)?))
}

/// A data block with at most `max` of `items`, one per line, and a note on the rest.
fn listed(label: &str, items: &[String], max: usize) -> Result<String, PromptError> {
    let shown: Vec<String> = items.iter().take(max).map(|item| one_line(item)).collect();
    let mut out = data(label, &shown.join("\n"));
    if items.len() > max {
        out.push_str(&omitted_note(items.len() - max)?);
    }
    Ok(out)
}

/// A data block of one-line JSON entries, filled up to `max_bytes` (an entry that does not fit is skipped, later ones
/// may still fit), with a note on the omitted entries.
fn lines_block(label: &str, entries: Vec<String>, max_bytes: usize) -> Result<String, PromptError> {
    let mut content = String::new();
    let mut omitted = 0;
    for entry in entries {
        if content.len() + entry.len() < max_bytes {
            content.push_str(&entry);
            content.push('\n');
        } else {
            omitted += 1;
        }
    }
    let mut out = data(label, &content);
    if omitted > 0 {
        out.push_str(&omitted_note(omitted)?);
    }
    Ok(out)
}

/// A data block of `text` cut to `max_bytes`, with a note on the clipped rest.
fn json_block(label: &str, text: &str, max_bytes: usize) -> Result<String, PromptError> {
    let (shown, omitted) = clip_bytes(text, max_bytes);
    Ok(data(label, shown) + &clipped_note(omitted)?)
}

/// A data block of `text` cut to `max_chars` characters, with a note on the clipped rest.
fn clipped_data(label: &str, text: &str, max_chars: usize) -> Result<String, PromptError> {
    let (shown, omitted) = clip(text, max_chars);
    Ok(data(label, &shown) + &clipped_note(omitted)?)
}

fn clipped_note(omitted: usize) -> Result<String, PromptError> {
    if omitted == 0 {
        return Ok(String::new());
    }
    let note = fill(part("clipped")?, &[("omitted", &omitted.to_string())])?;
    Ok(format!("{note}\n"))
}

fn omitted_note(omitted: usize) -> Result<String, PromptError> {
    let note = fill(part("omitted")?, &[("omitted", &omitted.to_string())])?;
    Ok(format!("{note}\n"))
}

/// `content` between fences whose id does not occur in it; `label` is server text.
fn data(label: &str, content: &str) -> String {
    let content = content.replace('\0', "\u{fffd}");
    let content = content.trim_end_matches('\n');
    let id = fence_id(label, content);
    format!("<<<DATA {id} {label}>>>\n{content}\n<<<END {id}>>>\n")
}

/// The first candidate id (deterministic in label and content) that does not occur in `content`.
fn fence_id(label: &str, content: &str) -> String {
    let candidates = (0..=u32::MAX).map(|salt| fence_candidate(label, content, salt));
    first_unused(content, candidates)
}

fn first_unused(content: &str, candidates: impl Iterator<Item = String>) -> String {
    let mut last = String::new();
    for candidate in candidates {
        if !content.contains(&candidate) {
            return candidate;
        }
        last = candidate;
    }
    // Every candidate occurs: an id longer than the content cannot.
    format!("{last}{}", "x".repeat(content.len() + 1))
}

fn fence_candidate(label: &str, content: &str, salt: u32) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt.to_be_bytes());
    hasher.update(label.as_bytes());
    hasher.update([0]);
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `{{name}}` placeholders replaced in one pass: inserted values are never scanned again.
fn fill(template: &str, values: &[(&str, &str)]) -> Result<String, PromptError> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let unknown = |name: &str| PromptError::Template {
            name: name.chars().take(40).collect(),
        };
        let end = after.find("}}").ok_or_else(|| unknown(after))?;
        let name = &after[..end];
        let (_, value) = values
            .iter()
            .find(|(key, _)| *key == name)
            .ok_or_else(|| unknown(name))?;
        out.push_str(value);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// A named section of `prompts/parts.md` (from `@@ name` to the next section), without its final newline.
fn part(name: &str) -> Result<&'static str, PromptError> {
    let marker = format!("@@ {name}\n");
    let start = PARTS
        .match_indices(&marker)
        .map(|(at, _)| at)
        .find(|at| *at == 0 || PARTS[..*at].ends_with('\n'))
        .ok_or_else(|| PromptError::Template {
            name: name.to_owned(),
        })?;
    let body = &PARTS[start + marker.len()..];
    let end = body.find("\n@@ ").map_or(body.len(), |at| at + 1);
    Ok(body[..end].trim_end_matches('\n'))
}

/// The path of a file in a hunk listing, with its change status.
fn file_label(file: &FileDiff) -> String {
    let path = one_line(&file.path);
    match (file.status, &file.old_path) {
        (FileStatus::Renamed, Some(old)) => format!("{path} (renamed from {})", one_line(old)),
        (FileStatus::Added, _) => format!("{path} (new file)"),
        (FileStatus::Deleted, _) => format!("{path} (deleted file)"),
        _ => path,
    }
}

/// The hunk's lines that are context without coverage duty, e.g. `; context only: base 99, head 112-150`.
fn context_note(
    file: &FileDiff,
    hunk: &Hunk,
    context: &BTreeMap<(LineSide, String), Vec<u32>>,
) -> String {
    let mut parts = Vec::new();
    for (side, word, span) in [
        (LineSide::Base, "base", hunk.base),
        (LineSide::Head, "head", hunk.head),
    ] {
        let Some(lines) = file
            .path_on(side)
            .and_then(|path| context.get(&(side, path.to_owned())))
        else {
            continue;
        };
        let inside: Vec<u32> = (span.start..span.start.saturating_add(span.len))
            .filter(|line| lines.binary_search(line).is_ok())
            .collect();
        if !inside.is_empty() {
            let ranges: Vec<String> = analysis::lines::ranges(&inside)
                .iter()
                .map(|r| {
                    if r.start_line == r.end_line {
                        r.start_line.to_string()
                    } else {
                        format!("{}-{}", r.start_line, r.end_line)
                    }
                })
                .collect();
            parts.push(format!("{word} {}", ranges.join(" ")));
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("; context only: {}", parts.join(", "))
    }
}

fn spans(hunk: &Hunk) -> String {
    format!(
        "base {},{} head {},{}",
        hunk.base.start, hunk.base.len, hunk.head.start, hunk.head.len
    )
}

fn repo(pr: &PrSummary) -> String {
    format!("{}/{}", plain(&pr.repo.owner), plain(&pr.repo.name))
}

/// An identifier for trusted prompt text: anything outside `[A-Za-z0-9._-]` becomes `_`.
fn plain(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Control characters (line breaks included) escaped, so one entry stays one line.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_control() {
            out.extend(ch.escape_default());
        } else {
            out.push(ch);
        }
    }
    out
}

/// `text` cut to `max_chars` characters and the number of characters cut.
fn clip(text: &str, max_chars: usize) -> (String, usize) {
    let total = text.chars().count();
    if total <= max_chars {
        return (text.to_owned(), 0);
    }
    (text.chars().take(max_chars).collect(), total - max_chars)
}

/// `text` of at most `max_chars` characters: a longer one keeps its start and end around `…`.
fn middle_clipped(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_owned();
    }
    let keep = max_chars.saturating_sub(1);
    let head: String = text.chars().take(keep / 2).collect();
    let tail: String = text.chars().skip(total - (keep - keep / 2)).collect();
    format!("{head}…{tail}")
}

/// `text` cut to at most `max_bytes` bytes on a character boundary and the number of bytes cut.
fn clip_bytes(text: &str, max_bytes: usize) -> (&str, usize) {
    if text.len() <= max_bytes {
        return (text, 0);
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], text.len() - end)
}

#[cfg(test)]
mod tests;
