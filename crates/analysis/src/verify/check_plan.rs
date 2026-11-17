//! `plan` checks: ids, ranks and references; hunk selections inside their hunks; every changed line of the
//! listed hunks in exactly one flow or `unassigned`. Selections are accounted as intervals per hunk
//! (O(H + R log R) for H hunks and R selections), so repeated selections of a large hunk cost no extra memory.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use contracts::agent::plan::PlanOutput;
use contracts::common::LineSide;
use contracts::ids::{FlowId, HunkId};

use super::spans::{self, Span};
use super::{Issues, VerifiedPlan, VerifyError, VerifyIssueKind, side_name, to_file_ranges};
use crate::diff::{DiffIndex, Hunk};

/// Owners named in one conflict message at most.
const MAX_NAMED_OWNERS: usize = 5;

/// A valid selection of a hunk: its owner (a flow index, or the flow count for `unassigned`), its head lines
/// and whether it takes the hunk's removed lines.
struct Selection {
    owner: usize,
    head: Option<Span>,
    removed: bool,
}

type Lines = BTreeMap<(LineSide, String), Vec<Span>>;

pub(super) fn plan(output: &PlanOutput, diff: &DiffIndex<'_>) -> Result<VerifiedPlan, VerifyError> {
    let mut issues = Issues::default();
    let mut ids = BTreeSet::new();
    let mut ranks: BTreeMap<u32, &FlowId> = BTreeMap::new();
    for (index, flow) in output.flows.iter().enumerate() {
        let at = format!("flows[{index}] \"{}\"", flow.id);
        if flow.classification == contracts::common::Classification::Test
            || (!flow.hunks.is_empty()
                && flow.hunks.iter().all(|selection| {
                    diff.hunk(&selection.hunk_id)
                        .is_some_and(|at| crate::tech::is_test_path(&at.file.path))
                }))
        {
            issues.push(
                VerifyIssueKind::UnknownReference,
                format!("{at}: test-only flows are outside the review scope; remove this flow"),
            );
        }
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

    let unassigned_owner = output.flows.len();
    let owner_name = |owner: usize| {
        output
            .flows
            .get(owner)
            .map_or_else(|| "unassigned".to_owned(), |flow| flow.id.to_string())
    };
    let mut selections: HashMap<&HunkId, Vec<Selection>> = HashMap::new();
    let flow_selections = output.flows.iter().enumerate().flat_map(|(owner, flow)| {
        flow.hunks
            .iter()
            .enumerate()
            .map(move |(number, selection)| {
                let at = format!("flows[{owner}] \"{}\".hunks[{number}]", flow.id);
                let range = (selection.head_start, selection.head_end);
                (at, owner, &selection.hunk_id, range)
            })
    });
    let unassigned_selections = output.unassigned.iter().enumerate().map(|(index, hunk)| {
        let range = (hunk.head_start, hunk.head_end);
        let at = format!("unassigned[{index}]");
        (at, unassigned_owner, &hunk.hunk_id, range)
    });
    for (at, owner, id, range) in flow_selections.chain(unassigned_selections) {
        if let Some((hunk, selection)) = select(diff, &mut issues, &at, id, range, owner) {
            selections.entry(&hunk.id).or_default().push(selection);
        }
    }
    for (index, hunk) in output.unassigned.iter().enumerate() {
        issues.text(&format!("unassigned[{index}].reason"), &hunk.reason);
    }

    let mut owned: Vec<Lines> = vec![Lines::new(); unassigned_owner + 1];
    for file in diff.files() {
        for hunk in &file.hunks {
            let chosen = selections.get(&hunk.id).map_or(&[][..], Vec::as_slice);
            let mut missing: BTreeMap<LineSide, Vec<Span>> = BTreeMap::new();
            let mut twice: BTreeMap<LineSide, Vec<Span>> = BTreeMap::new();
            let mut named = BTreeSet::new();
            if let Some(path) = file.path_on(LineSide::Head) {
                let changed = spans::hunk_spans(hunk, true);
                let picked: Vec<(usize, Span)> = chosen
                    .iter()
                    .filter_map(|s| s.head.map(|span| (s.owner, span)))
                    .collect();
                let union = spans::merge(picked.iter().map(|&(_, span)| span).collect());
                let gaps = spans::subtract(&changed, &union);
                if !gaps.is_empty() {
                    missing.insert(LineSide::Head, gaps);
                }
                for (span, owner) in layers(&picked, &mut named) {
                    let lines = spans::intersect(&changed, &[span]);
                    match owner {
                        Some(owner) => {
                            let key = (LineSide::Head, path.to_owned());
                            owned[owner].entry(key).or_default().extend(lines);
                        }
                        None => twice.entry(LineSide::Head).or_default().extend(lines),
                    }
                }
            }
            if let Some(path) = file.path_on(LineSide::Base) {
                let removed = spans::hunk_spans(hunk, false);
                let takers: Vec<usize> = chosen
                    .iter()
                    .filter(|s| s.removed)
                    .map(|s| s.owner)
                    .collect();
                match takers.as_slice() {
                    _ if removed.is_empty() => {}
                    [] => {
                        missing.insert(LineSide::Base, removed);
                    }
                    [owner] => {
                        let key = (LineSide::Base, path.to_owned());
                        owned[*owner].entry(key).or_default().extend(removed);
                    }
                    many => {
                        named.extend(many.iter().take(MAX_NAMED_OWNERS + 1));
                        twice.insert(LineSide::Base, removed);
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
            twice.retain(|_, lines| !lines.is_empty());
            if !twice.is_empty() {
                let lines = describe_sides(&twice);
                let mut by: Vec<String> = named
                    .iter()
                    .take(MAX_NAMED_OWNERS)
                    .map(|&o| owner_name(o))
                    .collect();
                if named.len() > MAX_NAMED_OWNERS {
                    by.push("and others".to_owned());
                }
                let by = by.join(", ");
                let message = format!("{at}: {lines} are selected more than once (by {by})");
                issues.push(VerifyIssueKind::LineAssignedTwice, message);
            }
        }
    }
    let unassigned = owned.pop().unwrap_or_default();
    let mut flow_lines = BTreeMap::new();
    for (flow, lines) in output.flows.iter().zip(owned) {
        flow_lines
            .entry(flow.id.clone())
            .or_insert_with(|| to_file_ranges(lines));
    }
    issues.finish(VerifiedPlan {
        output: output.clone(),
        flow_lines,
        unassigned: to_file_ranges(unassigned),
    })
}

/// The regions covered by `picked` (owner, span): `Some(owner)` where exactly one selection covers them, `None`
/// where several do (their owners, at most `MAX_NAMED_OWNERS + 1`, are added to `named`). A sweep over the
/// span ends, O(R log R).
fn layers(picked: &[(usize, Span)], named: &mut BTreeSet<usize>) -> Vec<(Span, Option<usize>)> {
    let mut events: Vec<(u64, usize, bool)> = Vec::with_capacity(picked.len() * 2);
    for &(owner, (first, last)) in picked {
        events.push((u64::from(first), owner, true));
        events.push((u64::from(last) + 1, owner, false));
    }
    events.sort_unstable_by_key(|&(position, _, _)| position);
    let mut active: BTreeMap<usize, u32> = BTreeMap::new();
    let mut depth = 0u32;
    let mut out = Vec::new();
    let mut index = 0;
    while let Some(&(position, _, _)) = events.get(index) {
        while let Some(&(_, owner, opens)) = events.get(index).filter(|event| event.0 == position) {
            if opens {
                *active.entry(owner).or_default() += 1;
                depth += 1;
            } else if let Some(count) = active.get_mut(&owner) {
                *count -= 1;
                depth -= 1;
                if *count == 0 {
                    active.remove(&owner);
                }
            }
            index += 1;
        }
        let Some(&(next, _, _)) = events.get(index) else {
            break;
        };
        if depth == 0 {
            continue;
        }
        let span = (to_line(position), to_line(next - 1));
        if depth == 1 {
            out.push((span, active.keys().next().copied()));
        } else {
            if named.len() <= MAX_NAMED_OWNERS {
                let room = MAX_NAMED_OWNERS + 1 - named.len();
                named.extend(active.keys().take(room));
            }
            out.push((span, None));
        }
    }
    out
}

fn to_line(position: u64) -> u32 {
    u32::try_from(position).unwrap_or(u32::MAX)
}

/// The hunk and the selection, or `None` with an issue.
fn select<'a>(
    diff: &DiffIndex<'a>,
    issues: &mut Issues,
    at: &str,
    id: &HunkId,
    range: (Option<u32>, Option<u32>),
    owner: usize,
) -> Option<(&'a Hunk, Selection)> {
    let Some(found) = diff.hunk(id) else {
        let message = format!("{at}: hunk {id} is not in the hunk list");
        issues.push(VerifyIssueKind::UnknownHunk, message);
        return None;
    };
    let hunk = found.hunk;
    let first = hunk.head.start;
    let last = first.saturating_add(hunk.head.len).saturating_sub(1);
    let whole = (hunk.head.len > 0).then_some((first, last));
    let (start, end) = match range {
        (None, None) => {
            let selection = Selection {
                owner,
                head: whole,
                removed: true,
            };
            return Some((hunk, selection));
        }
        (Some(start), Some(end)) => (start, end),
        _ => {
            let message = format!("{at}: give both head_start and head_end, or neither");
            issues.push(VerifyIssueKind::RangeOutsideHunk, message);
            return None;
        }
    };
    if whole.is_none() {
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
    // Removed lines sit at the marker after the head line before the hunk.
    let selection = Selection {
        owner,
        head: Some((start, end)),
        removed: spans::marker_inside(spans::marker_of(hunk), (start, end)),
    };
    Some((hunk, selection))
}

/// `head lines 3-5; base lines 7` for spans per side.
fn describe_sides(lines: &BTreeMap<LineSide, Vec<Span>>) -> String {
    lines
        .iter()
        .map(|(side, lines)| {
            let lines = spans::describe(&spans::merge(lines.clone()));
            format!("{} lines {lines}", side_name(*side))
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

    #[test]
    fn repeated_selections_of_a_large_hunk_are_bounded() {
        // 10,000 selections of a 100,000-line hunk: expanding lines per selection would be 10^9 owner entries.
        let lines = 100_000;
        let files = vec![FileDiff {
            path: "src/big.rs".to_owned(),
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            hunks: vec![hunk("h1", (1, lines), (1, lines))],
        }];
        let diff = DiffIndex::new(&files).expect("index");
        let started = std::time::Instant::now();
        let mut flows: Vec<PlannedFlow> = (1..=2_000)
            .map(|n| flow(&format!("f{n}"), n, vec![whole("h1"); 4]))
            .collect();
        flows[0].hunks.extend(vec![part("h1", 1, lines); 2_000]);
        let output = PlanOutput {
            flows,
            unassigned: Vec::new(),
        };
        let Err(VerifyError::Rejected(issues)) = plan(&output, &diff) else {
            panic!("rejected");
        };
        assert_eq!(
            issues,
            [VerifyIssue {
                kind: VerifyIssueKind::LineAssignedTwice,
                message: "hunk h1 (src/big.rs): head lines 1-100000; base lines 1-100000 are selected more than \
                          once (by f1, f2, f3, f4, f5, and others)"
                    .to_owned(),
            }]
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn partial_overlaps_are_reported_per_range() {
        let output = PlanOutput {
            flows: vec![
                flow("f1", 1, vec![part("h1", 10, 11), whole("h3"), whole("h5")]),
                flow("f2", 2, vec![part("h1", 11, 12), whole("h4")]),
            ],
            unassigned: vec![unassigned(whole("h2"))],
        };
        let issues = run(&output).expect_err("rejected");
        assert_eq!(
            issues,
            [VerifyIssue {
                kind: VerifyIssueKind::LineAssignedTwice,
                message:
                    "hunk h1 (src/a.rs): head lines 11 are selected more than once (by f1, f2)"
                        .to_owned(),
            }]
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
