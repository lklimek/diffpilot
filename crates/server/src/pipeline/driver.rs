//! The driver of one analysis: stages, generations (repair runs), progress and the end of the analysis.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use analysis::diff::FileDiff;
use analysis::lines::changed_line_count;
use analysis::prepass::batches::{merge_plans, plan_batches};
use analysis::verify::{VerifiedPlan, VerifyIssue, VerifyIssueKind};
use chrono::{DateTime, Utc};
use contracts::agent::plan::{PlanOutput, PlannedFlow, UnassignedHunk};
use contracts::agent::pr_context::PrContextOutput;
use contracts::agent::{self as agent, JobKind};
use contracts::api::analyses::{AnalysisState, AnalysisSummary, FlowRef};
use contracts::api::tour::UnexplainedReason;
use contracts::common::FileRanges;
use contracts::ids::{AnalysisId, FlowId, JobId};
use contracts::job::access::AgentAccess;
use contracts::job::result::{JobFailureKind, JobOutcome, JobResult};
use contracts::job::spec::{
    CredentialKind, JobLimits, JobPriority, JobSpec, LlmCredentialRef, RepoCheckout,
};
use db::error::DbError;
use db::repo::documents::DocumentKind;
use db::repo::flow_work::{NextFlow, RETRY_BY_USER};
use db::repo::jobs::{JobStage, MAX_REPAIR_RUNS};
use db::repo::settings::CallFailure;
use futures::StreamExt;
use futures::stream::{self, BoxStream, SelectAll};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::batching;
use super::context::StoredResults;
use super::unsent::{self, Unsent};
use super::verdict::{self, Facts, Outputs};
use super::{
    CREDENTIAL_ID, DriverEnd, PipelineError, control, diff_files, prepare, prepass, statistics,
};
use crate::api::settings::plain_failure;
use crate::github::oauth::endpoint;
use crate::jobs::executor::{ExecutorError, JobHandle, JobRequest, JobUpdate};
use crate::memcan::calibration::{CalibrationDirection, CalibrationExample, calibration_examples};
use crate::memcan::profile::{self, ProfileLookup};
use crate::model::{ModelContext, select_model};
use crate::prompts::{
    self, FlowInput, JobContext, PlanBatch, PlanInput, PrContextInput, Profile, PromptError,
    PromptThread, RepairInput, RepairOf,
};
use crate::review::{self, GithubSnapshot};
use crate::state::AppState;

/// One item of the merged streams of a stage at a generation.
#[expect(
    clippy::large_enum_variant,
    reason = "almost every item is a job update; a rare timer item does not justify boxing each update"
)]
enum Item {
    /// An update of the stage's job, or `None` at the end of its stream.
    Job(Option<Result<JobUpdate, ExecutorError>>),
    /// The time a pause of the stage's job was reported to end has come.
    PauseOver(DateTime<Utc>),
}

/// The merged job streams and pause timers.
type Updates = SelectAll<BoxStream<'static, (JobStage, u32, Item)>>;

/// How a planned flow ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FlowEnd {
    /// Its output passed verification and is stored.
    Accepted,
    /// No accepted output; the flow remains explicitly failed until requested again.
    Failed,
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
    /// Stages to submit before any flow is picked by demand: plan batches, then PR context, then flows whose
    /// succeeded job a restart left without a stored result.
    waiting: VecDeque<JobStage>,
    /// The last look for flow work found none admitted: the driver ends until work is admitted again.
    idle: bool,
    /// The one stage whose job runs now: the next is submitted only once its result is verified and stored (or
    /// the stage ended), so every job's context pack holds the results of all earlier stages.
    in_flight: Option<JobStage>,
    /// The analysis's MemCan project profile lookup (made once, stored).
    memcan_profile: ProfileLookup,
    /// MemCan severity calibration examples per flow; `None` = not checked (MemCan unreachable).
    calibration: HashMap<FlowId, Option<Vec<CalibrationExample>>>,
    /// MemCan failed once in this run (the profile lookup or a calibration read): later reads do not wait for it.
    memcan_down: bool,
    /// Stages whose job waits for the usage limit to reset, with when it is queued again if known.
    paused: BTreeMap<String, Option<DateTime<Utc>>>,
}

