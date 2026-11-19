//! Projection of a review-ready analysis into the `Tour`: the verified Tour skeleton (`analysis::verify`), the stored
//! pre-pass and the stored GitHub snapshot. Every fact comes from one of them or says "not checked" (brief §1
//! "silence is not safety"): CI and threads from the snapshot, counters, profile, instruction files, moves and
//! Not analysed from the pre-pass, Unexplained from the coverage of the accepted flows, step ids, graph and move
//! facts from verification.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use analysis::diff::FileDiff;
use analysis::lines::{changed_lines_within, contains_range, ranges};
use analysis::prepass::Prepass;
use analysis::verify::{OutputRef, Repair, TourSkeleton, VerifiedFlow};
use contracts::agent::plan::PlannedFlow;
use contracts::agent::pr_context::PrContextOutput;
use contracts::api::analyses::{AnalysisState, AnalysisSummary};
use contracts::api::tour::{
    Annotation, Flow, FlowAnalysisState, FlowAnalysisStatus, FlowFacts, FlowGroup, FlowLlm,
    GithubThread, ImpactRef, PatternRepetition, PatternSite, PrContextBlock, PrContextFacts,
    QuietRows, Step, StepFacts, StepLlm, Tour,
};
use contracts::common::{LineRange, LineSide};
use contracts::ids::{AnnotationId, FlowId, StepId};
use db::error::DbError;
use db::repo::documents::DocumentKind;
use url::Url;

use crate::memcan::profile;
use crate::pipeline::context::StoredResults;
use crate::pipeline::prepass::{self, PrepassRunError};
use crate::pipeline::unsent::{self, Unsent};
use crate::pipeline::verdict::{self, Facts, Outputs, VerdictError, thread_alias};
use crate::pipeline::{PrDiffError, pr_diff};
use crate::review::{GithubSnapshot, stored_github_snapshot};
use crate::state::AppState;

/// The Tour cannot be built from the stored documents.
#[derive(Debug, thiserror::Error)]
pub enum TourError {
    /// The stored plan fails verification: there is no Tour skeleton.
    #[error("the stored plan does not pass verification")]
    PlanRejected,
    /// Verification could not run.
    #[error("the stored outputs could not be checked")]
    Verdict(#[from] VerdictError),
}

/// GitHub permalinks into the analysed repository.
struct Links {
    repo_url: String,
    summary: AnalysisSummary,
}

impl Links {
    fn new(summary: &AnalysisSummary) -> Self {
        let suffix = format!("/pull/{}", summary.pr.number);
        let repo_url = summary
            .pr
            .html_url
            .strip_suffix(&suffix)
            .unwrap_or(&summary.pr.html_url)
            .to_owned();
        Self {
            repo_url,
            summary: summary.clone(),
        }
    }

