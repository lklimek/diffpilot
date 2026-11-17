//! Analysis pipeline orchestration over `JobExecutor` (brief §5 Pipeline, Agent sessions; plan §9 WP3-PIPE).
//!
//! One driver task per analysis ([`driver`]). It prepares the analysis (fetches base tip and head into the mirror
//! and stores their merge base as the analysis base, GitHub's three-dot comparison), reads the GitHub snapshot,
//! runs the deterministic pre-pass, then `plan` (in directory batches for huge pull requests) and `pr_context`,
//! then one `analyse_flow` per planned flow. Every output is checked against its schema and verified
//! (`analysis::verify`); a rejected output is repaired at most twice. Progress is derived from the stored
//! documents. The driver is restart-safe (`runner-protocol.md` "Pipeline recovery"): stages with a document are
//! skipped, the others are submitted again, which `JobExecutor::submit` answers with the existing live or
//! succeeded job of the stage and generation. A failure ends the analysis (Retry runs it again); a cancel
//! discards its partial results and agent session.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, PoisonError};

use analysis::diff::{self, DiffError, FileDiff};
use contracts::agent::StrictSchemaError;
use contracts::api::analyses::{AnalysisState, AnalysisSummary, FlowRef};
use contracts::ids::AnalysisId;
use db::error::DbError;
use db::repo::settings::CallFailure;

use crate::api::settings::plain_failure;
use crate::github::mirror::{self, MirrorError};
use crate::jobs::executor::ExecutorError;
use crate::prompts::PromptError;
use crate::state::AppState;

pub mod batching;
pub mod context;
pub mod control;
mod driver;
pub mod ingest;
pub mod lines;
pub mod prepass;
pub mod tour;
pub mod unsent;
pub mod verdict;

use prepass::PrepassRunError;
use verdict::VerdictError;

/// The LLM credential pipeline jobs run with: the id the runner reports by default (ADR-003: one credential).
pub const CREDENTIAL_ID: &str = "default";

/// Analyses with a driver task in this process.
static DRIVING: LazyLock<Mutex<HashSet<AnalysisId>>> = LazyLock::new(Mutex::default);

