//! Line arithmetic for showing a file by line range: the window of a file, removed blocks at their deletion
//! markers, changed lines, the lines GitHub accepts line threads on, where a comment on lines goes, and the
//! classic diff of a range. Pure functions over file bytes and hunks.

use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::api::code::{DeletedBlock, DiffHunk, DiffLine, DiffLineKind, LineEnding};
use contracts::api::comments::CommentTarget;
use contracts::common::{LineRange, LineSide};

use crate::diff::{FileDiff, Hunk, LineSpan};

/// Context lines GitHub shows around a change; lines inside them can carry a line thread (ADR-006).
pub const DIFF_CONTEXT: u32 = 3;

/// A deletion marker on the head side: base lines removed after a head line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeletionMarker {
    /// Head line after which the code was removed; 0 = before the first line.
    pub after_line: u32,
    /// The removed lines at the base commit.
    pub base: LineSpan,
    /// Replacement lines from the same zero-context diff hunk.
    pub replacement: Option<LineRange>,
}

/// A window of a file: its line count and the requested lines clamped to the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineWindow {
    /// Lines in the file.
    pub total_lines: u32,
    /// First requested line.
    pub from: u32,
    /// Last requested line, at most `total_lines`.
    pub to: u32,
    /// Lines `from..=to`.
    pub lines: Vec<String>,
}

/// The window `from..=to` of the file `bytes` (1-based, `str::lines` rules); only its lines are decoded.
pub fn line_window(bytes: &[u8], from: u32, to: u32) -> LineWindow {
    let mut lines = Vec::new();
    let mut total = 0u32;
    if !bytes.is_empty() {
        let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
        for line in body.split(|&byte| byte == b'\n') {
            total = total.saturating_add(1);
            if (from..=to).contains(&total) {
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                lines.push(String::from_utf8_lossy(line).into_owned());
            }
        }
    }
    LineWindow {
        total_lines: total,
        from,
        to: to.min(total),
        lines,
    }
}

/// The deletion markers of `file` between head lines `from - 1` and `to`, in file order.
pub fn deletion_markers(file: &FileDiff, from: u32, to: u32) -> Vec<DeletionMarker> {
    file.hunks
        .iter()
        .filter(|hunk| hunk.base.len > 0)
        .map(|hunk| DeletionMarker {
            after_line: hunk.deletion_marker(),
            base: hunk.base,
            replacement: (hunk.head.len > 0).then_some(LineRange {
                start_line: hunk.head.start,
                end_line: hunk
                    .head
                    .start
                    .saturating_add(hunk.head.len)
                    .saturating_sub(1),
            }),
        })
        .filter(|marker| marker.after_line + 1 >= from && marker.after_line <= to)
        .collect()
}

/// The removed code of every marker, read from the base file `base_text`; at most `budget` removed lines in
/// total, in marker order (`lines.len() < total_lines` marks a truncated block).
pub fn deleted_blocks(
    markers: &[DeletionMarker],
    base_text: &[u8],
    mut budget: u32,
) -> Vec<DeletedBlock> {
    let mut blocks = Vec::with_capacity(markers.len());
    let mut wanted = Vec::with_capacity(markers.len());
    for &DeletionMarker {
        after_line,
        base: span,
        replacement,
    } in markers
    {
        let take = span.len.min(budget);
        budget -= take;
        if take > 0 {
            wanted.push((blocks.len(), span.start..=span.start + take - 1));
        }
        blocks.push(DeletedBlock {
            after_line,
            base_start: span.start,
            lines: Vec::new(),
            total_lines: span.len,
            replacement,
            context_only: None,
        });
    }
    // One pass over the base file; hunks are in file order and do not overlap.
    let body = base_text.strip_suffix(b"\n").unwrap_or(base_text);
    let mut next = 0;
    for (number, line) in (1u32..).zip(body.split(|&byte| byte == b'\n')) {
        while wanted
            .get(next)
            .is_some_and(|(_, range)| *range.end() < number)
        {
            next += 1;
        }
        let Some((block, range)) = wanted.get(next) else {
            break;
        };
        if let Some(target) = blocks.get_mut(*block).filter(|_| range.contains(&number)) {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            target
                .lines
                .push(String::from_utf8_lossy(line).into_owned());
        }
    }
    blocks
}

