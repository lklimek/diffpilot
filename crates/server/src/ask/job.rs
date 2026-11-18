//! The `ask` job of one question: spec, submit, follow, answer check and the context pack built at claim.

use analysis::lines::changed_line_count;
use analysis::verify::{self, LineFacts, VerifyError, VerifyIssue, wording};
use chrono::Utc;
use contracts::agent::analyse_flow::{AgentStep, FlowAnalysisOutput};
use contracts::agent::ask::AskOutput;
use contracts::agent::{self as agent, JobKind, StrictSchemaError};
use contracts::api::analyses::AnalysisSummary;
use contracts::api::questions::AskState;
use contracts::ids::{AnalysisId, JobId, QuestionId, StepId};
use contracts::job::access::AgentAccess;
use contracts::job::result::{JobOutcome, JobResult};
use contracts::job::spec::{
    CredentialKind, JobLimits, JobPriority, JobSpec, LlmCredentialRef, RepoCheckout,
};
use db::error::DbError;
use db::repo::Db;
use db::repo::documents::DocumentKind;
use db::repo::jobs::JobStage;
use db::repo::questions::{QuestionRecord, QuestionUpdate};
use db::repo::settings::CallFailure;
use futures::StreamExt;
use serde_json::Value;

use super::{MAX_ANSWER_CHARS, MAX_EVIDENCE, set, superseded};
use crate::api::settings::plain_failure;
use crate::github::mirror::{self, MirrorError};
use crate::github::oauth::endpoint;
use crate::jobs::claim::ContextPackError;
use crate::jobs::executor::{ExecutorError, JobHandle, JobRequest, JobUpdate};
use crate::memcan::profile;
use crate::model::{ModelContext, select_model};
use crate::pipeline::context::{StoredResults, pack};
use crate::pipeline::prepass::PrepassRunError;
use crate::pipeline::{self, CREDENTIAL_ID, PrDiffError};
use crate::prompts::{
    self, AnsweredQuestion, AskContext, AskInput, JobContext, Profile, PromptError,
};
use crate::state::AppState;

/// Failure sentence of an answer that did not pass the checks; it is never shown.
pub(super) const UNFIT_ANSWER: &str =
    "The LLM's answer did not follow the answer rules, so it is not shown. Ask again.";
/// Failure sentence of a question cancelled by a re-analysis.
pub(super) const SUPERSEDED: &str = "The question was cancelled because the pull request was analysed again. Ask it on the new analysis.";
/// Failure sentence of a server-side failure.
pub(super) const SERVER_ERROR: &str =
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
    /// The pre-pass could not be read or computed.
    #[error("the pre-pass of the analysis is not available")]
    Prepass(#[from] PrepassRunError),
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
        Err((analysis, error)) => {
            tracing::error!(%question, error = ?error, "the question could not be answered");
            analysis
        }
    };
    let Some(analysis) = analysis else {
        return;
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
        .get(question, Utc::now())
        .await
        .map_err(|error| (None, error.into()))?;
    let analysis = record.analysis_id;
    follow_record(state, record)
        .await
        .map_err(|error| (Some(analysis), error))
}

