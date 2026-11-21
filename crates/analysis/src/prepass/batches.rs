//! Huge pull requests (brief §5 Pipeline 3): the agent's hunks split into directory batches, one `plan` job per
//! batch, and the batch plans merged into one plan.

use std::collections::BTreeMap;

use contracts::agent::plan::{PlanOutput, PlannedFlow};
use contracts::ids::FlowId;

use crate::diff::FileDiff;

/// Most changed lines (added plus removed) one `plan` job is given.
pub const PLAN_BATCH_LINES: u32 = 4000;
/// Most hunks one `plan` job is given (the hunk listing must fit the prompt budget).
pub const PLAN_BATCH_HUNKS: usize = 800;

/// The agent's `files` in plan batches of at most [`PLAN_BATCH_LINES`] changed lines and [`PLAN_BATCH_HUNKS`]
/// hunks, in path order. A directory stays in one batch when it fits; a larger directory is split by files and a
/// larger file by hunks. One batch when everything fits.
pub fn plan_batches(files: &[FileDiff]) -> Vec<Vec<FileDiff>> {
    let mut batches: Vec<Vec<FileDiff>> = Vec::new();
    let mut current = Batch::default();
    for group in directory_groups(files) {
        let size = Size::of(group);
        if !current.fits(size) {
            current.close(&mut batches);
        }
        if current.fits(size) {
            current.files.extend(group.iter().cloned());
            current.size = current.size.plus(size);
            continue;
        }
        // Larger than a whole batch: by files, then by hunks.
        for file in group {
            for part in file_parts(file) {
                let size = Size::of(std::slice::from_ref(&part));
                if !current.fits(size) {
                    current.close(&mut batches);
                }
                current.files.push(part);
                current.size = current.size.plus(size);
            }
        }
    }
    current.close(&mut batches);
    batches
}