/// Lines changed by the pull request in one file on one side, within `from..=to`, sorted.
pub fn changed_lines_within(
    files: &[FileDiff],
    path: &str,
    side: LineSide,
    from: u32,
    to: u32,
) -> Vec<u32> {
    let mut lines: Vec<u32> = files
        .iter()
        .filter(|file| file.path_on(side) == Some(path))
        .flat_map(|file| &file.hunks)
        .flat_map(|hunk| match side {
            LineSide::Head => &hunk.added,
            LineSide::Base => &hunk.removed,
        })
        .copied()
        .filter(|line| (from..=to).contains(line))
        .collect();
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// `file` with the whitespace-only `pairs` (base line, head line) read as unchanged lines: each hunk is split around
/// them, as `git diff -w` shows it. A pair outside a hunk or out of order is ignored.
pub fn without_whitespace(file: &FileDiff, pairs: &[(u32, u32)]) -> FileDiff {
    let mut hunks = Vec::with_capacity(file.hunks.len());
    for hunk in &file.hunks {
        let (base_end, head_end) = (
            hunk.base.start.saturating_add(hunk.base.len),
            hunk.head.start.saturating_add(hunk.head.len),
        );
        let (mut base, mut head) = (hunk.base.start, hunk.head.start);
        let mut cuts = Vec::new();
        for &(b, h) in pairs {
            let inside = hunk.base.len > 0 && hunk.head.len > 0 && b < base_end && h < head_end;
            if inside && b >= base && h >= head {
                cuts.push((b, h));
                (base, head) = (b + 1, h + 1);
            }
        }
        if cuts.is_empty() {
            hunks.push(hunk.clone());
            continue;
        }
        let (mut base, mut head) = (hunk.base.start, hunk.head.start);
        for cut in cuts.into_iter().map(Some).chain([None]) {
            let (to_base, to_head) = cut.unwrap_or((base_end, head_end));
            if to_base > base || to_head > head {
                hunks.push(part(hunk, base..to_base, head..to_head));
            }
            if let Some((b, h)) = cut {
                (base, head) = (b + 1, h + 1);
            }
        }
    }
    FileDiff {
        hunks,
        ..file.clone()
    }
}

/// The lines `base` and `head` of `hunk` as a hunk of their own (an empty side starts after the line before it).
fn part(hunk: &Hunk, base: std::ops::Range<u32>, head: std::ops::Range<u32>) -> Hunk {
    let span = |range: &std::ops::Range<u32>| match range.end.checked_sub(range.start) {
        Some(len) if len > 0 => LineSpan {
            start: range.start,
            len,
        },
        _ => LineSpan {
            start: range.start.saturating_sub(1),
            len: 0,
        },
    };
    Hunk {
        id: hunk.id.clone(),
        base: span(&base),
        head: span(&head),
        added: hunk
            .added
            .iter()
            .copied()
            .filter(|line| head.contains(line))
            .collect(),
        removed: hunk
            .removed
            .iter()
            .copied()
            .filter(|line| base.contains(line))
            .collect(),
    }
}

/// Lines changed by the pull request on either side, over all files (the size of the change).
pub fn changed_line_count(files: &[FileDiff]) -> u32 {
    files
        .iter()
        .flat_map(|file| &file.hunks)
        .map(|hunk| hunk.base.len.saturating_add(hunk.head.len))
        .fold(0, u32::saturating_add)
}

/// Whether `a` and `b` share a line.
pub fn ranges_overlap(a: &LineRange, b: &LineRange) -> bool {
    a.start_line <= b.end_line && b.start_line <= a.end_line
}

/// `inner` lies within `outer`.
pub fn contains_range(outer: &LineRange, inner: &LineRange) -> bool {
    outer.start_line <= inner.start_line && inner.end_line <= outer.end_line
}

/// Sorted lines as ranges of consecutive lines.
pub fn ranges(lines: &[u32]) -> Vec<LineRange> {
    let mut out: Vec<LineRange> = Vec::new();
    for &line in lines {
        match out.last_mut() {
            Some(last) if last.end_line + 1 == line => last.end_line = line,
            _ => out.push(LineRange {
                start_line: line,
                end_line: line,
            }),
        }
    }
    out
}

/// The diff hunks of `file` on `side` with GitHub's context lines, merged and clipped to `from..=to`.
pub fn thread_ranges(
    file: &FileDiff,
    side: LineSide,
    total: u32,
    from: u32,
    to: u32,
) -> Vec<LineRange> {
    let mut lines: Vec<u32> = Vec::new();
    for hunk in &file.hunks {
        let span = match side {
            LineSide::Head => hunk.head,
            LineSide::Base => hunk.base,
        };
        let (start, end) = if span.len == 0 {
            // Lines were added or removed after `start`: context on both sides of the gap.
            (
                span.start.saturating_sub(DIFF_CONTEXT - 1),
                span.start + DIFF_CONTEXT,
            )
        } else {
            (
                span.start.saturating_sub(DIFF_CONTEXT),
                span.start + span.len - 1 + DIFF_CONTEXT,
            )
        };
        lines.extend(start.max(from).max(1)..=end.min(total).min(to));
    }
    lines.sort_unstable();
    lines.dedup();
    ranges(&lines)
}

/// Both texts of a changed file; empty for a side where the file does not exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffTexts<'a> {
    /// The file at the base commit.
    pub base: &'a [u8],
    /// The file at the head commit.
    pub head: &'a [u8],
}

/// The classic diff of a range would show more lines of the other side than allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the diff of the range has more than {budget} changed lines on the other side")]
pub struct DiffTooLarge {
    /// Most lines of the other side one diff may show.
    pub budget: u32,
}

