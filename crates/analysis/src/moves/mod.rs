//! Moved-block detection and whitespace-only changes. Candidates are found liberally: removed and added runs that
//! share a whitespace-normalised line are aligned with `similar`, and a cluster of aligned lines is a candidate.
//! Confirmation is conservative: the whole spans are compared again line by line, byte for byte (only the line
//! terminator is normalised); every line that does not match, blank lines and indentation included, is a differing
//! line. Renamed files are moved blocks of the whole file.
//!
//! Work: anchors O(L); pairing and confirmation each bounded by `MOVE_WORK_BUDGET` (an alignment costs `(o + n)²`,
//! an upper bound of Myers); selection O(K log K); over budget, the remaining candidates give no move facts and the
//! report is marked `incomplete`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use contracts::agent::analyse_flow::MovedFrom;
use contracts::api::code::MovedLines;
use contracts::api::tour::{IdenticalMove, MoveFact};
use contracts::common::{FileRange, LineRange, LineSide, ProvenanceBadge};
use serde::{Deserialize, Serialize};
use similar::{Algorithm, DiffOp, capture_diff_slices};

use crate::diff::{FileDiff, FileStatus, Hunk};

/// A block moved between base and head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovedBlock {
    /// Original block at the base commit.
    pub from: FileRange,
    /// New block at the head commit.
    pub to: FileRange,
    /// Every line equal to its counterpart (badge "text identical"; a quiet row).
    pub identical: bool,
    /// Head lines that differ from the original.
    pub differing_head_lines: Vec<u32>,
    /// Base lines of the original that differ from the new block.
    pub differing_base_lines: Vec<u32>,
    /// Lines of `from` the pull request removed; a moved step claiming this block accounts for them.
    pub removed_base_lines: Vec<u32>,
    /// Equal base and head lines, in order: the line mapping of the block.
    pub aligned: Vec<AlignedRun>,
}

/// Consecutive equal lines of a moved block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlignedRun {
    /// First base line.
    pub base_start: u32,
    /// First head line.
    pub head_start: u32,
    /// Number of lines.
    pub len: u32,
}

/// Deterministic move and whitespace facts of a pull request.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovesReport {
    /// Moved blocks.
    pub moved: Vec<MovedBlock>,
    /// Changed head ranges whose change is whitespace only (kept in the agent's hunks, marked as a fact).
    pub whitespace_only: Vec<FileRange>,
    /// The work budget ran out: some candidate moves were not checked (their lines stay with the agent).
    pub incomplete: bool,
}

/// A moved step's claim confirmed against a deterministic moved block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmedMove {
    /// The move fact of the step.
    pub fact: MoveFact,
    /// Removed base lines of the claimed original: covered by the moved step, also across files.
    pub covered_base_lines: Vec<u32>,
}

/// Fewest aligned non-blank lines in a moved block.
const MIN_LINES: usize = 3;
/// Fewest alphanumeric characters in the aligned lines of a block (git's `--color-moved` default).
const MIN_ALNUM: usize = 20;
/// Most non-blank lines between two aligned stretches of one block.
const MAX_GAP: usize = 3;
/// A same-file block whose new place is at most this many lines from its old place is an edit, not a move.
const IN_PLACE_LINES: u32 = 3;
/// A line occurring in more added runs than this does not pair runs (`}`-like noise).
const MAX_ANCHOR_RUNS: usize = 64;
/// Fewest alphanumeric characters of a line that pairs runs.
const MIN_ANCHOR_ALNUM: usize = 3;
/// Longest run (non-blank lines) aligned with another.
const MAX_RUN_LINES: usize = 20_000;
/// Alignment work of one phase (pairing, confirmation) for one pull request, in `(o + n)²` units.
const MOVE_WORK_BUDGET: u64 = 400_000_000;

/// Moved blocks and whitespace-only changes of `files`. `text` gives a side's file text; sides without text
/// (binary, not read, or excluded by the caller) take no part.
pub fn analyse<'t>(
    files: &[FileDiff],
    text: &dyn Fn(LineSide, &str) -> Option<&'t str>,
) -> MovesReport {
    analyse_within(files, text, MOVE_WORK_BUDGET)
}

fn analyse_within<'t>(
    files: &[FileDiff],
    text: &dyn Fn(LineSide, &str) -> Option<&'t str>,
    budget: u64,
) -> MovesReport {
    let texts = Texts::new(files, text);
    let (mut moved, incomplete) = blocks(files, &texts, budget);
    moved.extend(renamed_files(files, &texts));
    moved.sort_by(|a, b| {
        (&a.to.file, a.to.range, &a.from.file, a.from.range).cmp(&(
            &b.to.file,
            b.to.range,
            &b.from.file,
            b.from.range,
        ))
    });
    let mut whitespace_only = Vec::new();
    for (index, file) in files.iter().enumerate() {
        for hunk in &file.hunks {
            let removed = normalised(texts.get(index, LineSide::Base), &hunk.removed);
            let added = normalised(texts.get(index, LineSide::Head), &hunk.added);
            if removed.is_some() && removed == added {
                whitespace_only.extend(hunk_range(file, hunk));
            }
        }
    }
    MovesReport {
        moved,
        whitespace_only,
        incomplete,
    }
}

impl MovesReport {
    /// Moves whose text is identical: the quiet rows.
    pub fn identical(&self) -> Vec<IdenticalMove> {
        self.moved
            .iter()
            .filter(|block| block.identical)
            .map(|block| IdenticalMove {
                from: block.from.clone(),
                to: block.to.clone(),
                badge: ProvenanceBadge::TextIdentical,
            })
            .collect()
    }

