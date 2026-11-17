//! `plan` checks: ids, ranks and references; hunk selections inside their hunks; every changed line of the
//! listed hunks in exactly one flow or `unassigned`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use contracts::agent::plan::PlanOutput;
use contracts::common::LineSide;
use contracts::ids::{FlowId, HunkId};

use super::{
    Issues, VerifiedPlan, VerifyError, VerifyIssueKind, describe_lines, marker_inside,
    to_file_ranges,
};
use crate::diff::{DiffIndex, Hunk};

/// Who a selected line belongs to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Owner {
    Flow(FlowId),
    Unassigned,
}

impl fmt::Display for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Flow(id) => id.fmt(f),
            Self::Unassigned => f.write_str("unassigned"),
        }
    }
}

type Lines = BTreeMap<(LineSide, String), BTreeSet<u32>>;

pub(super) fn plan(output: &PlanOutput, diff: &DiffIndex<'_>) -> Result<VerifiedPlan, VerifyError> {
    let mut issues = Issues::default();
    let mut ids = BTreeSet::new();
    let mut ranks: BTreeMap<u32, &FlowId> = BTreeMap::new();
    for (index, flow) in output.flows.iter().enumerate() {
        let at = format!("flows[{index}] \"{}\"", flow.id);
        if !ids.insert(&flow.id) {
            let message = format!("{at}: flow id {} is used twice", flow.id);
            issues.push(VerifyIssueKind::DuplicateId, message);
        }
        if let Some(other) = ranks.insert(flow.importance_rank, &flow.id) {
            let rank = flow.importance_rank;
            let message = format!("{at}: importance_rank {rank} is also the rank of {other}");
            issues.push(VerifyIssueKind::DuplicateId, message);
        }
        issues.text(&format!("{at}.title"), &flow.title);
        issues.text(&format!("{at}.intent"), &flow.intent);
    }
    for (index, flow) in output.flows.iter().enumerate() {
        for dependency in flow.depends_on.iter().filter(|id| !ids.contains(id)) {
            let at = format!("flows[{index}] \"{}\".depends_on", flow.id);
            let message = format!("{at}: {dependency} is not a flow of the plan");
            issues.push(VerifyIssueKind::UnknownReference, message);
        }
    }

    let mut owners: HashMap<(&HunkId, LineSide, u32), Vec<Owner>> = HashMap::new();
    let flow_selections = output.flows.iter().enumerate().flat_map(|(index, flow)| {
        flow.hunks
            .iter()
            .enumerate()
            .map(move |(number, selection)| {
                let at = format!("flows[{index}] \"{}\".hunks[{number}]", flow.id);
                let range = (selection.head_start, selection.head_end);
                (at, Owner::Flow(flow.id.clone()), &selection.hunk_id, range)
            })
    });
    let unassigned_selections = output.unassigned.iter().enumerate().map(|(index, hunk)| {
        let range = (hunk.head_start, hunk.head_end);
        (
            format!("unassigned[{index}]"),
            Owner::Unassigned,
            &hunk.hunk_id,
            range,
        )
    });
    for (at, owner, id, range) in flow_selections.chain(unassigned_selections) {
        let Some((hunk, head, base)) = select(diff, &mut issues, &at, id, range) else {
            continue;
        };
        let lines = head.into_iter().map(|line| (LineSide::Head, line));
        for (side, line) in lines.chain(base.into_iter().map(|line| (LineSide::Base, line))) {
            owners
                .entry((&hunk.id, side, line))
                .or_default()
                .push(owner.clone());
        }
    }
    for (index, hunk) in output.unassigned.iter().enumerate() {
        issues.text(&format!("unassigned[{index}].reason"), &hunk.reason);
    }

    let mut flow_lines: BTreeMap<FlowId, Lines> = output
        .flows
        .iter()
        .map(|flow| (flow.id.clone(), Lines::new()))
        .collect();
    let mut unassigned = Lines::new();
    for file in diff.files() {
        for hunk in &file.hunks {
            let mut missing: BTreeMap<LineSide, Vec<u32>> = BTreeMap::new();
            let mut twice: BTreeMap<LineSide, Vec<u32>> = BTreeMap::new();
            let mut twice_owners = BTreeSet::new();
            for (side, lines) in [
                (LineSide::Head, &hunk.added),
                (LineSide::Base, &hunk.removed),
            ] {
                let Some(path) = file.path_on(side) else {
                    continue;
                };
                for &line in lines {
                    match owners.get(&(&hunk.id, side, line)).map(Vec::as_slice) {
                        None | Some([]) => missing.entry(side).or_default().push(line),
                        Some([owner]) => {
                            let target = match owner {
                                Owner::Flow(id) => flow_lines.entry(id.clone()).or_default(),
                                Owner::Unassigned => &mut unassigned,
                            };
                            let key = (side, path.to_owned());
                            target.entry(key).or_default().insert(line);
                        }
                        Some(many) => {
                            twice.entry(side).or_default().push(line);
                            twice_owners.extend(many.iter().map(ToString::to_string));
                        }
                    }
                }
            }
            let path = file.path_on(LineSide::Head).unwrap_or(&file.path);
            let at = format!("hunk {} ({path})", hunk.id);
            if !missing.is_empty() {
                let lines = describe_sides(&missing);
                let message = format!("{at}: {lines} are in no flow and not in unassigned");
                issues.push(VerifyIssueKind::LineUnassigned, message);
            }
            if !twice.is_empty() {
                let lines = describe_sides(&twice);
                let by = twice_owners.into_iter().collect::<Vec<_>>().join(", ");
                let message = format!("{at}: {lines} are selected more than once (by {by})");
                issues.push(VerifyIssueKind::LineAssignedTwice, message);
            }
        }
    }
    issues.finish(VerifiedPlan {
        output: output.clone(),
        flow_lines: flow_lines
            .into_iter()
            .map(|(id, lines)| (id, to_file_ranges(lines)))
            .collect(),
        unassigned: to_file_ranges(unassigned),
    })
}

