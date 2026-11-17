//! Coverage of every changed line by flows, Not analysed items or Unexplained (brief §1 principle 4).

use std::collections::HashMap;

use contracts::common::{FileRanges, LineSide};

use crate::diff::FileDiff;
use crate::verify::VerifiedFlow;
use crate::verify::spans::{self, Span};

/// Result of the coverage check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Changed lines in no flow and no quiet item: "Unexplained — read these yourself".
    pub unexplained: Vec<FileRanges>,
    /// Changed lines covered more than once: by two flows, or by a flow and a quiet item.
    pub double_covered: Vec<FileRanges>,
}

/// The coverage check could not run.
#[derive(Debug, thiserror::Error)]
pub enum CoverageError {
    /// Input ranges refer to a file that is not in the diff.
    #[error("coverage input refers to {path}, which is not in the diff")]
    UnknownFile {
        /// The path.
        path: String,
    },
    /// Declared but not implemented yet.
    #[error("the analysis operation {operation} is not implemented yet")]
    NotImplemented {
        /// The operation.
        operation: &'static str,
    },
}

/// Changed lines not covered by `flows` or the deterministic `quiet` lines (Not analysed, identical moves).
/// Changed lines are added head lines and removed base lines; other lines inside the inputs are ignored. Each
/// input `FileRanges` is one cover (one flow or one quiet item). Output follows the diff's file order, head
/// before base. Interval arithmetic: O((H + C) log (H + C)) for H hunk spans and C input ranges.
pub fn check(
    files: &[FileDiff],
    quiet: &[FileRanges],
    flows: &[VerifiedFlow],
) -> Result<Coverage, CoverageError> {
    let mut order: Vec<(LineSide, &str)> = Vec::new();
    let mut changed: HashMap<(LineSide, &str), Vec<Span>> = HashMap::new();
    for file in files {
        for side in [LineSide::Head, LineSide::Base] {
            let Some(path) = file.path_on(side) else {
                continue;
            };
            let lines = changed.entry((side, path)).or_insert_with(|| {
                order.push((side, path));
                Vec::new()
            });
            for hunk in &file.hunks {
                lines.extend(spans::hunk_spans(hunk, side == LineSide::Head));
            }
        }
    }
    for lines in changed.values_mut() {
        *lines = spans::merge(std::mem::take(lines));
    }
    // Per (side, path): the lines of each cover, merged within the cover.
    let mut layers: HashMap<(LineSide, &str), Vec<Vec<Span>>> = HashMap::new();
    let covers = quiet
        .iter()
        .chain(flows.iter().flat_map(|flow| &flow.covered));
    for ranges in covers {
        let key = (ranges.side, ranges.file.as_str());
        let Some(lines) = changed.get(&key) else {
            return Err(CoverageError::UnknownFile {
                path: ranges.file.clone(),
            });
        };
        let cover = spans::intersect(lines, &spans::from_ranges(&ranges.ranges));
        layers.entry(key).or_default().push(cover);
    }
    let mut coverage = Coverage::default();
    for key in order {
        let lines = changed.get(&key).map_or(&[][..], Vec::as_slice);
        let layers = layers.remove(&key).unwrap_or_default();
        let union = spans::merge(layers.iter().flatten().copied().collect());
        let file_ranges = |found: Vec<Span>| FileRanges {
            file: key.1.to_owned(),
            side: key.0,
            ranges: spans::to_ranges(&found),
        };
        let unexplained = spans::subtract(lines, &union);
        if !unexplained.is_empty() {
            coverage.unexplained.push(file_ranges(unexplained));
        }
        let twice = spans::covered_twice(&layers);
        if !twice.is_empty() {
            coverage.double_covered.push(file_ranges(twice));
        }
    }
    Ok(coverage)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use contracts::agent::analyse_flow::{FlowAnalysisOutput, FlowIntroduction};
    use contracts::api::tour::FlowGraph;
    use contracts::ids::{FlowId, HunkId};
    use proptest::prelude::*;

    use super::*;
    use crate::diff::{FileStatus, Hunk, LineSpan};

    fn flow(id: &str, covered: Vec<FileRanges>) -> VerifiedFlow {
        let text = String::new;
        VerifiedFlow {
            flow_id: FlowId(id.to_owned()),
            output: FlowAnalysisOutput {
                flow_id: FlowId(id.to_owned()),
                introduction: FlowIntroduction {
                    problem_before: text(),
                    change: text(),
                    example_before_after: text(),
                },
                steps: Vec::new(),
                edges: Vec::new(),
                impact_refs: Vec::new(),
                pattern_repetitions: Vec::new(),
                thread_statuses: Vec::new(),
            },
            step_ids: Vec::new(),
            graph: FlowGraph {
                nodes: Vec::new(),
                edges: Vec::new(),
            },
            covered,
            moves: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn file_ranges(file: &str, side: LineSide, lines: &[u32]) -> FileRanges {
        FileRanges {
            file: file.to_owned(),
            side,
            ranges: crate::lines::ranges(lines),
        }
    }

    type Span = ((u32, u32), (u32, u32));

    fn file(path: &str, status: FileStatus, spans: &[Span]) -> FileDiff {
        let hunk = |&((base_start, base_len), (head_start, head_len)): &Span| Hunk {
            id: HunkId(String::new()),
            base: LineSpan {
                start: base_start,
                len: base_len,
            },
            head: LineSpan {
                start: head_start,
                len: head_len,
            },
            added: (head_start..head_start + head_len).collect(),
            removed: (base_start..base_start + base_len).collect(),
        };
        FileDiff {
            path: path.to_owned(),
            old_path: None,
            status,
            binary: false,
            hunks: spans.iter().map(hunk).collect(),
        }
    }

    #[test]
    fn unexplained_and_double_covered_lines_are_reported_per_side() {
        let files = [
            file("src/a.rs", FileStatus::Modified, &[((10, 2), (10, 3))]),
            file("src/gone.rs", FileStatus::Deleted, &[((1, 3), (0, 0))]),
        ];
        let quiet = [file_ranges("src/gone.rs", LineSide::Base, &[1, 2, 3, 4])];
        let flows = [
            flow(
                "f1",
                vec![file_ranges("src/a.rs", LineSide::Head, &[10, 11])],
            ),
            flow(
                "f2",
                vec![
                    file_ranges("src/a.rs", LineSide::Head, &[11]),
                    file_ranges("src/gone.rs", LineSide::Base, &[3]),
                ],
            ),
        ];
        let coverage = check(&files, &quiet, &flows).expect("coverage");
        assert_eq!(
            coverage.unexplained,
            [
                file_ranges("src/a.rs", LineSide::Head, &[12]),
                file_ranges("src/a.rs", LineSide::Base, &[10, 11]),
            ]
        );
        assert_eq!(
            coverage.double_covered,
            [
                file_ranges("src/a.rs", LineSide::Head, &[11]),
                file_ranges("src/gone.rs", LineSide::Base, &[3]),
            ]
        );
    }

    #[test]
    fn inputs_outside_the_diff_are_an_error() {
        let files = [file("src/a.rs", FileStatus::Added, &[((0, 0), (1, 3))])];
        let quiet = [file_ranges("src/a.rs", LineSide::Base, &[1])];
        assert!(matches!(
            check(&files, &quiet, &[]),
            Err(CoverageError::UnknownFile { path }) if path == "src/a.rs"
        ));
    }

    #[test]
    fn inverted_input_ranges_cover_nothing() {
        let files = [file("src/a.rs", FileStatus::Added, &[((0, 0), (1, 3))])];
        let mut quiet = file_ranges("src/a.rs", LineSide::Head, &[2]);
        quiet.ranges[0].start_line = 3;
        let coverage = check(&files, &[quiet], &[]).expect("coverage");
        assert_eq!(
            coverage.unexplained,
            [file_ranges("src/a.rs", LineSide::Head, &[1, 2, 3])]
        );
    }

    /// Up to two files with up to three hunks each, and per changed line a set of covers (bit 0 = quiet,
    /// bits 1-3 = flows f1-f3).
    fn scenario() -> impl Strategy<Value = (Vec<FileDiff>, Vec<u8>)> {
        let hunks = proptest::collection::vec((1u32..4, 0u32..3, 0u32..3), 1..4);
        proptest::collection::vec(hunks, 1..3).prop_flat_map(|files| {
            let diff: Vec<FileDiff> = files
                .iter()
                .enumerate()
                .map(|(index, hunks)| {
                    let (mut base, mut head) = (0, 0);
                    let spans: Vec<Span> = hunks
                        .iter()
                        .map(|&(gap, removed, added)| {
                            let span = ((base + gap, removed), (head + gap, added));
                            base += gap + removed;
                            head += gap + added;
                            span
                        })
                        .collect();
                    file(&format!("src/f{index}.rs"), FileStatus::Modified, &spans)
                })
                .collect();
            let lines = diff
                .iter()
                .flat_map(|file| &file.hunks)
                .map(|hunk| hunk.added.len() + hunk.removed.len())
                .sum::<usize>();
            (Just(diff), proptest::collection::vec(0u8..16, lines))
        })
    }

    proptest! {
        /// Unexplained is exactly the set of changed lines without a cover, double-covered exactly the set with
        /// two or more covers (so the rest is in exactly one flow or quiet item).
        #[test]
        fn proptest_coverage_invariant((files, covers) in scenario()) {
            let changed: Vec<(LineSide, String, u32)> = files
                .iter()
                .flat_map(|file| {
                    file.hunks.iter().flat_map(move |hunk| {
                        let head = hunk.added.iter().map(move |&l| (LineSide::Head, file.path.clone(), l));
                        head.chain(hunk.removed.iter().map(move |&l| (LineSide::Base, file.path.clone(), l)))
                    })
                })
                .collect();
            // cover bit → (side, path) → lines
            let mut sets: BTreeMap<u8, BTreeMap<(LineSide, String), Vec<u32>>> = BTreeMap::new();
            for ((side, path, line), mask) in changed.iter().zip(&covers) {
                for bit in (0..4).filter(|bit| mask & (1 << bit) != 0) {
                    let lines = sets.entry(bit).or_default().entry((*side, path.clone())).or_default();
                    lines.push(*line);
                }
            }
            let as_ranges = |lines: BTreeMap<(LineSide, String), Vec<u32>>| -> Vec<FileRanges> {
                lines.into_iter().map(|((side, file), lines)| file_ranges(&file, side, &lines)).collect()
            };
            let quiet = sets.remove(&0).map(as_ranges).unwrap_or_default();
            let flows: Vec<VerifiedFlow> = sets
                .into_iter()
                .map(|(bit, lines)| flow(&format!("f{bit}"), as_ranges(lines)))
                .collect();

            let coverage = check(&files, &quiet, &flows).expect("coverage");
            let expand = |list: &[FileRanges]| -> BTreeSet<(LineSide, String, u32)> {
                let mut lines = BTreeSet::new();
                for ranges in list {
                    for range in &ranges.ranges {
                        for line in range.start_line..=range.end_line {
                            lines.insert((ranges.side, ranges.file.clone(), line));
                        }
                    }
                }
                lines
            };
            let unexplained = expand(&coverage.unexplained);
            let double = expand(&coverage.double_covered);
            let with = |wanted: fn(u32) -> bool| -> BTreeSet<(LineSide, String, u32)> {
                changed
                    .iter()
                    .zip(&covers)
                    .filter(|(_, mask)| wanted(mask.count_ones()))
                    .map(|(line, _)| line.clone())
                    .collect()
            };
            prop_assert_eq!(unexplained, with(|count| count == 0), "Unexplained = lines without a cover");
            prop_assert_eq!(double, with(|count| count >= 2), "double-covered = lines with two or more covers");
        }
    }
}
