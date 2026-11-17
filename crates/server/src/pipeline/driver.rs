//! The driver of one analysis: stages, generations (repair runs), progress and the end of the analysis.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use analysis::diff::FileDiff;
use analysis::lines::changed_line_count;
use analysis::prepass::batches::{merge_plans, plan_batches};
use analysis::verify::{VerifiedPlan, VerifyIssue};
use chrono::{DateTime, Utc};
use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::agent::plan::{PlanOutput, PlannedFlow, UnassignedHunk};
use contracts::agent::pr_context::PrContextOutput;
use contracts::agent::{self as agent, JobKind};
use contracts::api::analyses::{AnalysisState, AnalysisSummary, FlowRef};
use contracts::ids::{AnalysisId, FlowId, JobId};
use contracts::job::access::AgentAccess;
use contracts::job::result::{JobOutcome, JobResult};
use contracts::job::spec::{
    CredentialKind, JobLimits, JobPriority, JobSpec, LlmCredentialRef, RepoCheckout,
};
use db::error::DbError;
use db::repo::documents::DocumentKind;
use db::repo::jobs::{JobStage, MAX_REPAIR_RUNS};
use db::repo::settings::CallFailure;
use futures::StreamExt;
use futures::stream::{self, BoxStream, SelectAll};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::batching;
use super::context::StoredResults;
use super::verdict::{self, Facts, Outputs};
use super::{CREDENTIAL_ID, PipelineError, control, diff_files, prepare, prepass};
use crate::github::oauth::endpoint;
use crate::jobs::executor::{ExecutorError, JobHandle, JobRequest, JobUpdate};
use crate::memcan::calibration::{CalibrationDirection, CalibrationExample, calibration_examples};
use crate::model::{ModelContext, select_model};
use crate::prompts::{
    self, FlowInput, JobContext, PlanBatch, PlanInput, PrContextInput, Profile, PromptError,
    PromptThread, RepairInput, RepairOf,
};
use crate::review::{self, GithubSnapshot};
use crate::state::AppState;

/// One item of the merged job streams: an update, or `None` at the end of a job's stream.
type Updates =
    SelectAll<BoxStream<'static, (JobStage, u32, Option<Result<JobUpdate, ExecutorError>>)>>;

/// How a planned flow ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FlowEnd {
    /// Its output passed verification and is stored.
    Accepted,
    /// No accepted output after every repair run: its lines are Unexplained.
    Unexplained,
}

/// A repair run's input: the rejected output and why.
struct Repair<'a> {
    issues: &'a [VerifyIssue],
    rejected: &'a Value,
    attempt: u32,
}

struct Driver<'a> {
    state: &'a AppState,
    analysis: AnalysisId,
    summary: AnalysisSummary,
    facts: Arc<Facts>,
    snapshot: GithubSnapshot,
    /// The agent hunks per plan batch (one batch unless the pull request is huge).
    batches: Vec<Vec<FileDiff>>,
    /// Agent hunks of files too large for any plan prompt: added to the plan as unassigned (Unexplained).
    unfit: Vec<UnassignedHunk>,
    batch_plans: BTreeMap<u32, PlanOutput>,
    verified_plan: Option<Arc<VerifiedPlan>>,
    /// Stored results: the plan, the PR context and the accepted flows (context packs).
    results: StoredResults,
    ended: BTreeMap<FlowId, FlowEnd>,
    /// The generation each submitted stage is at.
    live: HashMap<JobStage, u32>,
    /// Stages not submitted yet, in order: plan batches, PR context, then the flows in plan order.
    waiting: VecDeque<JobStage>,
    /// The one stage whose job runs now: the next is submitted only once its result is verified and stored (or
    /// the stage ended), so every job's context pack holds the results of all earlier stages.
    in_flight: Option<JobStage>,
    /// MemCan severity calibration examples per flow; `None` = not checked (MemCan unreachable).
    calibration: HashMap<FlowId, Option<Vec<CalibrationExample>>>,
    /// MemCan failed once in this run: later flows do not wait for it again.
    memcan_down: bool,
    /// Stages whose job waits for the usage limit to reset, with when it is queued again if known.
    paused: BTreeMap<String, Option<DateTime<Utc>>>,
}