/// The pipeline stopped.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// The database failed.
    #[error("the pipeline could not read or store analysis data")]
    Db(#[from] DbError),
    /// The repository could not be fetched or read.
    #[error("the repository mirror failed")]
    Mirror(#[from] MirrorError),
    /// The diff could not be computed.
    #[error("the diff could not be computed")]
    Diff(#[from] DiffError),
    /// The diff task panicked or was cancelled.
    #[error("the diff task did not finish")]
    DiffTask(#[source] tokio::task::JoinError),
    /// The stored base is not the merge base after preparing.
    #[error("the analysis base is not the merge base")]
    NotPrepared,
    /// The job executor failed.
    #[error("the job executor failed")]
    Executor(#[from] ExecutorError),
    /// The pre-pass failed.
    #[error("the pre-pass failed")]
    Prepass(#[from] PrepassRunError),
    /// Verification could not run.
    #[error("the agent output could not be checked")]
    Verdict(#[from] VerdictError),
    /// A prompt could not be rendered.
    #[error("the prompt could not be rendered")]
    Prompt(#[from] PromptError),
    /// The output schema of a job kind could not be generated.
    #[error("the output schema could not be generated")]
    Schema(#[from] StrictSchemaError),
    /// An agent output does not match its schema or its stage.
    #[error("the output of stage {stage} does not match its schema: {detail}")]
    InvalidOutput {
        /// The stage key.
        stage: String,
        /// The first schema error or the mismatch.
        detail: String,
        /// The flow, for `analyse_flow` stages.
        flow: Option<FlowRef>,
    },
    /// The plan or the PR context still failed verification after every repair run.
    #[error("the output of stage {stage} failed verification after every repair run")]
    RepairExhausted {
        /// The stage key.
        stage: String,
        /// What the output is, for the user ("plan of the flows", "PR context").
        what: &'static str,
    },
    /// A job failed.
    #[error("the job of stage {stage} failed ({failure:?}): {detail}")]
    JobFailed {
        /// The stage key.
        stage: String,
        /// Why, typed: the user sees only its fixed sentence (`api::settings::plain_failure`, shared with
        /// Settings).
        failure: CallFailure,
        /// The runner's text (provider output; logs only, never shown to the user).
        detail: String,
        /// The flow, for `analyse_flow` stages.
        flow: Option<FlowRef>,
    },
    /// A job was cancelled.
    #[error("the job of stage {stage} was cancelled")]
    Cancelled {
        /// The stage key.
        stage: String,
    },
    /// The analysis was ended meanwhile (cancelled by the user); the driver stops.
    #[error("the analysis was ended while the pipeline ran")]
    Stopped,
    /// A job's update stream ended without a result.
    #[error("the job of stage {stage} ended without a result")]
    NoResult {
        /// The stage key.
        stage: String,
    },
}

impl PipelineError {
    /// The analysis state this error ends in.
    fn final_state(&self) -> AnalysisState {
        if let Self::JobFailed {
            flow,
            failure: CallFailure::Failed(contracts::job::result::JobFailureKind::Configuration),
            ..
        } = self
        {
            return AnalysisState::Failed {
                reason: Some(contracts::api::error::ApiErrorCode::Configuration),
                flow: flow.clone(),
                message: plain_failure(Some(CallFailure::Failed(
                    contracts::job::result::JobFailureKind::Configuration,
                )))
                .to_owned(),
            };
        }
        let (flow, message): (Option<FlowRef>, Cow<'static, str>) = match self {
            Self::Cancelled { .. } | Self::Stopped => return AnalysisState::Cancelled,
            Self::Mirror(_) => (
                None,
                Cow::Borrowed("The repository could not be fetched. Retry the analysis."),
            ),
            Self::InvalidOutput { flow, .. } => (
                flow.clone(),
                Cow::Borrowed("The agent's answer did not have the expected structure."),
            ),
            Self::RepairExhausted { what, .. } => (
                None,
                Cow::Owned(format!(
                    "The LLM's {what} did not pass the checks, also after two repair runs. Retry the analysis."
                )),
            ),
            Self::JobFailed { flow, failure, .. } => (
                flow.clone(),
                Cow::Owned(format!(
                    "The agent run failed. {} Retry the analysis.",
                    plain_failure(Some(*failure))
                )),
            ),
            Self::Db(_)
            | Self::Diff(_)
            | Self::DiffTask(_)
            | Self::NotPrepared
            | Self::Executor(_)
            | Self::Prepass(_)
            | Self::Verdict(_)
            | Self::Prompt(_)
            | Self::Schema(_)
            | Self::NoResult { .. } => (
                None,
                Cow::Borrowed(
                    "The analysis stopped because of a server error. Retry the analysis.",
                ),
            ),
        };
        AnalysisState::Failed {
            reason: None,
            flow,
            message: message.into_owned(),
        }
    }
}

/// Starts the driver task of `analysis` unless one runs already.
pub fn start(state: &AppState, analysis: AnalysisId) {
    {
        let mut driving = DRIVING.lock().unwrap_or_else(PoisonError::into_inner);
        if !driving.insert(analysis) {
            return;
        }
    }
    let state = state.clone();
    tokio::spawn(async move {
        let ended = match driver::drive(&state, analysis).await {
            Ok(()) => false,
            Err(error) => end(&state, analysis, error).await,
        };
        DRIVING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&analysis);
        // A Retry between this driver's end and its removal found the slot taken: start the next driver now.
        if ended {
            match state.db.analyses().summary(analysis).await {
                Ok(summary) if control::is_live(&summary.state) => start(&state, analysis),
                Ok(_) => {}
                Err(error) => {
                    tracing::error!(%analysis, ?error, "could not re-read the ended analysis");
                }
            }
        }
    });
}

/// Ends the analysis after `error`: a failure cancels its jobs and stores `failed`; a cancel discards its partial
/// results and agent session. Returns whether the end was handled (state stored or already ended).
async fn end(state: &AppState, analysis: AnalysisId, error: PipelineError) -> bool {
    let final_state = error.final_state();
    let cancelled = matches!(final_state, AnalysisState::Cancelled);
    if cancelled {
        tracing::info!(%analysis, "analysis pipeline stopped: cancelled");
        if let Err(error) = control::discard(state, analysis).await {
            tracing::error!(%analysis, ?error, "could not discard the cancelled analysis's results");
        }
    } else {
        tracing::error!(%analysis, error = ?error, "analysis pipeline failed");
        // Every unfinished job of the analysis, also those submitted before a restart.
        if let Err(error) = state.executor.cancel_analysis(analysis).await {
            tracing::error!(%analysis, error = ?error, "could not cancel the jobs of the ended analysis");
        }
    }
    match control::set_state_if(state, analysis, &final_state, control::is_live).await {
        Ok(_) => true,
        Err(error) => {
            tracing::error!(%analysis, error = ?error, "could not store the ended analysis state");
            false
        }
    }
}

/// Starts a driver for every analysis left planning, running or paused (server start).
pub async fn resume_unfinished(state: &AppState) -> Result<usize, DbError> {
    let unfinished = state.db.analyses().unfinished().await?;
    for analysis in &unfinished {
        start(state, analysis.id);
    }
    if !unfinished.is_empty() {
        tracing::info!(analyses = unfinished.len(), "resuming unfinished analyses");
    }
    Ok(unfinished.len())
}

/// Fetches the base tip and head into the mirror and stores their merge base as the analysis base; returns the
/// mirror.
async fn prepare(
    state: &AppState,
    summary: &mut AnalysisSummary,
) -> Result<PathBuf, PipelineError> {
    let pr = &summary.pr;
    let mirror =
        mirror::ensure_commits(&state.config, &pr.repo, &[&pr.base_sha, &pr.head_sha]).await?;
    let merge_base = mirror::merge_base(&mirror, &pr.base_sha, &pr.head_sha).await?;
    if merge_base != pr.base_sha {
        state
            .db
            .analyses()
            .set_base_sha(summary.id, &merge_base)
            .await?;
        tracing::debug!(analysis = %summary.id, base = %merge_base.0, "analysis base set to the merge base");
        summary.pr.base_sha = merge_base;
    }
    Ok(mirror)
}

/// The pull request's diff could not be produced.
#[derive(Debug, thiserror::Error)]
pub enum PrDiffError {
    /// The analysis is not prepared yet: its stored base is not the merge base.
    #[error("the analysis base is not prepared yet")]
    NotPrepared,
    /// The repository could not be fetched or read.
    #[error("the repository mirror failed")]
    Mirror(#[from] MirrorError),
    /// The diff could not be computed.
    #[error("the diff could not be computed")]
    Diff(#[from] DiffError),
    /// The diff task panicked or was cancelled.
    #[error("the diff task did not finish")]
    Task(#[source] tokio::task::JoinError),
}

impl From<PrDiffError> for PipelineError {
    fn from(error: PrDiffError) -> Self {
        match error {
            PrDiffError::Mirror(error) => Self::Mirror(error),
            PrDiffError::Diff(error) => Self::Diff(error),
            PrDiffError::Task(error) => Self::DiffTask(error),
            PrDiffError::NotPrepared => Self::NotPrepared,
        }
    }
}

/// The mirror of the analysed repository (fetched again if missing) and the diff between the stored base and
/// head of the analysis; [`PrDiffError::NotPrepared`] until the pipeline has stored the merge base (preparing
/// is the only writer of the base, and the merge base of a merge base and the head is that merge base).
pub async fn pr_diff(
    state: &AppState,
    summary: &AnalysisSummary,
) -> Result<(PathBuf, Vec<FileDiff>), PrDiffError> {
    let pr = &summary.pr;
    let mirror =
        mirror::ensure_commits(&state.config, &pr.repo, &[&pr.base_sha, &pr.head_sha]).await?;
    if mirror::merge_base(&mirror, &pr.base_sha, &pr.head_sha).await? != pr.base_sha {
        return Err(PrDiffError::NotPrepared);
    }
    let files = diff_files(mirror.clone(), summary).await?;
    Ok((mirror, files))
}

async fn diff_files(
    mirror: PathBuf,
    summary: &AnalysisSummary,
) -> Result<Vec<FileDiff>, PrDiffError> {
    let (base, head) = (summary.pr.base_sha.clone(), summary.pr.head_sha.clone());
    tokio::task::spawn_blocking(move || diff::diff(&mirror, &base, &head))
        .await
        .map_err(PrDiffError::Task)?
        .map_err(PrDiffError::from)
}
