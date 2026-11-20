//! Material the server did not send to the LLM, stored with its reason (`DocumentKind::Unsent`): hunks of files
//! too large for any plan prompt (stored with the plan) and flows whose prompt is over budget. The Tour shows
//! these Unexplained lines with the reason; it is a deterministic fact, never an agent's claim.

use std::collections::{BTreeMap, BTreeSet};

use analysis::diff::FileDiff;
use contracts::agent::plan::{PlanOutput, UnassignedHunk};
use contracts::api::tour::{UnexplainedRanges, UnexplainedReason};
use contracts::common::{FileRanges, LineRange, LineSide};
use serde::{Deserialize, Serialize};

use super::verdict::{self, VerdictError};

/// Document key of the hunks left out of every plan prompt; a flow's key is its id.
pub const PLAN_KEY: &str = "plan";

/// Lines the server did not send to the LLM, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unsent {
    /// Why.
    pub reason: UnexplainedReason,
    /// The lines (head lines and the removed base lines that go with them).
    pub lines: Vec<FileRanges>,
}

/// The lines of `unfit` hunks of `listed` files, as plan verification resolves them.
pub fn unfit_lines(
    listed: &[FileDiff],
    unfit: &[UnassignedHunk],
) -> Result<Vec<FileRanges>, VerdictError> {
    let ids: BTreeSet<_> = unfit.iter().map(|hunk| &hunk.hunk_id).collect();
    let files: Vec<FileDiff> = listed
        .iter()
        .filter_map(|file| {
            let hunks: Vec<_> = file
                .hunks
                .iter()
                .filter(|hunk| ids.contains(&hunk.id))
                .cloned()
                .collect();
            (!hunks.is_empty()).then(|| FileDiff {
                hunks,
                ..file.clone()
            })
        })
        .collect();
    let plan = PlanOutput {
        flows: Vec::new(),
        unassigned: unfit.to_vec(),
        crate_notes: None,
    };
    Ok(match verdict::plan(&plan, &files)? {
        Ok(verified) => verified.unassigned,
        Err(issues) => {
            tracing::error!(
                issues = issues.len(),
                "the unsent hunks do not resolve to lines; their reason is not shown"
            );
            Vec::new()
        }
    })
}

/// `unexplained` with the reason of the lines the server did not send: a file's ranges are split into the
/// ones without a reason (first) and one entry per reason.
pub fn with_reasons(unexplained: &[FileRanges], unsent: &[Unsent]) -> Vec<UnexplainedRanges> {
    let mut held: BTreeMap<(&str, LineSide), Vec<(UnexplainedReason, LineRange)>> = BTreeMap::new();
    for entry in unsent {
        for file in &entry.lines {
            let ranges = held.entry((file.file.as_str(), file.side)).or_default();
            ranges.extend(file.ranges.iter().map(|range| (entry.reason, *range)));
        }
    }
    let mut out = Vec::with_capacity(unexplained.len());
    for file in unexplained {
        let held = held
            .get(&(file.file.as_str(), file.side))
            .map_or(&[][..], Vec::as_slice);
        let mut plain = Vec::new();
        let mut reasons: BTreeMap<UnexplainedReason, Vec<LineRange>> = BTreeMap::new();
        for range in &file.ranges {
            let mut rest = vec![*range];
            for (reason, unsent) in held {
                let mut next = Vec::with_capacity(rest.len());
                for part in rest {
                    let (inside, outside) = split(part, *unsent);
                    reasons.entry(*reason).or_default().extend(inside);
                    next.extend(outside);
                }
                rest = next;
            }
            plain.extend(rest);
        }
        let entry = |ranges: Vec<LineRange>, reason| UnexplainedRanges {
            file: file.file.clone(),
            side: file.side,
            ranges,
            reason,
        };
        if !plain.is_empty() {
            out.push(entry(plain, None));
        }
        for (reason, mut ranges) in reasons {
            ranges.sort_by_key(|range| range.start_line);
            out.push(entry(ranges, Some(reason)));
        }
    }
    out
}

/// The part of `range` inside `by`, and the parts outside it (in order).
fn split(range: LineRange, by: LineRange) -> (Option<LineRange>, Vec<LineRange>) {
    let start = range.start_line.max(by.start_line);
    let end = range.end_line.min(by.end_line);
    if start > end {
        return (None, vec![range]);
    }
    let mut outside = Vec::new();
    if range.start_line < start {
        outside.push(LineRange {
            start_line: range.start_line,
            end_line: start - 1,
        });
    }
    if end < range.end_line {
        outside.push(LineRange {
            start_line: end + 1,
            end_line: range.end_line,
        });
    }
    (
        Some(LineRange {
            start_line: start,
            end_line: end,
        }),
        outside,
    )
}

#[cfg(test)]
mod tests {
    use analysis::diff::{FileStatus, Hunk, LineSpan};
    use contracts::ids::HunkId;

    use super::*;

    fn range(start_line: u32, end_line: u32) -> LineRange {
        LineRange {
            start_line,
            end_line,
        }
    }

    fn ranges(file: &str, ranges: &[(u32, u32)]) -> FileRanges {
        FileRanges {
            file: file.to_owned(),
            side: LineSide::Head,
            ranges: ranges.iter().map(|(a, b)| range(*a, *b)).collect(),
        }
    }

    #[test]
    fn unfit_hunks_resolve_to_their_lines() {
        let file = |path: &str, id: &str, start: u32, len: u32| FileDiff {
            path: path.to_owned(),
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            hunks: vec![Hunk {
                id: HunkId(id.to_owned()),
                base: LineSpan { start: 0, len: 0 },
                head: LineSpan { start, len },
                added: (start..start + len).collect(),
                removed: Vec::new(),
            }],
        };
        let listed = vec![file("a.rs", "h1", 3, 2), file("b.rs", "h2", 10, 4)];
        let unfit = vec![UnassignedHunk {
            hunk_id: HunkId("h2".to_owned()),
            head_start: None,
            head_end: None,
            reason: "too large".to_owned(),
        }];
        assert_eq!(
            unfit_lines(&listed, &unfit).expect("lines"),
            vec![ranges("b.rs", &[(10, 13)])]
        );
    }

    #[test]
    fn unexplained_ranges_carry_the_reason_of_unsent_lines_only() {
        let unexplained = vec![
            ranges("a.rs", &[(1, 10), (20, 22)]),
            ranges("b.rs", &[(1, 2)]),
        ];
        let unsent = vec![Unsent {
            reason: UnexplainedReason::TooLargeForPrompt,
            lines: vec![ranges("a.rs", &[(4, 6), (20, 22)])],
        }];
        let entry = |file: &str, ranges: &[(u32, u32)], reason| UnexplainedRanges {
            file: file.to_owned(),
            side: LineSide::Head,
            ranges: ranges.iter().map(|(a, b)| range(*a, *b)).collect(),
            reason,
        };
        assert_eq!(
            with_reasons(&unexplained, &unsent),
            vec![
                entry("a.rs", &[(1, 3), (7, 10)], None),
                entry(
                    "a.rs",
                    &[(4, 6), (20, 22)],
                    Some(UnexplainedReason::TooLargeForPrompt)
                ),
                entry("b.rs", &[(1, 2)], None),
            ]
        );
    }
}