/// Runs the analysis to its end; `Ok` once it is finished (or was not live).
pub(super) async fn drive(state: &AppState, analysis: AnalysisId) -> Result<(), PipelineError> {
    let mut summary = state.db.analyses().summary(analysis).await?;
    if !control::is_live(&summary.state) {
        return Ok(());
    }
    let mirror = prepare(state, &mut summary).await?;
    let files = diff_files(mirror.clone(), &summary).await?;
    let snapshot = snapshot(state, &summary).await?;
    let prepass = prepass::ensure(state, &summary, &mirror, &files).await?;
    let listed = prepass.agent_files(&files);
    let fitted = {
        let job = JobContext {
            pr: &summary.pr,
            language: summary.language,
            sub_agents: prompts::SUB_AGENTS_DEFAULT,
            prepass: &prepass,
        };
        let profile = Profile {
            file: prepass.profile_file.as_deref(),
            memcan: None,
        };
        // The largest batch numbers bound the rendered size of every batch's prompt.
        let largest = PlanBatch {
            number: u32::MAX,
            total: u32::MAX,
        };
        batching::fit(plan_batches(&listed), &|files| {
            prompts::plan(&PlanInput {
                job,
                files,
                profile,
                batch: Some(largest),
            })
        })?
    };
    let results = StoredResults::load(&state.db, analysis).await?;
    let facts = Arc::new(Facts {
        mirror,
        base: summary.pr.base_sha.clone(),
        head: summary.pr.head_sha.clone(),
        files,
        listed,
        prepass,
    });
    let mut driver = Driver {
        state,
        analysis,
        summary,
        facts,
        snapshot,
        batches: fitted.batches,
        unfit: fitted.unfit,
        batch_plans: BTreeMap::new(),
        verified_plan: None,
        results,
        ended: BTreeMap::new(),
        live: HashMap::new(),
        waiting: VecDeque::new(),
        in_flight: None,
        calibration: HashMap::new(),
        memcan_down: false,
        paused: BTreeMap::new(),
    };
    driver.run().await
}

/// The stored GitHub snapshot, read and stored first when missing (with the analysis creator's token).
async fn snapshot(
    state: &AppState,
    summary: &AnalysisSummary,
) -> Result<GithubSnapshot, PipelineError> {
    let stored: Option<GithubSnapshot> = state
        .db
        .documents()
        .get(summary.id, DocumentKind::GithubSnapshot, "")
        .await?;
    if let Some(snapshot) = stored {
        return Ok(snapshot);
    }
    let creator = state.db.analyses().get(summary.id).await?.created_by;
    Ok(review::ingest_github_snapshot(state, summary, &creator).await?)
}

