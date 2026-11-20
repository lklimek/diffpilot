//! Plan batches that fit the plan prompt: the directory batches of `analysis::prepass::batches` split again
//! (deterministically) while their rendered prompt is over budget. Every agent hunk ends in exactly one batch, or
//! — when its file alone cannot fit — in the plan's `unassigned` list with a reason (its lines are Unexplained).

use std::collections::VecDeque;

use analysis::diff::FileDiff;
use contracts::agent::plan::UnassignedHunk;

use crate::prompts::PromptError;

/// Reason recorded for the hunks of a file too large for any plan prompt.
pub const UNFIT_REASON: &str = "The server did not give this file to the agent: its diff alone is larger than a plan prompt may be.";

/// Batches whose prompts fit, and the hunks of files that fit no prompt.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FittedBatches {
    /// Batches in order; each renders within the plan prompt budget.
    pub batches: Vec<Vec<FileDiff>>,
    /// Hunks given to no batch, with [`UNFIT_REASON`].
    pub unfit: Vec<UnassignedHunk>,
}

/// Splits every batch whose `render`ed prompt is over budget: halves by sorted path while it has several files,
/// then halves a file's hunks; a single hunk that still does not fit goes to `unfit`. Other render errors pass.
pub fn fit(
    batches: Vec<Vec<FileDiff>>,
    render: &dyn Fn(&[FileDiff]) -> Result<String, PromptError>,
) -> Result<FittedBatches, PromptError> {
    let mut queue: VecDeque<Vec<FileDiff>> = batches.into();
    let mut fitted = FittedBatches::default();
    while let Some(mut batch) = queue.pop_front() {
        match render(&batch) {
            Ok(_) => fitted.batches.push(batch),
            Err(PromptError::OverBudget { kind: "plan", .. }) => {
                batch.sort_by(|a, b| a.path.cmp(&b.path));
                let (left, right) = match batch.as_slice() {
                    [] => continue,
                    [file] if file.hunks.len() < 2 => {
                        fitted
                            .unfit
                            .extend(file.hunks.iter().map(|hunk| UnassignedHunk {
                                hunk_id: hunk.id.clone(),
                                head_start: None,
                                head_end: None,
                                reason: UNFIT_REASON.to_owned(),
                            }));
                        tracing::warn!(file = %file.path,
                            "a file's diff is larger than a plan prompt; its lines stay Unexplained");
                        continue;
                    }
                    [file] => {
                        let (first, second) = file.hunks.split_at(file.hunks.len() / 2);
                        let part = |hunks: &[analysis::diff::Hunk]| FileDiff {
                            hunks: hunks.to_vec(),
                            ..file.clone()
                        };
                        (vec![part(first)], vec![part(second)])
                    }
                    files => {
                        let (first, second) = files.split_at(files.len() / 2);
                        (first.to_vec(), second.to_vec())
                    }
                };
                queue.push_front(right);
                queue.push_front(left);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(fitted)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use analysis::diff::{FileStatus, Hunk, LineSpan};
    use analysis::prepass::batches::plan_batches;
    use contracts::api::pr::PrSummary;
    use contracts::common::LlmLanguage;
    use contracts::ids::HunkId;

    use super::*;
    use crate::prompts::{self, JobContext, PlanBatch, PlanInput, Profile};

    fn pr() -> PrSummary {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/pr-482/api-seed/default/pr-resolve.json");
        let seed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("seed")).expect("JSON");
        serde_json::from_value(seed["pr"].clone()).expect("PrSummary")
    }

    /// 800 one-line files in one directory with 200-byte paths: one directory batch, over the prompt budget.
    fn long_path_files() -> Vec<FileDiff> {
        let dir = "d".repeat(180);
        (0..800u32)
            .map(|n| FileDiff {
                path: format!("src/{dir}/file{n:05}.rs"),
                old_path: None,
                status: FileStatus::Modified,
                binary: false,
                hunks: vec![Hunk {
                    id: HunkId(format!("h{n}")),
                    base: LineSpan { start: 1, len: 1 },
                    head: LineSpan { start: 1, len: 1 },
                    added: vec![1],
                    removed: vec![1],
                }],
            })
            .collect()
    }

    #[test]
    fn over_budget_batches_are_split_until_every_prompt_fits() {
        let pr = pr();
        let files = long_path_files();
        let prepass =
            analysis::prepass::prepass(&pr.repo, &files, &[], &|_, _| None).expect("prepass");
        let job = JobContext {
            pr: &pr,
            language: LlmLanguage::En,
            sub_agents: prompts::SUB_AGENTS_DEFAULT,
            prepass: &prepass,
        };
        let render = |files: &[FileDiff]| {
            prompts::plan(&PlanInput {
                job,
                files,
                profile: Profile::default(),
                batch: Some(PlanBatch {
                    number: u32::MAX,
                    total: u32::MAX,
                }),
            })
        };
        let batches = plan_batches(&prepass.plan_files(&files));
        assert_eq!(
            batches.len(),
            1,
            "hunk and line counts fit one directory batch"
        );
        assert!(
            matches!(
                render(&batches[0]),
                Err(PromptError::OverBudget { kind: "plan", .. })
            ),
            "the batch's prompt is over budget"
        );
        let fitted = fit(batches, &render).expect("fit");
        assert!(fitted.batches.len() > 1);
        assert_eq!(fitted.unfit, Vec::new());
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for batch in &fitted.batches {
            assert!(render(batch).is_ok(), "every batch's prompt fits");
            for hunk in batch.iter().flat_map(|file| &file.hunks) {
                *seen.entry(hunk.id.0.clone()).or_default() += 1;
            }
        }
        assert_eq!(seen.len(), 800);
        assert!(
            seen.values().all(|count| *count == 1),
            "each hunk in exactly one batch"
        );
        assert_eq!(
            fit(plan_batches(&files), &render).expect("again"),
            fitted,
            "deterministic"
        );
    }

    #[test]
    fn a_hunk_that_fits_no_prompt_is_unassigned_with_a_reason() {
        let files = long_path_files().into_iter().take(3).collect::<Vec<_>>();
        let over = |_: &[FileDiff]| -> Result<String, PromptError> {
            Err(PromptError::OverBudget {
                kind: "plan",
                bytes: 1,
                budget: 0,
            })
        };
        let fitted = fit(vec![files], &over).expect("fit");
        assert_eq!(fitted.batches, Vec::<Vec<FileDiff>>::new());
        let unfit: Vec<&str> = fitted
            .unfit
            .iter()
            .map(|hunk| hunk.hunk_id.0.as_str())
            .collect();
        assert_eq!(unfit, ["h0", "h1", "h2"]);
        assert!(fitted.unfit.iter().all(|hunk| hunk.reason == UNFIT_REASON));
    }
}