    /// Moved blocks of the head file `path` overlapping lines `from..=to` (the lines endpoint).
    pub fn moved_lines(&self, path: &str, from: u32, to: u32) -> Vec<MovedLines> {
        let window = LineRange {
            start_line: from,
            end_line: to,
        };
        self.moved
            .iter()
            .filter(|block| block.to.file == path && overlaps(block.to.range, window))
            .map(|block| MovedLines {
                range: block.to.range,
                differing: block.differing_head_lines.clone(),
            })
            .collect()
    }

    /// The single check of a moved step's claim: the step at head lines `step` of `step_file` claims `claim` as its
    /// original. Confirmed only when a deterministic block contains both ranges and they correspond exactly: each
    /// end of the step maps through the block's aligned lines (or the block's own end) to the same end of the claim.
    /// Then the fact counts the differing lines inside the two ranges (`text_identical` when none) and
    /// `covered_base_lines` are the removed base lines of the claim, which the step accounts for. Otherwise `None`:
    /// no fact, and the claim's removed lines stay uncovered.
    pub fn confirm(
        &self,
        step_file: &str,
        step: LineRange,
        claim: &MovedFrom,
    ) -> Option<ConfirmedMove> {
        let claimed = LineRange {
            start_line: claim.base_start,
            end_line: claim.base_end,
        };
        if step.start_line > step.end_line || claimed.start_line > claimed.end_line {
            return None;
        }
        let mut blocks: Vec<&MovedBlock> = self
            .moved
            .iter()
            .filter(|block| {
                block.from.file == claim.file
                    && block.to.file == step_file
                    && contains(block.to.range, step)
                    && contains(block.from.range, claimed)
            })
            .collect();
        blocks.sort_by_key(|block| block.from.range.end_line - block.from.range.start_line);
        let block = blocks.into_iter().find(|block| {
            let start = block.base_of(step.start_line).or_else(|| {
                (step.start_line == block.to.range.start_line)
                    .then_some(block.from.range.start_line)
            });
            let end = block.base_of(step.end_line).or_else(|| {
                (step.end_line == block.to.range.end_line).then_some(block.from.range.end_line)
            });
            start == Some(claimed.start_line) && end == Some(claimed.end_line)
        })?;
        let differing = within(&block.differing_head_lines, step)
            .len()
            .max(within(&block.differing_base_lines, claimed).len());
        Some(ConfirmedMove {
            fact: MoveFact {
                from: file_range(
                    &claim.file,
                    LineSide::Base,
                    claimed.start_line,
                    claimed.end_line,
                ),
                badge: (differing == 0).then_some(ProvenanceBadge::TextIdentical),
                differing_lines: u32::try_from(differing).unwrap_or(u32::MAX),
            },
            covered_base_lines: within(&block.removed_base_lines, claimed),
        })
    }

    /// The origin of a moved block containing the whole step at head lines `step` of `step_file` when `claim`
    /// contradicts it: the claim names another file or base lines outside that origin. `None` when no block
    /// contains the step (a step wider than a block) or the claim overlaps an origin, confirmed by `confirm` or not.
    pub fn contradicted_origin(
        &self,
        step_file: &str,
        step: LineRange,
        claim: &MovedFrom,
    ) -> Option<&FileRange> {
        let claimed = LineRange {
            start_line: claim.base_start,
            end_line: claim.base_end,
        };
        if step.start_line > step.end_line {
            return None;
        }
        let mut containing = self
            .moved
            .iter()
            .filter(|block| block.to.file == step_file && contains(block.to.range, step))
            .peekable();
        let origin = &containing.peek()?.from;
        let consistent = containing
            .any(|block| block.from.file == claim.file && overlaps(block.from.range, claimed));
        (!consistent).then_some(origin)
    }
}

impl MovedBlock {
    /// The base line aligned with head line `head`, if it is aligned.
    fn base_of(&self, head: u32) -> Option<u32> {
        let at = self
            .aligned
            .partition_point(|run| run.head_start + run.len <= head);
        self.aligned
            .get(at)
            .filter(|run| run.head_start <= head)
            .map(|run| run.base_start + (head - run.head_start))
    }
}

/// Remaining alignment work.
struct Budget {
    left: u64,
    exhausted: bool,
}

impl Budget {
    fn new(left: u64) -> Self {
        Self {
            left,
            exhausted: false,
        }
    }

    /// Takes the cost of aligning `old` against `new` lines; false (nothing taken) when it does not fit.
    fn take(&mut self, old: usize, new: usize) -> bool {
        let size = u64::try_from(old + new).unwrap_or(u64::MAX);
        let cost = size.saturating_mul(size);
        let fits = cost <= self.left;
        if fits {
            self.left -= cost;
        } else {
            self.exhausted = true;
        }
        fits
    }
}

/// The lines of each file side, split once.
struct Texts<'t> {
    sides: Vec<[Option<Vec<&'t str>>; 2]>,
}

impl<'t> Texts<'t> {
    fn new(files: &[FileDiff], text: &dyn Fn(LineSide, &str) -> Option<&'t str>) -> Self {
        let sides = files
            .iter()
            .map(|file| {
                [LineSide::Base, LineSide::Head].map(|side| {
                    file.path_on(side)
                        .filter(|_| !file.binary)
                        .and_then(|path| text(side, path))
                        .map(|text| text.lines().collect())
                })
            })
            .collect();
        Self { sides }
    }

