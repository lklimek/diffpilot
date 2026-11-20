//! The Files view's file list: every file of the diff with its hunks and, per changed line, the owner the Tour
//! already decided. Built from the Tour and the inputs it was projected from (`tour::with_projection`), never from a
//! second analysis: Unexplained rows, Not analysed items and planned ranges are the Tour's own, accepted flows'
//! lines are the coverage's (`VerifiedFlow::covered`), quiet kinds follow the pre-pass priority.

use std::collections::HashMap;

use analysis::diff::{FileDiff, FileStatus, Hunk};
use analysis::lines::ranges;
use analysis::prepass::{Disposition, NotRenderedReason as PrepassNotRendered, Prepass};
use analysis::tech::is_line_rule_name;
use analysis::verify::spans::{Span, covered_twice, from_ranges as spans, marker_inside, touches};
use contracts::api::files::{
    FileChangeStatus, FileGroup, FileHunk, HunkPart, NotRenderedReason, PartOwner, PrFile,
};
use contracts::api::tour::{
    Flow, FlowAnalysisState, NotAnalysedItem, NotAnalysedSection, Step, UnexplainedRanges,
};
use contracts::common::{FileRanges, LineRange, LineSide, NotAnalysedCategory};
use contracts::ids::{FlowId, HunkId, StepId};

use crate::pipeline::tour::Projection;

/// Everything the file list is built from.
#[derive(Clone, Copy, Debug)]
pub struct FilesInput<'a> {
    /// The whole diff.
    pub files: &'a [FileDiff],
    /// The stored pre-pass.
    pub prepass: &'a Prepass,
    /// Accepted flows with the changed lines the plan places in them (coverage's input).
    pub covered: &'a [(&'a FlowId, &'a [FileRanges])],
    /// The Tour's flows in Tour order.
    pub flows: &'a [Flow],
    /// The Tour's Unexplained rows.
    pub unexplained: &'a [UnexplainedRanges],
    /// The Tour's Not analysed sections (binary files included).
    pub not_analysed: &'a [NotAnalysedSection],
}

/// The file list of the Tour `tour` projected from `projection`; Viewed marks are left unread.
pub fn from_tour(projection: &Projection<'_>, tour: &contracts::api::tour::Tour) -> Vec<PrFile> {
    let covered: Vec<(&FlowId, &[FileRanges])> = projection
        .skeleton
        .flows
        .iter()
        .map(|flow| (&flow.flow_id, flow.covered.as_slice()))
        .collect();
    project(&FilesInput {
        files: projection.files,
        prepass: projection.prepass,
        covered: &covered,
        flows: &tour.flows,
        unexplained: &tour.quiet.unexplained,
        not_analysed: &tour.quiet.not_analysed,
    })
}

/// Every file of the diff in diff order with its hunks and the owners of their changed lines.
pub fn project(input: &FilesInput<'_>) -> Vec<PrFile> {
    let dispositions: HashMap<&HunkId, Disposition> = input
        .prepass
        .hunks
        .iter()
        .map(|hunk| (&hunk.id, hunk.disposition))
        .collect();
    input
        .files
        .iter()
        .map(|file| {
            let group = group(input.not_analysed, file);
            let sides = [LineSide::Base, LineSide::Head].map(|side| {
                file.path_on(side)
                    .map(|path| SideOwners::new(input, side, path))
            });
            let hunks =
                file.hunks
                    .iter()
                    .map(|hunk| {
                        let not_analysed =
                            match dispositions.get(&hunk.id) {
                                Some(Disposition::NotAnalysed(category)) => Some(
                                    not_analysed_owner(input.not_analysed, file, *category, &group),
                                ),
                                _ => None,
                            };
                        file_hunk(file, hunk, &sides, not_analysed)
                    })
                    .collect();
            PrFile {
                path: file.path.clone(),
                old_path: file.old_path.clone(),
                status: match file.status {
                    FileStatus::Added => FileChangeStatus::Added,
                    FileStatus::Modified => FileChangeStatus::Modified,
                    FileStatus::Deleted => FileChangeStatus::Deleted,
                    FileStatus::Renamed => FileChangeStatus::Renamed,
                },
                binary: file.binary,
                added: count(file.hunks.iter().map(|hunk| hunk.added.len())),
                removed: count(file.hunks.iter().map(|hunk| hunk.removed.len())),
                group,
                not_rendered: not_rendered(input.prepass, file),
                viewed: None,
                hunks,
            }
        })
        .collect()
}

