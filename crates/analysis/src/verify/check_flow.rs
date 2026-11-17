//! `analyse_flow` checks: locations in the pinned trees, step and annotation rules, references and edges,
//! thread aliases, move claims and coverage of the flow's changed lines; then the derived graph, step ids,
//! trimmed ranges and move facts. The diff, the steps and the moved blocks are indexed once, so every pass is
//! O(n log n) in the size of the diff and the output.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use contracts::agent::analyse_flow::{AgentStep, FlowAnalysisOutput};
use contracts::api::tour::{FlowGraph, GraphEdge, GraphNode, MoveFact};
use contracts::common::{AnnotationType, Basis, FragmentKind, LineSide, ProvenanceBadge};
use contracts::ids::StepId;

use super::spans::{self, Span};
use super::{
    FileFacts, FlowContext, Issues, VerifiedFlow, VerifyError, VerifyIssueKind, side_name, wording,
};
use crate::diff::FileDiff;
use crate::moves::{MovedBlock, MovesReport};

/// Removed base lines, the deletion marker they show at, and the file's head path.
type Removed<'a> = (Span, u32, Option<&'a str>);

/// A moved block's head lines, the block and its differing head lines, sorted.
type IndexedBlock<'a> = (Span, &'a MovedBlock, Vec<u32>);

/// The changed lines of the diff, indexed by side and path.
struct Changes<'a> {
    /// Changed lines per path: added head lines and removed base lines.
    lines: HashMap<&'a str, [Vec<Span>; 2]>,
    /// Deletion markers per head path, ascending.
    markers: HashMap<&'a str, Vec<u32>>,
    /// Removed lines per base path with their marker and the file's head path.
    removed: HashMap<&'a str, Vec<Removed<'a>>>,
}

impl<'a> Changes<'a> {
    fn new(files: &'a [FileDiff]) -> Self {
        let mut changes = Self {
            lines: HashMap::new(),
            markers: HashMap::new(),
            removed: HashMap::new(),
        };
        for file in files {
            let head = file.path_on(LineSide::Head);
            for hunk in &file.hunks {
                let added = spans::hunk_spans(hunk, true);
                let removed = spans::hunk_spans(hunk, false);
                if let Some(head) = head {
                    changes.lines.entry(head).or_default()[0].extend(&added);
                    if !removed.is_empty() {
                        let markers = changes.markers.entry(head).or_default();
                        markers.push(spans::marker_of(hunk));
                    }
                }
                if let Some(base) = file.path_on(LineSide::Base) {
                    changes.lines.entry(base).or_default()[1].extend(&removed);
                    let marker = spans::marker_of(hunk);
                    let entries = changes.removed.entry(base).or_default();
                    entries.extend(removed.iter().map(|&span| (span, marker, head)));
                }
            }
        }
        for lines in changes.lines.values_mut().flatten() {
            *lines = spans::merge(std::mem::take(lines));
        }
        for markers in changes.markers.values_mut() {
            markers.sort_unstable();
            markers.dedup();
        }
        changes
    }

    fn lines(&self, side: LineSide, path: &str) -> &[Span] {
        let index = usize::from(side == LineSide::Base);
        self.lines
            .get(path)
            .map_or(&[], |lines| lines[index].as_slice())
    }

    fn markers(&self, path: &str) -> &[u32] {
        self.markers.get(path).map_or(&[], Vec::as_slice)
    }
}

/// Deterministic moved blocks per head path, ascending by head start, with their differing lines sorted.
struct Moves<'a> {
    blocks: HashMap<&'a str, Vec<IndexedBlock<'a>>>,
}

impl<'a> Moves<'a> {
    fn new(report: &'a MovesReport) -> Self {
        let mut blocks: HashMap<&str, Vec<_>> = HashMap::new();
        for block in report
            .moved
            .iter()
            .filter(|block| block.to.side == LineSide::Head)
        {
            let to = (block.to.range.start_line, block.to.range.end_line);
            let mut differing = block.differing_head_lines.clone();
            differing.sort_unstable();
            differing.dedup();
            blocks
                .entry(block.to.file.as_str())
                .or_default()
                .push((to, block, differing));
        }
        for list in blocks.values_mut() {
            list.sort_unstable_by_key(|(to, _, _)| *to);
        }
        Self { blocks }
    }

    /// The moved block whose head lines contain all of a head step's lines.
    fn containing(&self, step: &AgentStep) -> Option<&IndexedBlock<'a>> {
        if step.side != LineSide::Head {
            return None;
        }
        let list = self.blocks.get(step.file.as_str())?;
        let index = list.partition_point(|(to, _, _)| to.0 <= step.start_line);
        let candidate = list.get(index.checked_sub(1)?)?;
        (candidate.0.1 >= step.end_line).then_some(candidate)
    }
}