/// The head and base lines a selection takes from its hunk, or `None` with an issue.
fn select<'a>(
    diff: &DiffIndex<'a>,
    issues: &mut Issues,
    at: &str,
    id: &HunkId,
    range: (Option<u32>, Option<u32>),
) -> Option<(&'a Hunk, Vec<u32>, Vec<u32>)> {
    let Some(found) = diff.hunk(id) else {
        let message = format!("{at}: hunk {id} is not in the hunk list");
        issues.push(VerifyIssueKind::UnknownHunk, message);
        return None;
    };
    let hunk = found.hunk;
    let (start, end) = match range {
        (None, None) => return Some((hunk, hunk.added.clone(), hunk.removed.clone())),
        (Some(start), Some(end)) => (start, end),
        _ => {
            let message = format!("{at}: give both head_start and head_end, or neither");
            issues.push(VerifyIssueKind::RangeOutsideHunk, message);
            return None;
        }
    };
    let first = hunk.head.start;
    let last = first.saturating_add(hunk.head.len).saturating_sub(1);
    if hunk.head.len == 0 {
        let message = format!(
            "{at}: hunk {id} only removes lines; select it whole (head_start and head_end null)"
        );
        issues.push(VerifyIssueKind::RangeOutsideHunk, message);
        return None;
    }
    if start > end || start < first || end > last {
        let message = format!(
            "{at}: head lines {start}-{end} are outside hunk {id} (head lines {first}-{last})"
        );
        issues.push(VerifyIssueKind::RangeOutsideHunk, message);
        return None;
    }
    let head = hunk
        .added
        .iter()
        .copied()
        .filter(|line| (start..=end).contains(line));
    // Removed lines sit at the marker after the head line before the hunk.
    let marker = first.saturating_sub(1);
    let base = if marker_inside(marker, start, end) {
        hunk.removed.clone()
    } else {
        Vec::new()
    };
    Some((hunk, head.collect(), base))
}