impl Driver<'_> {
    async fn run(&mut self) -> Result<(), PipelineError> {
        let mut updates = Updates::new();
        self.set_progress().await?;
        let plan_stored = self.results.plan.is_some();
        if !plan_stored {
            for batch in self.batch_numbers() {
                let key = batch.to_string();
                let stored: Option<PlanOutput> = self.get(DocumentKind::Plan, &key).await?;
                match stored {
                    Some(plan) if self.batches.len() > 1 => {
                        self.batch_plans.insert(batch, plan);
                    }
                    _ => self.waiting.push_back(JobStage::Plan { batch }),
                }
            }
        }
        if self.results.pr_context.is_none() {
            self.waiting.push_back(JobStage::PrContext);
        }
        if let Some(plan) = self.results.plan.clone() {
            self.plan_known(plan, false).await?;
        } else if self.batches.is_empty() {
            // Nothing for the agent to plan: every change is quiet material or fits no prompt.
            let empty = PlanOutput {
                flows: Vec::new(),
                unassigned: Vec::new(),
            };
            let plan = self.with_unfit(empty);
            self.plan_known(plan, true).await?;
        } else {
            self.merge_when_planned().await?;
        }
        self.advance(&mut updates).await?;
        while let Some((stage, generation, item)) = updates.next().await {
            // Any update after a pause means the job runs (or ended) again.
            if !matches!(item, Some(Ok(JobUpdate::Paused { .. })))
                && self.paused.remove(&stage.key()).is_some()
            {
                tracing::info!(analysis = %self.analysis, stage = %stage.key(), "job resumed after the usage limit");
                self.set_progress().await?;
            }
            match item {
                Some(Ok(JobUpdate::Finished(result))) => {
                    self.finish(&mut updates, stage, generation, result).await?;
                    if self
                        .in_flight
                        .as_ref()
                        .is_some_and(|live| self.resolved(live))
                    {
                        self.in_flight = None;
                    }
                    self.advance(&mut updates).await?;
                }
                Some(Ok(JobUpdate::Paused { resumes_at })) => {
                    tracing::info!(analysis = %self.analysis, stage = %stage.key(), ?resumes_at,
                        "job paused by the usage limit");
                    self.paused.insert(stage.key(), resumes_at);
                    self.set_progress().await?;
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(error.into()),
                None if self.live.get(&stage) == Some(&generation) && !self.resolved(&stage) => {
                    return Err(PipelineError::NoResult { stage: stage.key() });
                }
                None => {}
            }
        }
        self.finish_analysis().await
    }

    /// Submits the next waiting stage once no job runs; stages resolved meanwhile are skipped.
    async fn advance(&mut self, updates: &mut Updates) -> Result<(), PipelineError> {
        while self.in_flight.is_none() {
            let Some(stage) = self.waiting.pop_front() else {
                return Ok(());
            };
            if !self.resolved(&stage) {
                self.submit_first(updates, stage).await?;
            }
        }
        Ok(())
    }

    fn batch_numbers(&self) -> Vec<u32> {
        (1u32..).take(self.batches.len()).collect()
    }

    /// Whether a stage has its result (a stored document, or a flow that ended Unexplained).
    fn resolved(&self, stage: &JobStage) -> bool {
        match stage {
            JobStage::Plan { batch } => {
                self.results.plan.is_some() || self.batch_plans.contains_key(batch)
            }
            JobStage::PrContext => self.results.pr_context.is_some(),
            JobStage::Flow(id) => self.ended.contains_key(id),
            JobStage::Ask(_) => true,
        }
    }

    /// Final verification over every stored output (wording warnings logged, incomplete move detection
    /// recorded), then `finished`.
    async fn finish_analysis(&mut self) -> Result<(), PipelineError> {
        let (Some(plan), Some(pr_context)) =
            (self.results.plan.clone(), self.results.pr_context.clone())
        else {
            return Err(PipelineError::NoResult {
                stage: "plan or pr_context".to_owned(),
            });
        };
        let outputs = Arc::new(Outputs {
            plan,
            pr_context,
            flows: self.results.flows.clone(),
        });
        let aliases = self.aliases(&outputs.plan);
        let verdict = verdict::analysis(Arc::clone(&self.facts), outputs, aliases).await?;
        for (output, warning) in &verdict.warnings {
            tracing::warn!(analysis = %self.analysis, ?output, phrase = %warning.phrase,
                location = %warning.location, "LLM text uses wording the trust rules ban");
        }
        if verdict.moves_incomplete {
            tracing::warn!(analysis = %self.analysis,
                "move detection ran out of its work budget; some moved blocks have no move fact");
        }
        if !verdict.repairs.is_empty() {
            tracing::error!(analysis = %self.analysis, repairs = verdict.repairs.len(),
                "stored outputs fail the whole-analysis check; their flows are Unexplained in the Tour");
        }
        let unexplained = self
            .ended
            .values()
            .filter(|end| **end == FlowEnd::Unexplained)
            .count();
        let total = self.flow_total();
        self.set_state(AnalysisState::Finished { total }).await?;
        tracing::info!(analysis = %self.analysis, flows = total, unexplained, "analysis finished");
        Ok(())
    }

    /// The plan is known (stored, merged or empty): verify it against the agent hunks, then queue the flows
    /// without a stored analysis, in plan order.
    async fn plan_known(&mut self, plan: PlanOutput, store: bool) -> Result<(), PipelineError> {
        let verified = match verdict::plan(&plan, &self.facts.listed)? {
            Ok(verified) => verified,
            Err(issues) => {
                tracing::error!(analysis = %self.analysis, issues = issues.len(),
                    "the merged plan fails verification against every agent hunk");
                return Err(PipelineError::RepairExhausted {
                    stage: "plan".to_owned(),
                    what: "plan of the flows",
                });
            }
        };
        if store {
            self.put(DocumentKind::Plan, "", &plan).await?;
        }
        self.verified_plan = Some(Arc::new(verified));
        let stored: Vec<FlowId> = self
            .results
            .flows
            .iter()
            .map(|flow| flow.flow_id.clone())
            .collect();
        for id in stored {
            self.ended.insert(id, FlowEnd::Accepted);
        }
        let ids: Vec<FlowId> = plan.flows.iter().map(|flow| flow.id.clone()).collect();
        self.results.plan = Some(plan);
        self.set_progress().await?;
        for id in ids {
            if !self.ended.contains_key(&id) {
                self.waiting.push_back(JobStage::Flow(id));
            }
        }
        Ok(())
    }

    /// Merges the batch plans once every batch is planned.
    async fn merge_when_planned(&mut self) -> Result<(), PipelineError> {
        if self.batches.len() < 2 || self.batch_plans.len() < self.batches.len() {
            return Ok(());
        }
        let plans: Vec<PlanOutput> = self.batch_plans.values().cloned().collect();
        let merged = self.with_unfit(merge_plans(&plans));
        tracing::info!(analysis = %self.analysis, batches = plans.len(), flows = merged.flows.len(),
            "batch plans merged");
        self.plan_known(merged, true).await
    }

    /// Handles a job's result: schema, verification, then storage, a repair run or the stage's end.
    async fn finish(
        &mut self,
        updates: &mut Updates,
        stage: JobStage,
        generation: u32,
        result: JobResult,
    ) -> Result<(), PipelineError> {
        let output = match result.outcome {
            JobOutcome::Succeeded { output } => output,
            JobOutcome::Failed { kind, message } => {
                return Err(PipelineError::JobFailed {
                    flow: self.flow_ref(&stage),
                    stage: stage.key(),
                    failure: CallFailure::Failed(kind),
                    detail: message,
                });
            }
            JobOutcome::Cancelled => return Err(PipelineError::Cancelled { stage: stage.key() }),
            JobOutcome::UsageLimited { .. } => {
                return Err(PipelineError::JobFailed {
                    flow: self.flow_ref(&stage),
                    stage: stage.key(),
                    failure: CallFailure::UsageLimited,
                    detail: "the usage limit was reached".to_owned(),
                });
            }
        };
        self.check_schema(&stage, stage.kind(), &output)?;
        let issues = match &stage {
            JobStage::Plan { batch } => {
                let plan: PlanOutput = self.parse(&stage, output.clone())?;
                let files = self.batch_files(*batch);
                match verdict::plan(&plan, &files)? {
                    Ok(_) => {
                        self.plan_accepted(*batch, plan).await?;
                        None
                    }
                    Err(issues) => Some(issues),
                }
            }
            JobStage::PrContext => {
                let context: PrContextOutput = self.parse(&stage, output.clone())?;
                match verdict::pr_context(Arc::clone(&self.facts), context.clone()).await? {
                    Ok(()) => {
                        self.put(DocumentKind::PrContext, "", &context).await?;
                        self.results.pr_context = Some(context);
                        None
                    }
                    Err(issues) => Some(issues),
                }
            }
            JobStage::Flow(id) => {
                let flow: FlowAnalysisOutput = self.parse(&stage, output.clone())?;
                let (Some(plan), Some(planned)) = (self.verified_plan.clone(), self.planned(id))
                else {
                    return Err(self.invalid(&stage, "the flow is not in the plan"));
                };
                let aliases = self.aliases(&plan.output).remove(id).unwrap_or_default();
                match verdict::flow(
                    Arc::clone(&self.facts),
                    plan,
                    planned,
                    flow.clone(),
                    aliases,
                )
                .await?
                {
                    Ok(_) => {
                        self.put(DocumentKind::Flow, &id.0, &flow).await?;
                        self.results.flows.push(flow);
                        self.ended.insert(id.clone(), FlowEnd::Accepted);
                        self.set_progress().await?;
                        None
                    }
                    Err(issues) => Some(issues),
                }
            }
            JobStage::Ask(_) => return Err(self.invalid(&stage, "not a pipeline stage")),
        };
        let Some(issues) = issues else {
            tracing::debug!(analysis = %self.analysis, stage = %stage.key(), generation, "stage output stored");
            return Ok(());
        };
        let codes: Vec<&str> = issues.iter().map(|issue| issue.kind.code()).collect();
        if generation < MAX_REPAIR_RUNS {
            tracing::info!(analysis = %self.analysis, stage = %stage.key(), generation, issues = ?codes,
                "agent output rejected by verification; submitting a repair run");
            let repair = Repair {
                issues: &issues,
                rejected: &output,
                attempt: generation + 1,
            };
            match self.prompt(&stage, Some(&repair)) {
                Ok(prompt) => return self.submit(updates, stage, generation + 1, prompt).await,
                // A rejected output too large to quote cannot be repaired: the stage ends as if exhausted.
                Err(PipelineError::Prompt(PromptError::OverBudget {
                    kind: "repair",
                    bytes,
                    ..
                })) => {
                    tracing::warn!(analysis = %self.analysis, stage = %stage.key(), bytes,
                        "rejected agent output too large for a repair run");
                }
                Err(error) => return Err(error),
            }
        } else {
            tracing::warn!(analysis = %self.analysis, stage = %stage.key(), issues = ?codes,
                "agent output still rejected after every repair run");
        }
        match stage {
            JobStage::Flow(id) => {
                self.ended.insert(id, FlowEnd::Unexplained);
                self.set_progress().await?;
                Ok(())
            }
            JobStage::Plan { .. } => Err(PipelineError::RepairExhausted {
                stage: stage.key(),
                what: "plan of the flows",
            }),
            JobStage::PrContext | JobStage::Ask(_) => Err(PipelineError::RepairExhausted {
                stage: stage.key(),
                what: "PR context",
            }),
        }
    }

    /// Stores an accepted batch plan; with one batch it is the plan.
    async fn plan_accepted(&mut self, batch: u32, plan: PlanOutput) -> Result<(), PipelineError> {
        if self.batches.len() < 2 {
            let plan = self.with_unfit(plan);
            return self.plan_known(plan, true).await;
        }
        self.put(DocumentKind::Plan, &batch.to_string(), &plan)
            .await?;
        self.batch_plans.insert(batch, plan);
        self.merge_when_planned().await
    }

    /// `plan` with the hunks no plan prompt could hold, listed as unassigned with their reason.
    fn with_unfit(&self, mut plan: PlanOutput) -> PlanOutput {
        plan.unassigned.extend(self.unfit.iter().cloned());
        plan
    }

    fn batch_files(&self, batch: u32) -> Vec<FileDiff> {
        usize::try_from(batch)
            .ok()
            .and_then(|batch| batch.checked_sub(1))
            .and_then(|index| self.batches.get(index))
            .cloned()
            .unwrap_or_default()
    }

    fn planned(&self, id: &FlowId) -> Option<PlannedFlow> {
        self.results
            .plan
            .as_ref()?
            .flows
            .iter()
            .find(|flow| &flow.id == id)
            .cloned()
    }

    fn aliases(&self, plan: &PlanOutput) -> BTreeMap<FlowId, Vec<String>> {
        verdict::alias_map(&verdict::flow_threads(
            &self.snapshot.threads,
            plan,
            &self.facts.listed,
        ))
    }

    /// The stage's prompt: its task, or a repair run of it.
    fn prompt(
        &self,
        stage: &JobStage,
        repair: Option<&Repair<'_>>,
    ) -> Result<String, PipelineError> {
        let prepass = &self.facts.prepass;
        let job = JobContext {
            pr: &self.summary.pr,
            language: self.summary.language,
            sub_agents: prompts::SUB_AGENTS_DEFAULT,
            prepass,
        };
        // TODO(WP3-MEMCAN merge): the MemCan profile memory; `server::memcan` has no profile read yet.
        let profile = Profile {
            file: prepass.profile_file.as_deref(),
            memcan: None,
        };
        let finish = |of: RepairOf<'_>, task: &dyn Fn() -> Result<String, prompts::PromptError>| {
            match repair {
                Some(repair) => prompts::repair(&RepairInput {
                    of,
                    attempt: repair.attempt,
                    issues: repair.issues,
                    rejected: repair.rejected,
                }),
                None => task(),
            }
        };
        let prompt = match stage {
            JobStage::Plan { batch } => {
                let files = self.batch_files(*batch);
                let input = PlanInput {
                    job,
                    files: &files,
                    profile,
                    batch: (self.batches.len() > 1).then(|| PlanBatch {
                        number: *batch,
                        total: u32::try_from(self.batches.len()).unwrap_or(u32::MAX),
                    }),
                };
                finish(RepairOf::Plan(&input), &|| prompts::plan(&input))?
            }
            JobStage::PrContext => {
                let input = PrContextInput {
                    job,
                    description: self.snapshot.description.as_deref(),
                    profile,
                };
                finish(RepairOf::PrContext(&input), &|| prompts::pr_context(&input))?
            }
            JobStage::Flow(id) => {
                let Some(plan) = self.results.plan.as_ref() else {
                    return Err(self.invalid(stage, "the flow is not in the plan"));
                };
                let Some(flow) = plan.flows.iter().find(|flow| &flow.id == id) else {
                    return Err(self.invalid(stage, "the flow is not in the plan"));
                };
                let listed =
                    verdict::flow_threads(&self.snapshot.threads, plan, &self.facts.listed);
                let threads: Vec<PromptThread<'_>> = listed
                    .get(id)
                    .map(|threads| {
                        threads
                            .iter()
                            .map(|(alias, thread)| PromptThread { alias, thread })
                            .collect()
                    })
                    .unwrap_or_default();
                let calibration: Option<Vec<prompts::Calibration<'_>>> = self
                    .calibration
                    .get(id)
                    .and_then(Option::as_ref)
                    .map(|examples| examples.iter().map(prompt_calibration).collect());
                let input = FlowInput {
                    job,
                    files: &self.facts.files,
                    flow,
                    profile,
                    threads: &threads,
                    calibration: calibration.as_deref(),
                };
                finish(RepairOf::Flow(&input), &|| prompts::analyse_flow(&input))?
            }
            JobStage::Ask(_) => return Err(self.invalid(stage, "not a pipeline stage")),
        };
        Ok(prompt)
    }

    /// Reads the flow's severity calibration examples from MemCan once (bounded read; an error means "not
    /// checked", never "no examples"). Their text is untrusted data, quoted as such by the prompt.
    async fn load_calibration(&mut self, id: &FlowId) {
        if self.calibration.contains_key(id) {
            return;
        }
        let Some(flow) = self.planned(id) else {
            return;
        };
        let examples = if self.memcan_down {
            None
        } else {
            let query = format!("{} {}", flow.title, flow.intent);
            let project = &self.summary.pr.repo.name;
            match calibration_examples(&self.state.config.memcan, project, &query).await {
                Ok(examples) => Some(examples),
                Err(error) => {
                    tracing::warn!(analysis = %self.analysis, flow = %id, %error,
                        "MemCan calibration examples not readable; the flow runs without them (not checked)");
                    self.memcan_down = true;
                    None
                }
            }
        };
        self.calibration.insert(id.clone(), examples);
    }

    /// Submits the stage's first run (generation 0).
    async fn submit_first(
        &mut self,
        updates: &mut Updates,
        stage: JobStage,
    ) -> Result<(), PipelineError> {
        if let JobStage::Flow(id) = &stage {
            self.load_calibration(id).await;
        }
        let prompt = self.prompt(&stage, None)?;
        self.submit(updates, stage, 0, prompt).await
    }

    /// Submits the stage's job at `generation` (idempotent per stage and generation) and follows its updates.
    /// Jobs carry a context pack of the stored results; the analysis's session is set when a runner claims it.
    async fn submit(
        &mut self,
        updates: &mut Updates,
        stage: JobStage,
        generation: u32,
        prompt: String,
    ) -> Result<(), PipelineError> {
        let kind = stage.kind();
        let pr = &self.summary.pr;
        let changed_lines = changed_line_count(&self.facts.files);
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
            // Built at claim from the results stored then (`context::StoredPacks`).
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
                generation,
                pr_repo: pr.repo.clone(),
            })
            .await?;
        tracing::debug!(analysis = %self.analysis, stage = %stage.key(), generation, job = %handle.job_id,
            "pipeline job submitted");
        self.live.insert(stage.clone(), generation);
        self.in_flight = Some(stage.clone());
        self.follow(updates, stage, generation, handle);
        // A cancel between the state check of the last write and this submit: stop and cancel the new job too.
        let current = self.state.db.analyses().summary(self.analysis).await?;
        if !control::is_live(&current.state) {
            return Err(PipelineError::Stopped);
        }
        Ok(())
    }

    fn follow(&self, updates: &mut Updates, stage: JobStage, generation: u32, handle: JobHandle) {
        let ended = stage.clone();
        let events = self
            .state
            .executor
            .events(&handle)
            .map(move |update| (stage.clone(), generation, Some(update)))
            .chain(stream::once(async move { (ended, generation, None) }));
        updates.push(events.boxed());
    }

    fn check_schema(
        &self,
        stage: &JobStage,
        kind: JobKind,
        output: &Value,
    ) -> Result<(), PipelineError> {
        let schema = agent::output_schema(kind)?;
        let validator = jsonschema::validator_for(&schema)
            .map_err(|error| self.invalid(stage, &format!("schema does not compile: {error}")))?;
        match validator.iter_errors(output).next() {
            Some(error) => Err(self.invalid(stage, &error.to_string())),
            None => Ok(()),
        }
    }

    fn parse<T: DeserializeOwned>(
        &self,
        stage: &JobStage,
        output: Value,
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
        let title = self.planned(id).map(|flow| flow.title).unwrap_or_default();
        Some(FlowRef {
            id: id.clone(),
            title,
        })
    }

    async fn get<T: DeserializeOwned>(
        &self,
        kind: DocumentKind,
        key: &str,
    ) -> Result<Option<T>, DbError> {
        self.state
            .db
            .documents()
            .get(self.analysis, kind, key)
            .await
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

    fn flow_total(&self) -> u32 {
        self.results.plan.as_ref().map_or(0, |plan| {
            u32::try_from(plan.flows.len()).unwrap_or(u32::MAX)
        })
    }

    /// Flows ended (accepted or Unexplained) and planned; total `None` before the plan.
    fn progress(&self) -> (u32, Option<u32>) {
        let done = u32::try_from(self.ended.len()).unwrap_or(u32::MAX);
        (done, self.results.plan.is_some().then(|| self.flow_total()))
    }

    /// `paused` while a job waits for the usage limit, else `running {done, total}` (`planning` until the plan
    /// is known).
    async fn set_progress(&self) -> Result<(), PipelineError> {
        let state = match (self.progress(), self.paused.is_empty()) {
            ((done, total), false) => AnalysisState::Paused {
                resumes_at: self.paused.values().flatten().max().copied(),
                done,
                total,
            },
            ((done, Some(total)), true) => AnalysisState::Running { done, total },
            ((_, None), true) => AnalysisState::Planning,
        };
        self.set_state(state).await
    }

    /// Stores a state while the analysis is live; a cancelled analysis stops the driver instead.
    async fn set_state(&self, state: AnalysisState) -> Result<(), PipelineError> {
        control::set_state_if(self.state, self.analysis, &state, control::is_live)
            .await?
            .map(|_| ())
            .ok_or(PipelineError::Stopped)
    }
}

/// A MemCan calibration example as the prompt quotes it.
fn prompt_calibration(example: &CalibrationExample) -> prompts::Calibration<'_> {
    prompts::Calibration {
        direction: match example.direction {
            CalibrationDirection::Overrated => prompts::CalibrationDirection::Overrated,
            CalibrationDirection::Underrated => prompts::CalibrationDirection::Underrated,
        },
        text: &example.text,
    }
}