    fn get(&self, file: usize, side: LineSide) -> Option<&[&'t str]> {
        let slot = usize::from(side == LineSide::Head);
        self.sides.get(file)?.get(slot)?.as_deref()
    }

    /// Lines `start..=end` of a file side.
    fn span(&self, file: usize, side: LineSide, start: u32, end: u32) -> Option<&[&'t str]> {
        let first = usize::try_from(start).ok()?.checked_sub(1)?;
        let last = usize::try_from(end).ok()?.checked_sub(1)?;
        self.get(file, side)?.get(first..=last)
    }
}

/// Removed or added lines of one hunk: non-blank line numbers and their normalised text.
struct Run {
    file: usize,
    hunk: usize,
    lines: Vec<u32>,
    norm: Vec<String>,
}

/// A candidate block before overlap resolution and confirmation.
struct Candidate {
    from_file: usize,
    to_file: usize,
    base: (u32, u32),
    head: (u32, u32),
    aligned: usize,
}

fn runs(files: &[FileDiff], texts: &Texts<'_>, side: LineSide) -> Vec<Run> {
    let mut out = Vec::new();
    for (file, diff) in files.iter().enumerate() {
        let Some(text) = texts.get(file, side) else {
            continue;
        };
        for (hunk, h) in diff.hunks.iter().enumerate() {
            let numbers = match side {
                LineSide::Base => &h.removed,
                LineSide::Head => &h.added,
            };
            let mut run = Run {
                file,
                hunk,
                lines: Vec::new(),
                norm: Vec::new(),
            };
            for &number in numbers {
                let norm = line_at(text, number).map(normalise).unwrap_or_default();
                if !norm.is_empty() {
                    run.lines.push(number);
                    run.norm.push(norm);
                }
            }
            if !run.lines.is_empty() && run.lines.len() <= MAX_RUN_LINES {
                out.push(run);
            }
        }
    }
    out
}

/// Candidates by pairing, then confirmation; each phase has its own work budget of `limit`. Also whether a budget
/// ran out.
fn blocks(files: &[FileDiff], texts: &Texts<'_>, limit: u64) -> (Vec<MovedBlock>, bool) {
    let mut budget = Budget::new(limit);
    let removed = runs(files, texts, LineSide::Base);
    let added = runs(files, texts, LineSide::Head);
    let mut anchors: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, run) in added.iter().enumerate() {
        for norm in run
            .norm
            .iter()
            .filter(|norm| alnum(norm) >= MIN_ANCHOR_ALNUM)
        {
            let list = anchors.entry(norm.as_str()).or_default();
            if list.last() != Some(&index) {
                list.push(index);
            }
        }
    }
    let mut candidates = Vec::new();
    'pairing: for old in &removed {
        let paired: BTreeSet<usize> = old
            .norm
            .iter()
            .filter_map(|norm| anchors.get(norm.as_str()))
            .filter(|list| list.len() <= MAX_ANCHOR_RUNS)
            .flatten()
            .copied()
            .collect();
        for new in paired.into_iter().filter_map(|index| added.get(index)) {
            if (old.file, old.hunk) == (new.file, new.hunk) {
                continue;
            }
            if !budget.take(old.norm.len(), new.norm.len()) {
                break 'pairing;
            }
            candidates.extend(clusters(files, old, new));
        }
    }
    candidates.sort_by(|a, b| {
        b.aligned.cmp(&a.aligned).then_with(|| {
            (a.from_file, a.base, a.to_file, a.head).cmp(&(b.from_file, b.base, b.to_file, b.head))
        })
    });
    let mut confirming = Budget::new(limit);
    let mut used: BTreeMap<(LineSide, usize), BTreeMap<u32, u32>> = BTreeMap::new();
    let mut out = Vec::new();
    for candidate in candidates {
        let spans = [
            ((LineSide::Base, candidate.from_file), candidate.base),
            ((LineSide::Head, candidate.to_file), candidate.head),
        ];
        // Taken spans are disjoint: only the last one starting at or before `end` can overlap.
        let free = spans.iter().all(|(key, (start, end))| {
            used.get(key).is_none_or(|taken| {
                taken
                    .range(..=*end)
                    .next_back()
                    .is_none_or(|(_, taken_end)| taken_end < start)
            })
        });
        if !free {
            continue;
        }
        for (key, (start, end)) in spans {
            used.entry(key).or_default().insert(start, end);
        }
        out.extend(confirmed(files, texts, &candidate, &mut confirming));
    }
    (out, budget.exhausted || confirming.exhausted)
}

