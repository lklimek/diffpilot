//! Analysis pipeline orchestration over `JobExecutor` (stage-2 slice: no pre-pass rules, no verification beyond
//! the output schema, no repair runs, no session resume).
//!
//! One driver task per analysis. It first prepares the analysis: fetches the base tip and head into the mirror
//! and stores their merge base as the analysis base (GitHub's three-dot comparison), before any job is submitted
//! and before [`pr_diff`] serves the diff. Then it diffs, runs `plan` and `pr_context`, then one
//! `analyse_flow` per planned flow. Each succeeded output is validated against its schema and stored as a
//! document; progress is derived from the stored documents. The driver is restart-safe
//! (`runner-protocol.md` "Pipeline recovery"): stages with a document are skipped, the others are submitted
//! again, which `JobExecutor::submit` answers with the existing live or succeeded job of the stage. Preparing
//! again is a no-op: the merge base of a merge base and the head is the merge base itself. Every failure ends
//! the analysis and cancels its unfinished jobs.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, PoisonError};

use analysis::diff::{self, DiffError, FileDiff};
use chrono::Utc;
use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::agent::plan::PlanOutput;
use contracts::agent::pr_context::PrContextOutput;
use contracts::agent::{self as agent, JobKind, StrictSchemaError};
use contracts::api::analyses::{AnalysisState, AnalysisSummary, FlowRef};
use contracts::ids::{AnalysisId, JobId};
use contracts::job::access::AgentAccess;
use contracts::job::result::{JobOutcome, JobResult};
use contracts::job::spec::{
    CredentialKind, JobLimits, JobPriority, JobSpec, LlmCredentialRef, RepoCheckout,
};
use db::error::DbError;
use db::repo::documents::DocumentKind;
use db::repo::jobs::JobStage;
use futures::StreamExt;
use futures::stream::{self, BoxStream, SelectAll};
use serde::Serialize;

use crate::github::mirror::{self, MirrorError};
use crate::github::oauth::endpoint;
use crate::jobs::executor::{ExecutorError, JobHandle, JobRequest, JobUpdate};
use crate::model::{ModelContext, select_model};
use crate::state::AppState;

pub mod ingest;
pub mod lines;
pub mod prompt;
pub mod tour;

/// The LLM credential pipeline jobs run with: the id the runner reports by default (ADR-003: one credential).
pub const CREDENTIAL_ID: &str = "default";
/// Generation of every stage-2 job: repair runs come with verification (WP3-PIPE).
const GENERATION: u32 = 0;

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
    /// A job failed.
    #[error("the job of stage {stage} failed: {message}")]
    JobFailed {
        /// The stage key.
        stage: String,
        /// Plain description from the runner.
        message: String,
        /// The flow, for `analyse_flow` stages.
        flow: Option<FlowRef>,
    },
    /// A job was cancelled.
    #[error("the job of stage {stage} was cancelled")]
    Cancelled {
        /// The stage key.
        stage: String,
    },
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
        let (flow, message): (Option<FlowRef>, Cow<'static, str>) = match self {
            Self::Cancelled { .. } => return AnalysisState::Cancelled,
            Self::Mirror(_) => (
                None,
                Cow::Borrowed("The repository could not be fetched. Retry the analysis."),
            ),
            Self::InvalidOutput { flow, .. } => (
                flow.clone(),
                Cow::Borrowed("The agent's answer did not have the expected structure."),
            ),
            Self::JobFailed { flow, message, .. } => (
                flow.clone(),
                Cow::Owned(format!("The agent run failed: {message}")),
            ),
            Self::Db(_)
            | Self::Diff(_)
            | Self::DiffTask(_)
            | Self::NotPrepared
            | Self::Executor(_)
            | Self::Schema(_)
            | Self::NoResult { .. } => (
                None,
                Cow::Borrowed(
                    "The analysis stopped because of a server error. Retry the analysis.",
                ),
            ),
        };
        AnalysisState::Failed {
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
        if let Err(error) = drive(&state, analysis).await {
            let final_state = error.final_state();
            if matches!(final_state, AnalysisState::Cancelled) {
                tracing::info!(%analysis, "analysis cancelled");
            } else {
                tracing::error!(%analysis, error = ?error, "analysis pipeline failed");
            }
            // Every unfinished job of the analysis, also those submitted before a restart.
            if let Err(error) = state.executor.cancel_analysis(analysis).await {
                tracing::error!(%analysis, error = ?error, "could not cancel the jobs of the ended analysis");
            }
            let stored = state
                .progress
                .set_analysis_state(&state.db, analysis, &final_state, Utc::now())
                .await;
            if let Err(error) = stored {
                tracing::error!(%analysis, error = ?error, "could not store the failed analysis state");
            }
        }
        DRIVING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&analysis);
    });
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

