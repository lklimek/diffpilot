//! Verification of agent output against the deterministic facts (`analysis::verify`): the inputs the server
//! gathers (line facts read from the mirror, the agent hunks, the quiet lines, the thread aliases) and blocking
//! reads moved off the async runtime.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use analysis::diff::{DiffError, DiffIndex, FileDiff};
use analysis::prepass::Prepass;
use analysis::verify::{
    self, AnalysisInput, AnalysisVerdict, FlowContext, LineFacts, VerifiedFlow, VerifiedPlan,
    VerifyError, VerifyIssue,
};
use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::agent::plan::{PlanOutput, PlannedFlow};
use contracts::agent::pr_context::PrContextOutput;
use contracts::api::tour::GithubThread;
use contracts::common::LineSide;
use contracts::ids::{CommitSha, FlowId};

use crate::prompts::{self, PromptThread};

/// Verification could not run (a rejection is not an error: it comes back as issues).
#[derive(Debug, thiserror::Error)]
pub enum VerdictError {
    /// Reading the files or computing coverage failed.
    #[error("the agent output could not be checked")]
    Verify(#[source] VerifyError),
    /// The diff has duplicate hunk ids.
    #[error("the diff could not be indexed")]
    Diff(#[from] DiffError),
    /// The blocking verification task did not finish.
    #[error("the verification task did not finish")]
    Task(#[source] tokio::task::JoinError),
}

/// What every check of one analysis reads: the mirror at the analysed commits, the diff and the pre-pass.
#[derive(Debug)]
pub struct Facts {
    /// The server's bare mirror.
    pub mirror: PathBuf,
    /// The analysis base (merge base).
    pub base: CommitSha,
    /// The analysed head.
    pub head: CommitSha,
    /// The whole diff.
    pub files: Vec<FileDiff>,
    /// The diff with only the hunks given to the agent.
    pub listed: Vec<FileDiff>,
    /// The stored pre-pass.
    pub prepass: Prepass,
}

/// Accepted output, or the issues that reject it.
pub type Checked<T> = Result<T, Vec<VerifyIssue>>;

/// Thread alias of the `index`-th thread of the stored snapshot: `t1`, `t2`, … (stable within the analysis).
pub fn thread_alias(index: usize) -> String {
    format!("t{}", index + 1)
}

/// Threads listed in each planned flow's prompt with their aliases: line threads on a line of a hunk the flow
/// selected (a head sub-range limits head lines). File-level threads are never listed.
pub fn flow_threads<'t>(
    threads: &'t [GithubThread],
    plan: &PlanOutput,
    listed: &[FileDiff],
) -> BTreeMap<FlowId, Vec<(String, &'t GithubThread)>> {
    let hunks: BTreeMap<_, _> = listed
        .iter()
        .flat_map(|file| file.hunks.iter().map(move |hunk| (&hunk.id, (file, hunk))))
        .collect();
    let mut out = BTreeMap::new();
    for flow in &plan.flows {
        let mut found = Vec::new();
        for (index, thread) in threads.iter().enumerate() {
            let (Some(line), Some(side)) = (thread.line, thread.side) else {
                continue;
            };
            let inside = flow.hunks.iter().any(|selection| {
                let Some((file, hunk)) = hunks.get(&selection.hunk_id) else {
                    return false;
                };
                let on_file = [LineSide::Head, LineSide::Base]
                    .iter()
                    .any(|&path_side| file.path_on(path_side) == Some(thread.path.as_str()));
                let span = match side {
                    LineSide::Head => hunk.head,
                    LineSide::Base => hunk.base,
                };
                let (mut first, mut last) = (span.start, span.start + span.len.saturating_sub(1));
                if let (LineSide::Head, Some(start), Some(end)) =
                    (side, selection.head_start, selection.head_end)
                {
                    (first, last) = (first.max(start), last.min(end));
                }
                on_file && span.len > 0 && first <= line && line <= last
            });
            if inside {
                found.push((thread_alias(index), thread));
            }
        }
        out.insert(flow.id.clone(), found);
    }
    out
}

/// The aliases each flow's prompt lists (`prompts::listed_thread_aliases`): threads past `prompts::MAX_THREADS`
/// are not in the prompt, so verification must not require a status for them.
pub fn alias_map(
    threads: &BTreeMap<FlowId, Vec<(String, &GithubThread)>>,
) -> BTreeMap<FlowId, Vec<String>> {
    threads
        .iter()
        .map(|(id, listed)| {
            let prompt_threads: Vec<PromptThread<'_>> = listed
                .iter()
                .map(|(alias, thread)| PromptThread { alias, thread })
                .collect();
            (id.clone(), prompts::listed_thread_aliases(&prompt_threads))
        })
        .collect()
}

/// Runs a blocking check off the async runtime.
async fn blocking<T: Send + 'static>(
    check: impl FnOnce() -> Result<T, VerdictError> + Send + 'static,
) -> Result<T, VerdictError> {
    tokio::task::spawn_blocking(check)
        .await
        .map_err(VerdictError::Task)?
}

/// Splits a verification result into accepted or rejected; other errors pass through.
fn checked<T>(result: Result<T, VerifyError>) -> Result<Checked<T>, VerdictError> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(VerifyError::Rejected(issues)) => Ok(Err(issues)),
        Err(error) => Err(VerdictError::Verify(error)),
    }
}

