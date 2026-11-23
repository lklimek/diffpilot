//! The `ask` job of one question: spec, submit, follow, answer check and the context pack built at claim.

use std::collections::HashSet;

use analysis::lines::{changed_line_count, ranges_overlap};
use analysis::verify::{self, LineFacts, VerifyError, VerifyIssue, wording};
use contracts::agent::analyse_flow::{AgentStep, FlowAnalysisOutput};
use contracts::agent::ask::AskOutput;
use contracts::agent::{self as agent, JobKind, StrictSchemaError};
use contracts::api::analyses::{AnalysisState, AnalysisSummary};
use contracts::api::code::{DiffLineKind, RangeQuery};
use contracts::api::files::{HunkPart, PartOwner};
use contracts::api::pr::PrSummary;
use contracts::api::questions::{AskAnchor, AskState};
use contracts::api::tour::{Flow, Step};
use contracts::common::{LineRange, LineSide};
use contracts::ids::{AnalysisId, FlowId, JobId, QuestionId, StepId};
use contracts::job::access::AgentAccess;
use contracts::job::result::{JobOutcome, JobResult};
use contracts::job::spec::{
    JobLimits, JobPriority, JobSpec, JobTarget, PullRequestTarget, RepoCheckout,
};
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::Db;
use db::repo::documents::DocumentKind;
use db::repo::jobs::JobStage;
use db::repo::questions::{QuestionRecord, QuestionUpdate};
use db::repo::settings::CallFailure;
use futures::StreamExt;
use serde_json::Value;

use super::{MAX_ANSWER_CHARS, MAX_EVIDENCE, publish_provisional, set, superseded};
use crate::api::settings::plain_failure;
use crate::github::mirror::{self, MirrorError};
use crate::github::oauth::endpoint;
use crate::jobs::claim::ContextPackError;
use crate::jobs::executor::{ExecutorError, JobHandle, JobRequest, JobUpdate};
use crate::memcan::profile;
use crate::model::{ModelContext, select_model};
use crate::pipeline::context::{StoredResults, pack};
use crate::pipeline::prepass::PrepassRunError;
use crate::pipeline::tour::{self, TourLoadError};
use crate::pipeline::{self, PrDiffError, files};
use crate::prompts::{
    self, AnsweredQuestion, AskContext, AskFlowInput, AskInput, AskLinesInput, AskPrInput, Asked,
    AskedLines, JobContext, Profile, PromptError, StepSummary, ThreadQuestion, TouchedStep,
};
use crate::review_loop::assess;
use crate::state::AppState;

/// Failure sentence of an answer that did not pass the checks; it is never shown.
pub(crate) const UNFIT_ANSWER: &str =
    "The LLM's answer did not follow the answer rules, so it is not shown. Ask again.";
/// Failure sentence of a question cancelled because its PR session was removed (a re-analysis replaced it).
pub(super) const SUPERSEDED: &str = "The question was cancelled because the pull request was analysed again. Ask it on the new analysis.";
/// Failure sentence of a question cancelled together with its analysis.
pub(super) const ANALYSIS_CANCELLED: &str =
    "The question was cancelled because its analysis was cancelled.";
/// Failure sentence of a question whose job was cancelled for another reason.
pub(crate) const CANCELLED: &str = "The question was cancelled before it was answered. Ask again.";
/// Failure sentence of a server-side failure.
pub(crate) const SERVER_ERROR: &str =
    "The question could not be answered because of a server error. Ask again later.";