/// One plan whose flows are the batch plans' flows, renumbered `f1…` in batch order, with `depends_on` renamed
/// within each batch and `importance_rank` interleaved (every batch's first flow, then every second, …);
/// `unassigned` is concatenated, and so are `crate_notes` (null only when every batch has none).
pub fn merge_plans(batches: &[PlanOutput]) -> PlanOutput {
    let mut flows: Vec<(u32, usize, PlannedFlow)> = Vec::new();
    let mut unassigned = Vec::new();
    let mut crate_notes: Option<Vec<_>> = None;
    let mut next = 1u32;
    for (batch, plan) in batches.iter().enumerate() {
        let renamed: BTreeMap<&FlowId, FlowId> = plan
            .flows
            .iter()
            .map(|flow| {
                let id = FlowId(format!("f{next}"));
                next += 1;
                (&flow.id, id)
            })
            .collect();
        for flow in &plan.flows {
            let mut merged = flow.clone();
            merged.id = renamed
                .get(&flow.id)
                .cloned()
                .unwrap_or_else(|| flow.id.clone());
            merged.depends_on = flow
                .depends_on
                .iter()
                .map(|id| renamed.get(id).cloned().unwrap_or_else(|| id.clone()))
                .collect();
            flows.push((flow.importance_rank, batch, merged));
        }
        unassigned.extend(plan.unassigned.iter().cloned());
        if let Some(notes) = &plan.crate_notes {
            crate_notes
                .get_or_insert_with(Vec::new)
                .extend(notes.iter().cloned());
        }
    }
    let mut ranked: Vec<usize> = (0..flows.len()).collect();
    ranked.sort_by_key(|&index| (flows[index].0, flows[index].1));
    for (rank, index) in (1u32..).zip(ranked) {
        flows[index].2.importance_rank = rank;
    }
    PlanOutput {
        flows: flows.into_iter().map(|(_, _, flow)| flow).collect(),
        unassigned,
        crate_notes,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Size {
    lines: u32,
    hunks: usize,
}

impl Size {
    fn of(files: &[FileDiff]) -> Self {
        let hunks = files.iter().flat_map(|file| &file.hunks);
        hunks.fold(Self::default(), |size, hunk| Self {
            lines: size.lines.saturating_add(
                u32::try_from(hunk.added.len() + hunk.removed.len()).unwrap_or(u32::MAX),
            ),
            hunks: size.hunks + 1,
        })
    }

    fn plus(self, other: Self) -> Self {
        Self {
            lines: self.lines.saturating_add(other.lines),
            hunks: self.hunks + other.hunks,
        }
    }
}

#[derive(Debug, Default)]
struct Batch {
    files: Vec<FileDiff>,
    size: Size,
}

impl Batch {
    fn fits(&self, size: Size) -> bool {
        let total = self.size.plus(size);
        total.lines <= PLAN_BATCH_LINES && total.hunks <= PLAN_BATCH_HUNKS
    }

    fn close(&mut self, batches: &mut Vec<Vec<FileDiff>>) {
        if !self.files.is_empty() {
            batches.push(std::mem::take(&mut self.files));
        }
        self.size = Size::default();
    }
}

/// Consecutive files of the same parent directory (files are in path order).
fn directory_groups(files: &[FileDiff]) -> Vec<&[FileDiff]> {
    let directory = |file: &FileDiff| {
        file.path
            .rsplit_once('/')
            .map_or("", |(dir, _)| dir)
            .to_owned()
    };
    let mut groups = Vec::new();
    let mut start = 0;
    for index in 1..=files.len() {
        if index == files.len() || directory(&files[index]) != directory(&files[start]) {
            if start < index {
                groups.push(&files[start..index]);
            }
            start = index;
        }
    }
    groups
}

/// A file whole, or split into single-hunk parts when it alone is larger than a batch.
fn file_parts(file: &FileDiff) -> Vec<FileDiff> {
    let size = Size::of(std::slice::from_ref(file));
    if size.lines <= PLAN_BATCH_LINES && size.hunks <= PLAN_BATCH_HUNKS {
        return vec![file.clone()];
    }
    file.hunks
        .iter()
        .map(|hunk| FileDiff {
            hunks: vec![hunk.clone()],
            ..file.clone()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use contracts::agent::plan::{HunkSelection, UnassignedHunk};
    use contracts::common::Classification;
    use contracts::ids::HunkId;

    use super::*;
    use crate::diff::{FileStatus, Hunk, LineSpan};

    fn file(path: &str, first: u32, hunks: u32, lines_per_hunk: u32) -> FileDiff {
        FileDiff {
            path: path.to_owned(),
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            hunks: (0..hunks)
                .map(|index| {
                    let start = 1 + index * (lines_per_hunk + 1);
                    Hunk {
                        id: HunkId(format!("h{}", first + index)),
                        base: LineSpan { start, len: 0 },
                        head: LineSpan {
                            start,
                            len: lines_per_hunk,
                        },
                        added: (start..start + lines_per_hunk).collect(),
                        removed: Vec::new(),
                    }
                })
                .collect(),
        }
    }

    fn paths(batches: &[Vec<FileDiff>]) -> Vec<Vec<String>> {
        batches
            .iter()
            .map(|batch| {
                batch
                    .iter()
                    .map(|file| format!("{}:{}", file.path, file.hunks.len()))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn small_pr_is_one_batch() {
        let files = [file("a/x.rs", 1, 2, 10), file("b/y.rs", 3, 1, 10)];
        assert_eq!(plan_batches(&files).len(), 1);
        assert!(plan_batches(&[]).is_empty());
    }

    #[test]
    fn huge_pr_plan_splits_by_directory() {
        let half = PLAN_BATCH_LINES / 2 + 100;
        let files = [
            file("a/one.rs", 1, 1, half / 2),
            file("a/two.rs", 2, 1, half / 2),
            file("b/three.rs", 3, 1, half),
            file("c/four.rs", 4, 1, 50),
        ];
        assert_eq!(
            paths(&plan_batches(&files)),
            [
                vec!["a/one.rs:1".to_owned(), "a/two.rs:1".to_owned()],
                vec!["b/three.rs:1".to_owned(), "c/four.rs:1".to_owned()],
            ],
            "a directory stays whole; the next directory starts a new batch when it does not fit"
        );
    }

    #[test]
    fn a_directory_or_file_larger_than_a_batch_is_split() {
        let files = [file("big/huge.rs", 1, 3, PLAN_BATCH_LINES / 2 + 1)];
        let batches = plan_batches(&files);
        assert_eq!(batches.len(), 3, "{:?}", paths(&batches));
        let many = [file("many/hunks.rs", 1, 1000, 1)];
        let batches = plan_batches(&many);
        assert_eq!(
            batches
                .iter()
                .map(|batch| Size::of(batch).hunks)
                .collect::<Vec<_>>(),
            [PLAN_BATCH_HUNKS, 200]
        );
    }

    fn flow(id: &str, rank: u32, hunk: &str, depends_on: &[&str]) -> PlannedFlow {
        PlannedFlow {
            id: FlowId(id.to_owned()),
            title: id.to_owned(),
            classification: Classification::Behavioural,
            importance_rank: rank,
            intent: "x".to_owned(),
            hunks: vec![HunkSelection {
                hunk_id: HunkId(hunk.to_owned()),
                head_start: None,
                head_end: None,
            }],
            depends_on: depends_on
                .iter()
                .map(|id| FlowId((*id).to_owned()))
                .collect(),
        }
    }

    #[test]
    fn batch_plans_merge_with_new_ids_and_interleaved_ranks() {
        let first = PlanOutput {
            flows: vec![flow("f1", 2, "h1", &["f2"]), flow("f2", 1, "h2", &[])],
            unassigned: vec![UnassignedHunk {
                hunk_id: HunkId("h3".to_owned()),
                head_start: None,
                head_end: None,
                reason: "r".to_owned(),
            }],
            crate_notes: None,
        };
        let second = PlanOutput {
            flows: vec![flow("f1", 1, "h9", &[])],
            unassigned: Vec::new(),
            crate_notes: None,
        };
        let merged = merge_plans(&[first, second]);
        let summary: Vec<(String, u32, Vec<String>)> = merged
            .flows
            .iter()
            .map(|flow| {
                (
                    flow.id.0.clone(),
                    flow.importance_rank,
                    flow.depends_on.iter().map(|id| id.0.clone()).collect(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("f1".to_owned(), 3, vec!["f2".to_owned()]),
                ("f2".to_owned(), 1, vec![]),
                ("f3".to_owned(), 2, vec![]),
            ]
        );
        assert_eq!(merged.flows[2].hunks[0].hunk_id, HunkId("h9".to_owned()));
        assert_eq!(merged.unassigned.len(), 1);
    }

    #[test]
    fn merged_plans_concatenate_crate_notes() {
        let plan = |notes: Option<Vec<&str>>| PlanOutput {
            flows: Vec::new(),
            unassigned: Vec::new(),
            crate_notes: notes.map(|list| {
                list.into_iter()
                    .map(|text| contracts::agent::plan::PlanCrateNote {
                        note: text.to_owned(),
                        hunks: vec![HunkId("h1".to_owned())],
                    })
                    .collect()
            }),
        };
        assert_eq!(merge_plans(&[plan(None), plan(None)]).crate_notes, None);
        let merged = merge_plans(&[
            plan(Some(vec!["a"])),
            plan(None),
            plan(Some(vec!["b", "c"])),
        ]);
        let notes: Vec<String> = merged
            .crate_notes
            .expect("notes")
            .into_iter()
            .map(|n| n.note)
            .collect();
        assert_eq!(notes, ["a", "b", "c"]);
    }
}