fn load(facts: &Facts, wanted: &[(LineSide, String)]) -> Result<LineFacts, VerdictError> {
    LineFacts::load(&facts.mirror, &facts.base, &facts.head, wanted).map_err(VerdictError::Verify)
}

/// Checks a `plan` output against the hunks listed in its prompt (`batch`: the batch's files).
pub fn plan(
    output: &PlanOutput,
    batch: &[FileDiff],
) -> Result<Checked<VerifiedPlan>, VerdictError> {
    checked(verify::plan(output, &DiffIndex::new(batch)?))
}

/// Checks a `pr_context` output (its evidence lines are read from the mirror).
pub async fn pr_context(
    facts: Arc<Facts>,
    output: PrContextOutput,
) -> Result<Checked<()>, VerdictError> {
    blocking(move || {
        let lines = load(&facts, &verify::pr_context_files(&output))?;
        checked(verify::pr_context(&output, &lines))
    })
    .await
}

/// Checks an `analyse_flow` output of `planned` against the verified plan.
pub async fn flow(
    facts: Arc<Facts>,
    plan: Arc<VerifiedPlan>,
    planned: PlannedFlow,
    output: FlowAnalysisOutput,
    aliases: Vec<String>,
) -> Result<Checked<VerifiedFlow>, VerdictError> {
    blocking(move || {
        let flow_lines = plan
            .flow_lines
            .get(&planned.id)
            .map_or(&[][..], Vec::as_slice);
        let lines = load(&facts, &verify::flow_files(&output, flow_lines))?;
        let diff = DiffIndex::new(&facts.files)?;
        checked(verify::flow(
            &output,
            FlowContext {
                planned: &planned,
                plan: &plan,
                facts: &lines,
                diff: &diff,
                moves: &facts.prepass.moves,
                thread_aliases: &aliases,
            },
        ))
    })
    .await
}

/// The stored outputs of one analysis.
#[derive(Clone, Debug)]
pub struct Outputs {
    /// The plan (merged over its batches).
    pub plan: PlanOutput,
    /// The PR context.
    pub pr_context: PrContextOutput,
    /// The accepted flow analyses.
    pub flows: Vec<FlowAnalysisOutput>,
}

