//! Projection of the stored agent output plus repository facts into the `Tour` (stage-2 slice).
//!
//! Server-set facts here: flow order, step ids and numbers, graph nodes, changed lines, GitHub links. Facts of
//! later work packages are empty: CI, counters, profile and instruction-file notices (WP3-PREPASS,
//! WP3-REVIEW), quiet rows (WP3-PREPASS, WP3-VERIFY), move facts, threads (WP3-REVIEW).

use std::collections::HashMap;

use analysis::diff::FileDiff;
use analysis::lines::{changed_lines_within, contains_range, ranges};
use analysis::order::{OrderInput, tour_order};
use contracts::agent::analyse_flow::{AgentStep, FlowAnalysisOutput};
use contracts::agent::plan::{PlanOutput, PlannedFlow};
use contracts::agent::pr_context::PrContextOutput;
use contracts::api::analyses::{AnalysisState, AnalysisSummary};
use contracts::api::tour::{
    Annotation, CiState, CiStatus, Counters, Flow, FlowFacts, FlowGraph, FlowGroup, FlowLlm,
    GraphEdge, GraphNode, ImpactRef, PatternRepetition, PatternSite, PrContextBlock,
    PrContextFacts, ProjectProfileStatus, QuietRows, Step, StepFacts, StepLlm, Tour,
};
use contracts::common::{LineRange, LineSide};
use contracts::ids::{AnnotationId, FlowId, StepId};
use db::error::DbError;
use db::repo::documents::DocumentKind;
use url::Url;

use crate::pipeline::{PrDiffError, pr_diff};
use crate::state::AppState;

