//! `analyse_flow` checks: locations in the pinned trees, step and annotation rules, references and edges,
//! thread aliases, move claims and coverage of the flow's changed lines; then the derived graph, step ids,
//! trimmed ranges and move facts.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use contracts::agent::analyse_flow::{AgentStep, FlowAnalysisOutput};
use contracts::api::tour::{FlowGraph, GraphEdge, GraphNode, MoveFact};
use contracts::common::{AnnotationType, Basis, FragmentKind, LineSide, ProvenanceBadge};
use contracts::ids::StepId;

use super::{
    FlowContext, Issues, VerifiedFlow, VerifyError, VerifyIssueKind, describe_lines, head_path_of,
    marker_inside, side_name, wording,
};
use crate::lines::{changed_lines_within, deletion_markers};
use crate::moves::MovedBlock;

pub(super) fn flow(
    output: &FlowAnalysisOutput,
    context: FlowContext<'_>,
) -> Result<VerifiedFlow, VerifyError> {
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
        check_step(&mut issues, context, &at, step);
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
        issues.location(context.facts, &at, LineSide::Head, &impact.file, lines);
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
    for ((side, file), lines) in uncovered(output, context) {
        let side = side_name(side);
        let lines = describe_lines(&lines);
        let message = format!(
            "{side} lines {lines} of {file} are changed lines of this flow outside every step (removed lines: in a \
             base step or at their deletion marker inside a head step)"
        );
        issues.push(VerifyIssueKind::ChangedLineUncovered, message);
    }
    issues.finish(())?;
    Ok(verified(output, context))
}

fn check_step(issues: &mut Issues, context: FlowContext<'_>, at: &str, step: &AgentStep) {
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
            let files = context.diff.files();
            let removed = changed_lines_within(files, &step.file, LineSide::Base, start, end);
            let kept: Vec<u32> = (start..=end)
                .filter(|line| removed.binary_search(line).is_err())
                .collect();
            if !kept.is_empty() {
                let lines = describe_lines(&kept);
                let message = format!(
                    "{at}: base lines {lines} of {} were not removed by this pull request; a base-side step holds \
                     removed lines only",
                    step.file
                );
                issues.push(VerifyIssueKind::BaseSideMisused, message);
            }
        }
    }
    check_move_claim(issues, context, at, step);
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

/// `moved_from` is set exactly on `moved` steps, names base lines that exist and agrees with the deterministic
/// moved block at the step's lines (a claim without such a block stays an LLM claim).
fn check_move_claim(issues: &mut Issues, context: FlowContext<'_>, at: &str, step: &AgentStep) {
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
    if let Some(block) = moved_block(context, step) {
        let origin = &block.from;
        let (first, last) = (origin.range.start_line, origin.range.end_line);
        if origin.file != from.file || !overlaps(claimed, (first, last)) {
            let file = &origin.file;
            let message = format!(
                "{at_from}: the block moved to these lines comes from {file} base lines {first}-{last}"
            );
            issues.push(VerifyIssueKind::MoveClaimMismatch, message);
        }
    }
}

/// The deterministic moved block whose head lines overlap a head step.
fn moved_block<'a>(context: FlowContext<'a>, step: &AgentStep) -> Option<&'a MovedBlock> {
    if step.side != LineSide::Head {
        return None;
    }
    let lines = (step.start_line, step.end_line);
    context.moves.moved.iter().find(|block| {
        let to = &block.to;
        to.side == LineSide::Head
            && to.file == step.file
            && overlaps(lines, (to.range.start_line, to.range.end_line))
    })
}

fn overlaps((a_start, a_end): (u32, u32), (b_start, b_end): (u32, u32)) -> bool {
    a_start <= b_end && b_start <= a_end
}