pub(super) fn flow(
    output: &FlowAnalysisOutput,
    context: FlowContext<'_>,
) -> Result<VerifiedFlow, VerifyError> {
    let changes = Changes::new(context.diff.files());
    let moves = Moves::new(context.moves);
    let mut issues = Issues::default();
    let flow_id = &context.planned.id;
    if &output.flow_id != flow_id {
        let message = format!(
            "flow_id: the output is for {} but this job analyses {flow_id}",
            output.flow_id
        );
        issues.push(VerifyIssueKind::UnknownReference, message);
    }
    let intro = &output.introduction;
    issues.text("introduction.problem_before", &intro.problem_before);
    issues.text("introduction.change", &intro.change);
    issues.text(
        "introduction.example_before_after",
        &intro.example_before_after,
    );

    let mut local: HashMap<&str, usize> = HashMap::new();
    for (index, step) in output.steps.iter().enumerate() {
        let at = format!("steps[{index}] \"{}\"", step.id);
        if step.id.trim().is_empty() {
            issues.push(VerifyIssueKind::EmptyText, format!("{at}.id is empty"));
        } else if local.insert(&step.id, index).is_some() {
            let message = format!("{at}: step id {} is used twice", step.id);
            issues.push(VerifyIssueKind::DuplicateId, message);
        }
        check_step(&mut issues, context, &changes, &moves, &at, step);
    }
    let unknown_step = |id: &str| !local.contains_key(id);
    for (index, edge) in output.edges.iter().enumerate() {
        for end in [&edge.from_step_id, &edge.to_step_id] {
            if unknown_step(end) {
                let message = format!("edges[{index}]: {end} is not a step id of this output");
                issues.push(VerifyIssueKind::DanglingEdge, message);
            }
        }
    }
    for (index, impact) in output.impact_refs.iter().enumerate() {
        let at = format!("impact_refs[{index}]");
        if unknown_step(&impact.step_id) {
            let message = format!(
                "{at}: step_id {} is not a step id of this output",
                impact.step_id
            );
            issues.push(VerifyIssueKind::UnknownReference, message);
        }
        let lines = (impact.start_line, impact.end_line);
        if issues.location(context.facts, &at, LineSide::Head, &impact.file, lines) {
            let inside = spans::intersect(changes.lines(LineSide::Head, &impact.file), &[lines]);
            if !inside.is_empty() {
                let changed = spans::describe(&inside);
                let message = format!(
                    "{at}: lines {changed} of {} are changed lines of this pull request; impact_refs name \
                     unchanged code outside the diff (changed lines belong in steps)",
                    impact.file
                );
                issues.push(VerifyIssueKind::ImpactInsideDiff, message);
            }
        }
        issues.text(&format!("{at}.reason"), &impact.reason);
    }
    for (index, pattern) in output.pattern_repetitions.iter().enumerate() {
        let at = format!("pattern_repetitions[{index}]");
        if unknown_step(&pattern.explained_in_step_id) {
            let id = &pattern.explained_in_step_id;
            let message =
                format!("{at}: explained_in_step_id {id} is not a step id of this output");
            issues.push(VerifyIssueKind::UnknownReference, message);
        }
        issues.text(&format!("{at}.description"), &pattern.description);
        for (number, site) in pattern.sites.iter().enumerate() {
            let at = format!("{at}.sites[{number}]");
            let lines = (site.start_line, site.end_line);
            issues.location(context.facts, &at, site.side, &site.file, lines);
        }
    }
    let mut answered = BTreeSet::new();
    for (index, thread) in output.thread_statuses.iter().enumerate() {
        let at = format!("thread_statuses[{index}]");
        let alias = &thread.thread_alias;
        if !context.thread_aliases.contains(alias) {
            let message = format!("{at}: thread {alias} was not listed for this flow");
            issues.push(VerifyIssueKind::UnknownThread, message);
        } else if !answered.insert(alias.as_str()) {
            let message = format!("{at}: thread {alias} has a second status");
            issues.push(VerifyIssueKind::DuplicateId, message);
        }
        issues.text(&format!("{at}.reason"), &thread.reason);
    }
    for alias in context.thread_aliases {
        if !answered.contains(alias.as_str()) {
            let message = format!("thread_statuses: listed thread {alias} has no status");
            issues.push(VerifyIssueKind::ThreadStatusMissing, message);
        }
    }
    for ((side, file), lines) in uncovered(output, context, &changes) {
        let side = side_name(side);
        let lines = spans::describe(&lines);
        let message = format!(
            "{side} lines {lines} of {file} are changed lines of this flow outside every step (removed lines: in a \
             base step or at their deletion marker inside a head step)"
        );
        issues.push(VerifyIssueKind::ChangedLineUncovered, message);
    }
    issues.finish(())?;
    Ok(verified(output, context, &changes, &moves))
}

fn check_step(
    issues: &mut Issues,
    context: FlowContext<'_>,
    changes: &Changes<'_>,
    moves: &Moves<'_>,
    at: &str,
    step: &AgentStep,
) {
    let texts = [
        ("title", &step.title),
        ("called_by", &step.called_by),
        ("decides", &step.decides),
        ("next", &step.next),
        ("why", &step.why),
    ];
    for (field, text) in texts {
        issues.text(&format!("{at}.{field}"), text);
    }
    let (start, end) = (step.start_line, step.end_line);
    let located = issues.location(context.facts, at, step.side, &step.file, (start, end));
    if step.side == LineSide::Base {
        if step.kind != FragmentKind::DeletedSite {
            let message = format!(
                "{at}: a base-side step shows removed code without a head line and must be deleted_site; \
                 otherwise use side head"
            );
            issues.push(VerifyIssueKind::BaseSideMisused, message);
        } else if located {
            let kept = spans::subtract(&[(start, end)], changes.lines(LineSide::Base, &step.file));
            if !kept.is_empty() {
                let lines = spans::describe(&kept);
                let message = format!(
                    "{at}: base lines {lines} of {} were not removed by this pull request; a base-side step holds \
                     removed lines only",
                    step.file
                );
                issues.push(VerifyIssueKind::BaseSideMisused, message);
            }
        }
    }
    check_move_claim(issues, context, moves, at, step);
    for (number, annotation) in step.annotations.iter().enumerate() {
        let at = format!("{at}.annotations[{number}]");
        let (first, last) = (annotation.start_line, annotation.end_line);
        if first > last || first < start || last > end {
            let message =
                format!("{at}: lines {first}-{last} are outside the step's lines {start}-{end}");
            issues.push(VerifyIssueKind::LineOutOfRange, message);
        }
        if annotation.was.trim().is_empty() || annotation.is.trim().is_empty() {
            let message =
                format!("{at}: was and is must both describe the behaviour of these lines");
            issues.push(VerifyIssueKind::BehaviourMissing, message);
        }
        let blank_text = annotation
            .text
            .as_deref()
            .is_none_or(|text| text.trim().is_empty());
        match annotation.kind {
            AnnotationType::Behaviour if annotation.text.is_some() => {
                let message = format!(
                    "{at}: text must be null on a behaviour annotation (its content is was and is)"
                );
                issues.push(VerifyIssueKind::RemarkMismatch, message);
            }
            AnnotationType::Consequence | AnnotationType::Question | AnnotationType::Note
                if blank_text =>
            {
                let message = format!("{at}: a consequence, question or note needs its text");
                issues.push(VerifyIssueKind::RemarkMismatch, message);
            }
            _ => {}
        }
        if annotation.basis == Basis::Fact && annotation.evidence.is_empty() {
            let message = format!(
                "{at}: basis fact needs at least one evidence line that was read; otherwise use inference"
            );
            issues.push(VerifyIssueKind::FactWithoutEvidence, message);
        }
        issues.evidence(context.facts, &at, &annotation.evidence);
    }
    if let Some(assessment) = &step.assessment {
        let at = format!("{at}.assessment");
        let texts = [
            ("headline", &assessment.headline),
            ("reason", &assessment.reason),
            ("basis_note", &assessment.basis_note),
            ("not_checked", &assessment.not_checked),
        ];
        for (field, text) in texts {
            issues.text(&format!("{at}.{field}"), text);
        }
        if let Some(correction) = &assessment.smallest_correction {
            issues.text(&format!("{at}.smallest_correction"), correction);
        }
    }
}