/// Blocks of `new` aligned with `old`: stretches of equal lines with gaps of at most `MAX_GAP` lines.
fn clusters(files: &[FileDiff], old: &Run, new: &Run) -> Vec<Candidate> {
    let equal = capture_diff_slices(Algorithm::Patience, &old.norm, &new.norm)
        .into_iter()
        .filter_map(|op| match op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => Some((old_index, new_index, len)),
            _ => None,
        });
    let mut groups: Vec<Vec<(usize, usize, usize)>> = Vec::new();
    for stretch in equal {
        let joins = groups
            .last()
            .and_then(|group| group.last())
            .is_some_and(|&(o, n, len)| {
                stretch.0.saturating_sub(o + len) <= MAX_GAP
                    && stretch.1.saturating_sub(n + len) <= MAX_GAP
            });
        match groups.last_mut() {
            Some(group) if joins => group.push(stretch),
            _ => groups.push(vec![stretch]),
        }
    }
    groups
        .into_iter()
        .filter_map(|group| {
            let (&(old_first, new_first, _), &(old_last, new_last, last_len)) =
                (group.first()?, group.last()?);
            let aligned: usize = group.iter().map(|&(_, _, len)| len).sum();
            let chars: usize = group
                .iter()
                .flat_map(|&(o, _, len)| o..o + len)
                .filter_map(|i| old.norm.get(i))
                .map(|norm| alnum(norm))
                .sum();
            let old_len = old_last + last_len - old_first;
            let new_len = new_last + last_len - new_first;
            let enough = aligned >= MIN_LINES
                && chars >= MIN_ALNUM
                && 2 * aligned >= old_len
                && 2 * aligned >= new_len;
            let base = (
                *old.lines.get(old_first)?,
                *old.lines.get(old_last + last_len - 1)?,
            );
            let head = (
                *new.lines.get(new_first)?,
                *new.lines.get(new_last + last_len - 1)?,
            );
            (enough && !in_place(files, old, new.file, head.0)).then_some(Candidate {
                from_file: old.file,
                to_file: new.file,
                base,
                head,
                aligned,
            })
        })
        .collect()
}

/// The candidate confirmed line by line over its whole spans (blank lines included); `None` when its text is
/// missing or the budget is spent.
fn confirmed(
    files: &[FileDiff],
    texts: &Texts<'_>,
    candidate: &Candidate,
    budget: &mut Budget,
) -> Option<MovedBlock> {
    let from = files.get(candidate.from_file)?.path_on(LineSide::Base)?;
    let to = files.get(candidate.to_file)?.path_on(LineSide::Head)?;
    let (base_start, base_end) = candidate.base;
    let (head_start, head_end) = candidate.head;
    let base = texts.span(candidate.from_file, LineSide::Base, base_start, base_end)?;
    let head = texts.span(candidate.to_file, LineSide::Head, head_start, head_end)?;
    if !budget.take(base.len(), head.len()) {
        return None;
    }
    let mut aligned = Vec::new();
    let (mut differing_base, mut differing_head) = (Vec::new(), Vec::new());
    let line = |start: u32, index: usize| start + u32::try_from(index).unwrap_or(u32::MAX);
    for op in capture_diff_slices(Algorithm::Myers, base, head) {
        let (_, old, new) = op.as_tag_tuple();
        if let DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = op
        {
            aligned.push(AlignedRun {
                base_start: line(base_start, old_index),
                head_start: line(head_start, new_index),
                len: u32::try_from(len).unwrap_or(u32::MAX),
            });
        } else {
            differing_base.extend(old.map(|index| line(base_start, index)));
            differing_head.extend(new.map(|index| line(head_start, index)));
        }
    }
    Some(MovedBlock {
        from: file_range(from, LineSide::Base, base_start, base_end),
        to: file_range(to, LineSide::Head, head_start, head_end),
        identical: differing_base.is_empty() && differing_head.is_empty(),
        differing_head_lines: differing_head,
        differing_base_lines: differing_base,
        removed_base_lines: (base_start..=base_end).collect(),
        aligned,
    })
}

/// Lines aligned in the same file next to where they were removed: an edit (re-indent), not a move.
fn in_place(files: &[FileDiff], old: &Run, new_file: usize, first_head_line: u32) -> bool {
    let Some(hunk) = files
        .get(old.file)
        .and_then(|file| file.hunks.get(old.hunk))
    else {
        return false;
    };
    let marker = hunk.deletion_marker();
    new_file == old.file && (marker + 1).abs_diff(first_head_line) <= IN_PLACE_LINES
}

/// A renamed file is a moved block of the whole file: unchanged lines are aligned, every added and removed line
/// differs; identical only when git reports no hunk.
fn renamed_files(files: &[FileDiff], texts: &Texts<'_>) -> Vec<MovedBlock> {
    let mut out = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let (FileStatus::Renamed, Some(base_path), Some(base), Some(head)) = (
            file.status,
            file.path_on(LineSide::Base),
            texts.get(index, LineSide::Base),
            texts.get(index, LineSide::Head),
        ) else {
            continue;
        };
        let (Ok(base_end @ 1..), Ok(head_end @ 1..)) =
            (u32::try_from(base.len()), u32::try_from(head.len()))
        else {
            continue;
        };
        let mut aligned = Vec::new();
        let (mut next_base, mut next_head) = (1u32, 1u32);
        let mut equal = |until_base: u32, next_base: &mut u32, next_head: &mut u32| {
            let len = (until_base + 1).saturating_sub(*next_base);
            if len > 0 {
                aligned.push(AlignedRun {
                    base_start: *next_base,
                    head_start: *next_head,
                    len,
                });
            }
            *next_base += len;
            *next_head += len;
        };
        for hunk in &file.hunks {
            let before = if hunk.base.len > 0 {
                hunk.base.start.saturating_sub(1)
            } else {
                hunk.base.start
            };
            equal(before, &mut next_base, &mut next_head);
            next_base += hunk.base.len;
            next_head += hunk.head.len;
        }
        let remaining = (base_end + 1)
            .saturating_sub(next_base)
            .min((head_end + 1).saturating_sub(next_head));
        equal(next_base + remaining - 1, &mut next_base, &mut next_head);
        out.push(MovedBlock {
            from: file_range(base_path, LineSide::Base, 1, base_end),
            to: file_range(&file.path, LineSide::Head, 1, head_end),
            identical: file.hunks.is_empty(),
            differing_head_lines: file.hunks.iter().flat_map(|h| h.added.clone()).collect(),
            differing_base_lines: file.hunks.iter().flat_map(|h| h.removed.clone()).collect(),
            removed_base_lines: file.hunks.iter().flat_map(|h| h.removed.clone()).collect(),
            aligned,
        });
    }
    out
}