    /// `<repo>/blob/<sha of side>/<path>#L<a>-L<b>`.
    fn lines(&self, path: &str, side: LineSide, range: LineRange) -> String {
        let sha = match side {
            LineSide::Head => &self.summary.pr.head_sha,
            LineSide::Base => &self.summary.pr.base_sha,
        };
        let raw = format!("{}/blob/{sha}", self.repo_url);
        let Ok(mut url) = Url::parse(&raw) else {
            return raw;
        };
        if let Ok(mut segments) = url.path_segments_mut() {
            segments.extend(path.split('/'));
        }
        url.set_fragment(Some(&format!("L{}-L{}", range.start_line, range.end_line)));
        url.into()
    }
}

/// The Tour could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum TourLoadError {
    /// The Tour requires an accepted plan and PR context.
    #[error("the analysis has not finished")]
    NotFinished,
    /// A stored document is missing or the database failed.
    #[error("the stored documents could not be read")]
    Db(#[from] DbError),
    /// The diff of the analysis is not available.
    #[error("the diff of the analysis is not available")]
    Diff(#[from] PrDiffError),
    /// The pre-pass could not be computed.
    #[error("the pre-pass is not available")]
    Prepass(#[from] PrepassRunError),
    /// The stored documents do not form a Tour.
    #[error("the Tour could not be built")]
    Tour(#[from] TourError),
}

/// The Tour of a review-ready analysis: its stored outputs verified again (verification is deterministic), then
/// projected with the stored pre-pass and GitHub snapshot.
pub async fn load(state: &AppState, summary: &AnalysisSummary) -> Result<Tour, TourLoadError> {
    if summary.review_ready != Some(true)
        && !matches!(summary.state, AnalysisState::Finished { .. })
    {
        return Err(TourLoadError::NotFinished);
    }
    let results = StoredResults::load(&state.db, summary.id).await?;
    load_from(state, summary, results).await
}

/// The Tour of `results`, the stored outputs read earlier: a flow stored since is still shown as it was then.
pub async fn load_from(
    state: &AppState,
    summary: &AnalysisSummary,
    results: StoredResults,
) -> Result<Tour, TourLoadError> {
    let (Some(plan), Some(pr_context)) = (results.plan, results.pr_context) else {
        return Err(DbError::NotFound.into());
    };
    // The status belongs to the documents read: `analysed` only for a flow whose document `results` holds.
    let mut waiting = BTreeMap::new();
    for planned in &plan.flows {
        if !results.flows.iter().any(|flow| flow.flow_id == planned.id) {
            let status = state.db.flow_work().status(summary.id, &planned.id).await?;
            waiting.insert(planned.id.clone(), status);
        }
    }
    let snapshot = stored_github_snapshot(state, summary.id).await?;
    let (mirror, files) = pr_diff(state, summary).await?;
    // Stored by the driver; computed here for an analysis finished without one (deterministic).
    let prepass = prepass::ensure_for_read(state, summary, &mirror, &files).await?;
    let facts = Arc::new(Facts {
        mirror,
        base: summary.pr.base_sha.clone(),
        head: summary.pr.head_sha.clone(),
        listed: prepass.agent_files(&files),
        files,
        prepass,
    });
    let aliases = verdict::alias_map(&verdict::flow_threads(
        &snapshot.threads,
        &plan,
        &facts.listed,
    ));
    let outputs = Arc::new(Outputs {
        plan,
        pr_context: pr_context.clone(),
        flows: results.flows,
    });
    let verdict = verdict::analysis(Arc::clone(&facts), outputs, aliases.clone(), true)
        .await
        .map_err(TourError::from)?;
    let repairs = verdict.repairs;
    let skeleton = verdict.tour.ok_or(TourError::PlanRejected)?;
    // A stored flow today's checks reject: failed, its lines listed, never silently gone.
    for planned in &skeleton.plan.output.flows {
        let accepted = skeleton.flows.iter().any(|flow| flow.flow_id == planned.id);
        if !accepted && !waiting.contains_key(&planned.id) {
            let codes = rejected_codes(&repairs, &planned.id);
            tracing::warn!(analysis = %summary.id, flow = %planned.id, issues = ?codes,
                "a stored flow fails today's checks; the Tour shows it failed with its lines");
            let status = FlowAnalysisStatus {
                state: FlowAnalysisState::Failed,
                started_at: None,
                message: Some(rejected_message(&summary.state)),
            };
            waiting.insert(planned.id.clone(), status);
        }
    }
    // Stored with the accepted PR context: its agent had the GitHub tool.
    let related_work_checked = state
        .db
        .documents()
        .github_access(summary.id, DocumentKind::PrContext, "")
        .await?
        == Some(true);
    let unsent: Vec<Unsent> = state
        .db
        .documents()
        .list(summary.id, DocumentKind::Unsent)
        .await?
        .into_iter()
        .map(|(_, unsent)| unsent)
        .collect();
    Ok(project(&Projection {
        summary,
        skeleton: &skeleton,
        pr_context,
        prepass: &facts.prepass,
        snapshot: &snapshot,
        files: &facts.files,
        aliases: &aliases,
        related_work_checked,
        unsent: &unsent,
        memcan_profile: profile::stored(&state.db, summary.id).await?.found(),
        waiting: &waiting,
    }))
}

/// The server's issue codes of a stored flow that today's checks reject (for the log, never user text).
fn rejected_codes<'a>(repairs: &'a [Repair], flow: &FlowId) -> Vec<&'a str> {
    let mut codes: Vec<&str> = repairs
        .iter()
        .filter(|repair| repair.output == OutputRef::Flow(flow.clone()))
        .flat_map(|repair| &repair.issues)
        .map(|issue| issue.kind.code())
        .collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

/// Plain failure text of a stored flow that today's checks reject, naming the action that works in `state`.
fn rejected_message(state: &AnalysisState) -> String {
    let action = if matches!(state, AnalysisState::Failed { .. }) {
        "Retry the analysis; then analyse this flow again."
    } else {
        "Analyse this flow again."
    };
    format!(
        "The stored analysis of this flow does not pass the server's current checks, so its changes are listed \
         as not analysed. {action}"
    )
}

/// Flows whose stored document fails today's checks (the Tour shows them failed), each with the document text
/// verified, so only that version is discarded.
pub async fn rejected_stored_flows(
    state: &AppState,
    summary: &AnalysisSummary,
) -> Result<Vec<(FlowId, String)>, TourLoadError> {
    let documents = state.db.documents();
    let stored = documents.list_raw(summary.id, DocumentKind::Flow).await?;
    if stored.is_empty() {
        return Ok(Vec::new());
    }
    let flows = stored
        .iter()
        .map(|(_, body)| serde_json::from_str(body).map_err(DbError::from))
        .collect::<Result<Vec<_>, _>>()?;
    let results = StoredResults {
        plan: documents.get(summary.id, DocumentKind::Plan, "").await?,
        pr_context: documents
            .get(summary.id, DocumentKind::PrContext, "")
            .await?,
        flows,
    };
    let tour = load_from(state, summary, results).await?;
    let failed = |flow: &str| {
        tour.flows.iter().any(|shown| {
            shown.id.0 == flow
                && shown
                    .analysis_status
                    .as_ref()
                    .is_some_and(|status| status.state == FlowAnalysisState::Failed)
        })
    };
    Ok(stored
        .into_iter()
        .filter(|(key, _)| failed(key))
        .map(|(key, body)| (FlowId(key), body))
        .collect())
}

/// Everything the Tour is projected from.
pub struct Projection<'a> {
    /// The analysis.
    pub summary: &'a AnalysisSummary,
    /// Accepted flows in tour order, Unexplained flows and coverage.
    pub skeleton: &'a TourSkeleton,
    /// The accepted PR context.
    pub pr_context: PrContextOutput,
    /// The stored pre-pass.
    pub prepass: &'a Prepass,
    /// The stored GitHub snapshot.
    pub snapshot: &'a GithubSnapshot,
    /// The whole diff.
    pub files: &'a [FileDiff],
    /// Thread aliases listed in each flow's prompt.
    pub aliases: &'a BTreeMap<FlowId, Vec<String>>,
    /// The accepted PR context was written by an agent with the GitHub tool.
    pub related_work_checked: bool,
    /// Lines the server did not send to the LLM, with the reason (Unexplained rows show it).
    pub unsent: &'a [Unsent],
    /// The analysis's MemCan profile lookup: `None` not checked, else whether a profile was found.
    pub memcan_profile: Option<bool>,
    /// The status of each planned flow without a stored document (not analysed, queued, analysing, failed).
    pub waiting: &'a BTreeMap<FlowId, FlowAnalysisStatus>,
}

/// Builds a Tour with accepted steps and explicit placeholders for pending flows.
pub fn project(input: &Projection<'_>) -> Tour {
    let links = Links::new(input.summary);
    let mut flows: Vec<Flow> = Vec::with_capacity(input.skeleton.flows.len());
    for (verified, position) in input.skeleton.flows.iter().zip(1u32..) {
        let Some(planned) = input
            .skeleton
            .plan
            .output
            .flows
            .iter()
            .find(|flow| flow.id == verified.flow_id)
        else {
            continue;
        };
        flows.push(project_flow(
            planned,
            verified,
            position,
            (input.files, input.prepass),
            &links,
        ));
    }
    let analysed = FlowAnalysisStatus {
        state: FlowAnalysisState::Analysed,
        started_at: None,
        message: None,
    };
    for planned in &input.skeleton.plan.output.flows {
        let details = contracts::api::tour::PlannedFlowDetails {
            intent: planned.intent.clone(),
            importance_rank: planned.importance_rank,
            ranges: input
                .skeleton
                .plan
                .flow_lines
                .get(&planned.id)
                .cloned()
                .unwrap_or_default(),
        };
        if let Some(flow) = flows.iter_mut().find(|f| f.id == planned.id) {
            flow.planned = Some(details);
            flow.analysis_status = Some(analysed.clone());
        } else {
            flows.push(Flow {
                id: planned.id.clone(),
                position: 0,
                group: planned.classification.into(),
                // Every planned flow outside the skeleton has a status in `waiting` (a rejected stored one: failed).
                analysis_status: Some(
                    input
                        .waiting
                        .get(&planned.id)
                        .cloned()
                        .unwrap_or_else(|| analysed.clone()),
                ),
                facts: FlowFacts {
                    files: details.ranges.iter().map(|r| r.file.clone()).collect(),
                    step_count: 0,
                },
                llm: FlowLlm {
                    title: planned.title.clone(),
                    classification: planned.classification,
                    risk: None,
                    introduction: contracts::agent::analyse_flow::FlowIntroduction {
                        problem_before: String::new(),
                        change: String::new(),
                        example_before_after: String::new(),
                    },
                    depends_on: planned.depends_on.clone(),
                    pattern_repetitions: Vec::new(),
                },
                graph: contracts::api::tour::FlowGraph {
                    nodes: Vec::new(),
                    edges: Vec::new(),
                },
                steps: Vec::new(),
                planned: Some(details),
            });
        }
    }
    flows.sort_by_key(|f| {
        (
            f.group,
            f.planned
                .as_ref()
                .map(|p| p.importance_rank)
                .unwrap_or_default(),
        )
    });
    for (flow, position) in flows.iter_mut().zip(1u32..) {
        flow.position = position;
    }
    // Lines of flows still waiting are shown under their flow; a failed flow's lines stay Unexplained.
    let failed = |flow: &Flow| {
        flow.analysis_status
            .as_ref()
            .is_some_and(|status| status.state == FlowAnalysisState::Failed)
    };
    let pending: Vec<_> = flows
        .iter()
        .filter(|f| f.steps.is_empty() && !failed(f))
        .flat_map(|f| f.planned.iter().flat_map(|p| p.ranges.iter()))
        .collect();
    let mut unexplained = unsent::with_reasons(&input.skeleton.coverage.unexplained, input.unsent);
    for item in &mut unexplained {
        for excluded in pending
            .iter()
            .filter(|p| p.file == item.file && p.side == item.side)
        {
            for cut in &excluded.ranges {
                item.ranges = item
                    .ranges
                    .iter()
                    .flat_map(|r| {
                        if cut.end_line < r.start_line || cut.start_line > r.end_line {
                            return vec![*r];
                        }
                        let mut parts = Vec::new();
                        if r.start_line < cut.start_line {
                            parts.push(LineRange {
                                start_line: r.start_line,
                                end_line: cut.start_line - 1,
                            });
                        }
                        if r.end_line > cut.end_line {
                            parts.push(LineRange {
                                start_line: cut.end_line + 1,
                                end_line: r.end_line,
                            });
                        }
                        parts
                    })
                    .collect();
            }
        }
    }
    unexplained.retain(|r| !r.ranges.is_empty());
    let threads = place_threads(input, &mut flows);
    let step_count = flows.iter().map(|flow| flow.facts.step_count).sum();
    let prepass = input.prepass;
    let mut llm = input.pr_context.clone();
    let related_work_checked = input.related_work_checked;
    if !related_work_checked && !llm.related_work.is_empty() {
        // Without the GitHub tool the agent could not check: whatever it lists is not checked related work.
        tracing::warn!(analysis = %input.summary.id, items = llm.related_work.len(),
            "related work listed by an agent without GitHub access; it is not shown");
        llm.related_work.clear();
    }
    Tour {
        analysis: input.summary.clone(),
        pr_context: PrContextBlock {
            facts: PrContextFacts {
                related_work_links: Some(
                    llm.related_work
                        .iter()
                        .filter_map(|item| {
                            crate::github::related::target(item, &input.summary.pr.repo)
                        })
                        .collect(),
                ),
                memcan_status: None,
                ci: input.snapshot.ci.clone(),
                threads_status: input.snapshot.threads_status,
                counters: prepass.counters.counters.clone(),
                project_profile: prepass.profile_status(input.memcan_profile),
                agent_instruction_files: prepass.agent_instruction_files.clone(),
                related_work_checked,
                flow_count: u32::try_from(flows.len()).unwrap_or(u32::MAX),
                step_count,
            },
            llm,
        },
        flows,
        quiet: QuietRows {
            unexplained,
            moves: prepass.identical_moves(),
            not_analysed: prepass.not_analysed_with_binaries(),
        },
        threads,
    }
}

/// Every snapshot thread with the step it is located in; the LLM status only when that step's flow listed the
/// thread in its prompt and rated it. Steps get the ids of their threads.
fn place_threads(input: &Projection<'_>, flows: &mut [Flow]) -> Vec<GithubThread> {
    let statuses: HashMap<(&FlowId, &str), (contracts::common::ThreadLlmStatus, &str)> = input
        .skeleton
        .flows
        .iter()
        .flat_map(|flow| {
            flow.output.thread_statuses.iter().map(move |status| {
                (
                    (&flow.flow_id, status.thread_alias.as_str()),
                    (status.status, status.reason.as_str()),
                )
            })
        })
        .collect();
    let mut out = Vec::with_capacity(input.snapshot.threads.len());
    for (index, thread) in input.snapshot.threads.iter().enumerate() {
        let mut placed = thread.clone();
        let alias = thread_alias(index);
        let located = thread.line.zip(thread.side).and_then(|(line, side)| {
            flows.iter_mut().find_map(|flow| {
                let step = flow.steps.iter_mut().find(|step| {
                    step.facts.file == thread.path
                        && step.facts.side == side
                        && step.facts.range.start_line <= line
                        && line <= step.facts.range.end_line
                })?;
                step.facts.thread_ids.push(thread.id.clone());
                Some((flow.id.clone(), step.id.clone()))
            })
        });
        placed.step_id = None;
        placed.llm_status = None;
        placed.llm_reason = None;
        if let Some((flow, step)) = located {
            placed.step_id = Some(step);
            let listed = input
                .aliases
                .get(&flow)
                .is_some_and(|aliases| aliases.contains(&alias));
            if let (true, Some((status, reason))) = (listed, statuses.get(&(&flow, alias.as_str())))
            {
                placed.llm_status = Some(*status);
                placed.llm_reason = Some((*reason).to_owned());
            }
        }
        out.push(placed);
    }
    out
}

fn project_flow(
    planned: &PlannedFlow,
    verified: &VerifiedFlow,
    position: u32,
    (files, prepass): (&[FileDiff], &Prepass),
    links: &Links,
) -> Flow {
    let output = &verified.output;
    let server_ids: HashMap<&str, &StepId> = output
        .steps
        .iter()
        .zip(&verified.step_ids)
        .map(|(step, id)| (step.id.as_str(), id))
        .collect();
    let server_id = |local: &str| server_ids.get(local).map(|id| (*id).clone());
    let moves: HashMap<&StepId, _> = verified.moves.iter().map(|(id, fact)| (id, fact)).collect();
    let mut files_touched: Vec<String> = Vec::new();
    for step in &output.steps {
        if !files_touched.contains(&step.file) {
            files_touched.push(step.file.clone());
        }
    }
    let risk = output
        .steps
        .iter()
        .filter_map(|step| step.assessment.as_ref().map(|assessment| assessment.risk))
        .max();
    let steps: Vec<Step> = output
        .steps
        .iter()
        .zip(&verified.step_ids)
        .zip(1u32..)
        .map(|((step, id), number)| {
            let range = LineRange {
                start_line: step.start_line,
                end_line: step.end_line,
            };
            let whitespace = prepass.whitespace_lines(step.side, &step.file);
            let mut inside = changed_lines_within(
                files,
                &step.file,
                step.side,
                range.start_line,
                range.end_line,
            );
            inside.retain(|line| whitespace.binary_search(line).is_err());
            let annotations = step
                .annotations
                .iter()
                .zip(1u32..)
                .map(|(annotation, n)| {
                    Annotation::from_agent(AnnotationId::of(id, n), annotation.clone())
                })
                .collect();
            let impact = output
                .impact_refs
                .iter()
                .filter(|impact| impact.step_id == step.id)
                .map(|impact| {
                    let range = LineRange {
                        start_line: impact.start_line,
                        end_line: impact.end_line,
                    };
                    ImpactRef {
                        file: impact.file.clone(),
                        range,
                        symbol: impact.symbol.clone(),
                        reason: impact.reason.clone(),
                        basis: impact.basis,
                        github_url: links.lines(&impact.file, LineSide::Head, range),
                    }
                })
                .collect();
            Step {
                id: id.clone(),
                number,
                facts: StepFacts {
                    file: step.file.clone(),
                    side: step.side,
                    range,
                    github_url: links.lines(&step.file, step.side, range),
                    changed_lines: ranges(&inside),
                    moved: moves.get(id).map(|fact| (*fact).clone()),
                    call_hierarchy: None,
                    thread_ids: Vec::new(),
                },
                llm: StepLlm {
                    title: step.title.clone(),
                    kind: step.kind,
                    symbol: step.symbol.clone(),
                    called_by: step.called_by.clone(),
                    decides: step.decides.clone(),
                    next: step.next.clone(),
                    why: step.why.clone(),
                    annotations,
                    assessment: step.assessment.clone(),
                    moved_from: step.moved_from.clone(),
                    impact,
                },
            }
        })
        .collect();
    let pattern_repetitions = output
        .pattern_repetitions
        .iter()
        .filter_map(|pattern| {
            Some(PatternRepetition {
                description: pattern.description.clone(),
                explained_in: server_id(&pattern.explained_in_step_id)?,
                sites: pattern
                    .sites
                    .iter()
                    .map(|site| {
                        let range = LineRange {
                            start_line: site.start_line,
                            end_line: site.end_line,
                        };
                        let step_id = output
                            .steps
                            .iter()
                            .find(|step| {
                                let step_range = LineRange {
                                    start_line: step.start_line,
                                    end_line: step.end_line,
                                };
                                step.file == site.file
                                    && step.side == site.side
                                    && contains_range(&step_range, &range)
                            })
                            .and_then(|step| server_id(&step.id));
                        PatternSite {
                            file: site.file.clone(),
                            side: site.side,
                            range,
                            step_id,
                            github_url: links.lines(&site.file, site.side, range),
                        }
                    })
                    .collect(),
            })
        })
        .collect();
    Flow {
        analysis_status: None,
        planned: None,
        id: planned.id.clone(),
        position,
        group: FlowGroup::from(planned.classification),
        facts: FlowFacts {
            files: files_touched,
            step_count: u32::try_from(steps.len()).unwrap_or(u32::MAX),
        },
        llm: FlowLlm {
            title: planned.title.clone(),
            classification: planned.classification,
            risk,
            introduction: output.introduction.clone(),
            depends_on: planned.depends_on.clone(),
            pattern_repetitions,
        },
        graph: verified.graph.clone(),
        steps,
    }
}