/// Unified-diff hunks of `file` that overlap lines `from..=to` on `side`, with [`DIFF_CONTEXT`] context lines;
/// hunks at most two contexts apart are merged (as `git diff`). Lines of `side` are clipped to the range widened
/// by the context; changed lines of the other side are shown whole, at most `budget` of them.
pub fn classic_diff(
    file: &FileDiff,
    side: LineSide,
    texts: DiffTexts<'_>,
    from: u32,
    to: u32,
    budget: u32,
) -> Result<Vec<DiffHunk>, DiffTooLarge> {
    let hunks = &file.hunks;
    let on_side = |hunk: &Hunk| match side {
        LineSide::Head => hunk.head,
        LineSide::Base => hunk.base,
    };
    let Some(first) = hunks
        .iter()
        .position(|hunk| overlaps(on_side(hunk), from, to))
    else {
        return Ok(Vec::new());
    };
    let end = first
        + hunks[first..]
            .iter()
            .take_while(|hunk| overlaps(on_side(hunk), from, to))
            .count();
    let mut writer = DiffWriter {
        side,
        base: split_lines(texts.base),
        head: split_lines(texts.head),
        clip: from.saturating_sub(DIFF_CONTEXT)..=to.saturating_add(DIFF_CONTEXT),
        budget,
        other_lines: 0,
        hunks: Vec::new(),
        current: Vec::new(),
        anchor: (0, 0),
    };
    let side_total = match side {
        LineSide::Head => writer.head.len(),
        LineSide::Base => writer.base.len(),
    };
    let side_total = u32::try_from(side_total).unwrap_or(u32::MAX);
    let mut group_start = first;
    for index in first..end {
        let hunk = &hunks[index];
        if index > group_start
            && begin(on_side(hunk)).saturating_sub(after(on_side(&hunks[index - 1])))
                > 2 * DIFF_CONTEXT
        {
            writer.group(hunks, group_start..index, side_total)?;
            group_start = index;
        }
    }
    writer.group(hunks, group_start..end, side_total)?;
    Ok(writer.hunks)
}

/// A hunk touches lines `from..=to` of its side; an empty span lies between line `start` and the next.
fn overlaps(span: LineSpan, from: u32, to: u32) -> bool {
    if span.len == 0 {
        span.start + 1 >= from && span.start <= to
    } else {
        span.start <= to && span.start + span.len > from
    }
}

/// First line of a hunk's span, or the line after the gap of an empty span.
fn begin(span: LineSpan) -> u32 {
    if span.len == 0 {
        span.start + 1
    } else {
        span.start
    }
}

/// First unchanged line after a hunk's span.
fn after(span: LineSpan) -> u32 {
    begin(span) + span.len
}

/// Lines of a file with their ending: none for LF, CR LF, or no newline after the last line.
fn split_lines(text: &[u8]) -> Vec<(&[u8], Option<LineEnding>)> {
    if text.is_empty() {
        return Vec::new();
    }
    let (body, last_terminated) = match text.strip_suffix(b"\n") {
        Some(body) => (body, true),
        None => (text, false),
    };
    let count = body.split(|&byte| byte == b'\n').count();
    body.split(|&byte| byte == b'\n')
        .enumerate()
        .map(|(index, line)| {
            if index + 1 == count && !last_terminated {
                return (line, Some(LineEnding::NoNewline));
            }
            match line.strip_suffix(b"\r") {
                Some(line) => (line, Some(LineEnding::Crlf)),
                None => (line, None),
            }
        })
        .collect()
}

/// Writes the hunks of a classic diff, clipping `side` lines and counting other-side lines.
struct DiffWriter<'a> {
    side: LineSide,
    base: Vec<(&'a [u8], Option<LineEnding>)>,
    head: Vec<(&'a [u8], Option<LineEnding>)>,
    clip: std::ops::RangeInclusive<u32>,
    budget: u32,
    other_lines: u32,
    hunks: Vec<DiffHunk>,
    current: Vec<DiffLine>,
    /// Base and head line before the first written line of the current hunk.
    anchor: (u32, u32),
}

impl DiffWriter<'_> {
    /// Writes the merged hunk of `hunks[group]`; context stops at the neighbouring hunks of the file.
    fn group(
        &mut self,
        hunks: &[Hunk],
        group: std::ops::Range<usize>,
        side_total: u32,
    ) -> Result<(), DiffTooLarge> {
        let side = self.side;
        let on_side = |hunk: &Hunk| match side {
            LineSide::Head => hunk.head,
            LineSide::Base => hunk.base,
        };
        let (first, last) = (&hunks[group.start], &hunks[group.end - 1]);
        let previous_end = match group.start.checked_sub(1) {
            Some(index) => after(on_side(&hunks[index])),
            None => 1,
        };
        let lead = begin(on_side(first))
            .saturating_sub(previous_end)
            .min(DIFF_CONTEXT);
        let (mut base, mut head) = (
            begin(first.base).saturating_sub(lead),
            begin(first.head).saturating_sub(lead),
        );
        for hunk in &hunks[group.clone()] {
            while match side {
                LineSide::Head => head < begin(hunk.head),
                LineSide::Base => base < begin(hunk.base),
            } {
                self.write(DiffLineKind::Context, base, head)?;
                base += 1;
                head += 1;
            }
            for line in hunk.base.start..hunk.base.start + hunk.base.len {
                self.write(DiffLineKind::Removed, line, head)?;
            }
            base = after(hunk.base);
            for line in hunk.head.start..hunk.head.start + hunk.head.len {
                self.write(DiffLineKind::Added, base, line)?;
            }
            head = after(hunk.head);
        }
        let end_of_file = side_total.saturating_add(1);
        let next = hunks
            .get(group.end)
            .map_or(end_of_file, |hunk| begin(on_side(hunk)));
        let trail = next
            .min(end_of_file)
            .saturating_sub(after(on_side(last)))
            .min(DIFF_CONTEXT);
        for _ in 0..trail {
            self.write(DiffLineKind::Context, base, head)?;
            base += 1;
            head += 1;
        }
        self.finish_hunk();
        Ok(())
    }

    /// Writes a line at base line `base` and head line `head` (the next line of the side it does not exist on).
    fn write(&mut self, kind: DiffLineKind, base: u32, head: u32) -> Result<(), DiffTooLarge> {
        let (old_line, new_line) = match kind {
            DiffLineKind::Context => (Some(base), Some(head)),
            DiffLineKind::Removed => (Some(base), None),
            DiffLineKind::Added => (None, Some(head)),
        };
        let on_side = match self.side {
            LineSide::Head => new_line,
            LineSide::Base => old_line,
        };
        match on_side {
            Some(line) if !self.clip.contains(&line) => return Ok(()),
            Some(_) => {}
            None => {
                self.other_lines += 1;
                if self.other_lines > self.budget {
                    return Err(DiffTooLarge {
                        budget: self.budget,
                    });
                }
            }
        }
        if self.current.is_empty() {
            self.anchor = (base.saturating_sub(1), head.saturating_sub(1));
        }
        let (lines, line) = match (kind, self.side) {
            (DiffLineKind::Removed, _) | (DiffLineKind::Context, LineSide::Base) => {
                (&self.base, base)
            }
            _ => (&self.head, head),
        };
        let (text, eol) = (line as usize)
            .checked_sub(1)
            .and_then(|index| lines.get(index))
            .map(|(text, eol)| (String::from_utf8_lossy(text).into_owned(), *eol))
            .unwrap_or_default();
        self.current.push(DiffLine {
            kind,
            old_line,
            new_line,
            text,
            eol,
        });
        Ok(())
    }

    /// Ends the current hunk with a `@@ -start,count +start,count @@` header over its written lines.
    fn finish_hunk(&mut self) {
        if self.current.is_empty() {
            return;
        }
        let lines = std::mem::take(&mut self.current);
        let span = |numbers: Vec<u32>, anchor: u32| {
            let count = numbers.len();
            (numbers.first().copied().unwrap_or(anchor), count)
        };
        let (old_start, old_count) = span(
            lines.iter().filter_map(|line| line.old_line).collect(),
            self.anchor.0,
        );
        let (new_start, new_count) = span(
            lines.iter().filter_map(|line| line.new_line).collect(),
            self.anchor.1,
        );
        self.hunks.push(DiffHunk {
            header: format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@"),
            lines,
        });
    }
}