/// The non-blank lines of `numbers`, normalised; `None` when the text or a line is missing.
fn normalised(text: Option<&[&str]>, numbers: &[u32]) -> Option<Vec<String>> {
    let text = text?;
    let mut out = Vec::with_capacity(numbers.len());
    for &number in numbers {
        let norm = normalise(line_at(text, number)?);
        if !norm.is_empty() {
            out.push(norm);
        }
    }
    Some(out)
}

/// The changed range of a hunk on the head side, or on the base side when it only removes lines.
fn hunk_range(file: &FileDiff, hunk: &Hunk) -> Option<FileRange> {
    let (side, span) = if hunk.head.len > 0 {
        (LineSide::Head, hunk.head)
    } else {
        (LineSide::Base, hunk.base)
    };
    if span.len == 0 {
        return None;
    }
    let path = file.path_on(side)?;
    Some(file_range(
        path,
        side,
        span.start,
        span.start + span.len - 1,
    ))
}

fn line_at<'t>(lines: &[&'t str], number: u32) -> Option<&'t str> {
    lines
        .get(usize::try_from(number).ok()?.checked_sub(1)?)
        .copied()
}

/// Whitespace runs collapsed to one space, ends trimmed: for finding candidates only, never for a fact of identity.
fn normalise(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn alnum(text: &str) -> usize {
    text.chars().filter(|c| c.is_alphanumeric()).count()
}

fn within(lines: &[u32], range: LineRange) -> Vec<u32> {
    lines
        .iter()
        .copied()
        .filter(|line| (range.start_line..=range.end_line).contains(line))
        .collect()
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

fn overlaps(a: LineRange, b: LineRange) -> bool {
    a.start_line <= b.end_line && b.start_line <= a.end_line
}

fn contains(outer: LineRange, inner: LineRange) -> bool {
    outer.start_line <= inner.start_line && inner.end_line <= outer.end_line
}

#[cfg(test)]
mod tests {
    use contracts::ids::HunkId;

    use super::*;
    use crate::diff::LineSpan;

    fn hunk(base: (u32, u32), head: (u32, u32)) -> Hunk {
        let base = LineSpan {
            start: base.0,
            len: base.1,
        };
        let head = LineSpan {
            start: head.0,
            len: head.1,
        };
        Hunk {
            id: HunkId("h".to_owned()),
            base,
            head,
            added: (head.start..head.start + head.len).collect(),
            removed: (base.start..base.start + base.len).collect(),
        }
    }

    fn file(path: &str, old_path: Option<&str>, status: FileStatus, hunks: Vec<Hunk>) -> FileDiff {
        FileDiff {
            path: path.to_owned(),
            old_path: old_path.map(str::to_owned),
            status,
            binary: false,
            hunks,
        }
    }

    fn range(start_line: u32, end_line: u32) -> LineRange {
        LineRange {
            start_line,
            end_line,
        }
    }

    type Sources = HashMap<(LineSide, String), String>;

    fn src(side: LineSide, path: &str, lines: &[&str]) -> ((LineSide, String), String) {
        let text = lines.iter().map(|line| format!("{line}\n")).collect();
        ((side, path.to_owned()), text)
    }

    fn sources<const N: usize>(entries: [((LineSide, String), String); N]) -> Sources {
        entries.into_iter().collect()
    }

    fn run(files: &[FileDiff], sources: &Sources) -> MovesReport {
        analyse(files, &|side, path| {
            sources.get(&(side, path.to_owned())).map(String::as_str)
        })
    }

    const FUNCTION: [&str; 5] = [
        "fn build(config: &Config) -> Request {",
        "    let url = config.endpoint.join(PATH);",
        "    let request = Request::new(url).header(CLIENT, NAME);",
        "    request.with_timeout(config.timeout)",
        "}",
    ];

    fn indented(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| format!("    {line}")).collect()
    }

    #[test]
    fn reindented_cross_file_move_is_a_differing_move() {
        let moved = indented(&FUNCTION);
        let mut head_b = vec!["mod x {"];
        head_b.extend(moved.iter().map(String::as_str));
        head_b.push("}");
        let mut base_a = vec!["use x;", ""];
        base_a.extend(FUNCTION);
        base_a.extend(["", "fn other() {}"]);
        let files = vec![
            file(
                "src/a.rs",
                None,
                FileStatus::Modified,
                vec![hunk((3, 6), (2, 0))],
            ),
            file(
                "src/b.rs",
                None,
                FileStatus::Added,
                vec![hunk((0, 0), (1, 7))],
            ),
        ];
        let sources = sources([
            src(LineSide::Base, "src/a.rs", &base_a),
            src(LineSide::Head, "src/a.rs", &["use x;", "", "fn other() {}"]),
            src(LineSide::Head, "src/b.rs", &head_b),
        ]);
        let report = run(&files, &sources);
        assert_eq!(report.moved.len(), 1, "{report:?}");
        let block = &report.moved[0];
        assert_eq!(
            (block.from.clone(), block.to.clone()),
            (
                file_range("src/a.rs", LineSide::Base, 3, 7),
                file_range("src/b.rs", LineSide::Head, 2, 6)
            ),
            "the trailing blank line is not part of the block"
        );
        assert!(!block.identical, "indentation is part of the bytes");
        assert_eq!(block.differing_head_lines, [2, 3, 4, 5, 6]);
        assert_eq!(block.differing_base_lines, [3, 4, 5, 6, 7]);
        assert_eq!(block.removed_base_lines, [3, 4, 5, 6, 7]);
        assert!(report.identical().is_empty());
        assert_eq!(
            report.moved_lines("src/b.rs", 1, 3),
            [MovedLines {
                range: range(2, 6),
                differing: vec![2, 3, 4, 5, 6]
            }]
        );
        assert!(report.moved_lines("src/b.rs", 7, 9).is_empty());
        let claim = MovedFrom {
            file: "src/a.rs".to_owned(),
            base_start: 3,
            base_end: 7,
        };
        let confirmed = report
            .confirm("src/b.rs", range(2, 6), &claim)
            .expect("confirmed");
        assert_eq!(confirmed.fact.badge, None);
        assert_eq!(confirmed.fact.differing_lines, 5);
        assert_eq!(confirmed.covered_base_lines, [3, 4, 5, 6, 7]);
    }

    #[test]
    fn differing_cross_file_move_covers_its_removed_lines() {
        let mut changed = FUNCTION.to_vec();
        changed[3] = "    request.with_timeout(config.timeout * 2)";
        let mut head_b = vec!["use y;"];
        head_b.extend(&changed);
        let files = vec![
            file(
                "src/a.rs",
                None,
                FileStatus::Deleted,
                vec![hunk((1, 5), (0, 0))],
            ),
            file(
                "src/b.rs",
                None,
                FileStatus::Modified,
                vec![hunk((1, 0), (2, 5))],
            ),
        ];
        let sources = sources([
            src(LineSide::Base, "src/a.rs", &FUNCTION),
            src(LineSide::Base, "src/b.rs", &["use y;"]),
            src(LineSide::Head, "src/b.rs", &head_b),
        ]);
        let report = run(&files, &sources);
        assert_eq!(report.moved.len(), 1, "{report:?}");
        let block = &report.moved[0];
        assert!(!block.identical);
        assert_eq!(block.differing_head_lines, [5]);
        assert_eq!(block.differing_base_lines, [4]);
        assert!(
            report.identical().is_empty(),
            "differing moves are no quiet rows"
        );
        let claim = MovedFrom {
            file: "src/a.rs".to_owned(),
            base_start: 1,
            base_end: 5,
        };
        let confirmed = report
            .confirm("src/b.rs", range(2, 6), &claim)
            .expect("confirmed");
        assert_eq!(
            confirmed.fact,
            MoveFact {
                from: file_range("src/a.rs", LineSide::Base, 1, 5),
                badge: None,
                differing_lines: 1
            }
        );
        assert_eq!(
            confirmed.covered_base_lines,
            [1, 2, 3, 4, 5],
            "the removed lines of the other file belong to the moved step"
        );
        let elsewhere = MovedFrom {
            file: "src/c.rs".to_owned(),
            ..claim
        };
        assert_eq!(report.confirm("src/b.rs", range(2, 6), &elsewhere), None);
        assert_eq!(
            report.contradicted_origin("src/b.rs", range(3, 5), &elsewhere),
            Some(&block.from),
            "another origin file contradicts the block"
        );
        let shifted = MovedFrom {
            base_start: 2,
            ..claim.clone()
        };
        assert_eq!(report.confirm("src/b.rs", range(2, 6), &shifted), None);
        assert_eq!(
            report.contradicted_origin("src/b.rs", range(2, 6), &shifted),
            None,
            "an overlapping but unconfirmed claim is no contradiction"
        );
        assert_eq!(
            report.contradicted_origin("src/b.rs", range(1, 6), &elsewhere),
            None,
            "no block contains a wider step"
        );
    }

    #[test]
    fn renamed_file_is_a_moved_block_and_confirms_a_claimed_part() {
        let base: Vec<String> = (1..=20)
            .map(|n| format!("const VALUE_{n}: u32 = {n};"))
            .collect();
        let mut head = base.clone();
        head[9] = "const VALUE_10: u32 = 100;".to_owned();
        head.insert(0, "// moved here".to_owned());
        let files = vec![file(
            "src/new.rs",
            Some("src/old.rs"),
            FileStatus::Renamed,
            vec![hunk((0, 0), (1, 1)), hunk((10, 1), (11, 1))],
        )];
        let base: Vec<&str> = base.iter().map(String::as_str).collect();
        let head: Vec<&str> = head.iter().map(String::as_str).collect();
        let report = run(
            &files,
            &sources([
                src(LineSide::Base, "src/old.rs", &base),
                src(LineSide::Head, "src/new.rs", &head),
            ]),
        );
        assert_eq!(report.moved.len(), 1, "{report:?}");
        let block = &report.moved[0];
        assert_eq!(block.from, file_range("src/old.rs", LineSide::Base, 1, 20));
        assert_eq!(block.to, file_range("src/new.rs", LineSide::Head, 1, 21));
        assert!(!block.identical);
        assert_eq!(block.differing_head_lines, [1, 11]);
        assert_eq!(block.removed_base_lines, [10]);
        let claim = MovedFrom {
            file: "src/old.rs".to_owned(),
            base_start: 5,
            base_end: 20,
        };
        let confirmed = report
            .confirm("src/new.rs", range(6, 21), &claim)
            .expect("confirmed");
        assert_eq!(confirmed.fact.differing_lines, 1);
        assert_eq!(confirmed.fact.badge, None);
        assert_eq!(confirmed.fact.from.range, range(5, 20));
        assert_eq!(confirmed.covered_base_lines, [10]);

        let pure = vec![file(
            "src/new.rs",
            Some("src/old.rs"),
            FileStatus::Renamed,
            vec![],
        )];
        let report = run(
            &pure,
            &sources([
                src(LineSide::Base, "src/old.rs", &["a", "b"]),
                src(LineSide::Head, "src/new.rs", &["a", "b"]),
            ]),
        );
        assert_eq!(
            report.identical().len(),
            1,
            "a pure rename is an identical move"
        );
    }

    #[test]
    fn in_place_reindent_and_short_coincidences_are_not_moves() {
        let body = &FUNCTION[1..4];
        let wrapped = indented(body);
        let mut head: Vec<&str> = vec!["if ready {"];
        head.extend(wrapped.iter().map(String::as_str));
        head.push("}");
        // Wrapping code in a block re-indents it in place, also when the diff splits it in two hunks.
        let split = vec![file(
            "src/a.rs",
            None,
            FileStatus::Modified,
            vec![hunk((1, 3), (0, 0)), hunk((3, 0), (2, 3))],
        )];
        let in_place = sources([
            src(LineSide::Base, "src/a.rs", body),
            src(LineSide::Head, "src/a.rs", &head),
        ]);
        assert!(run(&split, &in_place).moved.is_empty());
        // The same lines added far from where they were removed are a move; re-indented, a differing one.
        let mut far: Vec<&str> = vec!["x();"; 9];
        far.extend(wrapped.iter().map(String::as_str));
        let moved = vec![file(
            "src/a.rs",
            None,
            FileStatus::Modified,
            vec![hunk((1, 3), (0, 0)), hunk((3, 0), (10, 3))],
        )];
        let far = sources([
            src(LineSide::Base, "src/a.rs", body),
            src(LineSide::Head, "src/a.rs", &far),
        ]);
        let far = run(&moved, &far);
        assert_eq!(far.moved.len(), 1);
        assert!(far.identical().is_empty());

        // Two equal lines in otherwise different code are no move.
        let files = vec![
            file(
                "src/a.rs",
                None,
                FileStatus::Modified,
                vec![hunk((1, 3), (0, 0))],
            ),
            file(
                "src/b.rs",
                None,
                FileStatus::Modified,
                vec![hunk((0, 0), (1, 3))],
            ),
        ];
        let sources = sources([
            src(
                LineSide::Base,
                "src/a.rs",
                &[FUNCTION[1], FUNCTION[2], "send_once(request);"],
            ),
            src(LineSide::Head, "src/a.rs", &[]),
            src(LineSide::Base, "src/b.rs", &[]),
            src(
                LineSide::Head,
                "src/b.rs",
                &[FUNCTION[1], FUNCTION[2], "loop { retry(request); }"],
            ),
        ]);
        assert!(run(&files, &sources).moved.is_empty());
    }

    /// A four-line function moved from `old` to `new` (both added/removed whole), returning the report.
    fn moved_between(old_path: &str, old: &[&str], new_path: &str, new: &[&str]) -> MovesReport {
        let len = |lines: &[&str]| u32::try_from(lines.len()).expect("small");
        let files = vec![
            file(
                old_path,
                None,
                FileStatus::Deleted,
                vec![hunk((1, len(old)), (0, 0))],
            ),
            file(
                new_path,
                None,
                FileStatus::Added,
                vec![hunk((0, 0), (1, len(new)))],
            ),
        ];
        run(
            &files,
            &sources([
                src(LineSide::Base, old_path, old),
                src(LineSide::Head, new_path, new),
            ]),
        )
    }

    #[test]
    fn confirmation_keeps_interior_and_significant_whitespace() {
        let body = [
            "fn grant(user: &User) -> Result<(), Error> {",
            "    let scope = lookup(user)?;",
            "    send(\"read  admin\", scope)",
            "}",
        ];
        let mut changed = body;
        changed[2] = "    send(\"read admin\", scope)";
        let report = moved_between("src/a.rs", &body, "src/b.rs", &changed);
        assert_eq!(report.moved.len(), 1, "a candidate: {report:?}");
        assert!(!report.moved[0].identical, "interior whitespace is content");
        assert_eq!(report.moved[0].differing_head_lines, [3]);
        assert!(report.identical().is_empty());

        let python = [
            "def grant(user):",
            "    scope = lookup(user)",
            "    if scope:",
            "        send(scope)",
        ];
        let reindented: Vec<String> = python.iter().map(|line| format!("    {line}")).collect();
        let reindented: Vec<&str> = reindented.iter().map(String::as_str).collect();
        let report = moved_between("a.py", &python, "b.py", &reindented);
        assert!(
            report.identical().is_empty(),
            "indentation is significant in Python: {report:?}"
        );

        let mut with_blank: Vec<&str> = body.to_vec();
        with_blank.insert(2, "");
        let report = moved_between("src/a.rs", &body, "src/b.rs", &with_blank);
        assert!(
            report.identical().is_empty(),
            "a blank line inside the block counts"
        );
        assert_eq!(report.moved[0].differing_head_lines, [3]);

        let indented: Vec<String> = body.iter().map(|line| format!("    {line}  ")).collect();
        let indented: Vec<&str> = indented.iter().map(String::as_str).collect();
        let report = moved_between("src/a.rs", &body, "src/b.rs", &indented);
        assert!(report.identical().is_empty(), "no trimming in any language");

        let template = [
            "const payload = `",
            "    privileged payload with extra text",
            "`;",
            "export const handler = build(payload);",
        ];
        let mut changed = template;
        changed[1] = "privileged payload with extra text";
        let report = moved_between("web/a.ts", &template, "web/b.ts", &changed);
        assert_eq!(report.moved.len(), 1, "a candidate: {report:?}");
        assert!(report.identical().is_empty(), "the literal's value changed");
        assert_eq!(report.moved[0].differing_head_lines, [2]);
    }

    #[test]
    fn confirm_requires_corresponding_ranges() {
        let report = MovesReport {
            moved: vec![MovedBlock {
                from: file_range("a.rs", LineSide::Base, 10, 14),
                to: file_range("b.rs", LineSide::Head, 100, 104),
                identical: true,
                differing_head_lines: vec![],
                differing_base_lines: vec![],
                removed_base_lines: (10..=14).collect(),
                aligned: vec![AlignedRun {
                    base_start: 10,
                    head_start: 100,
                    len: 5,
                }],
            }],
            whitespace_only: vec![],
            incomplete: false,
        };
        let claim = |base_start, base_end| MovedFrom {
            file: "a.rs".to_owned(),
            base_start,
            base_end,
        };
        let check = |step: (u32, u32), base: (u32, u32)| {
            report.confirm("b.rs", range(step.0, step.1), &claim(base.0, base.1))
        };
        assert_eq!(
            check((100, 200), (1, 200)),
            None,
            "extra code in the step and the claim"
        );
        assert_eq!(
            check((100, 100), (10, 14)),
            None,
            "one line does not account for the block"
        );
        assert_eq!(check((101, 103), (12, 14)), None, "shifted claim");
        let one = check((100, 100), (10, 10)).expect("corresponding line");
        assert_eq!(one.covered_base_lines, [10]);
        assert_eq!(one.fact.badge, Some(ProvenanceBadge::TextIdentical));
        let part = check((101, 103), (11, 13)).expect("corresponding part");
        assert_eq!(part.covered_base_lines, [11, 12, 13]);
        assert_eq!(part.fact.from.range, range(11, 13));
    }

    #[test]
    fn move_pairing_stops_at_the_work_budget() {
        let files = vec![
            file(
                "src/a.rs",
                None,
                FileStatus::Deleted,
                vec![hunk((1, 5), (0, 0))],
            ),
            file(
                "src/b.rs",
                None,
                FileStatus::Added,
                vec![hunk((0, 0), (1, 5))],
            ),
        ];
        let texts = sources([
            src(LineSide::Base, "src/a.rs", &FUNCTION),
            src(LineSide::Head, "src/b.rs", &FUNCTION),
        ]);
        let text = |side, path: &str| texts.get(&(side, path.to_owned())).map(String::as_str);
        assert_eq!(analyse_within(&files, &text, 1_000).identical().len(), 1);
        let within = analyse_within(&files, &text, 1_000);
        assert!(!within.incomplete);
        let over = analyse_within(&files, &text, 99);
        assert!(over.moved.is_empty(), "over budget: no move facts");
        assert!(over.incomplete, "over budget: marked incomplete");

        // 20 groups of 50 removed and 50 added runs that all pair within the group: 50 000 alignments of
        // (60 + 60)² = 7.2e8 units, over the budget; detection stops early and in bounded time.
        let mut files = Vec::new();
        let mut entries = Vec::new();
        for group in 0..20 {
            let body: Vec<String> = (0..60)
                .map(|n| format!("let value_{n} = compute_value(group_{group}, {n});"))
                .collect();
            let body: Vec<&str> = body.iter().map(String::as_str).collect();
            for n in 0..50 {
                let (old, new) = (format!("old/{group}/{n}.rs"), format!("new/{group}/{n}.rs"));
                files.push(file(
                    &old,
                    None,
                    FileStatus::Deleted,
                    vec![hunk((1, 60), (0, 0))],
                ));
                files.push(file(
                    &new,
                    None,
                    FileStatus::Added,
                    vec![hunk((0, 0), (1, 60))],
                ));
                entries.push(src(LineSide::Base, &old, &body));
                entries.push(src(LineSide::Head, &new, &body));
            }
        }
        let texts: Sources = entries.into_iter().collect();
        let started = std::time::Instant::now();
        let report = run(&files, &texts);
        let elapsed = started.elapsed();
        assert!(elapsed < std::time::Duration::from_secs(60), "{elapsed:?}");
        assert!(
            !report.moved.is_empty() && report.moved.len() < 1000,
            "{}",
            report.moved.len()
        );
        assert!(report.incomplete, "the budget ran out");
    }

    #[test]
    fn whitespace_only_hunks_are_reported_on_their_side() {
        let files = vec![file(
            "src/a.rs",
            None,
            FileStatus::Modified,
            vec![
                hunk((1, 1), (1, 1)),
                hunk((3, 1), (3, 1)),
                hunk((5, 1), (4, 0)),
            ],
        )];
        let sources = sources([
            src(
                LineSide::Base,
                "src/a.rs",
                &["let a =  1;", "x", "let b = 2;", "y", ""],
            ),
            src(
                LineSide::Head,
                "src/a.rs",
                &["    let a = 1;", "x", "let b = 3;", "y"],
            ),
        ]);
        assert_eq!(
            run(&files, &sources).whitespace_only,
            [
                file_range("src/a.rs", LineSide::Head, 1, 1),
                file_range("src/a.rs", LineSide::Base, 5, 5)
            ]
        );
        assert!(
            run(&files, &HashMap::new()).whitespace_only.is_empty(),
            "no text, no fact"
        );
    }
}