fn count(lengths: impl Iterator<Item = usize>) -> u32 {
    u32::try_from(lengths.sum::<usize>()).unwrap_or(u32::MAX)
}

fn not_rendered(prepass: &Prepass, file: &FileDiff) -> Option<NotRenderedReason> {
    [LineSide::Head, LineSide::Base].iter().find_map(|&side| {
        let path = file.path_on(side)?;
        let item = prepass
            .not_rendered
            .iter()
            .find(|item| item.side == side && item.file == path)?;
        Some(match item.reason {
            PrepassNotRendered::TooLarge => NotRenderedReason::TooLarge,
            PrepassNotRendered::Generated => NotRenderedReason::Generated,
            PrepassNotRendered::Binary => NotRenderedReason::Binary,
        })
    })
}

/// Whether `item` names `file` on its side.
fn names(item: &NotAnalysedItem, file: &FileDiff) -> bool {
    file.path_on(item.side) == Some(item.file.as_str())
}

/// The file's group: the first path-rule item naming it (head side first), else code.
fn group(sections: &[NotAnalysedSection], file: &FileDiff) -> FileGroup {
    let items = || {
        sections.iter().flat_map(|section| {
            section
                .items
                .iter()
                .filter(|item| !is_line_rule_name(&item.rule) && names(item, file))
                .map(move |item| (section.category, item))
        })
    };
    let head = items().find(|(_, item)| item.side == LineSide::Head);
    match head.or_else(|| items().next()) {
        Some((category, item)) => FileGroup::NotAnalysed {
            category,
            badge: item.badge,
            rule: item.rule.clone(),
        },
        None => FileGroup::Code,
    }
}

/// Owner of a hunk the pre-pass took out by a path rule: the file's item of that category.
fn not_analysed_owner(
    sections: &[NotAnalysedSection],
    file: &FileDiff,
    category: NotAnalysedCategory,
    group: &FileGroup,
) -> PartOwner {
    let item = sections
        .iter()
        .filter(|section| section.category == category)
        .flat_map(|section| &section.items)
        .find(|item| !is_line_rule_name(&item.rule) && names(item, file));
    match (item, group) {
        (Some(item), _) => PartOwner::NotAnalysed {
            category,
            badge: item.badge,
            rule: item.rule.clone(),
        },
        (None, FileGroup::NotAnalysed { badge, rule, .. }) => PartOwner::NotAnalysed {
            category,
            badge: *badge,
            rule: rule.clone(),
        },
        (None, FileGroup::Code) => {
            tracing::debug!(path = %file.path, "a not-analysed hunk without its rule; shown as Unexplained");
            PartOwner::Unexplained { reason: None }
        }
    }
}

/// Who owns changed lines of one file side, by priority.
enum Layer<'a> {
    /// A fixed owner (quiet kinds, Unexplained rows, planned lines of a waiting flow).
    Fixed(PartOwner),
    /// Lines an accepted flow covers; the step is resolved per segment.
    Covered(&'a FlowId),
}

/// A step's lines on one file side, with its flow.
struct StepAt<'a> {
    flow: &'a FlowId,
    step: &'a Step,
}

impl StepAt<'_> {
    fn span(&self) -> Span {
        (
            self.step.facts.range.start_line,
            self.step.facts.range.end_line,
        )
    }
}