/// `moved_from` is set exactly on `moved` steps and names base lines that exist. When a deterministic moved
/// block contains the whole step, the claim must name that block's file and overlap its base lines; any other
/// claim (a step wider than a block, across two blocks, without a block) stays an LLM claim without a move fact.
fn check_move_claim(
    issues: &mut Issues,
    context: FlowContext<'_>,
    moves: &Moves<'_>,
    at: &str,
    step: &AgentStep,
) {
    let moved = step.kind == FragmentKind::Moved;
    let Some(from) = &step.moved_from else {
        if moved {
            let message =
                format!("{at}: a moved step needs moved_from (the base location of the block)");
            issues.push(VerifyIssueKind::MoveClaimMismatch, message);
        }
        return;
    };
    if !moved {
        let message = format!("{at}: moved_from is set on a step whose kind is not moved");
        issues.push(VerifyIssueKind::MoveClaimMismatch, message);
    }
    let claimed = (from.base_start, from.base_end);
    let at_from = format!("{at}.moved_from");
    if !issues.location(context.facts, &at_from, LineSide::Base, &from.file, claimed) {
        return;
    }
    if let Some((_, block, _)) = moves.containing(step) {
        let origin = &block.from;
        let (first, last) = (origin.range.start_line, origin.range.end_line);
        if origin.file != from.file || !spans::touches(&[(first, last)], claimed) {
            let file = &origin.file;
            let message = format!(
                "{at_from}: the block moved to these lines comes from {file} base lines {first}-{last}"
            );
            issues.push(VerifyIssueKind::MoveClaimMismatch, message);
        }
    }
}

/// The deterministic move fact of a `moved` step: only when one moved block contains every line of the step and
/// the claimed origin lies inside that block's base lines; the differing lines are counted inside the step.
fn move_fact(moves: &Moves<'_>, step: &AgentStep) -> Option<MoveFact> {
    let from = step
        .moved_from
        .as_ref()
        .filter(|_| step.kind == FragmentKind::Moved)?;
    let (_, block, differing) = moves.containing(step)?;
    let origin = &block.from;
    let base = (origin.range.start_line, origin.range.end_line);
    if origin.file != from.file || !spans::covers(&[base], (from.base_start, from.base_end)) {
        return None;
    }
    let first = differing.partition_point(|&line| line < step.start_line);
    let last = differing.partition_point(|&line| line <= step.end_line);
    Some(MoveFact {
        from: origin.clone(),
        badge: block.identical.then_some(ProvenanceBadge::TextIdentical),
        differing_lines: u32::try_from(last - first).unwrap_or(u32::MAX),
    })
}

/// Planned changed lines of the flow outside every step; blank lines need no step. Interval arithmetic over the
/// planned ranges, the merged step ranges and the removed spans: O((P + S + H + B) log) for P planned ranges, S
/// steps, H hunk spans and B blank lines of the planned files.
fn uncovered(
    output: &FlowAnalysisOutput,
    context: FlowContext<'_>,
    changes: &Changes<'_>,
) -> BTreeMap<(LineSide, String), Vec<Span>> {
    let mut steps: HashMap<(LineSide, String), Vec<Span>> = HashMap::new();
    for step in &output.steps {
        let key = (step.side, step.file.clone());
        steps
            .entry(key)
            .or_default()
            .push((step.start_line, step.end_line));
    }
    for spans in steps.values_mut() {
        *spans = spans::merge(std::mem::take(spans));
    }
    let steps_of = |side: LineSide, file: &str| {
        let key = (side, file.to_owned());
        steps.get(&key).map_or(&[][..], Vec::as_slice)
    };
    let mut missing = BTreeMap::new();
    let planned = context
        .plan
        .flow_lines
        .get(&context.planned.id)
        .into_iter()
        .flatten();
    for lines in planned {
        let file = lines.file.as_str();
        let mut rest = spans::subtract(
            &spans::from_ranges(&lines.ranges),
            steps_of(lines.side, file),
        );
        if lines.side == LineSide::Base && !rest.is_empty() {
            let at_markers: Vec<Span> = changes
                .removed
                .get(file)
                .into_iter()
                .flatten()
                .filter(|(_, marker, head)| {
                    head.is_some_and(|head| {
                        let marker_gap = (*marker, marker.saturating_add(1));
                        spans::touches(steps_of(LineSide::Head, head), marker_gap)
                    })
                })
                .map(|(span, _, _)| *span)
                .collect();
            rest = spans::subtract(&rest, &spans::merge(at_markers));
        }
        if let Some(facts) = context
            .facts
            .get(lines.side, file)
            .filter(|_| !rest.is_empty())
        {
            rest = spans::subtract(&rest, &blank_spans(facts));
        }
        if !rest.is_empty() {
            missing.insert((lines.side, lines.file.clone()), rest);
        }
    }
    missing
}

fn blank_spans(facts: &FileFacts) -> Vec<Span> {
    crate::lines::ranges(&facts.blank_lines)
        .into_iter()
        .map(|range| (range.start_line, range.end_line))
        .collect()
}