async fn follow_record(state: &AppState, record: QuestionRecord) -> Result<(), FollowError> {
    let (id, analysis) = (record.question.id, record.analysis_id);
    // A provisional answer is followed on: the job's result is authoritative.
    if record.question.state == AskState::Failed || record.answered_at.is_some() {
        return Ok(());
    }
    let summary = state.db.analyses().summary(analysis).await?;
    let handle = match record.job_id {
        Some(job_id) => JobHandle { job_id },
        None => {
            if superseded(&state.db.analyses().get(analysis).await?) {
                set(
                    state,
                    analysis,
                    id,
                    &QuestionUpdate::Failed(SUPERSEDED.to_owned()),
                )
                .await?;
                return Ok(());
            }
            let handle = submit(state, &summary, &record).await?;
            state.db.questions().set_job(id, handle.job_id).await?;
            handle
        }
    };
    // A re-analysis between the check and the submit (or while the server was down): stop the job; its
    // cancelled result fails the question.
    if superseded(&state.db.analyses().get(analysis).await?)
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
            // Published at once (ADR-004); the result is authoritative and may replace it.
            JobUpdate::OutputReady(output) => match check(state, &summary, output).await? {
                Ok(answer) => Some(QuestionUpdate::Provisional(answer)),
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
                let end = finish(state, &summary, id, result).await?;
                set(state, analysis, id, &end).await?;
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
    summary: &AnalysisSummary,
    question: QuestionId,
    result: JobResult,
) -> Result<QuestionUpdate, FollowError> {
    let failure = match result.outcome {
        JobOutcome::Succeeded { output } => {
            return Ok(match check(state, summary, output).await? {
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
            tracing::info!(%question, "question cancelled");
            return Ok(QuestionUpdate::Failed(SUPERSEDED.to_owned()));
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
    summary: &AnalysisSummary,
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
        let pr = &summary.pr;
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
    let (_, step) = stored_step(&state.db, summary.id, &question.step_id)
        .await?
        .ok_or(FollowError::UnknownStep)?;
    let (mirror_dir, files) = pipeline::pr_diff(state, summary).await?;
    let prepass = pipeline::prepass::ensure_for_read(state, summary, &mirror_dir, &files).await?;
    let memcan_profile = profile::stored(&state.db, summary.id).await?;
    let pr = &summary.pr;
    let prompt = prompts::ask(&AskInput {
        job: JobContext {
            pr,
            language: summary.language,
            sub_agents: prompts::SUB_AGENTS_DEFAULT,
            prepass: &prepass,
        },
        step_id: &question.step_id,
        step: &step,
        question: &question.text,
        profile: Profile {
            file: prepass.profile_file.as_deref(),
            memcan: memcan_profile.text(),
        },
    })?;
    let spec = JobSpec {
        job_id: JobId::new_random(),
        attempt: 0,
        kind: JobKind::Ask,
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
        prompt,
        output_schema: agent::output_schema(JobKind::Ask)?,
        flow_file: None,
        model: select_model(
            JobKind::Ask,
            &ModelContext {
                changed_lines: changed_line_count(&files),
            },
            state
                .db
                .settings()
                .llm_selection()
                .await?
                .as_ref()
                .map(|s| &s.choice),
        ),
        limits: JobLimits::default(),
        credential: LlmCredentialRef {
            id: CREDENTIAL_ID.to_owned(),
            kind: CredentialKind::CodexChatgpt,
        },
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
            pr_repo: pr.repo.clone(),
        })
        .await?;
    tracing::debug!(question = %question.id, job = %handle.job_id, "ask job submitted");
    Ok(handle)
}

/// The context pack of an `ask` job, built at claim: the stored results with the asked step in full, the rest of
/// its flow analysis and this user's questions answered earlier across the PR. Without the stored step (never for a
/// finished analysis) the pack holds the stored results only, so the claim is never blocked.
pub async fn context_pack(
    db: &Db,
    analysis: AnalysisId,
    question: QuestionId,
) -> Result<Option<String>, ContextPackError> {
    let record = db.questions().get(question, Utc::now()).await?;
    let summary = db.analyses().summary(analysis).await?;
    let results = StoredResults::load(db, analysis).await?;
    let step_id = &record.question.step_id;
    let Some(flow) = results
        .flows
        .iter()
        .find(|flow| step_of(flow, step_id).is_some())
    else {
        tracing::warn!(%analysis, %question, "the asked step is not stored; the context pack holds the analysis only");
        return Ok(pack(&summary.pr, &results, None)?);
    };
    let answered = db.questions().history_before(question).await?;
    let earlier: Vec<AnsweredQuestion<'_>> = answered
        .iter()
        .map(|(question, output)| AnsweredQuestion {
            question,
            answer: &output.answer,
        })
        .collect();
    let ask = AskContext {
        step_id,
        flow,
        earlier: &earlier,
    };
    Ok(pack(&summary.pr, &results, Some(ask))?)
}