/// `head lines 3-5; base lines 7` for lines per side.
fn describe_sides(lines: &BTreeMap<LineSide, Vec<u32>>) -> String {
    lines
        .iter()
        .map(|(side, lines)| {
            format!(
                "{} lines {}",
                super::side_name(*side),
                describe_lines(lines)
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use contracts::agent::plan::{HunkSelection, PlannedFlow, UnassignedHunk};
    use contracts::common::{Classification, FileRanges, LineRange};
    use proptest::prelude::*;

    use super::*;
    use crate::diff::{FileDiff, FileStatus, LineSpan};
    use crate::verify::VerifyIssue;

    fn hunk(id: &str, base: (u32, u32), head: (u32, u32)) -> Hunk {
        let base = LineSpan {
            start: base.0,
            len: base.1,
        };
        let head = LineSpan {
            start: head.0,
            len: head.1,
        };
        Hunk {
            id: HunkId(id.to_owned()),
            base,
            head,
            added: (head.start..head.start + head.len).collect(),
            removed: (base.start..base.start + base.len).collect(),
        }
    }

    fn files() -> Vec<FileDiff> {
        let file = |path: &str, old: Option<&str>, status, hunks| FileDiff {
            path: path.to_owned(),
            old_path: old.map(str::to_owned),
            status,
            binary: false,
            hunks,
        };
        vec![
            // Modified in place: base 10-11 → head 10-12; pure addition after 20; pure deletion of base 30-31.
            file(
                "src/a.rs",
                None,
                FileStatus::Modified,
                vec![
                    hunk("h1", (10, 2), (10, 3)),
                    hunk("h2", (20, 0), (21, 2)),
                    hunk("h3", (30, 2), (31, 0)),
                ],
            ),
            // Same line numbers in another file.
            file(
                "src/b.rs",
                None,
                FileStatus::Modified,
                vec![hunk("h4", (10, 2), (10, 3))],
            ),
            file(
                "src/new.rs",
                Some("src/old.rs"),
                FileStatus::Renamed,
                vec![hunk("h5", (5, 1), (5, 1))],
            ),
        ]
    }

    fn whole(id: &str) -> HunkSelection {
        HunkSelection {
            hunk_id: HunkId(id.to_owned()),
            head_start: None,
            head_end: None,
        }
    }

    fn part(id: &str, start: u32, end: u32) -> HunkSelection {
        HunkSelection {
            hunk_id: HunkId(id.to_owned()),
            head_start: Some(start),
            head_end: Some(end),
        }
    }

    fn flow(id: &str, rank: u32, hunks: Vec<HunkSelection>) -> PlannedFlow {
        PlannedFlow {
            id: FlowId(id.to_owned()),
            title: format!("Flow {id}"),
            classification: Classification::Behavioural,
            importance_rank: rank,
            intent: "Do the thing.".to_owned(),
            hunks,
            depends_on: Vec::new(),
        }
    }

    fn unassigned(selection: HunkSelection) -> UnassignedHunk {
        UnassignedHunk {
            hunk_id: selection.hunk_id,
            head_start: selection.head_start,
            head_end: selection.head_end,
            reason: "Unused helper.".to_owned(),
        }
    }

    fn ranges(file: &str, side: LineSide, ranges: &[(u32, u32)]) -> FileRanges {
        FileRanges {
            file: file.to_owned(),
            side,
            ranges: ranges
                .iter()
                .map(|&(start_line, end_line)| LineRange {
                    start_line,
                    end_line,
                })
                .collect(),
        }
    }

    fn run(output: &PlanOutput) -> Result<VerifiedPlan, Vec<VerifyIssue>> {
        let files = files();
        let diff = DiffIndex::new(&files).expect("index");
        match plan(output, &diff) {
            Ok(verified) => Ok(verified),
            Err(VerifyError::Rejected(issues)) => Err(issues),
            Err(other) => panic!("{other:?}"),
        }
    }

    fn kinds(issues: &[VerifyIssue]) -> Vec<VerifyIssueKind> {
        issues.iter().map(|issue| issue.kind).collect()
    }

    #[test]
    fn plan_resolves_selections_to_lines_on_both_sides() {
        let output = PlanOutput {
            flows: vec![
                flow("f1", 1, vec![part("h1", 10, 11), whole("h3"), whole("h5")]),
                flow("f2", 2, vec![part("h1", 12, 12), whole("h4")]),
            ],
            unassigned: vec![unassigned(whole("h2"))],
        };
        let verified = run(&output).expect("accepted");
        assert_eq!(
            verified.flow_lines[&FlowId("f1".to_owned())],
            [
                ranges("src/a.rs", LineSide::Head, &[(10, 11)]),
                ranges("src/new.rs", LineSide::Head, &[(5, 5)]),
                ranges("src/a.rs", LineSide::Base, &[(10, 11), (30, 31)]),
                ranges("src/old.rs", LineSide::Base, &[(5, 5)]),
            ],
            "removed lines go with the sub-range that holds their marker; renamed files keep both paths"
        );
        assert_eq!(
            verified.flow_lines[&FlowId("f2".to_owned())],
            [
                ranges("src/a.rs", LineSide::Head, &[(12, 12)]),
                ranges("src/b.rs", LineSide::Head, &[(10, 12)]),
                ranges("src/b.rs", LineSide::Base, &[(10, 11)]),
            ]
        );
        assert_eq!(
            verified.unassigned,
            [ranges("src/a.rs", LineSide::Head, &[(21, 22)])]
        );
    }

    #[test]
    fn plan_rejects_unknown_hunks_and_ranges_outside_their_hunk() {
        let mut half = part("h2", 21, 21);
        half.head_end = None;
        let output = PlanOutput {
            flows: vec![flow(
                "f1",
                1,
                vec![
                    whole("h1"),
                    whole("h99"),
                    part("h2", 20, 22),
                    part("h3", 31, 31),
                    half,
                    whole("h4"),
                    whole("h5"),
                ],
            )],
            unassigned: vec![unassigned(whole("h2"))],
        };
        let issues = run(&output).expect_err("rejected");
        assert_eq!(
            kinds(&issues),
            [
                VerifyIssueKind::UnknownHunk,
                VerifyIssueKind::RangeOutsideHunk,
                VerifyIssueKind::RangeOutsideHunk,
                VerifyIssueKind::RangeOutsideHunk,
                VerifyIssueKind::LineUnassigned,
            ],
            "{issues:#?}"
        );
        assert!(
            issues[1]
                .message
                .contains("head lines 20-22 are outside hunk h2 (head lines 21-22)")
        );
        assert!(issues[2].message.contains("only removes lines"));
        assert!(
            issues[4]
                .message
                .starts_with("hunk h3 (src/a.rs): base lines 30-31")
        );
    }

    #[test]
    fn plan_rejects_lines_in_no_flow_or_in_two() {
        let output = PlanOutput {
            flows: vec![
                flow("f1", 1, vec![part("h1", 10, 11), whole("h4")]),
                flow("f2", 2, vec![whole("h4"), whole("h5")]),
            ],
            unassigned: vec![unassigned(whole("h3"))],
        };
        let issues = run(&output).expect_err("rejected");
        assert_eq!(
            issues,
            [
                VerifyIssue {
                    kind: VerifyIssueKind::LineUnassigned,
                    message: "hunk h1 (src/a.rs): head lines 12 are in no flow and not in unassigned"
                        .to_owned(),
                },
                VerifyIssue {
                    kind: VerifyIssueKind::LineUnassigned,
                    message: "hunk h2 (src/a.rs): head lines 21-22 are in no flow and not in unassigned"
                        .to_owned(),
                },
                VerifyIssue {
                    kind: VerifyIssueKind::LineAssignedTwice,
                    message:
                        "hunk h4 (src/b.rs): head lines 10-12; base lines 10-11 are selected more than once (by f1, f2)"
                            .to_owned(),
                },
            ]
        );
    }

    #[test]
    fn plan_rejects_duplicate_ids_ranks_unknown_dependencies_and_empty_texts() {
        let mut second = flow("f1", 1, vec![whole("h4")]);
        second.depends_on = vec![FlowId("f9".to_owned())];
        second.title = " ".to_owned();
        let mut reason = unassigned(whole("h2"));
        reason.reason = String::new();
        let output = PlanOutput {
            flows: vec![
                flow("f1", 1, vec![whole("h1"), whole("h3"), whole("h5")]),
                second,
            ],
            unassigned: vec![reason],
        };
        let issues = run(&output).expect_err("rejected");
        assert_eq!(
            kinds(&issues),
            [
                VerifyIssueKind::DuplicateId,
                VerifyIssueKind::DuplicateId,
                VerifyIssueKind::EmptyText,
                VerifyIssueKind::UnknownReference,
                VerifyIssueKind::EmptyText,
            ],
            "{issues:#?}"
        );
    }

    proptest! {
        /// Whole-hunk plans: accepted exactly when every hunk is selected once; then the flows and `unassigned`
        /// partition the changed lines.
        #[test]
        fn plan_partition_matches_the_selections(owners in proptest::collection::vec(
            proptest::collection::vec(0usize..3, 0..3), 5,
        )) {
            // owners[h] = owners of hunk h: 0, 1 = flows f1, f2; 2 = unassigned.
            let ids = ["h1", "h2", "h3", "h4", "h5"];
            let mut flows = vec![flow("f1", 1, Vec::new()), flow("f2", 2, Vec::new())];
            let mut unassigned_hunks = Vec::new();
            for (hunk, hunk_owners) in ids.iter().zip(&owners) {
                for &owner in hunk_owners {
                    match owner {
                        2 => unassigned_hunks.push(unassigned(whole(hunk))),
                        flow => flows[flow].hunks.push(whole(hunk)),
                    }
                }
            }
            let output = PlanOutput { flows, unassigned: unassigned_hunks };
            let result = run(&output);
            prop_assert_eq!(result.is_ok(), owners.iter().all(|hunk| hunk.len() == 1), "{:?}", result);
            if let Ok(verified) = result {
                let mut seen: Vec<(String, LineSide, u32)> = Vec::new();
                let all = verified.flow_lines.values().flatten().chain(&verified.unassigned);
                for ranges in all {
                    for range in &ranges.ranges {
                        for line in range.start_line..=range.end_line {
                            seen.push((ranges.file.clone(), ranges.side, line));
                        }
                    }
                }
                seen.sort();
                let mut changed: Vec<(String, LineSide, u32)> = Vec::new();
                for file in files() {
                    for hunk in &file.hunks {
                        for (side, lines) in [(LineSide::Head, &hunk.added), (LineSide::Base, &hunk.removed)] {
                            let path = file.path_on(side).expect("path").to_owned();
                            changed.extend(lines.iter().map(|&line| (path.clone(), side, line)));
                        }
                    }
                }
                changed.sort();
                prop_assert_eq!(seen, changed);
            }
        }
    }
}