/// The accepted flow: trimmed ranges, server step ids, derived graph, covered lines, move facts, warnings.
fn verified(
    output: &FlowAnalysisOutput,
    context: FlowContext<'_>,
    changes: &Changes<'_>,
    moves: &Moves<'_>,
) -> VerifiedFlow {
    let flow_id = &context.planned.id;
    let step_ids: Vec<StepId> = (1..=output.steps.len())
        .map(|number| StepId(format!("{flow_id}.{number}")))
        .collect();
    let server_ids: HashMap<&str, &StepId> = output
        .steps
        .iter()
        .zip(&step_ids)
        .map(|(step, id)| (step.id.as_str(), id))
        .collect();
    let nodes = output
        .steps
        .iter()
        .zip(&step_ids)
        .zip(1u32..)
        .map(|((step, id), number)| GraphNode {
            step_id: id.clone(),
            number,
            kind: step.kind.into(),
            label: step.title.clone(),
        })
        .collect();
    let edges = output
        .edges
        .iter()
        .filter_map(|edge| {
            Some(GraphEdge {
                from: (*server_ids.get(edge.from_step_id.as_str())?).clone(),
                to: (*server_ids.get(edge.to_step_id.as_str())?).clone(),
                label: edge.label.clone(),
            })
        })
        .collect();
    let facts = output
        .steps
        .iter()
        .zip(&step_ids)
        .filter_map(|(step, id)| Some((id.clone(), move_fact(moves, step)?)))
        .collect();
    let mut trimmed_output = output.clone();
    for step in &mut trimmed_output.steps {
        (step.start_line, step.end_line) = trimmed(step, context, changes);
    }
    VerifiedFlow {
        flow_id: flow_id.clone(),
        output: trimmed_output,
        step_ids,
        graph: FlowGraph { nodes, edges },
        covered: context
            .plan
            .flow_lines
            .get(flow_id)
            .cloned()
            .unwrap_or_default(),
        moves: facts,
        warnings: wording::flow(output),
    }
}

/// The step range without blank edge lines; changed lines, annotated lines and deletion markers stay inside (a
/// marker after line m stays inside while the range starts at or before m + 1 and ends at or after m). Binary
/// searches over the blank runs, changed spans and markers: O(log + annotations) per step.
fn trimmed(step: &AgentStep, context: FlowContext<'_>, changes: &Changes<'_>) -> (u32, u32) {
    let (start, end) = (step.start_line, step.end_line);
    let Some(facts) = context.facts.get(step.side, &step.file) else {
        return (start, end);
    };
    let blank = &facts.blank_lines;
    let changed = changes.lines(step.side, &step.file);
    let markers = match step.side {
        LineSide::Head => changes.markers(&step.file),
        LineSide::Base => &[],
    };
    let annotations = || {
        step.annotations
            .iter()
            .filter(|a| a.start_line <= a.end_line)
            .map(|a| (a.start_line, a.end_line))
    };
    let mut first = start;
    if facts.is_blank(start) {
        first = blank_run(blank, start).1.saturating_add(1).min(end);
        let next_changed = changed.partition_point(|&(_, last)| last < start);
        if let Some(&(changed_first, _)) = changed.get(next_changed) {
            first = first.min(changed_first.max(start));
        }
        for (a_first, _) in annotations().filter(|&(_, a_last)| a_last >= start) {
            first = first.min(a_first.max(start));
        }
        let next_marker = markers.partition_point(|&marker| marker.saturating_add(1) < start);
        if let Some(&marker) = markers.get(next_marker) {
            first = first.min(marker.saturating_add(1));
        }
    }
    let mut last = end;
    if first < end && facts.is_blank(end) {
        last = blank_run(blank, end).0.saturating_sub(1).max(first);
        let before = changed.partition_point(|&(changed_first, _)| changed_first <= end);
        if let Some(&(_, changed_last)) = before.checked_sub(1).and_then(|i| changed.get(i)) {
            last = last.max(changed_last.min(end));
        }
        for (_, a_last) in annotations().filter(|&(a_first, _)| a_first <= end) {
            last = last.max(a_last.min(end));
        }
        let before = markers.partition_point(|&marker| marker <= end);
        if let Some(&marker) = before.checked_sub(1).and_then(|i| markers.get(i)) {
            last = last.max(marker);
        }
    }
    (first, last)
}