/// Runs the analysis until it is finished (or was not live) or no flow work is admitted.
pub(super) async fn drive(
    state: &AppState,
    analysis: AnalysisId,
) -> Result<DriverEnd, PipelineError> {
    let mut summary = state.db.analyses().summary(analysis).await?;
    if !control::is_live(&summary.state) {
        return Ok(DriverEnd::Finished);
    }
    control::wait_for_resume(state, analysis).await?;
    let mirror = prepare(state, &mut summary).await?;
    let files = diff_files(mirror.clone(), &summary).await?;
    let snapshot = snapshot(state, &summary).await?;
    let prepass = prepass::ensure(state, &summary, &mirror, &files).await?;
    let profile::Ensured {
        lookup: memcan_profile,
        memcan_failed,
    } = profile::ensure(&state.db, &state.config.memcan, &summary).await?;
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
            memcan: memcan_profile.text(),
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
    state.statistics.planned(
        analysis,
        u64::from(changed_line_count(&files)),
        files.len() as u64,
        listed.len() as u64,
        fitted.batches.len() as u64,
    );
    tracing::info!(%analysis, plan_batches = fitted.batches.len(), agent_files = listed.len(),
        changed_lines = changed_line_count(&files), "analysis work planned");
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
        idle: false,
        in_flight: None,
        memcan_profile,
        calibration: HashMap::new(),
        memcan_down: memcan_failed,
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
    async fn run(&mut self) -> Result<DriverEnd, PipelineError> {
        let mut events = self.state.progress.subscribe(self.analysis);
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
        // Results a restart interrupted before they were stored are replayed whatever work is admitted now.
        if self.results.plan.is_some() && self.results.pr_context.is_some() {
            let unstored = self.state.db.flow_work().unstored(self.analysis).await?;
            self.waiting
                .extend(unstored.into_iter().map(JobStage::Flow));
        }
        self.advance(&mut updates).await?;
        loop {
            self.advance(&mut updates).await?;
            if self.in_flight.is_none() {
                if self.results.pr_context.is_some()
                    && self.results.flows.len() == self.flow_total() as usize
                {
                    break;
                }
                if self.idle {
                    return self.park().await;
                }
                let _ = events.recv().await;
                control::wait_for_resume(self.state, self.analysis).await?;
                continue;
            }
            let Some((stage, generation, item)) = updates.next().await else {
                return Err(PipelineError::NoResult {
                    stage: "flow".into(),
                });
            };
            let item = match item {
                Item::Job(item) => item,
                // The limit reset: the job waits for a runner again (unless a later pause was reported since).
                Item::PauseOver(at) => {
                    if self.paused.get(&stage.key()) == Some(&Some(at)) {
                        self.paused.remove(&stage.key());
                        tracing::info!(analysis = %self.analysis, stage = %stage.key(),
                            "usage limit reset; the job waits for a runner");
                        self.set_progress().await?;
                    }
                    continue;
                }
            };
            // Any update after a pause means the job runs (or ended) again.
            if !matches!(item, Some(Ok(JobUpdate::Paused { .. })))
                && self.paused.remove(&stage.key()).is_some()
            {
                tracing::info!(analysis = %self.analysis, stage = %stage.key(), "job resumed after the usage limit");
                self.set_progress().await?;
            }
            match item {
                Some(Ok(JobUpdate::Finished {
                    result,
                    github_access,
                })) => {
                    self.finish(&mut updates, (stage, generation), result, github_access)
                        .await?;
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
                    if let Some(at) = resumes_at {
                        updates.push(pause_timer(stage, generation, at));
                    }
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
        self.finish_analysis().await?;
        Ok(DriverEnd::Finished)
    }

    /// Submits the next waiting stage once no job runs; stages resolved meanwhile are skipped.
    async fn advance(&mut self, updates: &mut Updates) -> Result<(), PipelineError> {
        self.idle = false;
        while self.in_flight.is_none() {
            control::wait_for_resume(self.state, self.analysis).await?;
            let stage = if let Some(stage) = self.waiting.pop_front() {
                stage
            } else if self.results.pr_context.is_some() {
                match self.state.db.flow_work().next(self.analysis).await? {
                    NextFlow::Run(id) => {
                        self.ended.remove(&id);
                        JobStage::Flow(id)
                    }
                    next => {
                        self.idle = next == NextFlow::Idle;
                        return Ok(());
                    }
                }
            } else {
                return Ok(());
            };
            if self.resolved(&stage) {
                continue;
            }
            match (self.submit_first(updates, stage.clone()).await, &stage) {
                // Sent now (a Retry may fit what did not fit before): no stale reason stays behind.
                (Ok(()), JobStage::Flow(id)) => {
                    self.state
                        .db
                        .documents()
                        .delete(self.analysis, DocumentKind::Unsent, &id.0)
                        .await?;
                }
                (Ok(()), _) => {}
                // A flow whose prompt is over budget is not sent: it stays failed with that reason.
                (
                    Err(PipelineError::Prompt(PromptError::OverBudget {
                        kind: "analyse_flow",
                        bytes,
                        ..
                    })),
                    JobStage::Flow(id),
                ) => {
                    tracing::warn!(analysis = %self.analysis, flow = %id, bytes,
                        "the flow's prompt is over budget; it stays failed");
                    let lines = self
                        .verified_plan
                        .as_ref()
                        .and_then(|plan| plan.flow_lines.get(id).cloned())
                        .unwrap_or_default();
                    self.store_unsent(&id.0, lines).await?;
                    self.state
                        .db
                        .flow_work()
                        .fail(
                            self.analysis,
                            id,
                            "This flow is too large for the agent request.",
                        )
                        .await?;
                    self.ended.insert(id.clone(), FlowEnd::Failed);
                    self.set_progress().await?;
                }
                (Err(PipelineError::Executor(ExecutorError::Superseded)), _) => continue,
                (Err(error), _) => return Err(error),
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

    /// Ends the driver while flows remain but none is admitted (failed for good, or not requested with background
    /// analysis off): the analysis stays `running` without a stage, and a flow request, background analysis
    /// switched on or a restart with admitted work starts a driver again.
    async fn park(&self) -> Result<DriverEnd, PipelineError> {
        // `idle` can change without a state write (a repair refused after the demand changed): open pages hear it.
        self.state
            .progress
            .publish_analysis(&self.state.db, self.analysis)
            .await?;
        let (done, total) = self.progress();
        tracing::info!(analysis = %self.analysis, done, total,
            "no flow work is admitted; the analysis waits for a flow request or background analysis");
        let summary = self.state.db.analyses().summary(self.analysis).await?;
        if let Err(error) = statistics::log_parked(self.state, &summary).await {
            tracing::warn!(analysis = %self.analysis, ?error,
                "analysis statistics unavailable when the flow work stopped");
        }
        Ok(DriverEnd::Parked)
    }

    /// Final verification over every stored output (wording warnings logged, incomplete move detection
    /// recorded), then `finished`.
    async fn finish_analysis(&mut self) -> Result<(), PipelineError> {
        control::wait_for_resume(self.state, self.analysis).await?;
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
        let verdict = verdict::analysis(Arc::clone(&self.facts), outputs, aliases, false).await?;
        for (output, warning) in &verdict.warnings {
            tracing::warn!(analysis = %self.analysis, ?output, phrase = %warning.phrase,
                location = %warning.location, "LLM text uses wording the trust rules ban");
        }
        if verdict.moves_incomplete {
            tracing::warn!(analysis = %self.analysis,
                "move detection ran out of its work budget; some moved blocks have no move fact");
        }
        let coverage_only = verdict
            .repairs
            .iter()
            .flat_map(|repair| &repair.issues)
            .all(|issue| {
                matches!(
                    issue.kind,
                    VerifyIssueKind::LineUnassigned | VerifyIssueKind::ChangedLineUncovered
                )
            });
        if !verdict.repairs.is_empty() && coverage_only {
            tracing::warn!(analysis = %self.analysis, repairs = verdict.repairs.len(),
                "stored outputs leave lines uncovered that today's rules require; the Tour lists them as Unexplained");
        } else if !verdict.repairs.is_empty() {
            tracing::error!(analysis = %self.analysis, repairs = verdict.repairs.len(),
                "stored outputs fail the whole-analysis check; the Tour shows their flows as failed with their lines");
        }
        let total = self.flow_total();
        self.set_state(AnalysisState::Finished { total }).await?;
        tracing::info!(analysis = %self.analysis, flows = total, "analysis finished");
        Ok(())
    }

    /// The plan is known (stored, merged or empty): verify it and recover accepted flow results. A plan read from the
    /// database (`store == false`) was accepted earlier: lines today's rules add are Unexplained, not a rejection.
    async fn plan_known(&mut self, plan: PlanOutput, store: bool) -> Result<(), PipelineError> {
        let checked = if store {
            verdict::plan(&plan, &self.facts.listed)?
        } else {
            verdict::stored_plan(&plan, &self.facts.listed)?
        };
        let verified = match checked {
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
            // Stored before the plan: a stored plan always has its unsent hunks.
            if !self.unfit.is_empty() {
                let lines = unsent::unfit_lines(&self.facts.listed, &self.unfit)?;
                self.store_unsent(unsent::PLAN_KEY, lines).await?;
            }
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

        self.results.plan = Some(plan);
        self.set_progress().await?;
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

    /// Handles a job's result: schema, verification, then storage, a repair run or the stage's end. The PR
    /// context is stored with whether its attempt ran with GitHub access (the Tour's related-work fact).
    async fn finish(
        &mut self,
        updates: &mut Updates,
        (stage, generation): (JobStage, u32),
        result: JobResult,
        github_access: bool,
    ) -> Result<(), PipelineError> {
        let output = match result.outcome {
            JobOutcome::Succeeded { output } => output,
            JobOutcome::Failed { kind, message } => {
                // No flow retry helps a misconfigured server or an unusable credential: those end the analysis.
                if let JobStage::Flow(id) = &stage
                    && !matches!(
                        kind,
                        JobFailureKind::Configuration | JobFailureKind::CredentialUnusable
                    )
                {
                    tracing::warn!(analysis = %self.analysis, flow = %id, ?kind,
                        "flow job failed; the flow is marked failed with its cause");
                    let cause = plain_failure(Some(CallFailure::Failed(kind)));
                    self.state
                        .db
                        .flow_work()
                        .fail(
                            self.analysis,
                            id,
                            &format!("The flow analysis failed. {cause} {RETRY_BY_USER}"),
                        )
                        .await?;
                    self.ended.insert(id.clone(), FlowEnd::Failed);
                    self.set_progress().await?;
                    return Ok(());
                }
                return Err(PipelineError::JobFailed {
                    flow: self.flow_ref(&stage),
                    stage: stage.key(),
                    failure: CallFailure::Failed(kind),
                    detail: message,
                });
            }
            JobOutcome::Cancelled => {
                if matches!(stage, JobStage::Flow(_))
                    && control::is_live(&self.state.db.analyses().get(self.analysis).await?.state)
                {
                    self.in_flight = None;
                    self.live.remove(&stage);
                    return Ok(());
                }
                return Err(PipelineError::Cancelled { stage: stage.key() });
            }
            JobOutcome::UsageLimited { .. } => {
                return Err(PipelineError::JobFailed {
                    flow: self.flow_ref(&stage),
                    stage: stage.key(),
                    failure: CallFailure::UsageLimited,
                    detail: "the usage limit was reached".to_owned(),
                });
            }
        };
        let issues = if let Some(issues) = self.check_schema(&stage, stage.kind(), &output)? {
            Some(issues)
        } else {
            match &stage {
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
                            self.state
                                .db
                                .documents()
                                .put_agent_output(
                                    self.analysis,
                                    DocumentKind::PrContext,
                                    "",
                                    &context,
                                    github_access,
                                    Utc::now(),
                                )
                                .await?;
                            self.results.pr_context = Some(context);
                            self.set_progress().await?;
                            None
                        }
                        Err(issues) => Some(issues),
                    }
                }
                JobStage::Flow(id) => {
                    let context = self.flow_context(id)?;
                    match verdict::flow_file(Arc::clone(&self.facts), context, output.clone())
                        .await?
                    {
                        Ok(flow) => {
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
            }
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
                Ok(prompt) => {
                    let repaired = stage.clone();
                    return match self
                        .submit(
                            updates,
                            stage,
                            generation + 1,
                            prompt,
                            &codes,
                            Some(&output),
                        )
                        .await
                    {
                        // The demand changed while the flow ran: its repair waits for the flow's next turn, when
                        // the stored rejected output is replayed and repaired without a new first run.
                        Err(PipelineError::Executor(ExecutorError::Superseded)) => {
                            tracing::info!(analysis = %self.analysis, stage = %repaired.key(), generation,
                                "repair run postponed: other flow work is admitted first");
                            self.in_flight = None;
                            self.live.remove(&repaired);
                            Ok(())
                        }
                        other => other,
                    };
                }
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
                self.state
                    .db
                    .flow_work()
                    .fail(
                        self.analysis,
                        &id,
                        &format!(
                            "The flow output failed verification after repairs. {RETRY_BY_USER}"
                        ),
                    )
                    .await?;
                self.ended.insert(id, FlowEnd::Failed);
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

    /// Stores lines the server did not send to the LLM because their request is over budget.
    async fn store_unsent(&self, key: &str, lines: Vec<FileRanges>) -> Result<(), PipelineError> {
        let unsent = Unsent {
            reason: UnexplainedReason::TooLargeForPrompt,
            lines,
        };
        Ok(self.put(DocumentKind::Unsent, key, &unsent).await?)
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
        let profile = Profile {
            file: prepass.profile_file.as_deref(),
            memcan: self.memcan_profile.text(),
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
        self.submit(updates, stage, 0, prompt, &[], None).await
    }

    /// Submits the stage's job at `generation` (idempotent per stage and generation) and follows its updates.
    /// Jobs carry a context pack of the stored results; the PR's session is set when a runner claims it.
    async fn submit(
        &mut self,
        updates: &mut Updates,
        stage: JobStage,
        generation: u32,
        prompt: String,
        issue_names: &[&str],
        previous: Option<&Value>,
    ) -> Result<(), PipelineError> {
        if self
            .state
            .db
            .jobs()
            .current_for_stage(self.analysis, &stage, generation)
            .await?
            .is_none()
        {
            control::wait_for_resume(self.state, self.analysis).await?;
        }
        let kind = stage.kind();
        let pr = &self.summary.pr;
        let changed_lines = changed_line_count(&self.facts.files);
        let spec = JobSpec {
            job_id: JobId::new_random(),
            attempt: 0,
            kind,
            analysis_id: self.analysis,
            pull_request_id: self
                .state
                .db
                .analyses()
                .get(self.analysis)
                .await?
                .pull_request_id,
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
            output_schema: if matches!(stage, JobStage::Flow(_)) {
                Value::Null
            } else {
                agent::output_schema(kind)?
            },
            flow_file: match &stage {
                JobStage::Flow(id) => Some(contracts::job::spec::FlowFile {
                    context: serde_json::to_value(self.flow_context(id)?)
                        .map_err(|e| self.invalid(&stage, &e.to_string()))?,
                    previous: previous.and_then(Value::as_str).map(str::to_owned),
                }),
                _ => None,
            },
            model: select_model(
                kind,
                &ModelContext { changed_lines },
                self.state
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
            // Built at claim from the results stored then (`context::StoredPacks`).
            working_session: None,
            context_pack: None,
            skills: prompts::required_skills(),
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
        log_submitted(
            self.analysis,
            &stage,
            generation,
            handle.job_id,
            self.batches.len(),
            issue_names,
        );
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
        let events = self
            .state
            .executor
            .events(&handle)
            .map(Some)
            .chain(stream::once(async { None }))
            .scan(false, move |finished, update| {
                // A finished stream must not close a later attempt of this same stage.
                let suppress = update.is_none() && *finished;
                if matches!(update, Some(Ok(JobUpdate::Finished { .. }))) {
                    *finished = true;
                }
                futures::future::ready(
                    (!suppress).then(|| (stage.clone(), generation, Item::Job(update))),
                )
            });
        updates.push(events.boxed());
    }

    fn flow_context(&self, id: &FlowId) -> Result<analysis::tagged::Context, PipelineError> {
        let stage = JobStage::Flow(id.clone());
        let plan = self
            .results
            .plan
            .as_ref()
            .ok_or_else(|| self.invalid(&stage, "missing plan"))?;
        Ok(analysis::tagged::Context {
            version: 1,
            base: self.facts.base.clone(),
            head: self.facts.head.clone(),
            files: self.facts.files.clone(),
            listed: self.facts.listed.clone(),
            plan: plan.clone(),
            flow_id: id.clone(),
            moves: self.facts.prepass.moves.clone(),
            thread_aliases: self.aliases(plan).remove(id).unwrap_or_default(),
        })
    }

    fn check_schema(
        &self,
        stage: &JobStage,
        kind: JobKind,
        output: &Value,
    ) -> Result<Option<Vec<VerifyIssue>>, PipelineError> {
        // A flow's tagged file has no JSON schema: its grammar is checked with its content.
        if matches!(stage, JobStage::Flow(_)) {
            return Ok(None);
        }
        let schema = agent::output_schema(kind)?;
        let validator = jsonschema::validator_for(&schema)
            .map_err(|error| self.invalid(stage, &format!("schema does not compile: {error}")))?;
        Ok(validator.iter_errors(output).next().map(|error| {
            vec![VerifyIssue {
                kind: VerifyIssueKind::SchemaMismatch,
                message: format!(
                    "{}: value does not match the job output schema; include every required field with its declared type",
                    error.instance_path()
                ),
            }]
        }))
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
        let done = u32::try_from(self.results.flows.len()).unwrap_or(u32::MAX);
        (done, self.results.plan.is_some().then(|| self.flow_total()))
    }

    /// `paused` while a job waits for the usage limit, else `running {done, total}` (`planning` until the plan
    /// is known).
    async fn set_progress(&self) -> Result<(), PipelineError> {
        let state = match (self.progress(), self.paused.is_empty()) {
            ((done, total), false) => AnalysisState::Paused {
                reason: Some(contracts::api::analyses::PauseReason::UsageLimit),
                paused_by: None,
                paused_at: None,
                resumes_at: self.paused.values().flatten().max().copied(),
                done,
                total,
                stage: None,
            },
            ((done, Some(total)), true) => AnalysisState::Running {
                done,
                total,
                idle: false,
                stage: None,
            },
            ((_, None), true) => AnalysisState::Planning { stage: None },
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

/// Yields [`Item::PauseOver`] for the stage once `at` has come.
fn pause_timer(
    stage: JobStage,
    generation: u32,
    at: DateTime<Utc>,
) -> BoxStream<'static, (JobStage, u32, Item)> {
    stream::once(async move {
        let wait = (at - Utc::now()).to_std().unwrap_or_default();
        tokio::time::sleep(wait).await;
        (stage, generation, Item::PauseOver(at))
    })
    .boxed()
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

/// Submission facts, without agent input or output text.
fn log_submitted(
    analysis: AnalysisId,
    stage: &JobStage,
    generation: u32,
    job: JobId,
    parts: usize,
    issue_names: &[&str],
) {
    let reason = if generation > 0 {
        format!("repair of {}", stage.key())
    } else {
        match stage {
            JobStage::Plan { batch } => format!("plan part {batch} of {parts}"),
            _ => stage.key(),
        }
    };
    tracing::info!(%analysis, stage = %stage.key(), generation, %job, %reason, issues = ?issue_names,
        "pipeline job submitted");
}

#[cfg(test)]
mod progress_log_tests {
    use super::*;

    #[test]
    fn each_job_logs_its_purpose_at_info_and_repairs_name_the_issues() {
        let capture = crate::test_support::logs::LogCapture::start();
        let analysis = AnalysisId::new_random();
        let job = JobId::new_random();
        for (stage, generation, issues, expected) in [
            (JobStage::Plan { batch: 6 }, 0, vec![], "plan part 6 of 23"),
            (JobStage::PrContext, 0, vec![], "pr_context"),
            (JobStage::Flow(FlowId("f1".into())), 0, vec![], "flow:f1"),
            (
                JobStage::Flow(FlowId("f2".into())),
                1,
                vec!["missing_assessment"],
                "repair of flow:f2",
            ),
        ] {
            log_submitted(analysis, &stage, generation, job, 23, &issues);
            let text = capture.text();
            let line = text.lines().last().expect("log line");
            assert!(
                line.contains("INFO") && line.contains(expected) && line.contains(&job.to_string()),
                "{line}"
            );
            if generation > 0 {
                assert!(line.contains("missing_assessment"), "{line}");
            }
        }
    }
}
