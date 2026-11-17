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
use contracts::agent::analyse_flow::{AgentStep, FlowAnalysisOutput, FlowIntroduction};
use contracts::agent::plan::{PlanOutput, PlannedFlow};
use contracts::agent::pr_context::PrContextOutput;
use contracts::api::pr::PrSummary;
use contracts::api::tour::GithubThread;
use contracts::common::{Classification, FragmentKind, LineSide, LlmLanguage};
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
const REPAIR: &str = include_str!("../../../prompts/repair.md");
const CONTEXT_PACK: &str = include_str!("../../../prompts/context_pack.md");

/// Owner default for sub-agents: off (ADR-002; threat model T5, `multi_agent = false`).
pub const SUB_AGENTS_DEFAULT: bool = false;
/// Largest `plan`, `pr_context`, `analyse_flow` or `ask` prompt in bytes (about 32k tokens); a repair prompt
/// may add its quoted output. Untrusted texts are clipped to fixed caps, so only a hunk listing can exceed it:
/// the pipeline then plans in directory batches ([`PlanBatch`]).
pub const PROMPT_BUDGET_BYTES: usize = 128 * 1024;
/// Largest rejected output quoted in a repair prompt, in bytes.
pub const MAX_REJECTED_BYTES: usize = 160 * 1024;
/// Largest context pack in bytes, kept by omitting entries (never an error for valid stored results).
pub const CONTEXT_PACK_BUDGET_BYTES: usize = 160 * 1024;
/// Repair runs allowed per output (brief §5 Pipeline 6).
pub const MAX_REPAIRS: u32 = 2;
/// Calibration examples quoted, at most.
pub const MAX_CALIBRATION_EXAMPLES: usize = 8;
/// Characters quoted per calibration example, at most.
pub const MAX_CALIBRATION_CHARS: usize = 1000;

const MAX_TITLE_CHARS: usize = 300;
const MAX_DESCRIPTION_CHARS: usize = 16_000;
const MAX_PROFILE_CHARS: usize = 16_000;
const MAX_EXCERPT_CHARS: usize = 2_000;
const MAX_QUESTION_CHARS: usize = 4_000;
const MAX_ISSUE_CHARS: usize = 1_000;
const MAX_LISTED_PATHS: usize = 50;
const MAX_LISTED_MOVES: usize = 100;
const MAX_THREADS: usize = 50;
const PACK_PLAN_BYTES: usize = 16 * 1024;
const PACK_PR_CONTEXT_BYTES: usize = 16 * 1024;
const PACK_FLOWS_BYTES: usize = 48 * 1024;
const PACK_ASK_FLOW_BYTES: usize = 48 * 1024;
const PACK_ANSWERS_BYTES: usize = 16 * 1024;

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
    /// The reviewer's question.
    pub question: &'a str,
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

/// An earlier question of a step with its answer.
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
    /// The step asked about.
    pub step_id: &'a StepId,
    /// The flow analysis containing the step.
    pub flow: &'a FlowAnalysisOutput,
    /// Earlier questions of the step, oldest first.
    pub earlier: &'a [AnsweredQuestion<'a>],
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
    title: &'a str,
    file: &'a str,
    side: LineSide,
    start_line: u32,
    end_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<&'a str>,
    kind: FragmentKind,
}

impl<'a> StepEntry<'a> {
    fn of(id: &'a str, step: &'a AgentStep, symbol: bool) -> Self {
        Self {
            id,
            title: &step.title,
            file: &step.file,
            side: step.side,
            start_line: step.start_line,
            end_line: step.end_line,
            symbol: step.symbol.as_deref().filter(|_| symbol),
            kind: step.kind,
        }
    }
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
    let step = serde_json::to_string(&StepEntry::of(&input.step_id.0, input.step, true))?;
    let question = clipped_data("question", input.question, MAX_QUESTION_CHARS)?;
    let task = fill(
        ASK,
        &[
            ("step_id", &plain(&input.step_id.0)),
            ("lenses", part("lenses")?),
            ("step", &data("step", &step)),
            ("question", &question),
        ],
    )?;
    let prompt = frame("ask", input.job, &task)?;
    within_budget("ask", prompt, PROMPT_BUDGET_BYTES)
}