/// The owners and steps of one side of one file.
struct SideOwners<'a> {
    side: LineSide,
    /// (sorted merged spans, owner) by priority.
    layers: Vec<(Vec<Span>, Layer<'a>)>,
    /// Steps of every flow on this side and path, Tour order.
    steps: Vec<StepAt<'a>>,
    /// Steps of every flow on the head path of the file (for removed lines at a deletion marker).
    head_steps: Vec<StepAt<'a>>,
    /// Owners of base lines covered by a moved step's original.
    moved_from: Vec<(Span, StepAt<'a>)>,
    /// Segment boundaries: a span starting at `b` or ending at `b - 1` changes owners.
    breaks: Vec<u32>,
    /// Per boundary `breaks[i]`: the indices into `steps` of the steps holding the lines from it to the next one.
    active: Vec<Vec<usize>>,
}

impl<'a> SideOwners<'a> {
    fn new(input: &FilesInput<'a>, side: LineSide, path: &str) -> Self {
        let mut layers: Vec<(Vec<Span>, Layer<'a>)> = Vec::new();
        for quiet in &input.prepass.quiet_moves {
            for end in [&quiet.from, &quiet.to] {
                if end.side == side && end.file == path {
                    let owner = PartOwner::IdenticalMove {
                        from: quiet.from.clone(),
                        to: quiet.to.clone(),
                    };
                    layers.push((vec![span_of(end.range)], Layer::Fixed(owner)));
                }
            }
        }
        let whitespace = ranges(&input.prepass.whitespace_lines(side, path));
        layers.push((spans(&whitespace), Layer::Fixed(PartOwner::WhitespaceOnly)));
        for section in input.not_analysed {
            for item in &section.items {
                if let (true, Some(lines)) = (is_line_rule_name(&item.rule), &item.ranges)
                    && item.side == side
                    && item.file == path
                {
                    // Test code is not analysed at all; comments and imports are context for the agent.
                    let owner = if section.category == NotAnalysedCategory::Tests {
                        PartOwner::NotAnalysed {
                            category: section.category,
                            badge: item.badge,
                            rule: item.rule.clone(),
                        }
                    } else {
                        PartOwner::ContextRule {
                            badge: item.badge,
                            rule: item.rule.clone(),
                        }
                    };
                    layers.push((spans(lines), Layer::Fixed(owner)));
                }
            }
        }
        for row in input.unexplained {
            if row.side == side && row.file == path {
                let owner = PartOwner::Unexplained { reason: row.reason };
                layers.push((spans(&row.ranges), Layer::Fixed(owner)));
            }
        }
        for (flow, covered) in input.covered {
            let lines: Vec<LineRange> = covered
                .iter()
                .filter(|lines| lines.side == side && lines.file == path)
                .flat_map(|lines| lines.ranges.iter().copied())
                .collect();
            layers.push((spans(&lines), Layer::Covered(flow)));
        }
        for flow in input.flows.iter().filter(|flow| waiting(flow)) {
            let lines: Vec<LineRange> = flow
                .planned
                .iter()
                .flat_map(|planned| &planned.ranges)
                .filter(|lines| lines.side == side && lines.file == path)
                .flat_map(|lines| lines.ranges.iter().copied())
                .collect();
            let owner = PartOwner::Flow {
                flow_id: flow.id.clone(),
            };
            layers.push((spans(&lines), Layer::Fixed(owner)));
        }
        layers.retain(|(spans, _)| !spans.is_empty());
        let covered: Vec<Vec<Span>> = layers
            .iter()
            .filter(|(_, layer)| matches!(layer, Layer::Covered(_)))
            .map(|(spans, _)| spans.clone())
            .collect();
        let twice = covered_twice(&covered);
        if !twice.is_empty() {
            // Verification normally keeps flows apart; the first flow in Tour order owns such lines.
            tracing::debug!(
                path,
                ?side,
                spans = twice.len(),
                "changed lines covered by two accepted flows"
            );
        }
        let steps_on = |side: LineSide, path: Option<&str>| -> Vec<StepAt<'a>> {
            input
                .flows
                .iter()
                .flat_map(|flow| {
                    flow.steps.iter().map(move |step| StepAt {
                        flow: &flow.id,
                        step,
                    })
                })
                .filter(|at| {
                    at.step.facts.side == side && Some(at.step.facts.file.as_str()) == path
                })
                .collect()
        };
        let head_path = input
            .files
            .iter()
            .find(|file| file.path_on(side) == Some(path))
            .and_then(|file| file.path_on(LineSide::Head));
        let moved_from: Vec<(Span, StepAt<'a>)> = if side == LineSide::Base {
            input
                .flows
                .iter()
                .flat_map(|flow| {
                    flow.steps.iter().map(move |step| StepAt {
                        flow: &flow.id,
                        step,
                    })
                })
                .filter_map(|at| {
                    let from = &at.step.facts.moved.as_ref()?.from;
                    (from.side == LineSide::Base && from.file == path)
                        .then(|| (span_of(from.range), at))
                })
                .collect()
        } else {
            Vec::new()
        };
        let steps = steps_on(side, Some(path));
        let mut breaks: Vec<u32> = layers
            .iter()
            .flat_map(|(spans, _)| spans.iter())
            .chain(moved_from.iter().map(|(span, _)| span))
            .copied()
            .chain(steps.iter().map(StepAt::span))
            .flat_map(|(first, last)| [first, last.saturating_add(1)])
            .collect();
        breaks.sort_unstable();
        breaks.dedup();
        let active = active_steps(&steps, &breaks);
        Self {
            side,
            layers,
            active,
            steps,
            head_steps: if side == LineSide::Base {
                steps_on(LineSide::Head, head_path)
            } else {
                Vec::new()
            },
            moved_from,
            breaks,
        }
    }

    /// The parts of the changed lines `span` of a hunk whose removed lines sit after head line `marker`.
    fn parts(&self, (first, last): Span, marker: Option<u32>, out: &mut Vec<HunkPart>) {
        // Boundaries at or before `first`; the segment holding `first` starts at boundary `next - 1`.
        let mut next = self.breaks.partition_point(|&b| b <= first);
        let mut start = first;
        loop {
            let end = match self.breaks.get(next) {
                Some(&b) if b <= last => b - 1,
                _ => last,
            };
            let active = next
                .checked_sub(1)
                .map_or(&[][..], |at| self.active[at].as_slice());
            let (owner, step) = self.owner(start, marker, active);
            let also_in: Vec<StepId> = active
                .iter()
                .map(|&at| &self.steps[at].step.id)
                .filter(|id| Some(*id) != step)
                .cloned()
                .collect();
            match out.last_mut() {
                Some(previous)
                    if previous.side == self.side
                        && previous.range.end_line + 1 == start
                        && previous.owner == owner
                        && previous.also_in == also_in =>
                {
                    previous.range.end_line = end;
                }
                _ => out.push(HunkPart {
                    side: self.side,
                    range: LineRange {
                        start_line: start,
                        end_line: end,
                    },
                    owner,
                    also_in,
                }),
            }
            if end == last {
                return;
            }
            start = end + 1;
            next += 1;
        }
    }

    /// The owner of `line` (and of its segment), with the owning step.
    fn owner(
        &self,
        line: u32,
        marker: Option<u32>,
        active: &[usize],
    ) -> (PartOwner, Option<&'a StepId>) {
        let layer = self
            .layers
            .iter()
            .find(|(spans, _)| covers(spans, line))
            .map(|(_, layer)| layer);
        match layer {
            Some(Layer::Fixed(owner)) => (owner.clone(), None),
            Some(Layer::Covered(flow)) => match self.step_of(flow, line, marker, active) {
                Some(step) => (
                    PartOwner::Step {
                        flow_id: (*flow).clone(),
                        step_id: step.clone(),
                    },
                    Some(step),
                ),
                None => (
                    PartOwner::Flow {
                        flow_id: (*flow).clone(),
                    },
                    None,
                ),
            },
            None => {
                tracing::debug!(line, side = ?self.side, "a changed line without an owner; shown as Unexplained");
                (PartOwner::Unexplained { reason: None }, None)
            }
        }
    }

    /// The step of `flow` that explains `line` (`active`: the steps holding it): a step holding it; for a removed
    /// line the moved step of its original, else the head step at its deletion marker (the verification's rule).
    fn step_of(
        &self,
        flow: &FlowId,
        line: u32,
        marker: Option<u32>,
        active: &[usize],
    ) -> Option<&'a StepId> {
        if let Some(at) = active
            .iter()
            .map(|&at| &self.steps[at])
            .find(|at| at.flow == flow)
        {
            return Some(&at.step.id);
        }
        if let Some((_, at)) = self
            .moved_from
            .iter()
            .find(|(span, at)| at.flow == flow && contains(*span, line))
        {
            return Some(&at.step.id);
        }
        let marker = marker?;
        self.head_steps
            .iter()
            .filter(|at| at.flow == flow)
            .find(|at| marker_inside(marker, at.span()))
            .map(|at| &at.step.id)
    }
}