/// Verifies every stored output of one analysis and orders the Tour (`analysis::verify::analysis`).
pub async fn analysis(
    facts: Arc<Facts>,
    outputs: Arc<Outputs>,
    aliases: BTreeMap<FlowId, Vec<String>>,
) -> Result<AnalysisVerdict, VerdictError> {
    blocking(move || {
        let diff = DiffIndex::new(&facts.files)?;
        let listed = DiffIndex::new(&facts.listed)?;
        let quiet = facts.prepass.quiet_lines(&facts.files);
        // Each output is read with its own bounded load, as when it was accepted.
        let read = |wanted: &[(LineSide, String)]| {
            LineFacts::load(&facts.mirror, &facts.base, &facts.head, wanted)
        };
        verify::analysis(AnalysisInput {
            diff: &diff,
            listed: &listed,
            quiet: &quiet,
            facts: &read,
            moves: &facts.prepass.moves,
            plan: &outputs.plan,
            pr_context: &outputs.pr_context,
            flows: &outputs.flows,
            thread_aliases: &aliases,
        })
        .map_err(VerdictError::Verify)
    })
    .await
}

#[cfg(test)]
mod tests {
    use analysis::diff::{FileStatus, Hunk, LineSpan};
    use contracts::agent::plan::HunkSelection;
    use contracts::common::Classification;
    use contracts::ids::HunkId;

    use super::*;

    fn thread(path: &str, line: Option<u32>, side: LineSide) -> GithubThread {
        GithubThread {
            id: format!("T_{path}_{line:?}"),
            path: path.to_owned(),
            line,
            start_line: None,
            side: line.map(|_| side),
            author: "bob".to_owned(),
            excerpt: "x".to_owned(),
            comment_count: 1,
            url: "https://github.com/acme/r/pull/1".to_owned(),
            is_resolved: false,
            is_outdated: false,
            step_id: None,
            llm_status: None,
            llm_reason: None,
        }
    }

    #[test]
    fn threads_are_listed_for_the_flow_whose_hunks_hold_their_line() {
        let files = vec![FileDiff {
            path: "src/a.rs".to_owned(),
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            hunks: vec![Hunk {
                id: HunkId("h1".to_owned()),
                base: LineSpan { start: 9, len: 1 },
                head: LineSpan { start: 10, len: 5 },
                added: (10..15).collect(),
                removed: vec![9],
            }],
        }];
        let selection = |start, end| HunkSelection {
            hunk_id: HunkId("h1".to_owned()),
            head_start: start,
            head_end: end,
        };
        let flow = |id: &str, hunks| PlannedFlow {
            id: FlowId(id.to_owned()),
            title: "t".to_owned(),
            classification: Classification::Behavioural,
            importance_rank: 1,
            intent: "i".to_owned(),
            hunks,
            depends_on: Vec::new(),
        };
        let plan = PlanOutput {
            flows: vec![
                flow("f1", vec![selection(Some(10), Some(11))]),
                flow("f2", vec![selection(Some(12), Some(14))]),
            ],
            unassigned: Vec::new(),
        };
        let threads = [
            thread("src/a.rs", Some(13), LineSide::Head),
            thread("src/a.rs", Some(9), LineSide::Base),
            thread("src/a.rs", None, LineSide::Head),
            thread("README.md", Some(13), LineSide::Head),
            thread("src/a.rs", Some(30), LineSide::Head),
        ];
        let listed = alias_map(&flow_threads(&threads, &plan, &files));
        assert_eq!(
            listed[&FlowId("f1".to_owned())],
            ["t2"],
            "base line of the hunk"
        );
        assert_eq!(listed[&FlowId("f2".to_owned())], ["t1", "t2"]);
    }

    #[test]
    fn verification_requires_only_the_threads_in_the_prompt() {
        let many: Vec<GithubThread> = (0..prompts::MAX_THREADS + 5)
            .map(|_| thread("src/a.rs", Some(10), LineSide::Head))
            .collect();
        let listed: Vec<(String, &GithubThread)> = many
            .iter()
            .enumerate()
            .map(|(index, thread)| (thread_alias(index), thread))
            .collect();
        let aliases = alias_map(&BTreeMap::from([(FlowId("f1".to_owned()), listed)]));
        let aliases = &aliases[&FlowId("f1".to_owned())];
        assert_eq!(aliases.len(), prompts::MAX_THREADS);
        assert_eq!(aliases.last().map(String::as_str), Some("t50"));
    }
}