/// The run of consecutive blank lines around the blank `line`: `blank[j] - j` is non-decreasing and constant
/// along a run, so both ends are binary searches.
fn blank_run(blank: &[u32], line: u32) -> Span {
    let index = blank.partition_point(|&b| b < line);
    let key = |j: usize| i64::from(blank[j]) - i64::try_from(j).unwrap_or(i64::MAX);
    let target = key(index);
    let (mut low, mut high) = (index, blank.len());
    while low < high {
        let middle = low + (high - low) / 2;
        if key(middle) == target {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    let run_last = blank[low - 1];
    let (mut low, mut high) = (0, index);
    while low < high {
        let middle = low + (high - low) / 2;
        if key(middle) < target {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    (blank[low], run_last)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use contracts::agent::analyse_flow::{
        AgentAnnotation, AgentEdge, AgentImpactRef, AgentPatternRepetition, AgentPatternSite,
        AgentThreadStatus, Assessment, FlowIntroduction, MovedFrom,
    };
    use contracts::agent::plan::{HunkSelection, PlannedFlow};
    use contracts::api::tour::FlowGroup;
    use contracts::common::{
        Blocking, Classification, Evidence, FileRange, FileRanges, LineRange, NodeKind, Origin,
        RiskBand, ThreadLlmStatus,
    };
    use contracts::ids::{FlowId, HunkId};

    use super::*;
    use crate::diff::{DiffIndex, FileDiff, FileStatus, Hunk, LineSpan};
    use crate::moves::MovesReport;
    use crate::verify::{FileFacts, LineFacts, VerifiedPlan, VerifyIssue};

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

    /// `src/a.rs`: base 10-11 → head 10-12 (marker after head 9); `src/gone.rs` deleted (base 1-5).
    fn files() -> Vec<FileDiff> {
        vec![
            FileDiff {
                path: "src/a.rs".to_owned(),
                old_path: None,
                status: FileStatus::Modified,
                binary: false,
                hunks: vec![hunk("h1", (10, 2), (10, 3))],
            },
            FileDiff {
                path: "src/gone.rs".to_owned(),
                old_path: None,
                status: FileStatus::Deleted,
                binary: false,
                hunks: vec![hunk("h2", (1, 5), (0, 0))],
            },
        ]
    }

    fn facts() -> LineFacts {
        let mut facts = LineFacts::default();
        let mut add = |side, path: &str, line_count, blank_lines: &[u32]| {
            let blank_lines = blank_lines.to_vec();
            facts.insert(
                side,
                path,
                FileFacts {
                    line_count,
                    blank_lines,
                },
            );
        };
        add(LineSide::Head, "src/a.rs", 40, &[8, 13, 14]);
        add(LineSide::Base, "src/a.rs", 39, &[]);
        add(LineSide::Base, "src/gone.rs", 5, &[5]);
        add(LineSide::Head, "src/caller.rs", 50, &[]);
        add(LineSide::Base, "src/old.rs", 30, &[]);
        facts
    }

    fn ranges(file: &str, side: LineSide, start_line: u32, end_line: u32) -> FileRanges {
        FileRanges {
            file: file.to_owned(),
            side,
            ranges: vec![LineRange {
                start_line,
                end_line,
            }],
        }
    }

    fn planned() -> PlannedFlow {
        let whole = |id: &str| HunkSelection {
            hunk_id: HunkId(id.to_owned()),
            head_start: None,
            head_end: None,
        };
        PlannedFlow {
            id: FlowId("f1".to_owned()),
            title: "Retry on timeout".to_owned(),
            classification: Classification::Behavioural,
            importance_rank: 1,
            intent: "Retry.".to_owned(),
            hunks: vec![whole("h1"), whole("h2")],
            depends_on: Vec::new(),
        }
    }

    fn verified_plan() -> VerifiedPlan {
        let lines = vec![
            ranges("src/a.rs", LineSide::Head, 10, 12),
            ranges("src/a.rs", LineSide::Base, 10, 11),
            ranges("src/gone.rs", LineSide::Base, 1, 5),
        ];
        VerifiedPlan {
            output: contracts::agent::plan::PlanOutput {
                flows: vec![planned()],
                unassigned: Vec::new(),
            },
            flow_lines: BTreeMap::from([(FlowId("f1".to_owned()), lines)]),
            unassigned: Vec::new(),
        }
    }

    fn evidence(file: &str, side: LineSide, line: u32) -> Evidence {
        Evidence {
            file: file.to_owned(),
            side,
            line,
        }
    }

    fn annotation(
        kind: AnnotationType,
        basis: Basis,
        lines: (u32, u32),
        evidence: Vec<Evidence>,
    ) -> AgentAnnotation {
        AgentAnnotation {
            kind,
            basis,
            start_line: lines.0,
            end_line: lines.1,
            was: "Sent once.".to_owned(),
            is: "Retried.".to_owned(),
            text: (kind != AnnotationType::Behaviour).then(|| "Can it be sent twice?".to_owned()),
            evidence,
        }
    }

    fn step(
        id: &str,
        file: &str,
        side: LineSide,
        lines: (u32, u32),
        kind: FragmentKind,
    ) -> AgentStep {
        AgentStep {
            id: id.to_owned(),
            title: format!("Step {id}"),
            file: file.to_owned(),
            side,
            start_line: lines.0,
            end_line: lines.1,
            symbol: None,
            kind,
            moved_from: None,
            called_by: "caller".to_owned(),
            decides: "whether to retry".to_owned(),
            next: "send".to_owned(),
            why: "timeouts".to_owned(),
            annotations: Vec::new(),
            assessment: None,
        }
    }

    fn edge(from: &str, to: &str) -> AgentEdge {
        AgentEdge {
            from_step_id: from.to_owned(),
            to_step_id: to.to_owned(),
            label: None,
        }
    }

    /// A flow every rule accepts.
    fn output() -> FlowAnalysisOutput {
        let mut caller = step(
            "s1",
            "src/caller.rs",
            LineSide::Head,
            (40, 45),
            FragmentKind::Context,
        );
        caller.annotations.push(annotation(
            AnnotationType::Consequence,
            Basis::Fact,
            (41, 42),
            vec![evidence("src/caller.rs", LineSide::Head, 41)],
        ));
        let mut changed = step(
            "s2",
            "src/a.rs",
            LineSide::Head,
            (8, 14),
            FragmentKind::Modified,
        );
        changed.annotations.push(annotation(
            AnnotationType::Behaviour,
            Basis::Fact,
            (10, 12),
            vec![evidence("src/a.rs", LineSide::Base, 10)],
        ));
        changed.assessment = Some(Assessment {
            headline: "Payments may be sent twice.".to_owned(),
            risk: RiskBand::High,
            reason: "Retries are not idempotent.".to_owned(),
            basis_note: "Read 2 files; nothing executed.".to_owned(),
            not_checked: "The node's deduplication.".to_owned(),
            origin: Origin::Introduced,
            blocking: Blocking::BlocksThisPr,
            smallest_correction: None,
        });
        let removed = step(
            "s3",
            "src/gone.rs",
            LineSide::Base,
            (1, 4),
            FragmentKind::DeletedSite,
        );
        FlowAnalysisOutput {
            flow_id: FlowId("f1".to_owned()),
            introduction: FlowIntroduction {
                problem_before: "Timeouts failed payments.".to_owned(),
                change: "Retry on timeout.".to_owned(),
                example_before_after: "Before: error; after: retried.".to_owned(),
            },
            steps: vec![caller, changed, removed],
            edges: vec![edge("s1", "s2"), edge("s2", "s3")],
            impact_refs: vec![AgentImpactRef {
                step_id: "s2".to_owned(),
                file: "src/caller.rs".to_owned(),
                start_line: 1,
                end_line: 3,
                symbol: None,
                reason: "Calls send.".to_owned(),
                basis: Basis::Inference,
            }],
            pattern_repetitions: vec![AgentPatternRepetition {
                description: "Same retry wrapper.".to_owned(),
                explained_in_step_id: "s2".to_owned(),
                sites: vec![AgentPatternSite {
                    file: "src/caller.rs".to_owned(),
                    side: LineSide::Head,
                    start_line: 44,
                    end_line: 45,
                }],
            }],
            thread_statuses: vec![AgentThreadStatus {
                thread_alias: "t1".to_owned(),
                status: ThreadLlmStatus::StillValid,
                reason: "Still retries every method.".to_owned(),
            }],
        }
    }

    struct Setup {
        files: Vec<FileDiff>,
        facts: LineFacts,
        plan: VerifiedPlan,
        planned: PlannedFlow,
        moves: MovesReport,
        aliases: Vec<String>,
    }

    impl Setup {
        fn new() -> Self {
            Self {
                files: files(),
                facts: facts(),
                plan: verified_plan(),
                planned: planned(),
                moves: MovesReport::default(),
                aliases: vec!["t1".to_owned()],
            }
        }

        fn run(&self, output: &FlowAnalysisOutput) -> Result<VerifiedFlow, Vec<VerifyIssue>> {
            let diff = DiffIndex::new(&self.files).expect("index");
            let context = FlowContext {
                planned: &self.planned,
                plan: &self.plan,
                facts: &self.facts,
                diff: &diff,
                moves: &self.moves,
                thread_aliases: &self.aliases,
            };
            match flow(output, context) {
                Ok(verified) => Ok(verified),
                Err(VerifyError::Rejected(issues)) => Err(issues),
                Err(other) => panic!("{other:?}"),
            }
        }

        fn issues(&self, output: &FlowAnalysisOutput) -> Vec<VerifyIssue> {
            self.run(output).expect_err("rejected")
        }
    }

    fn kinds(issues: &[VerifyIssue]) -> Vec<VerifyIssueKind> {
        issues.iter().map(|issue| issue.kind).collect()
    }

    #[test]
    fn verify_accepts_a_consistent_flow_and_derives_ids_graph_and_trimmed_ranges() {
        let setup = Setup::new();
        let verified = setup.run(&output()).expect("accepted");
        let ids: Vec<&str> = verified.step_ids.iter().map(|id| id.0.as_str()).collect();
        assert_eq!(ids, ["f1.1", "f1.2", "f1.3"]);
        let nodes: Vec<(&str, u32, NodeKind, &str)> = verified
            .graph
            .nodes
            .iter()
            .map(|n| (n.step_id.0.as_str(), n.number, n.kind, n.label.as_str()))
            .collect();
        assert_eq!(
            nodes,
            [
                ("f1.1", 1, NodeKind::Unchanged, "Step s1"),
                ("f1.2", 2, NodeKind::Changed, "Step s2"),
                ("f1.3", 3, NodeKind::Changed, "Step s3"),
            ],
            "one node per step"
        );
        let edges: Vec<(&str, &str)> = verified
            .graph
            .edges
            .iter()
            .map(|e| (e.from.0.as_str(), e.to.0.as_str()))
            .collect();
        assert_eq!(edges, [("f1.1", "f1.2"), ("f1.2", "f1.3")]);
        let s2 = &verified.output.steps[1];
        assert_eq!(
            (s2.start_line, s2.end_line),
            (9, 12),
            "blank 8 trimmed but the marker after 9 stays inside; blank 13-14 trimmed"
        );
        assert_eq!(
            verified.covered,
            setup.plan.flow_lines[&FlowId("f1".to_owned())]
        );
        assert!(verified.moves.is_empty() && verified.warnings.is_empty());
        assert_eq!(
            FlowGroup::from(setup.planned.classification),
            FlowGroup::Behaviour
        );
    }

    #[test]
    fn verify_rejects_path_outside_pinned_tree() {
        let mut output = output();
        output.steps[0].file = "../../etc/passwd".to_owned();
        output.steps[1].annotations[0]
            .evidence
            .push(evidence("/etc/passwd", LineSide::Head, 1));
        output.impact_refs[0].file = "src/not_in_head.rs".to_owned();
        output.pattern_repetitions[0].sites[0].side = LineSide::Base;
        let issues = Setup::new().issues(&output);
        assert_eq!(
            kinds(&issues),
            [VerifyIssueKind::UnknownFile; 4],
            "{issues:#?}"
        );
        assert_eq!(
            issues[0].message,
            "steps[0] \"s1\": ../../etc/passwd is not a file of the head commit"
        );
    }

    #[test]
    fn verify_rejects_out_of_range_lines() {
        let mut output = output();
        output.steps[0].end_line = 51;
        output.steps[1].annotations[0].end_line = 15;
        output.steps[1].annotations[0].evidence[0].line = 0;
        output.impact_refs[0].start_line = 4;
        output.pattern_repetitions[0].sites[0].end_line = 99;
        let issues = Setup::new().issues(&output);
        assert_eq!(
            kinds(&issues),
            [VerifyIssueKind::LineOutOfRange; 5],
            "{issues:#?}"
        );
        assert_eq!(
            issues[0].message,
            "steps[0] \"s1\": lines 40-51 are not lines of src/caller.rs on head (1-50)"
        );
        assert_eq!(
            issues[1].message,
            "steps[1] \"s2\".annotations[0]: lines 10-15 are outside the step's lines 8-14"
        );
    }

    #[test]
    fn verify_rejects_facts_without_evidence_and_annotation_shape_errors() {
        let mut output = output();
        let notes = &mut output.steps[1].annotations;
        notes[0].evidence.clear();
        notes[0].text = Some("A behaviour remark.".to_owned());
        notes.push(annotation(
            AnnotationType::Question,
            Basis::Inference,
            (10, 10),
            Vec::new(),
        ));
        notes[1].text = Some("  ".to_owned());
        notes[1].is = String::new();
        let issues = Setup::new().issues(&output);
        assert_eq!(
            kinds(&issues),
            [
                VerifyIssueKind::RemarkMismatch,
                VerifyIssueKind::FactWithoutEvidence,
                VerifyIssueKind::BehaviourMissing,
                VerifyIssueKind::RemarkMismatch,
            ],
            "{issues:#?}"
        );
    }

    #[test]
    fn verify_rejects_empty_required_texts() {
        let mut output = output();
        output.introduction.change = " ".to_owned();
        output.steps[0].why = String::new();
        if let Some(assessment) = output.steps[1].assessment.as_mut() {
            assessment.reason = String::new();
            assessment.smallest_correction = Some(String::new());
        }
        output.thread_statuses[0].reason = String::new();
        let issues = Setup::new().issues(&output);
        let messages: Vec<&str> = issues.iter().map(|i| i.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "introduction.change is empty",
                "steps[0] \"s1\".why is empty",
                "steps[1] \"s2\".assessment.reason is empty",
                "steps[1] \"s2\".assessment.smallest_correction is empty",
                "thread_statuses[0].reason is empty",
            ]
        );
        assert!(issues.iter().all(|i| i.kind == VerifyIssueKind::EmptyText));
    }

    #[test]
    fn verify_rejects_dangling_edges_and_unmapped_nodes() {
        let mut output = output();
        output.edges.push(edge("s3", "s9"));
        output.edges.push(edge("", "s1"));
        output.impact_refs[0].step_id = "s7".to_owned();
        output.pattern_repetitions[0].explained_in_step_id = "x".to_owned();
        let mut twin = output.steps[0].clone();
        twin.annotations.clear();
        output.steps.push(twin);
        let issues = Setup::new().issues(&output);
        assert_eq!(
            kinds(&issues),
            [
                VerifyIssueKind::DuplicateId,
                VerifyIssueKind::DanglingEdge,
                VerifyIssueKind::DanglingEdge,
                VerifyIssueKind::UnknownReference,
                VerifyIssueKind::UnknownReference,
            ],
            "{issues:#?}"
        );
        assert_eq!(
            issues[1].message,
            "edges[2]: s9 is not a step id of this output"
        );
    }

    #[test]
    fn verify_rejects_changed_lines_outside_every_step() {
        let mut output = output();
        output.steps[1].start_line = 11;
        output.steps[1].annotations[0].start_line = 11;
        output.steps[2].end_line = 2;
        let issues = Setup::new().issues(&output);
        let messages: Vec<&str> = issues.iter().map(|i| i.message.as_str()).collect();
        assert_eq!(
            kinds(&issues),
            [VerifyIssueKind::ChangedLineUncovered; 3],
            "{issues:#?}"
        );
        assert!(
            messages[0].starts_with("head lines 10 of src/a.rs are changed lines"),
            "{messages:?}"
        );
        assert!(
            messages[1].starts_with("base lines 10-11 of src/a.rs"),
            "the marker after line 9 is outside 11-14: {messages:?}"
        );
        assert!(
            messages[2].starts_with("base lines 3-4 of src/gone.rs"),
            "blank line 5 needs no step: {messages:?}"
        );
    }

    #[test]
    fn verify_rejects_misused_base_side() {
        let mut output = output();
        output.steps[2].kind = FragmentKind::Modified;
        output.steps.push(step(
            "s4",
            "src/a.rs",
            LineSide::Base,
            (11, 13),
            FragmentKind::DeletedSite,
        ));
        let issues = Setup::new().issues(&output);
        assert_eq!(
            kinds(&issues),
            [VerifyIssueKind::BaseSideMisused; 2],
            "{issues:#?}"
        );
        assert!(
            issues[1]
                .message
                .contains("base lines 12-13 of src/a.rs were not removed")
        );
    }

    #[test]
    fn move_claims_are_checked_against_deterministic_moves() {
        let mut setup = Setup::new();
        let file_range = |file: &str, side, start_line, end_line| FileRange {
            file: file.to_owned(),
            side,
            range: LineRange {
                start_line,
                end_line,
            },
        };
        setup.moves.moved.push(MovedBlock {
            from: file_range("src/old.rs", LineSide::Base, 1, 10),
            to: file_range("src/a.rs", LineSide::Head, 20, 29),
            identical: false,
            differing_head_lines: vec![22, 23],
        });
        let mut output = output();
        let mut moved = step(
            "s4",
            "src/a.rs",
            LineSide::Head,
            (20, 29),
            FragmentKind::Moved,
        );
        moved.moved_from = Some(MovedFrom {
            file: "src/old.rs".to_owned(),
            base_start: 1,
            base_end: 10,
        });
        output.steps.push(moved);
        let verified = setup
            .run(&output)
            .expect("a claim matching the moved block");
        assert_eq!(
            verified.moves,
            [(
                StepId("f1.4".to_owned()),
                MoveFact {
                    from: file_range("src/old.rs", LineSide::Base, 1, 10),
                    badge: None,
                    differing_lines: 2,
                }
            )]
        );
        setup.moves.moved[0].identical = true;
        let verified = setup.run(&output).expect("identical move");
        assert_eq!(
            verified.moves[0].1.badge,
            Some(ProvenanceBadge::TextIdentical)
        );

        let mut wrong = output.clone();
        if let Some(from) = wrong.steps[3].moved_from.as_mut() {
            from.base_start = 20;
            from.base_end = 25;
        }
        wrong.steps[0].kind = FragmentKind::Moved;
        wrong.steps[1].moved_from = output.steps[3].moved_from.clone();
        let issues = setup.issues(&wrong);
        assert_eq!(
            kinds(&issues),
            [VerifyIssueKind::MoveClaimMismatch; 3],
            "{issues:#?}"
        );
        assert!(
            issues[2]
                .message
                .contains("comes from src/old.rs base lines 1-10")
        );

        setup.moves.moved.clear();
        let unconfirmed = setup
            .run(&output)
            .expect("a claim without a moved block stays an LLM claim");
        assert!(unconfirmed.moves.is_empty());
    }

    #[test]
    fn thread_aliases_must_be_listed_and_answered_once() {
        let mut setup = Setup::new();
        setup.aliases = vec!["t1".to_owned(), "t2".to_owned()];
        let mut output = output();
        let status = output.thread_statuses[0].clone();
        output.thread_statuses.push(AgentThreadStatus {
            thread_alias: "t3".to_owned(),
            ..status.clone()
        });
        output.thread_statuses.push(status);
        let issues = setup.issues(&output);
        assert_eq!(
            kinds(&issues),
            [
                VerifyIssueKind::UnknownThread,
                VerifyIssueKind::DuplicateId,
                VerifyIssueKind::ThreadStatusMissing,
            ],
            "{issues:#?}"
        );
    }

    #[test]
    fn output_for_another_flow_is_rejected() {
        let mut output = output();
        output.flow_id = FlowId("f2".to_owned());
        let issues = Setup::new().issues(&output);
        assert_eq!(kinds(&issues), [VerifyIssueKind::UnknownReference]);
    }

    #[test]
    fn wording_hits_are_warnings_not_rejections() {
        let mut output = output();
        output.steps[1].why = "No risk: verified.".to_owned();
        let verified = Setup::new().run(&output).expect("accepted");
        let phrases: Vec<&str> = verified
            .warnings
            .iter()
            .map(|w| w.phrase.as_str())
            .collect();
        assert_eq!(phrases, ["No risk", "verified"]);
        assert_eq!(verified.warnings[0].location, "steps[1].why");
    }

    fn file_range(file: &str, side: LineSide, start_line: u32, end_line: u32) -> FileRange {
        FileRange {
            file: file.to_owned(),
            side,
            range: LineRange {
                start_line,
                end_line,
            },
        }
    }

    /// The output plus one `moved` step at head lines `lines` of `src/a.rs` claiming `claim` (file, base lines).
    fn with_moved_step(lines: (u32, u32), claim: (&str, u32, u32)) -> FlowAnalysisOutput {
        let mut output = output();
        let mut moved = step("s4", "src/a.rs", LineSide::Head, lines, FragmentKind::Moved);
        moved.moved_from = Some(MovedFrom {
            file: claim.0.to_owned(),
            base_start: claim.1,
            base_end: claim.2,
        });
        output.steps.push(moved);
        output
    }

    #[test]
    fn move_fact_only_covers_lines_inside_one_moved_block() {
        let mut setup = Setup::new();
        // Identical move old.rs 1-10 → a.rs 20-29; a second move old.rs 11-15 → a.rs 30-34 with line 31 changed.
        setup.moves.moved.push(MovedBlock {
            from: file_range("src/old.rs", LineSide::Base, 1, 10),
            to: file_range("src/a.rs", LineSide::Head, 20, 29),
            identical: true,
            differing_head_lines: Vec::new(),
        });
        setup.moves.moved.push(MovedBlock {
            from: file_range("src/old.rs", LineSide::Base, 11, 15),
            to: file_range("src/a.rs", LineSide::Head, 30, 34),
            identical: false,
            differing_head_lines: vec![34, 31],
        });
        let fact = |lines, claim| {
            let verified = setup.run(&with_moved_step(lines, claim)).expect("accepted");
            verified.moves.into_iter().map(|(_, fact)| fact).next()
        };
        let identical = MoveFact {
            from: file_range("src/old.rs", LineSide::Base, 1, 10),
            badge: Some(ProvenanceBadge::TextIdentical),
            differing_lines: 0,
        };
        assert_eq!(
            fact((20, 29), ("src/old.rs", 1, 10)),
            Some(identical.clone())
        );
        assert_eq!(
            fact((22, 27), ("src/old.rs", 3, 8)),
            Some(identical),
            "a part of the block"
        );
        assert_eq!(
            fact((18, 25), ("src/old.rs", 1, 10)),
            None,
            "starts before the block"
        );
        assert_eq!(
            fact((20, 30), ("src/old.rs", 1, 10)),
            None,
            "ends after the identical block: line 30 is not covered by the identical comparison"
        );
        assert_eq!(
            fact((25, 32), ("src/old.rs", 1, 15)),
            None,
            "across two blocks"
        );
        assert_eq!(
            fact((22, 27), ("src/old.rs", 1, 12)),
            None,
            "claim wider than the block's origin"
        );
        assert_eq!(
            fact((31, 33), ("src/old.rs", 12, 14)),
            Some(MoveFact {
                from: file_range("src/old.rs", LineSide::Base, 11, 15),
                badge: None,
                differing_lines: 1,
            }),
            "differing lines are counted inside the step"
        );

        let issues = setup.issues(&with_moved_step((22, 27), ("src/old.rs", 20, 25)));
        assert_eq!(
            kinds(&issues),
            [VerifyIssueKind::MoveClaimMismatch],
            "{issues:#?}"
        );
        let issues = setup.issues(&with_moved_step((22, 27), ("src/a.rs", 1, 10)));
        assert_eq!(
            kinds(&issues),
            [VerifyIssueKind::MoveClaimMismatch],
            "other origin file"
        );
    }

    #[test]
    fn impact_refs_must_be_outside_the_diff() {
        let setup = Setup::new();
        let with_impact = |file: &str, lines: (u32, u32)| {
            let mut output = output();
            output.impact_refs[0].file = file.to_owned();
            (
                output.impact_refs[0].start_line,
                output.impact_refs[0].end_line,
            ) = lines;
            output
        };
        assert!(
            setup.run(&with_impact("src/a.rs", (1, 9))).is_ok(),
            "outside"
        );
        let inside = setup.issues(&with_impact("src/a.rs", (10, 12)));
        assert_eq!(kinds(&inside), [VerifyIssueKind::ImpactInsideDiff]);
        let partial = setup.issues(&with_impact("src/a.rs", (12, 15)));
        assert_eq!(kinds(&partial), [VerifyIssueKind::ImpactInsideDiff]);
        assert!(
            partial[0]
                .message
                .starts_with("impact_refs[0]: lines 12 of src/a.rs are changed lines")
        );
    }

    #[test]
    fn file_too_large_to_read_is_its_own_issue() {
        let mut setup = Setup::new();
        setup
            .facts
            .insert_unread(LineSide::Head, "src/big.rs", 9_000_000);
        let mut output = output();
        output.impact_refs[0].file = "src/big.rs".to_owned();
        let issues = setup.issues(&output);
        assert_eq!(kinds(&issues), [VerifyIssueKind::FileTooLarge]);
        assert!(
            issues[0].message.contains("has 9000000 bytes"),
            "{issues:#?}"
        );
    }

    #[test]
    fn large_deletion_coverage_is_not_quadratic() {
        // 200,000 removed lines at one marker, covered by one head step; a per-line marker search is ~2·10^10
        // comparisons, interval accounting is a handful.
        let removed = 200_000;
        let mut setup = Setup::new();
        setup.files = vec![FileDiff {
            path: "src/big.rs".to_owned(),
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            hunks: vec![hunk("h1", (1000, removed), (999, 0))],
        }];
        let lines = vec![ranges("src/big.rs", LineSide::Base, 1000, 999 + removed)];
        setup.plan.flow_lines = BTreeMap::from([(FlowId("f1".to_owned()), lines)]);
        let file = |line_count| FileFacts {
            line_count,
            blank_lines: Vec::new(),
        };
        setup.facts.insert(LineSide::Head, "src/big.rs", file(2000));
        setup
            .facts
            .insert(LineSide::Base, "src/big.rs", file(300_000));
        let mut output = output();
        output.steps.truncate(1);
        output.steps[0].annotations.clear();
        output.steps.push(step(
            "s2",
            "src/big.rs",
            LineSide::Head,
            (990, 1000),
            FragmentKind::DeletedSite,
        ));
        output.edges = vec![edge("s1", "s2")];
        output.impact_refs.clear();
        output.pattern_repetitions.clear();
        let started = std::time::Instant::now();
        let verified = setup
            .run(&output)
            .expect("the marker after line 999 is inside the step");
        assert_eq!(verified.covered[0].ranges.len(), 1);
        output.steps.pop();
        output.edges.clear();
        let issues = setup.issues(&output);
        assert_eq!(kinds(&issues), [VerifyIssueKind::ChangedLineUncovered]);
        assert!(
            issues[0]
                .message
                .starts_with("base lines 1000-200999 of src/big.rs")
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }
}