/// For each boundary, the steps (indices, Tour order) holding the lines from it to the next boundary: one sweep,
/// as every step starts and ends at boundaries.
fn active_steps(steps: &[StepAt<'_>], breaks: &[u32]) -> Vec<Vec<usize>> {
    let mut starts: Vec<(u32, usize)> = steps
        .iter()
        .enumerate()
        .map(|(at, step)| (step.span().0, at))
        .collect();
    let mut ends: Vec<(u32, usize)> = steps
        .iter()
        .enumerate()
        .map(|(at, step)| (step.span().1.saturating_add(1), at))
        .collect();
    starts.sort_unstable();
    ends.sort_unstable();
    let (mut started, mut ended) = (starts.iter().peekable(), ends.iter().peekable());
    let mut open = std::collections::BTreeSet::new();
    breaks
        .iter()
        .map(|&at| {
            while let Some(&(_, step)) = started.next_if(|(line, _)| *line <= at) {
                open.insert(step);
            }
            while let Some(&(_, step)) = ended.next_if(|(line, _)| *line <= at) {
                open.remove(&step);
            }
            open.iter().copied().collect()
        })
        .collect()
}

/// A flow whose planned lines are shown under it: planned, without steps yet, not failed.
fn waiting(flow: &Flow) -> bool {
    flow.steps.is_empty()
        && flow
            .analysis_status
            .as_ref()
            .is_none_or(|status| status.state != FlowAnalysisState::Failed)
}

fn span_of(range: LineRange) -> Span {
    (range.start_line, range.end_line)
}

fn contains((first, last): Span, line: u32) -> bool {
    first <= line && line <= last
}

/// Whether the sorted, merged `spans` hold `line`: a binary search.
fn covers(spans: &[Span], line: u32) -> bool {
    touches(spans, (line, line))
}

fn file_hunk(
    file: &FileDiff,
    hunk: &Hunk,
    sides: &[Option<SideOwners<'_>>; 2],
    not_analysed: Option<PartOwner>,
) -> FileHunk {
    let base = run(&hunk.removed);
    let head = run(&hunk.added);
    let has_head = file.path_on(LineSide::Head).is_some();
    let marker = has_head.then(|| hunk.deletion_marker());
    let mut parts = Vec::new();
    for (lines, owners) in [(&hunk.removed, &sides[0]), (&hunk.added, &sides[1])] {
        let Some(owners) = owners else {
            continue;
        };
        for range in ranges(lines) {
            match &not_analysed {
                Some(owner) => parts.push(HunkPart {
                    side: owners.side,
                    range,
                    owner: owner.clone(),
                    also_in: Vec::new(),
                }),
                None => owners.parts(span_of(range), marker, &mut parts),
            }
        }
    }
    FileHunk {
        id: hunk.id.clone(),
        base,
        head,
        after_line: if head.is_none() { marker } else { None },
        parts,
    }
}

/// The lines as one range (a hunk's lines on one side are contiguous); null when there are none.
fn run(lines: &[u32]) -> Option<LineRange> {
    Some(LineRange {
        start_line: *lines.iter().min()?,
        end_line: *lines.iter().max()?,
    })
}

#[cfg(test)]
mod tests;