/// The repair prompt: exactly the verify issues, the rejected output and the original task.
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
    let rejected = json_block(
        "rejected output",
        &input.rejected.to_string(),
        MAX_REJECTED_BYTES,
    )?;
    let body = fill(
        REPAIR,
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

/// The context pack: stored results of the analysis as data, within [`CONTEXT_PACK_BUDGET_BYTES`].
pub fn context_pack(input: &ContextPackInput<'_>) -> Result<String, PromptError> {
    let mut blocks = String::new();
    if let Some(ask) = &input.ask {
        let label = format!("flow analysis containing step {}", plain(&ask.step_id.0));
        let flow = serde_json::to_string(ask.flow)?;
        blocks.push_str(&json_block(&label, &flow, PACK_ASK_FLOW_BYTES)?);
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
        let label = "earlier questions of this step";
        blocks.push_str(&lines_block(label, earlier, PACK_ANSWERS_BYTES)?);
    }
    if let Some(context) = input.pr_context {
        let context = serde_json::to_string(context)?;
        blocks.push_str(&json_block(
            "pr_context output",
            &context,
            PACK_PR_CONTEXT_BYTES,
        )?);
    }
    if let Some(plan) = input.plan {
        let flows = plan
            .flows
            .iter()
            .map(|flow| serde_json::to_string(&FlowEntry::of(flow)))
            .collect::<Result<Vec<_>, _>>()?;
        blocks.push_str(&lines_block("plan output: flows", flows, PACK_PLAN_BYTES)?);
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
                        .map(|step| StepEntry::of(&step.id, step, false))
                        .collect(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let label = "analysed flows (summary)";
        blocks.push_str(&lines_block(label, flows, PACK_FLOWS_BYTES)?);
    }
    let pr = input.pr;
    let pack = fill(
        CONTEXT_PACK,
        &[
            ("repo", &repo(pr)),
            ("number", &pr.number.to_string()),
            ("head", &plain(&pr.head_sha.0)),
            ("blocks", &blocks),
        ],
    )?;
    within_budget("context pack", pack, CONTEXT_PACK_BUDGET_BYTES)
}

fn plan_task(input: &PlanInput<'_>) -> Result<String, PromptError> {
    let prepass = input.job.prepass;
    let dispositions: BTreeMap<&HunkId, Disposition> = prepass
        .hunks
        .iter()
        .map(|hunk| (&hunk.id, hunk.disposition))
        .collect();
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
            })
            .collect();
        if hunks.is_empty() {
            continue;
        }
        listing.push_str(&file_label(file));
        listing.push('\n');
        for hunk in hunks {
            listing.push_str(&format!("  {} {}\n", plain(&hunk.id.0), spans(hunk)));
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
    let moved = prepass.moves.moved.iter().filter(|block| !block.identical);
    fill(
        PLAN,
        &[
            ("batch", &batch),
            ("profile", &profile(input.profile)?),
            ("moves", &moves(prepass, moved)?),
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
        ],
    )
}

fn flow_task(input: &FlowInput<'_>) -> Result<String, PromptError> {
    let flow = input.flow;
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
        paths.extend(file.old_path.as_deref());
        paths.push(&file.path);
        let id = plain(&hunk.id.0);
        hunks.push_str(&format!("{id} {} {}", file_label(file), spans(hunk)));
        if let (Some(start), Some(end)) = (selection.head_start, selection.head_end) {
            hunks.push_str(&format!(", only head lines {start}-{end}"));
        }
        hunks.push('\n');
    }
    let moved = input.job.prepass.moves.moved.iter().filter(|block| {
        !block.identical
            && (paths.contains(&block.from.file.as_str())
                || paths.contains(&block.to.file.as_str()))
    });
    let test_flow = if flow.classification == Classification::Test {
        part("test_flow")?
    } else {
        ""
    };
    let planned = serde_json::to_string(&FlowEntry::of(flow))?;
    fill(
        ANALYSE_FLOW,
        &[
            ("flow_id", &plain(&flow.id.0)),
            ("lenses", part("lenses")?),
            ("test_flow", test_flow),
            ("flow", &data("planned flow", &planned)),
            ("hunks", &data("flow hunks", &hunks)),
            ("moves", &moves(input.job.prepass, moved)?),
            ("profile", &profile(input.profile)?),
            ("threads", &threads(input.threads)?),
            ("calibration", &calibration(input.calibration)?),
        ],
    )
}

/// Header, rules block and pre-pass notices, then the task.
fn frame(job: &str, context: JobContext<'_>, task: &str) -> Result<String, PromptError> {
    let pr = context.pr;
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
    let language = match context.language {
        LlmLanguage::En => part("language_en")?,
        LlmLanguage::Pl => part("language_pl")?,
    };
    let sub_agents = if context.sub_agents {
        part("sub_agents_on")?
    } else {
        part("sub_agents_off")?
    };
    let rules = fill(RULES, &[("language", language), ("sub_agents", sub_agents)])?;
    let prepass = context.prepass;
    let generated: BTreeSet<&str> = prepass
        .not_rendered
        .iter()
        .filter(|item| item.reason == NotRenderedReason::Generated)
        .map(|item| item.file.as_str())
        .collect();
    let generated: Vec<String> = generated.into_iter().map(str::to_owned).collect();
    let instruction_files = &prepass.agent_instruction_files;
    let label = "agent instruction files";
    let mut notices = path_list("instruction_files", label, instruction_files)?;
    notices.push_str(&path_list(
        "generated_files",
        "generated files",
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
            let differ = format!(
                "text differs in {} head and {} base lines",
                block.differing_head_lines.len(),
                block.differing_base_lines.len()
            );
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

/// A data block of one-line JSON entries, filled up to `max_bytes`, with a note on the omitted entries.
fn lines_block(label: &str, entries: Vec<String>, max_bytes: usize) -> Result<String, PromptError> {
    let mut content = String::new();
    let mut omitted = 0;
    for entry in entries {
        if omitted == 0 && content.len() + entry.len() < max_bytes {
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