/// Following a question stopped on a server-side error; the question fails with [`SERVER_ERROR`].
#[derive(Debug, thiserror::Error)]
pub(super) enum FollowError {
    /// The database failed.
    #[error("the question or its analysis could not be read or stored")]
    Db(#[from] DbError),
    /// The job executor failed.
    #[error("the ask job could not be submitted or followed")]
    Executor(#[from] ExecutorError),
    /// The asked step is no longer stored.
    #[error("the asked step is not stored")]
    UnknownStep,
    /// The asked flow's analysis is no longer stored.
    #[error("the asked flow is not stored")]
    UnknownFlow,
    /// The pre-pass could not be read or computed.
    #[error("the pre-pass of the analysis is not available")]
    Prepass(#[from] PrepassRunError),
    /// The Tour (owners of the asked lines) could not be built.
    #[error("the Tour of the analysis is not available")]
    Tour(#[from] TourLoadError),
    /// The diff of the analysis is not available.
    #[error("the diff of the analysis is not available")]
    Diff(#[from] PrDiffError),
    /// The prompt could not be rendered.
    #[error("the ask prompt could not be rendered")]
    Prompt(#[from] PromptError),
    /// The output schema could not be generated.
    #[error("the ask output schema could not be generated")]
    Schema(#[from] StrictSchemaError),
    /// The repository mirror failed while checking the answer's evidence.
    #[error("the repository mirror failed")]
    Mirror(#[from] MirrorError),
    /// The answer's evidence could not be checked.
    #[error("the answer's evidence could not be checked")]
    Verify(#[source] VerifyError),
    /// The check task panicked or was cancelled.
    #[error("the answer check did not finish")]
    Task(#[source] tokio::task::JoinError),
    /// The job's update stream ended without a result.
    #[error("the ask job ended without a result")]
    NoResult,
}

/// Why an answer is not shown; logged by kind only (the output is untrusted text).
#[derive(Debug)]
enum Unfit {
    /// Not the schema's shape.
    Schema,
    /// Over [`MAX_ANSWER_CHARS`] or [`MAX_EVIDENCE`].
    TooLarge,
    /// Banned wording (brief §1): a verdict or a percentage.
    Wording(Vec<String>),
    /// Rejected by verification (blank, markup or links, evidence outside the pinned tree).
    Rejected(Vec<VerifyIssue>),
}

impl Unfit {
    fn codes(&self) -> Vec<&str> {
        match self {
            Self::Schema => vec!["schema"],
            Self::TooLarge => vec!["too_large"],
            Self::Wording(phrases) => phrases.iter().map(String::as_str).collect(),
            Self::Rejected(issues) => issues.iter().map(|issue| issue.kind.code()).collect(),
        }
    }
}

/// The stored flow and step of the canonical Tour step id `step` (`f<flow>.<n>`, `StepId::of`); aliases such as
/// `f1.01` name no step.
pub(super) async fn stored_step(
    db: &Db,
    analysis: AnalysisId,
    step: &StepId,
) -> Result<Option<(FlowAnalysisOutput, AgentStep)>, DbError> {
    let flows: Vec<(String, FlowAnalysisOutput)> =
        db.documents().list(analysis, DocumentKind::Flow).await?;
    Ok(flows
        .into_iter()
        .find_map(|(_, flow)| step_of(&flow, step).map(|found| (flow.clone(), found.clone()))))
}

/// The stored analysis of `flow`, if any.
pub(super) async fn stored_flow(
    db: &Db,
    analysis: AnalysisId,
    flow: &FlowId,
) -> Result<Option<FlowAnalysisOutput>, DbError> {
    let flows: Vec<(String, FlowAnalysisOutput)> =
        db.documents().list(analysis, DocumentKind::Flow).await?;
    Ok(flows
        .into_iter()
        .map(|(_, stored)| stored)
        .find(|stored| stored.flow_id == *flow))
}

fn step_of<'f>(flow: &'f FlowAnalysisOutput, step: &StepId) -> Option<&'f AgentStep> {
    flow.steps
        .iter()
        .zip(1u32..)
        .find(|(_, number)| StepId::of(&flow.flow_id, *number) == *step)
        .map(|(found, _)| found)
}

/// Drives the question to `answered` or `failed`; a server-side error fails it with a plain sentence.
pub(super) async fn run(state: &AppState, question: QuestionId) {
    let analysis = match follow(state, question).await {
        Ok(()) => return,
        // Removed with its flow's steps (the flow is analysed again): nothing is left to answer.
        Err((_, FollowError::Db(DbError::NotFound)))
            if matches!(
                state.db.questions().get(question, Timestamp::now()).await,
                Err(DbError::NotFound)
            ) =>
        {
            tracing::debug!(%question, "question removed with its flow's steps; its answer is dropped");
            return;
        }
        Err((analysis, error)) => {
            tracing::error!(%question, error = ?error, "the question could not be answered");
            analysis
        }
    };
    if let Err(error) = set(
        state,
        analysis,
        question,
        &QuestionUpdate::Failed(SERVER_ERROR.to_owned()),
    )
    .await
    {
        tracing::error!(%question, error = ?error, "could not store the failed question");
    }
}

type Followed = Result<(), (Option<AnalysisId>, FollowError)>;

async fn follow(state: &AppState, question: QuestionId) -> Followed {
    let record = state
        .db
        .questions()
        .get(question, Timestamp::now())
        .await
        .map_err(|error| (None, error.into()))?;
    let analysis = record.analysis_id;
    follow_record(state, record)
        .await
        .map_err(|error| (analysis, error))
}

async fn follow_record(state: &AppState, record: QuestionRecord) -> Result<(), FollowError> {
    let id = record.question.id;
    let analysis = record.analysis_id;
    // A provisional answer is followed on: the job's result is authoritative.
    if record.question.state == AskState::Failed || record.answered_at.is_some() {
        return Ok(());
    }
    let summary = match analysis {
        Some(analysis) => Some(state.db.analyses().summary(analysis).await?),
        None => None,
    };
    let mut pr = match &summary {
        Some(summary) => summary.pr.clone(),
        None => {
            state
                .db
                .pull_requests()
                .get(record.pull_request_id.ok_or(DbError::NotFound)?)
                .await?
                .pr
        }
    };
    let handle = match record.job_id {
        Some(job_id) => JobHandle { job_id },
        None => {
            if let Some(analysis) = analysis
                && superseded(&state.db.analyses().get(analysis).await?)
            {
                set(
                    state,
                    Some(analysis),
                    id,
                    &QuestionUpdate::Failed(SUPERSEDED.to_owned()),
                )
                .await?;
                return Ok(());
            }
            let handle = match &summary {
                Some(summary) => submit(state, summary, &record).await?,
                None => super::private::submit(state, &pr, &record).await?,
            };
            state.db.questions().set_job(id, handle.job_id).await?;
            handle
        }
    };
    if analysis.is_none() {
        match state.db.jobs().get(handle.job_id).await {
            Ok(job) => {
                if let Some(checkout) = job.spec.target.checkout() {
                    pr.base_sha = checkout.base_sha.clone();
                    pr.head_sha = checkout.head_sha.clone();
                }
            }
            Err(DbError::NotFound) => {} // Scripted executors store their jobs outside the DB.
            Err(error) => return Err(error.into()),
        }
    }
    // A re-analysis between the check and the submit (or while the server was down): stop the job; its
    // cancelled result fails the question.
    if let Some(analysis) = analysis
        && superseded(&state.db.analyses().get(analysis).await?)
        && let Err(error) = state.executor.cancel(&handle).await
    {
        tracing::error!(question = %id, ?error, "could not cancel the ask job of a superseded analysis");
    }
    let mut current = record.question.state;
    // Re-attached to a provisional answer: its attempt may be gone (lease expired, requeued). The executor replays
    // the current attempt's ready output or the result, so the answer is shown again only if it still stands.
    if current == AskState::Answered {
        current = set(state, analysis, id, &QuestionUpdate::Answering)
            .await?
            .state;
    }
    let mut updates = state.executor.events(&handle);
    while let Some(update) = updates.next().await {
        let next = match update? {
            // The job started (or runs again after a pause); events after a ready answer keep it shown.
            JobUpdate::Event(_) => {
                (current == AskState::Pending).then_some(QuestionUpdate::Answering)
            }
            // A new attempt: a ready answer of the lost attempt is withdrawn.
            JobUpdate::Requeued { .. } => {
                (current != AskState::Answering).then_some(QuestionUpdate::Answering)
            }
            // Published at once (ADR-004); the result is authoritative and may replace it. The check takes time:
            // whether the answer may still be shown is decided again under the lock that publishes it.
            JobUpdate::OutputReady(output) => match check(state, &pr, output).await? {
                Ok(answer) => {
                    match publish_provisional(state, analysis, id, handle.job_id, answer).await? {
                        Some(question) => current = question.state,
                        None => {
                            tracing::debug!(question = %id,
                                "ready answer not shown: the question was cancelled while it was checked");
                        }
                    }
                    None
                }
                Err(unfit) => {
                    tracing::debug!(question = %id, reasons = ?unfit.codes(),
                        "ready answer not shown; the job's result decides");
                    None
                }
            },
            JobUpdate::Paused { resumes_at } => {
                tracing::info!(question = %id, ?resumes_at, "question waits for the LLM usage limit");
                Some(QuestionUpdate::Pending)
            }
            JobUpdate::Finished { result, .. } => {
                let end = finish(state, analysis, &pr, id, result).await?;
                let question = set(state, analysis, id, &end).await?;
                if matches!(end, QuestionUpdate::Answered(_)) {
                    assess::answer_landed(state, &question.anchor).await;
                }
                return Ok(());
            }
        };
        if let Some(next) = next {
            current = set(state, analysis, id, &next).await?.state;
        }
    }
    Err(FollowError::NoResult)
}

/// The question's end from the job's result: the checked answer or a plain failure sentence.
async fn finish(
    state: &AppState,
    analysis: Option<AnalysisId>,
    pr: &PrSummary,
    question: QuestionId,
    result: JobResult,
) -> Result<QuestionUpdate, FollowError> {
    let failure = match result.outcome {
        JobOutcome::Succeeded { output } => {
            return Ok(match check(state, pr, output).await? {
                Ok(answer) => {
                    tracing::info!(%question, "question answered");
                    QuestionUpdate::Answered(answer)
                }
                Err(unfit) => {
                    tracing::warn!(%question, reasons = ?unfit.codes(),
                        "the LLM's answer did not pass the checks; the question failed");
                    QuestionUpdate::Failed(UNFIT_ANSWER.to_owned())
                }
            });
        }
        JobOutcome::Cancelled => {
            let record = match analysis {
                Some(analysis) => Some(state.db.analyses().get(analysis).await?),
                None => None,
            };
            let sentence = if record.as_ref().is_some_and(superseded) {
                SUPERSEDED
            } else if record
                .as_ref()
                .is_some_and(|r| matches!(r.state, AnalysisState::Cancelled))
            {
                ANALYSIS_CANCELLED
            } else {
                CANCELLED
            };
            tracing::info!(%question, analysis = ?analysis, "question cancelled");
            return Ok(QuestionUpdate::Failed(sentence.to_owned()));
        }
        JobOutcome::Failed { kind, .. } => CallFailure::Failed(kind),
        JobOutcome::UsageLimited { .. } => CallFailure::UsageLimited,
    };
    // The runner's text is provider output: neither shown nor logged here (it stays in the job's result).
    tracing::warn!(%question, ?failure, "the ask job failed");
    Ok(QuestionUpdate::Failed(format!(
        "{} Ask again.",
        plain_failure(Some(failure))
    )))
}

/// Checks an `ask` output: schema, size caps, wording rules of brief §1, plain text without markup or foreign
/// links, evidence lines in the pinned tree. `Ok(Err(..))` = not shown.
async fn check(
    state: &AppState,
    pr: &PrSummary,
    output: Value,
) -> Result<Result<AskOutput, Unfit>, FollowError> {
    let schema = agent::output_schema(JobKind::Ask)?;
    let Ok(validator) = jsonschema::validator_for(&schema) else {
        return Ok(Err(Unfit::Schema));
    };
    if !validator.is_valid(&output) {
        return Ok(Err(Unfit::Schema));
    }
    let Ok(answer) = serde_json::from_value::<AskOutput>(output) else {
        return Ok(Err(Unfit::Schema));
    };
    if answer.answer.chars().count() > MAX_ANSWER_CHARS || answer.evidence.len() > MAX_EVIDENCE {
        return Ok(Err(Unfit::TooLarge));
    }
    let wording = wording::ask(&answer);
    if !wording.is_empty() {
        return Ok(Err(Unfit::Wording(
            wording.into_iter().map(|warning| warning.phrase).collect(),
        )));
    }
    let files = verify::ask_files(&answer);
    let facts = if files.is_empty() {
        LineFacts::default()
    } else {
        let repo =
            mirror::ensure_commits(&state.config, &pr.repo, &[&pr.base_sha, &pr.head_sha]).await?;
        let (base, head) = (pr.base_sha.clone(), pr.head_sha.clone());
        tokio::task::spawn_blocking(move || LineFacts::load(&repo, &base, &head, &files))
            .await
            .map_err(FollowError::Task)?
            .map_err(FollowError::Verify)?
    };
    match verify::ask(&answer, &facts) {
        Ok(()) => Ok(Ok(answer)),
        Err(VerifyError::Rejected(issues)) => Ok(Err(Unfit::Rejected(issues))),
        Err(error) => Err(FollowError::Verify(error)),
    }
}

/// Submits the question's `ask` job (idempotent per question). The analysis's session is set at claim and the
/// context pack built then (`context_pack`).
async fn submit(
    state: &AppState,
    summary: &AnalysisSummary,
    record: &QuestionRecord,
) -> Result<JobHandle, FollowError> {
    let question = &record.question;
    let (mirror_dir, files) = pipeline::pr_diff(state, summary).await?;
    let prepass = pipeline::prepass::ensure_for_read(state, summary, &mirror_dir, &files).await?;
    let memcan_profile = profile::stored(&state.db, summary.id).await?;
    let pr = &summary.pr;
    let job = JobContext {
        pr,
        language: summary.language,
        sub_agents: prompts::SUB_AGENTS_DEFAULT,
        prepass: &prepass,
    };
    let profile = Profile {
        file: prepass.profile_file.as_deref(),
        memcan: memcan_profile.text(),
    };
    // The thread rides in the prompt, so a follow-up keeps it when the session is lost or forked again.
    let earlier = state
        .db
        .questions()
        .thread_before(question.id, Timestamp::now())
        .await?;
    let thread: Vec<ThreadQuestion<'_>> = earlier
        .iter()
        .map(|record| ThreadQuestion {
            question: &record.question.text,
            state: record.question.state,
            answer: record
                .question
                .answer
                .as_ref()
                .map(|answer| answer.answer.as_str()),
        })
        .collect();
    let prompt = match &question.anchor {
        AskAnchor::Flow { flow_id } => {
            let flow = stored_flow(&state.db, summary.id, flow_id)
                .await?
                .ok_or(FollowError::UnknownFlow)?;
            prompts::ask_flow(&AskFlowInput {
                job,
                flow: &flow,
                question: &question.text,
                thread: &thread,
                profile,
            })?
        }
        AskAnchor::Pr | AskAnchor::CommentItem { .. } => {
            let results = StoredResults::load(&state.db, summary.id).await?;
            // A question about a review comment carries the comment as its subject (T-L4).
            let item = match &question.anchor {
                AskAnchor::CommentItem { item } => Some(assess::verify_item(
                    &state.db.review_loop().item(*item).await?,
                )),
                _ => None,
            };
            prompts::ask_pr(&AskPrInput {
                job,
                plan: results.plan.as_ref(),
                pr_context: results.pr_context.as_ref(),
                item: item.as_ref(),
                question: &question.text,
                thread: &thread,
                profile,
            })?
        }
        AskAnchor::Step { step_id, lines } => {
            let (_, step) = stored_step(&state.db, summary.id, step_id)
                .await?
                .ok_or(FollowError::UnknownStep)?;
            prompts::ask(&AskInput {
                job,
                step_id,
                step: &step,
                lines: *lines,
                question: &question.text,
                thread: &thread,
                profile,
            })?
        }
        AskAnchor::Lines { path, side, range } => {
            let about = lines_about(state, summary, path, *side, *range).await?;
            let steps: Vec<TouchedStep<'_>> =
                touched(&about.flows, &about.facts, path, *side, *range)
                    .map(|(flow, step)| TouchedStep {
                        step_id: &step.id,
                        flow_id: &flow.id,
                        flow_title: &flow.llm.title,
                        step: StepSummary {
                            title: &step.llm.title,
                            file: &step.facts.file,
                            side: step.facts.side,
                            start_line: step.facts.range.start_line,
                            end_line: step.facts.range.end_line,
                            kind: step.llm.kind,
                        },
                    })
                    .collect();
            let ids: Vec<StepId> = steps.iter().map(|step| step.step_id.clone()).collect();
            // Stored before the job exists, so its claim finds them.
            state
                .db
                .questions()
                .set_touched_steps(question.id, &ids)
                .await?;
            prompts::ask_lines(&AskLinesInput {
                job,
                lines: AskedLines {
                    path,
                    side: *side,
                    range: *range,
                },
                diff: &about.diff,
                facts: &about.facts,
                steps: &steps,
                question: &question.text,
                thread: &thread,
                profile,
            })?
        }
    };
    let spec = JobSpec {
        job_id: JobId::new_random(),
        attempt: 0,
        kind: JobKind::Ask,
        target: JobTarget::PullRequest(PullRequestTarget {
            analysis_id: summary.id,
            pull_request_id: state.db.analyses().get(summary.id).await?.pull_request_id,
            repo: RepoCheckout {
                clone_url: endpoint(
                    &state.config.github.clone_url,
                    &format!("{}/{}.git", pr.repo.owner, pr.repo.name),
                )
                .to_string(),
                base_sha: pr.base_sha.clone(),
                head_sha: pr.head_sha.clone(),
            },
        }),
        prompt,
        output_schema: agent::output_schema(JobKind::Ask)?,
        flow_file: None,
        assessment: None,
        pr_data: None,
        model: select_model(
            JobKind::Ask,
            &ModelContext {
                changed_lines: changed_line_count(&files),
            },
            state.db.settings().llm_selection().await?.as_ref(),
        ),
        limits: JobLimits::default(),
        payer: contracts::ids::UserId(record.user_id.clone()),
        session: None,
        working_session: None,
        context_pack: None,
        skills: prompts::required_skills(),
        access: AgentAccess::default(),
    };
    let handle = state
        .executor
        .submit(JobRequest {
            spec,
            priority: JobPriority::Interactive,
            stage: JobStage::Ask(question.id),
            generation: 0,
            pr_repo: Some(pr.repo.clone()),
        })
        .await?;
    tracing::debug!(question = %question.id, job = %handle.job_id, "ask job submitted");
    Ok(handle)
}

/// What the server knows about asked lines: their classic diff as text, the owners of their changed lines and the
/// Tour's flows.
struct LinesAbout {
    diff: String,
    facts: Vec<HunkPart>,
    flows: Vec<Flow>,
}

/// The owners of the changed lines of `path` on `side` within `range` (all of them for the whole file), cut to
/// `range`, and the Tour's flows: from the Files view's projection of the Tour, one source with the review.
async fn lines_facts(
    state: &AppState,
    summary: &AnalysisSummary,
    path: &str,
    side: LineSide,
    range: Option<LineRange>,
) -> Result<(Vec<HunkPart>, Vec<Flow>), TourLoadError> {
    let results = StoredResults::load(&state.db, summary.id).await?;
    tour::with_projection(state, summary, results, |projection, tour| {
        let listed = files::from_tour(projection, &tour);
        let within =
            |part: &HunkPart| range.is_none_or(|range| ranges_overlap(&part.range, &range));
        let facts = projection
            .files
            .iter()
            .zip(&listed)
            .filter(|(file, _)| file.path_on(side) == Some(path))
            .flat_map(|(_, listed)| &listed.hunks)
            .flat_map(|hunk| &hunk.parts)
            .filter(|part| part.side == side && within(part))
            .map(|part| {
                let mut part = part.clone();
                if let Some(range) = range {
                    part.range.start_line = part.range.start_line.max(range.start_line);
                    part.range.end_line = part.range.end_line.min(range.end_line);
                }
                part
            })
            .collect();
        (facts, tour.flows)
    })
    .await
}

/// The steps the asked lines touch, in Tour order: every step owning or also showing one of their changed lines
/// (`facts`; for removed lines possibly a head step at the deletion marker or a moved step), and every step on
/// `path` and `side` overlapping `range` (the whole file when `None`).
fn touched<'f>(
    flows: &'f [Flow],
    facts: &[HunkPart],
    path: &str,
    side: LineSide,
    range: Option<LineRange>,
) -> impl Iterator<Item = (&'f Flow, &'f Step)> {
    let named: HashSet<StepId> = facts
        .iter()
        .flat_map(|part| {
            let owner = match &part.owner {
                PartOwner::Step { step_id, .. } => Some(step_id),
                _ => None,
            };
            owner.into_iter().chain(&part.also_in).cloned()
        })
        .collect();
    let path = path.to_owned();
    flows
        .iter()
        .flat_map(|flow| flow.steps.iter().map(move |step| (flow, step)))
        .filter(move |(_, step)| {
            named.contains(&step.id)
                || (step.facts.file == path
                    && step.facts.side == side
                    && range.is_none_or(|range| ranges_overlap(&step.facts.range, &range)))
        })
}

/// A note when a whole-file question's diff (lines 1 to [`RangeQuery::MAX_LINES`]) leaves out changed lines that
/// `facts` lists further down.
pub(super) fn clipped_diff_note(facts: &[HunkPart], range: Option<LineRange>) -> Option<String> {
    let last = facts.iter().map(|part| part.range.end_line).max()?;
    (range.is_none() && last > RangeQuery::MAX_LINES).then(|| {
        format!(
            "This diff covers lines 1-{} only; the file has changed lines up to line {last} (see the owners below). \
             Read the rest in the repository.",
            RangeQuery::MAX_LINES
        )
    })
}

/// The classic diff, owners and flows of `path` on `side` (`range`: the lines, or the whole file).
async fn lines_about(
    state: &AppState,
    summary: &AnalysisSummary,
    path: &str,
    side: LineSide,
    range: Option<LineRange>,
) -> Result<LinesAbout, FollowError> {
    let (facts, flows) = lines_facts(state, summary, path, side, range).await?;
    let (from, to) = range.map_or((1, RangeQuery::MAX_LINES), |range| {
        (range.start_line, range.end_line)
    });
    let query = RangeQuery {
        path: path.to_owned(),
        side,
        from,
        to,
    };
    let mut diff = match pipeline::lines::diff(state, summary, query).await {
        Ok(diff) if diff.hunks.is_empty() => {
            "No line of this range is changed by the pull request.\n".to_owned()
        }
        Ok(diff) => {
            let mut text = String::new();
            for hunk in &diff.hunks {
                text.push_str(&hunk.header);
                text.push('\n');
                for line in &hunk.lines {
                    text.push(match line.kind {
                        DiffLineKind::Context => ' ',
                        DiffLineKind::Added => '+',
                        DiffLineKind::Removed => '-',
                    });
                    text.push_str(&line.text);
                    text.push('\n');
                }
            }
            text
        }
        Err(error) => {
            tracing::debug!(analysis = %summary.id, ?error, "the classic diff of asked lines is not included");
            "The classic diff is not included (too large or binary); read the file at both commits.\n"
                .to_owned()
        }
    };
    if let Some(note) = clipped_diff_note(&facts, range) {
        diff.push_str(&note);
    }
    Ok(LinesAbout { diff, facts, flows })
}

/// The context pack of an `ask` job, built at claim: the stored results with the asked step in full, the rest of
/// its flow analysis (asked lines: the steps chosen at submit) and this user's questions answered earlier across the
/// PR. Without the stored step (never for a finished analysis) the pack holds the stored results only, so the claim
/// is never blocked.
pub async fn context_pack(
    db: &Db,
    analysis: AnalysisId,
    question: QuestionId,
) -> Result<Option<String>, ContextPackError> {
    let record = db.questions().get(question, Timestamp::now()).await?;
    let summary = db.analyses().summary(analysis).await?;
    let results = StoredResults::load(db, analysis).await?;
    let answered = db.questions().history_before(question).await?;
    let earlier: Vec<AnsweredQuestion<'_>> = answered
        .iter()
        .map(|(question, output)| AnsweredQuestion {
            question,
            answer: &output.answer,
        })
        .collect();
    let touched_steps: Vec<(StepId, &AgentStep)>;
    let asked = match &record.question.anchor {
        AskAnchor::Flow { flow_id } => {
            let Some(flow) = results.flows.iter().find(|flow| flow.flow_id == *flow_id) else {
                tracing::warn!(%analysis, %question, "the asked flow is not stored; the context pack holds the analysis only");
                return Ok(pack(&summary.pr, &results, None)?);
            };
            Asked::Flow { flow }
        }
        AskAnchor::Pr | AskAnchor::CommentItem { .. } => Asked::Pr,
        AskAnchor::Step { step_id, .. } => {
            let Some(flow) = results
                .flows
                .iter()
                .find(|flow| step_of(flow, step_id).is_some())
            else {
                tracing::warn!(%analysis, %question, "the asked step is not stored; the context pack holds the analysis only");
                return Ok(pack(&summary.pr, &results, None)?);
            };
            Asked::Step { step_id, flow }
        }
        AskAnchor::Lines { path, side, range } => {
            // Chosen from the Tour at submit; the claim never rebuilds it.
            let ids = record.touched_steps.clone().unwrap_or_else(|| {
                tracing::warn!(%analysis, %question,
                    "the steps of asked lines were not stored; the context pack holds no step of them");
                Vec::new()
            });
            touched_steps = ids
                .into_iter()
                .filter_map(|id| {
                    let step = results.flows.iter().find_map(|flow| step_of(flow, &id))?;
                    Some((id, step))
                })
                .collect();
            Asked::Lines {
                lines: AskedLines {
                    path,
                    side: *side,
                    range: *range,
                },
                steps: &touched_steps,
            }
        }
    };
    let ask = AskContext {
        asked,
        earlier: &earlier,
    };
    Ok(pack(&summary.pr, &results, Some(ask))?)
}