/// Planned changed lines of the flow outside every step; blank lines need no step.
fn uncovered(
    output: &FlowAnalysisOutput,
    context: FlowContext<'_>,
) -> BTreeMap<(LineSide, String), Vec<u32>> {
    let files = context.diff.files();
    let in_step = |side: LineSide, file: &str, line: u32| {
        output.steps.iter().any(|step| {
            step.side == side
                && step.file == file
                && (step.start_line..=step.end_line).contains(&line)
        })
    };
    let at_marker = |base_path: &str, line: u32| {
        let marker = context.diff.deletion_marker(base_path, line);
        let head_path = head_path_of(files, base_path);
        let (Some(marker), Some(head_path)) = (marker, head_path) else {
            return false;
        };
        output.steps.iter().any(|step| {
            step.side == LineSide::Head
                && step.file == head_path
                && marker_inside(marker, step.start_line, step.end_line)
        })
    };
    let mut missing: BTreeMap<(LineSide, String), Vec<u32>> = BTreeMap::new();
    let planned = context
        .plan
        .flow_lines
        .get(&context.planned.id)
        .into_iter()
        .flatten();
    for lines in planned {
        let facts = context.facts.get(lines.side, &lines.file);
        for range in &lines.ranges {
            for line in range.start_line..=range.end_line {
                let covered = facts.is_some_and(|facts| facts.is_blank(line))
                    || in_step(lines.side, &lines.file, line)
                    || (lines.side == LineSide::Base && at_marker(&lines.file, line));
                if !covered {
                    missing
                        .entry((lines.side, lines.file.clone()))
                        .or_default()
                        .push(line);
                }
            }
        }
    }
    missing
}

/// The accepted flow: trimmed ranges, server step ids, derived graph, covered lines, move facts, warnings.
fn verified(output: &FlowAnalysisOutput, context: FlowContext<'_>) -> VerifiedFlow {
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
    let moves = output
        .steps
        .iter()
        .zip(&step_ids)
        .filter(|(step, _)| step.kind == FragmentKind::Moved)
        .filter_map(|(step, id)| {
            let block = moved_block(context, step)?;
            Some((
                id.clone(),
                MoveFact {
                    from: block.from.clone(),
                    badge: block.identical.then_some(ProvenanceBadge::TextIdentical),
                    differing_lines: u32::try_from(block.differing_head_lines.len())
                        .unwrap_or(u32::MAX),
                },
            ))
        })
        .collect();
    let mut trimmed_output = output.clone();
    for step in &mut trimmed_output.steps {
        (step.start_line, step.end_line) = trimmed(step, context);
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
        moves,
        warnings: wording::flow(output),
    }
}

/// The step range without blank edge lines; changed lines, annotated lines and deletion markers stay inside.
fn trimmed(step: &AgentStep, context: FlowContext<'_>) -> (u32, u32) {
    let (start, end) = (step.start_line, step.end_line);
    let Some(facts) = context.facts.get(step.side, &step.file) else {
        return (start, end);
    };
    let files = context.diff.files();
    let changed = changed_lines_within(files, &step.file, step.side, start, end);
    let markers: Vec<u32> = match step.side {
        LineSide::Head => files
            .iter()
            .filter(|file| file.path_on(LineSide::Head) == Some(step.file.as_str()))
            .flat_map(|file| deletion_markers(file, start, end))
            .map(|marker| marker.after_line)
            .collect(),
        LineSide::Base => Vec::new(),
    };
    // A marker after line m stays inside while the range starts at or before m + 1 and ends at or after m.
    let kept = |line: u32, marker_pins: &dyn Fn(u32) -> bool| {
        !facts.is_blank(line)
            || changed.binary_search(&line).is_ok()
            || step
                .annotations
                .iter()
                .any(|a| (a.start_line..=a.end_line).contains(&line))
            || markers.iter().any(|&marker| marker_pins(marker))
    };
    let mut first = start;
    while first < end && !kept(first, &|marker| marker.saturating_add(1) == first) {
        first += 1;
    }
    let mut last = end;
    while last > first && !kept(last, &|marker| marker == last) {
        last -= 1;
    }
    (first, last)
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
}