/// Where a comment on displayed lines goes on GitHub (ADR-006).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommentPosition {
    /// Every target line lies in one diff hunk (with GitHub's context): a line thread on the target's side.
    Line {
        /// Path of the file in the pull request (the head path; the base path of a deleted file).
        path: String,
        /// First line of a multi-line thread.
        start_line: Option<u32>,
        /// Last line.
        line: u32,
    },
    /// The file is in the diff, the lines are not all in one hunk: a file-level thread linking to the lines.
    File {
        /// Path of the file in the pull request.
        path: String,
    },
    /// The file is not in the diff, or the lines are not in the file.
    NotCommentable,
}

/// Classifies `target` (`1 ≤ start ≤ line`) against the diff; `total_lines` is the length of the target's file
/// on its side.
pub fn comment_position(
    files: &[FileDiff],
    target: &CommentTarget,
    total_lines: u32,
) -> CommentPosition {
    let Some(file) = files
        .iter()
        .find(|file| file.path_on(target.side) == Some(target.path.as_str()))
    else {
        return CommentPosition::NotCommentable;
    };
    if target.line > total_lines {
        return CommentPosition::NotCommentable;
    }
    let start = target.start_line.unwrap_or(target.line);
    let wanted = LineRange {
        start_line: start,
        end_line: target.line,
    };
    if thread_ranges(file, target.side, total_lines, start, target.line) == [wanted] {
        CommentPosition::Line {
            path: file.path.clone(),
            start_line: (start < target.line).then_some(start),
            line: target.line,
        }
    } else {
        CommentPosition::File {
            path: file.path.clone(),
        }
    }
}

/// The first changed line of `target` that no annotation of `flows` covers (brief §5 Comments: such a comment
/// is also `missed_by_llm` feedback); `None` when the target has no such line.
pub fn missed_line(
    files: &[FileDiff],
    flows: &[FlowAnalysisOutput],
    target: &CommentTarget,
) -> Option<u32> {
    let start = target.start_line.unwrap_or(target.line);
    let annotated: Vec<(u32, u32)> = flows
        .iter()
        .flat_map(|flow| &flow.steps)
        .filter(|step| step.file == target.path && step.side == target.side)
        .flat_map(|step| &step.annotations)
        .map(|annotation| (annotation.start_line, annotation.end_line))
        .collect();
    changed_lines_within(files, &target.path, target.side, start, target.line)
        .into_iter()
        .find(|line| !annotated.iter().any(|(a, b)| (*a..=*b).contains(line)))
}

#[cfg(test)]
mod tests {
    use crate::diff::{FileStatus, Hunk};
    use contracts::api::code::{DiffHunk, DiffLine, DiffLineKind, LineEnding};
    use contracts::ids::HunkId;

    use super::*;

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

    fn range(start_line: u32, end_line: u32) -> LineRange {
        LineRange {
            start_line,
            end_line,
        }
    }

    fn window(total_lines: u32, from: u32, to: u32, lines: &[&str]) -> LineWindow {
        LineWindow {
            total_lines,
            from,
            to,
            lines: lines.iter().map(|line| (*line).to_owned()).collect(),
        }
    }