/// One item of the merged job streams: an update, or `None` at the end of a job's stream.
type Updates = SelectAll<BoxStream<'static, (JobStage, Option<Result<JobUpdate, ExecutorError>>)>>;

struct Driver<'a> {
    state: &'a AppState,
    analysis: AnalysisId,
    summary: AnalysisSummary,
    files: Vec<FileDiff>,
    plan: Option<PlanOutput>,
    paused: bool,
}

async fn drive(state: &AppState, analysis: AnalysisId) -> Result<(), PipelineError> {
    let mut summary = state.db.analyses().summary(analysis).await?;
    if !matches!(
        summary.state,
        AnalysisState::Planning | AnalysisState::Running { .. } | AnalysisState::Paused { .. }
    ) {
        return Ok(());
    }
    let mirror = prepare(state, &mut summary).await?;
    let files = diff_files(mirror, &summary).await?;
    let plan = state
        .db
        .documents()
        .get(analysis, DocumentKind::Plan, "")
        .await?;
    let mut driver = Driver {
        state,
        analysis,
        summary,
        files,
        plan,
        paused: false,
    };
    driver.run_stages().await
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

impl Driver<'_> {
    async fn run_stages(&mut self) -> Result<(), PipelineError> {
        let mut updates = Updates::new();
        if self.plan.is_some() {
            self.submit_flows(&mut updates).await?;
        } else {
            self.submit(&mut updates, JobStage::Plan { batch: 1 })
                .await?;
        }
        if !self.stored(&JobStage::PrContext).await? {
            self.submit(&mut updates, JobStage::PrContext).await?;
        }
        while let Some((stage, item)) = updates.next().await {
            match item {
                Some(Ok(JobUpdate::Finished(result))) => {
                    self.finish(&mut updates, stage, result).await?;
                }
                Some(Ok(JobUpdate::Paused { resumes_at })) => {
                    self.paused = true;
                    self.set_state(AnalysisState::Paused { resumes_at }).await?;
                }
                Some(Ok(_)) if self.paused => {
                    self.paused = false;
                    self.set_progress().await?;
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(error.into()),
                None => {
                    if !self.stored(&stage).await? {
                        return Err(PipelineError::NoResult { stage: stage.key() });
                    }
                }
            }
        }
        let total = self.flow_total();
        self.set_state(AnalysisState::Finished { total }).await?;
        tracing::info!(analysis = %self.analysis, flows = total, "analysis finished");
        Ok(())
    }

    /// Submits the stage's job (idempotent per stage and generation) and follows its updates.
    async fn submit(
        &mut self,
        updates: &mut Updates,
        stage: JobStage,
    ) -> Result<(), PipelineError> {
        let kind = stage.kind();
        let pr = &self.summary.pr;
        let prompt = match &stage {
            JobStage::Plan { .. } => prompt::plan(pr, &self.files),
            JobStage::PrContext => prompt::pr_context(pr),
            JobStage::Flow(id) => {
                let planned = self
                    .plan
                    .as_ref()
                    .and_then(|plan| plan.flows.iter().find(|flow| &flow.id == id));
                match planned {
                    Some(flow) => prompt::analyse_flow(pr, flow, &self.files),
                    None => return Err(self.invalid(&stage, "the flow is not in the plan")),
                }
            }
            JobStage::Ask(_) => return Err(self.invalid(&stage, "not a pipeline stage")),
        };
        let changed_lines = self
            .files
            .iter()
            .flat_map(|file| &file.hunks)
            .map(|hunk| hunk.base.len.saturating_add(hunk.head.len))
            .fold(0u32, u32::saturating_add);
        let spec = JobSpec {
            job_id: JobId::new_random(),
            attempt: 0,
            kind,
            analysis_id: self.analysis,
            repo: RepoCheckout {
                clone_url: endpoint(
                    &self.state.config.github.clone_url,
                    &format!("{}/{}.git", pr.repo.owner, pr.repo.name),
                )
                .to_string(),
                base_sha: pr.base_sha.clone(),
                head_sha: pr.head_sha.clone(),
            },
            prompt,
            output_schema: agent::output_schema(kind)?,
            model: select_model(kind, &ModelContext { changed_lines }),
            limits: JobLimits::default(),
            credential: LlmCredentialRef {
                id: CREDENTIAL_ID.to_owned(),
                kind: CredentialKind::CodexChatgpt,
            },
            session: None,
            context_pack: None,
            skills: Vec::new(),
            access: AgentAccess::default(),
        };
        let handle = self
            .state
            .executor
            .submit(JobRequest {
                spec,
                priority: JobPriority::Batch,
                stage: stage.clone(),
                generation: GENERATION,
                pr_repo: pr.repo.clone(),
            })
            .await?;
        tracing::debug!(analysis = %self.analysis, stage = %stage.key(), job = %handle.job_id, "pipeline job submitted");
        self.follow(updates, stage, handle);
        Ok(())
    }

    fn follow(&self, updates: &mut Updates, stage: JobStage, handle: JobHandle) {
        let ended = stage.clone();
        let events = self
            .state
            .executor
            .events(&handle)
            .map(move |update| (stage.clone(), Some(update)))
            .chain(stream::once(async move { (ended, None) }));
        updates.push(events.boxed());
    }

    async fn submit_flows(&mut self, updates: &mut Updates) -> Result<(), PipelineError> {
        self.set_progress().await?;
        let ids: Vec<_> = self
            .plan
            .iter()
            .flat_map(|plan| &plan.flows)
            .map(|flow| flow.id.clone())
            .collect();
        for id in ids {
            let stage = JobStage::Flow(id);
            if !self.stored(&stage).await? {
                self.submit(updates, stage).await?;
            }
        }
        Ok(())
    }

    async fn finish(
        &mut self,
        updates: &mut Updates,
        stage: JobStage,
        result: JobResult,
    ) -> Result<(), PipelineError> {
        let output = match result.outcome {
            JobOutcome::Succeeded { output } => output,
            JobOutcome::Failed { message, .. } => {
                return Err(PipelineError::JobFailed {
                    flow: self.flow_ref(&stage),
                    stage: stage.key(),
                    message,
                });
            }
            JobOutcome::Cancelled => return Err(PipelineError::Cancelled { stage: stage.key() }),
            JobOutcome::UsageLimited { .. } => {
                return Err(PipelineError::JobFailed {
                    flow: self.flow_ref(&stage),
                    stage: stage.key(),
                    message: "the usage limit was reached".to_owned(),
                });
            }
        };
        let kind = stage.kind();
        self.check_schema(&stage, kind, &output)?;
        match &stage {
            JobStage::Plan { .. } => {
                let plan: PlanOutput = self.parse(&stage, output)?;
                self.put(DocumentKind::Plan, "", &plan).await?;
                self.plan = Some(plan);
                self.submit_flows(updates).await?;
            }
            JobStage::PrContext => {
                let context: PrContextOutput = self.parse(&stage, output)?;
                self.put(DocumentKind::PrContext, "", &context).await?;
            }
            JobStage::Flow(id) => {
                let flow: FlowAnalysisOutput = self.parse(&stage, output)?;
                if &flow.flow_id != id {
                    return Err(self.invalid(&stage, "the output names another flow"));
                }
                self.put(DocumentKind::Flow, &id.0, &flow).await?;
                self.set_progress().await?;
            }
            JobStage::Ask(_) => return Err(self.invalid(&stage, "not a pipeline stage")),
        }
        tracing::debug!(analysis = %self.analysis, stage = %stage.key(), "stage output stored");
        Ok(())
    }

    fn check_schema(
        &self,
        stage: &JobStage,
        kind: JobKind,
        output: &serde_json::Value,
    ) -> Result<(), PipelineError> {
        let schema = agent::output_schema(kind)?;
        let validator = jsonschema::validator_for(&schema)
            .map_err(|error| self.invalid(stage, &format!("schema does not compile: {error}")))?;
        match validator.iter_errors(output).next() {
            Some(error) => Err(self.invalid(stage, &error.to_string())),
            None => Ok(()),
        }
    }

    fn parse<T: serde::de::DeserializeOwned>(
        &self,
        stage: &JobStage,
        output: serde_json::Value,
    ) -> Result<T, PipelineError> {
        serde_json::from_value(output).map_err(|error| self.invalid(stage, &error.to_string()))
    }

    fn invalid(&self, stage: &JobStage, detail: &str) -> PipelineError {
        PipelineError::InvalidOutput {
            stage: stage.key(),
            detail: detail.to_owned(),
            flow: self.flow_ref(stage),
        }
    }

    fn flow_ref(&self, stage: &JobStage) -> Option<FlowRef> {
        let JobStage::Flow(id) = stage else {
            return None;
        };
        let title = self
            .plan
            .iter()
            .flat_map(|plan| &plan.flows)
            .find(|flow| &flow.id == id)
            .map(|flow| flow.title.clone())
            .unwrap_or_default();
        Some(FlowRef {
            id: id.clone(),
            title,
        })
    }

    async fn put<T: Serialize + Sync>(
        &self,
        kind: DocumentKind,
        key: &str,
        body: &T,
    ) -> Result<(), DbError> {
        self.state
            .db
            .documents()
            .put(self.analysis, kind, key, body, Utc::now())
            .await
    }

    async fn stored(&self, stage: &JobStage) -> Result<bool, DbError> {
        let (kind, key) = match stage {
            JobStage::Plan { .. } => (DocumentKind::Plan, ""),
            JobStage::PrContext => (DocumentKind::PrContext, ""),
            JobStage::Flow(id) => (DocumentKind::Flow, id.0.as_str()),
            JobStage::Ask(_) => return Ok(false),
        };
        let document: Option<serde_json::Value> = self
            .state
            .db
            .documents()
            .get(self.analysis, kind, key)
            .await?;
        Ok(document.is_some())
    }

    fn flow_total(&self) -> u32 {
        self.plan.as_ref().map_or(0, |plan| {
            u32::try_from(plan.flows.len()).unwrap_or(u32::MAX)
        })
    }

    /// `running {done, total}` from the stored flow documents (`planning` until the plan is stored).
    async fn set_progress(&self) -> Result<(), DbError> {
        if self.plan.is_none() {
            return self.set_state(AnalysisState::Planning).await;
        }
        let done = self
            .state
            .db
            .documents()
            .list::<serde_json::Value>(self.analysis, DocumentKind::Flow)
            .await?
            .len();
        self.set_state(AnalysisState::Running {
            done: u32::try_from(done).unwrap_or(u32::MAX),
            total: self.flow_total(),
        })
        .await
    }

    async fn set_state(&self, state: AnalysisState) -> Result<(), DbError> {
        self.state
            .progress
            .set_analysis_state(&self.state.db, self.analysis, &state, Utc::now())
            .await
            .map(|_| ())
    }
}