/// The Tour cannot be built from the stored documents.
#[derive(Debug, thiserror::Error)]
pub enum TourError {
    /// A planned flow has no stored analysis.
    #[error("flow {0} has no stored analysis")]
    MissingFlow(FlowId),
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
    /// The Tour exists only for finished analyses.
    #[error("the analysis has not finished")]
    NotFinished,
    /// A stored document is missing or the database failed.
    #[error("the stored documents could not be read")]
    Db(#[from] DbError),
    /// The diff of the analysis is not available.
    #[error("the diff of the analysis is not available")]
    Diff(#[from] PrDiffError),
    /// The stored documents do not form a Tour.
    #[error("the Tour could not be built")]
    Tour(#[from] TourError),
}

/// The Tour of a finished analysis, projected from its stored documents and diff.
pub async fn load(state: &AppState, summary: &AnalysisSummary) -> Result<Tour, TourLoadError> {
    if !matches!(summary.state, AnalysisState::Finished { .. }) {
        return Err(TourLoadError::NotFinished);
    }
    let documents = state.db.documents();
    let plan: PlanOutput = documents
        .get(summary.id, DocumentKind::Plan, "")
        .await?
        .ok_or(DbError::NotFound)?;
    let pr_context: PrContextOutput = documents
        .get(summary.id, DocumentKind::PrContext, "")
        .await?
        .ok_or(DbError::NotFound)?;
    let flows: Vec<FlowAnalysisOutput> = documents
        .list(summary.id, DocumentKind::Flow)
        .await?
        .into_iter()
        .map(|(_, flow)| flow)
        .collect();
    let (_, files) = pr_diff(state, summary).await?;
    Ok(project(summary, &plan, pr_context, flows, &files)?)
}

/// Builds the Tour of a finished analysis.
pub fn project(
    summary: &AnalysisSummary,
    plan: &PlanOutput,
    pr_context: PrContextOutput,
    flows: Vec<FlowAnalysisOutput>,
    files: &[FileDiff],
) -> Result<Tour, TourError> {
    let links = Links::new(summary);
    let mut outputs: HashMap<FlowId, FlowAnalysisOutput> = flows
        .into_iter()
        .map(|flow| (flow.flow_id.clone(), flow))
        .collect();
    let order = tour_order(
        &plan
            .flows
            .iter()
            .map(|flow| OrderInput {
                id: flow.id.clone(),
                group: FlowGroup::from(flow.classification),
                importance_rank: flow.importance_rank,
            })
            .collect::<Vec<_>>(),
    );
    let mut tour_flows = Vec::with_capacity(order.len());
    for (index, id) in order.into_iter().enumerate() {
        let Some(planned) = plan.flows.iter().find(|flow| flow.id == id) else {
            continue;
        };
        let output = outputs
            .remove(&id)
            .ok_or_else(|| TourError::MissingFlow(id.clone()))?;
        let position = u32::try_from(index + 1).unwrap_or(u32::MAX);
        tour_flows.push(project_flow(planned, output, position, files, &links));
    }
    let step_count = tour_flows.iter().map(|flow| flow.facts.step_count).sum();
    Ok(Tour {
        analysis: summary.clone(),
        pr_context: PrContextBlock {
            facts: PrContextFacts {
                ci: CiStatus {
                    state: CiState::None,
                    total: 0,
                    failing: Vec::new(),
                },
                counters: Counters {
                    lines_removed_from_tests: 0,
                    dependencies_changed: Vec::new(),
                },
                project_profile: ProjectProfileStatus {
                    sources: Vec::new(),
                },
                agent_instruction_files: Vec::new(),
                // The stage-2 agent has no GitHub tool (no installation token yet, WP3-APPTOKEN).
                related_work_checked: false,
                flow_count: u32::try_from(tour_flows.len()).unwrap_or(u32::MAX),
                step_count,
            },
            llm: pr_context,
        },
        flows: tour_flows,
        quiet: QuietRows {
            unexplained: Vec::new(),
            moves: Vec::new(),
            not_analysed: Vec::new(),
        },
        threads: Vec::new(),
    })
}

fn project_flow(
    planned: &PlannedFlow,
    output: FlowAnalysisOutput,
    position: u32,
    files: &[FileDiff],
    links: &Links,
) -> Flow {
    let step_ids: HashMap<String, StepId> = output
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            (
                step.id.clone(),
                StepId(format!("{}.{}", planned.id, index + 1)),
            )
        })
        .collect();
    let server_id = |local: &str| step_ids.get(local).cloned();
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
    let nodes = output
        .steps
        .iter()
        .enumerate()
        .filter_map(|(index, step)| {
            Some(GraphNode {
                step_id: server_id(&step.id)?,
                number: u32::try_from(index + 1).ok()?,
                kind: step.kind.into(),
                label: step.title.clone(),
            })
        })
        .collect();
    let edges = output
        .edges
        .iter()
        .filter_map(|edge| {
            Some(GraphEdge {
                from: server_id(&edge.from_step_id)?,
                to: server_id(&edge.to_step_id)?,
                label: edge.label.clone(),
            })
        })
        .collect();
    let steps: Vec<Step> = output
        .steps
        .iter()
        .enumerate()
        .filter_map(|(index, step)| {
            let id = server_id(&step.id)?;
            Some(project_step(
                step,
                id,
                u32::try_from(index + 1).ok()?,
                &output,
                files,
                links,
            ))
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
        graph: FlowGraph { nodes, edges },
        steps,
    }
}

fn project_step(
    step: &AgentStep,
    id: StepId,
    number: u32,
    output: &FlowAnalysisOutput,
    files: &[FileDiff],
    links: &Links,
) -> Step {
    let range = LineRange {
        start_line: step.start_line,
        end_line: step.end_line,
    };
    let inside = changed_lines_within(
        files,
        &step.file,
        step.side,
        range.start_line,
        range.end_line,
    );
    let annotations = step
        .annotations
        .iter()
        .enumerate()
        .map(|(index, annotation)| {
            Annotation::from_agent(
                AnnotationId(format!("{id}.a{}", index + 1)),
                annotation.clone(),
            )
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
        id,
        number,
        facts: StepFacts {
            file: step.file.clone(),
            side: step.side,
            range,
            github_url: links.lines(&step.file, step.side, range),
            changed_lines: ranges(&inside),
            moved: None,
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
}