    #[test]
    fn replacements_carry_the_paired_head_range() {
        let file = FileDiff {
            path: "a.rs".into(),
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            hunks: vec![hunk((2, 1), (2, 2)), hunk((5, 1), (5, 0))],
        };
        let blocks = deleted_blocks(
            &deletion_markers(&file, 1, 8),
            b"one\nold\nthree\nfour\ndeleted\n",
            20,
        );
        let wire = serde_json::to_value(&blocks).expect("serialize");
        assert_eq!(
            wire[0]["replacement"],
            serde_json::json!({"start_line":2,"end_line":3})
        );
        assert!(wire[1]["replacement"].is_null());
    }

    #[test]
    fn line_window_counts_all_lines_and_clamps_the_window() {
        assert_eq!(
            line_window(b"a\r\nb\nc\n", 2, 5),
            window(3, 2, 3, &["b", "c"])
        );
        assert_eq!(line_window(b"", 1, 1), window(0, 1, 0, &[]));
        assert_eq!(line_window(b"a\nb", 1, 1), window(2, 1, 1, &["a"]));
    }

    #[test]
    fn changed_lines_are_counted_and_kept_within_a_range() {
        let file = |path: &str| FileDiff {
            path: path.to_owned(),
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            hunks: vec![hunk((2, 1), (2, 3)), hunk((9, 2), (11, 0))],
        };
        let files = [file("a.rs"), file("b.rs")];
        assert_eq!(
            changed_lines_within(&files, "a.rs", LineSide::Head, 3, 9),
            [3, 4]
        );
        assert_eq!(
            changed_lines_within(&files, "a.rs", LineSide::Base, 1, 100),
            [2, 9, 10]
        );
        assert!(changed_lines_within(&files, "c.rs", LineSide::Head, 1, 100).is_empty());
        assert_eq!(changed_line_count(&files), 12);
        let outer = range(3, 9);
        assert!(contains_range(&outer, &range(3, 9)));
        assert!(contains_range(&outer, &range(4, 5)));
        assert!(!contains_range(&outer, &range(2, 5)));
        assert!(!contains_range(&outer, &range(8, 10)));
    }

    #[test]
    fn deleted_blocks_share_the_budget_and_keep_every_marker() {
        let base: String = (1..=10).map(|n| format!("line {n}\n")).collect();
        let file = FileDiff {
            path: "a.txt".to_owned(),
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            hunks: vec![
                hunk((2, 3), (1, 0)),
                hunk((7, 1), (3, 0)),
                hunk((9, 1), (4, 0)),
            ],
        };
        let markers = deletion_markers(&file, 1, 5);
        assert_eq!(
            markers.iter().map(|m| m.after_line).collect::<Vec<_>>(),
            [1, 3, 4]
        );
        let blocks = deleted_blocks(&markers, base.as_bytes(), 4);
        let shape: Vec<(u32, u32, Vec<&str>, u32)> = blocks
            .iter()
            .map(|b| {
                let lines = b.lines.iter().map(String::as_str).collect();
                (b.after_line, b.base_start, lines, b.total_lines)
            })
            .collect();
        assert_eq!(
            shape,
            [
                (1, 2, vec!["line 2", "line 3", "line 4"], 3),
                (3, 7, vec!["line 7"], 1),
                (4, 9, vec![], 1),
            ]
        );
        assert!(deletion_markers(&file, 6, 6).is_empty());
    }

    #[test]
    fn thread_ranges_follow_github_context_lines() {
        // src/rpc/client.rs of the fixture (api-seed lines, both sides).
        let file = FileDiff {
            path: "src/rpc/client.rs".to_owned(),
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            hunks: vec![
                hunk((34, 0), (35, 1)),
                hunk((41, 1), (42, 1)),
                hunk((52, 11), (52, 0)),
                hunk((99, 1), (89, 1)),
                hunk((101, 3), (91, 19)),
                hunk((105, 0), (112, 41)),
                hunk((173, 42), (219, 0)),
            ],
        };
        assert_eq!(
            thread_ranges(&file, LineSide::Head, 224, 1, 224),
            [
                range(32, 45),
                range(50, 55),
                range(86, 155),
                range(217, 222)
            ]
        );
        assert_eq!(
            thread_ranges(&file, LineSide::Base, 219, 1, 219),
            [
                range(32, 44),
                range(49, 65),
                range(96, 108),
                range(170, 217)
            ]
        );
        assert_eq!(
            thread_ranges(&file, LineSide::Head, 224, 40, 52),
            [range(40, 45), range(50, 52)]
        );
    }

    fn numbered(lines: std::ops::RangeInclusive<u32>) -> String {
        lines.map(|n| format!("line {n}\n")).collect()
    }

    fn modified(hunks: Vec<Hunk>) -> FileDiff {
        FileDiff {
            path: "a.txt".to_owned(),
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            hunks,
        }
    }

    type Row = (
        DiffLineKind,
        Option<u32>,
        Option<u32>,
        String,
        Option<LineEnding>,
    );

    /// Header and `(kind, old, new, text, eol)` rows of every hunk.
    fn shape(hunks: &[DiffHunk]) -> Vec<(String, Vec<Row>)> {
        hunks
            .iter()
            .map(|hunk| {
                let lines = hunk
                    .lines
                    .iter()
                    .map(|l| (l.kind, l.old_line, l.new_line, l.text.clone(), l.eol))
                    .collect();
                (hunk.header.clone(), lines)
            })
            .collect()
    }

    fn headers(hunks: &[DiffHunk]) -> Vec<&str> {
        hunks.iter().map(|h| h.header.as_str()).collect()
    }

    fn context(old: u32, new: u32) -> Row {
        (
            DiffLineKind::Context,
            Some(old),
            Some(new),
            format!("line {old}"),
            None,
        )
    }

    fn removed(old: u32) -> Row {
        (
            DiffLineKind::Removed,
            Some(old),
            None,
            format!("line {old}"),
            None,
        )
    }

    fn added(new: u32, text: &str) -> Row {
        (DiffLineKind::Added, None, Some(new), text.to_owned(), None)
    }

    fn ending(mut row: Row, eol: LineEnding) -> Row {
        row.4 = Some(eol);
        row
    }

    fn texts<'a>(base: &'a str, head: &'a str) -> DiffTexts<'a> {
        DiffTexts {
            base: base.as_bytes(),
            head: head.as_bytes(),
        }
    }

    #[test]
    fn classic_diff_keeps_line_ending_changes() {
        let one = modified(vec![hunk((1, 1), (1, 1))]);
        let cases = [
            (
                "x",
                "x\n",
                LineSide::Head,
                Some(LineEnding::NoNewline),
                None,
            ),
            (
                "x\n",
                "x",
                LineSide::Base,
                None,
                Some(LineEnding::NoNewline),
            ),
            ("x\r\n", "x\n", LineSide::Head, Some(LineEnding::Crlf), None),
            ("x\n", "x\r\n", LineSide::Base, None, Some(LineEnding::Crlf)),
        ];
        for (base, head, side, removed_eol, added_eol) in cases {
            let diff = classic_diff(&one, side, texts(base, head), 1, 1, 100);
            let rows = vec![
                (
                    DiffLineKind::Removed,
                    Some(1),
                    None,
                    "x".to_owned(),
                    removed_eol,
                ),
                (
                    DiffLineKind::Added,
                    None,
                    Some(1),
                    "x".to_owned(),
                    added_eol,
                ),
            ];
            assert_eq!(
                diff.map(|h| shape(&h)),
                Ok(vec![("@@ -1,1 +1,1 @@".to_owned(), rows)]),
                "{base:?} -> {head:?}"
            );
        }
        // A CR before a missing final newline is text, not half of a CR LF.
        let diff = classic_diff(&one, LineSide::Head, texts("x\r", "x\r\n"), 1, 1, 100);
        let rows = vec![
            (
                DiffLineKind::Removed,
                Some(1),
                None,
                "x\r".to_owned(),
                Some(LineEnding::NoNewline),
            ),
            (
                DiffLineKind::Added,
                None,
                Some(1),
                "x".to_owned(),
                Some(LineEnding::Crlf),
            ),
        ];
        assert_eq!(
            diff.map(|h| shape(&h)),
            Ok(vec![("@@ -1,1 +1,1 @@".to_owned(), rows)])
        );
        // A last line without newline as unchanged context on both sides keeps its marker.
        let file = modified(vec![hunk((1, 1), (1, 1))]);
        let diff = classic_diff(&file, LineSide::Head, texts("a\nb", "A\nb"), 1, 2, 100);
        let last = diff.map(|h| h[0].lines.last().map(|l| (l.text.clone(), l.eol)));
        assert_eq!(
            last,
            Ok(Some(("b".to_owned(), Some(LineEnding::NoNewline))))
        );
    }

    #[test]
    fn classic_diff_shows_a_change_with_three_context_lines() {
        let base = numbered(1..=10);
        let head = base.replace("line 5\n", "five\r\nfive b\n");
        let file = modified(vec![hunk((5, 1), (5, 2))]);
        let diff = classic_diff(&file, LineSide::Head, texts(&base, &head), 5, 6, 100);
        let expected = vec![(
            "@@ -2,7 +2,8 @@".to_owned(),
            vec![
                context(2, 2),
                context(3, 3),
                context(4, 4),
                removed(5),
                ending(added(5, "five"), LineEnding::Crlf),
                added(6, "five b"),
                context(6, 7),
                context(7, 8),
                context(8, 9),
            ],
        )];
        assert_eq!(diff.map(|hunks| shape(&hunks)), Ok(expected));
        // Lines of the side beyond the range and its context are left out.
        let diff = classic_diff(&file, LineSide::Head, texts(&base, &head), 5, 5, 100);
        assert_eq!(
            diff.map(|h| headers(&h).join(" ")),
            Ok("@@ -2,6 +2,7 @@".to_owned())
        );
    }

    #[test]
    fn classic_diff_merges_hunks_at_most_two_contexts_apart() {
        let base = numbered(1..=20);
        let changed = |lines: [u32; 2]| {
            let head: String = (1..=20)
                .map(|n| {
                    if lines.contains(&n) {
                        "changed\n".to_owned()
                    } else {
                        format!("line {n}\n")
                    }
                })
                .collect();
            let file = modified(lines.iter().map(|&l| hunk((l, 1), (l, 1))).collect());
            (head, file)
        };
        let (head, near) = changed([5, 11]);
        let diff = classic_diff(&near, LineSide::Head, texts(&base, &head), 1, 20, 100);
        assert_eq!(
            diff.map(|h| headers(&h).join(" ")),
            Ok("@@ -2,13 +2,13 @@".to_owned())
        );

        let (head, far) = changed([5, 13]);
        let diff = classic_diff(&far, LineSide::Head, texts(&base, &head), 1, 20, 100);
        assert_eq!(
            diff.map(|h| headers(&h).join(" ")),
            Ok("@@ -2,7 +2,7 @@ @@ -10,7 +10,7 @@".to_owned())
        );
        // Only hunks overlapping the range are shown.
        let diff = classic_diff(&far, LineSide::Head, texts(&base, &head), 12, 14, 100);
        assert_eq!(
            diff.map(|h| headers(&h).join(" ")),
            Ok("@@ -10,7 +10,7 @@".to_owned())
        );

        // Context never runs into a neighbouring hunk outside the range, before or after.
        let (head, tight) = changed([5, 7]);
        let diff = classic_diff(&tight, LineSide::Head, texts(&base, &head), 5, 5, 100);
        assert_eq!(
            diff.map(|h| headers(&h).join(" ")),
            Ok("@@ -2,5 +2,5 @@".to_owned())
        );
        let diff =
            classic_diff(&tight, LineSide::Head, texts(&base, &head), 7, 7, 100).expect("diff");
        assert_eq!(headers(&diff), ["@@ -6,5 +6,5 @@"]);
        assert_eq!(
            diff[0].lines[0],
            DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(6),
                new_line: Some(6),
                text: "line 6".to_owned(),
                eol: None,
            }
        );
    }

    #[test]
    fn classic_diff_shows_removed_code_at_its_marker_on_either_side() {
        let base = numbered(1..=10);
        let head = numbered(1..=3) + &numbered(7..=10);
        let file = modified(vec![hunk((4, 3), (3, 0))]);
        let moved = |old: u32| context(old, old - 3);
        let expected = vec![(
            "@@ -1,9 +1,6 @@".to_owned(),
            vec![
                context(1, 1),
                context(2, 2),
                context(3, 3),
                removed(4),
                removed(5),
                removed(6),
                moved(7),
                moved(8),
                moved(9),
            ],
        )];
        for (side, from, to) in [
            (LineSide::Head, 3, 3),
            (LineSide::Head, 4, 4),
            (LineSide::Base, 4, 6),
        ] {
            let diff = classic_diff(&file, side, texts(&base, &head), from, to, 100);
            assert_eq!(
                diff.map(|h| shape(&h)),
                Ok(expected.clone()),
                "{side:?} {from}..{to}"
            );
        }
        for (side, from, to) in [
            (LineSide::Head, 5, 7),
            (LineSide::Base, 1, 3),
            (LineSide::Base, 7, 10),
        ] {
            let diff = classic_diff(&file, side, texts(&base, &head), from, to, 100);
            assert_eq!(diff, Ok(Vec::new()), "{side:?} {from}..{to}: no change");
        }
    }

    #[test]
    fn classic_diff_of_added_and_deleted_files() {
        let text = numbered(1..=2);
        let added_file = modified(vec![hunk((0, 0), (1, 2))]);
        let diff = classic_diff(&added_file, LineSide::Head, texts("", &text), 1, 2000, 100);
        assert_eq!(
            diff.map(|h| shape(&h)),
            Ok(vec![(
                "@@ -0,0 +1,2 @@".to_owned(),
                vec![added(1, "line 1"), added(2, "line 2")]
            )])
        );
        let deleted_file = modified(vec![hunk((1, 2), (0, 0))]);
        let diff = classic_diff(
            &deleted_file,
            LineSide::Base,
            texts(&text, ""),
            1,
            2000,
            100,
        );
        assert_eq!(
            diff.map(|h| shape(&h)),
            Ok(vec![(
                "@@ -1,2 +0,0 @@".to_owned(),
                vec![removed(1), removed(2)]
            )])
        );
    }

    #[test]
    fn classic_diff_clips_the_side_to_the_range_and_caps_the_other_side() {
        let base = numbered(1..=3);
        let block: String = (1..=100).map(|n| format!("new {n}\n")).collect();
        let head = numbered(1..=1) + &block + &numbered(2..=3);
        let file = modified(vec![hunk((1, 0), (2, 100))]);
        let diff = classic_diff(&file, LineSide::Head, texts(&base, &head), 50, 52, 0)
            .expect("no other-side lines");
        assert_eq!(headers(&diff), ["@@ -1,0 +47,9 @@"]);
        assert_eq!(
            diff[0].lines.iter().map(|l| l.new_line).collect::<Vec<_>>(),
            (47..=55).map(Some).collect::<Vec<_>>()
        );
        assert_eq!(diff[0].lines[0].text, "new 46");

        let base = numbered(1..=10);
        let head = numbered(1..=3) + &numbered(10..=10);
        let file = modified(vec![hunk((4, 6), (3, 0))]);
        assert_eq!(
            classic_diff(&file, LineSide::Head, texts(&base, &head), 3, 3, 5),
            Err(DiffTooLarge { budget: 5 })
        );
        let diff = classic_diff(&file, LineSide::Head, texts(&base, &head), 3, 3, 6);
        assert_eq!(
            diff.map(|h| headers(&h).join(" ")),
            Ok("@@ -1,10 +1,4 @@".to_owned())
        );
    }

    #[test]
    fn consecutive_lines_merge_into_ranges() {
        let range = |start_line, end_line| LineRange {
            start_line,
            end_line,
        };
        assert_eq!(
            ranges(&[3, 5, 6, 7, 10]),
            [range(3, 3), range(5, 7), range(10, 10)]
        );
        assert!(ranges(&[]).is_empty());
    }

    /// `src/a.rs`: base 10–11 replaced by head 10–12; base 49 replaced by head 50. `new.rs` renamed from
    /// `old.rs` with base 5 replaced by head 5.
    fn comment_files() -> Vec<FileDiff> {
        vec![
            FileDiff {
                path: "src/a.rs".to_owned(),
                old_path: None,
                status: FileStatus::Modified,
                binary: false,
                hunks: vec![hunk((10, 2), (10, 3)), hunk((49, 1), (50, 1))],
            },
            FileDiff {
                path: "new.rs".to_owned(),
                old_path: Some("old.rs".to_owned()),
                status: FileStatus::Renamed,
                binary: false,
                hunks: vec![hunk((5, 1), (5, 1))],
            },
        ]
    }

    fn target(path: &str, side: LineSide, start_line: Option<u32>, line: u32) -> CommentTarget {
        CommentTarget {
            path: path.to_owned(),
            side,
            start_line,
            line,
        }
    }

    fn line_thread(path: &str, start_line: Option<u32>, line: u32) -> CommentPosition {
        CommentPosition::Line {
            path: path.to_owned(),
            start_line,
            line,
        }
    }

    #[test]
    fn target_inside_one_hunk_is_a_line_thread() {
        let files = comment_files();
        let head = LineSide::Head;
        assert_eq!(
            comment_position(&files, &target("src/a.rs", head, None, 11), 100),
            line_thread("src/a.rs", None, 11)
        );
        assert_eq!(
            comment_position(&files, &target("src/a.rs", head, Some(8), 12), 100),
            line_thread("src/a.rs", Some(8), 12),
            "context lines around the hunk take a line thread"
        );
        assert_eq!(
            comment_position(&files, &target("src/a.rs", LineSide::Base, None, 10), 100),
            line_thread("src/a.rs", None, 10)
        );
        assert_eq!(
            comment_position(&files, &target("old.rs", LineSide::Base, None, 5), 10),
            line_thread("new.rs", None, 5),
            "GitHub names a renamed file by its new path"
        );
    }

    #[test]
    fn outside_diff_comment_is_file_level() {
        let files = comment_files();
        let file = CommentPosition::File {
            path: "src/a.rs".to_owned(),
        };
        assert_eq!(
            comment_position(&files, &target("src/a.rs", LineSide::Head, None, 30), 100),
            file
        );
        assert_eq!(
            comment_position(
                &files,
                &target("src/a.rs", LineSide::Head, Some(12), 50),
                100
            ),
            file,
            "a range across two hunks is not one line thread"
        );
    }

    #[test]
    fn file_outside_the_diff_or_line_outside_the_file_is_not_commentable() {
        let files = comment_files();
        for (target, total) in [
            (target("src/other.rs", LineSide::Head, None, 1), 100),
            (target("src/a.rs", LineSide::Head, None, 101), 100),
            (target("new.rs", LineSide::Base, None, 5), 10),
            (target("old.rs", LineSide::Head, None, 5), 10),
        ] {
            assert_eq!(
                comment_position(&files, &target, total),
                CommentPosition::NotCommentable,
                "{target:?}"
            );
        }
    }

    fn flow(steps: serde_json::Value) -> FlowAnalysisOutput {
        serde_json::from_value(serde_json::json!({
            "flow_id": "f1",
            "introduction": {"problem_before": "p", "change": "c", "example_before_after": "e"},
            "steps": steps, "edges": [], "impact_refs": [], "pattern_repetitions": [], "thread_statuses": []
        }))
        .expect("flow")
    }

    fn step(file: &str, side: &str, annotations: &[(u32, u32)]) -> serde_json::Value {
        let annotations: Vec<serde_json::Value> = annotations
            .iter()
            .map(|(start, end)| {
                serde_json::json!({"type": "behaviour", "basis": "inference", "start_line": start,
                    "end_line": end, "was": "w", "is": "i", "text": null, "evidence": []})
            })
            .collect();
        serde_json::json!({"id": "s1", "title": "t", "file": file, "side": side, "start_line": 1,
            "end_line": 60, "symbol": null, "kind": "modified", "moved_from": null, "called_by": "c",
            "decides": "d", "next": "n", "why": "w", "annotations": annotations, "assessment": null})
    }

    #[test]
    fn missed_by_llm_only_for_changed_lines_without_annotation() {
        let files = comment_files();
        let flows = [flow(serde_json::json!([
            step("src/a.rs", "head", &[(10, 10)]),
            step("src/b.rs", "head", &[(11, 11)])
        ]))];
        let head = LineSide::Head;
        for (target, expected) in [
            (target("src/a.rs", head, None, 10), None),
            (target("src/a.rs", head, None, 11), Some(11)),
            (target("src/a.rs", head, Some(10), 12), Some(11)),
            (target("src/a.rs", head, None, 30), None),
            (target("src/a.rs", head, None, 50), Some(50)),
            (target("src/a.rs", LineSide::Base, None, 10), Some(10)),
            (target("src/other.rs", head, None, 11), None),
        ] {
            assert_eq!(missed_line(&files, &flows, &target), expected, "{target:?}");
        }
        assert_eq!(
            missed_line(&files, &[], &target("src/a.rs", head, None, 10)),
            Some(10),
            "without annotations every changed line is unannotated"
        );
    }
}
