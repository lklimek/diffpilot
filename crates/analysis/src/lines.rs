//! Line arithmetic for showing a file by line range: the window of a file, removed blocks at their deletion
//! markers, changed lines, the lines GitHub accepts line threads on and the classic diff of a range. Pure
//! functions over file bytes and hunks.

use contracts::api::code::{DeletedBlock, DiffHunk, DiffLine, DiffLineKind};
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
            after_line: if hunk.head.len == 0 {
                hunk.head.start
            } else {
                hunk.head.start.saturating_sub(1)
            },
            base: hunk.base,
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

/// Lines changed by the pull request on either side, over all files (the size of the change).
pub fn changed_line_count(files: &[FileDiff]) -> u32 {
    files
        .iter()
        .flat_map(|file| &file.hunks)
        .map(|hunk| hunk.base.len.saturating_add(hunk.head.len))
        .fold(0, u32::saturating_add)
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

fn split_lines(text: &[u8]) -> Vec<&[u8]> {
    if text.is_empty() {
        return Vec::new();
    }
    let body = text.strip_suffix(b"\n").unwrap_or(text);
    body.split(|&byte| byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .collect()
}

/// Writes the hunks of a classic diff, clipping `side` lines and counting other-side lines.
struct DiffWriter<'a> {
    side: LineSide,
    base: Vec<&'a [u8]>,
    head: Vec<&'a [u8]>,
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
        let text = (line as usize)
            .checked_sub(1)
            .and_then(|index| lines.get(index))
            .map(|text| String::from_utf8_lossy(text).into_owned())
            .unwrap_or_default();
        self.current.push(DiffLine {
            kind,
            old_line,
            new_line,
            text,
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

#[cfg(test)]
mod tests {
    use crate::diff::{FileStatus, Hunk};
    use contracts::api::code::{DiffHunk, DiffLine, DiffLineKind};
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

    type Row = (DiffLineKind, Option<u32>, Option<u32>, String);

    /// Header and `(kind, old, new, text)` rows of every hunk.
    fn shape(hunks: &[DiffHunk]) -> Vec<(String, Vec<Row>)> {
        hunks
            .iter()
            .map(|hunk| {
                let lines = hunk
                    .lines
                    .iter()
                    .map(|l| (l.kind, l.old_line, l.new_line, l.text.clone()))
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
        )
    }

    fn removed(old: u32) -> Row {
        (
            DiffLineKind::Removed,
            Some(old),
            None,
            format!("line {old}"),
        )
    }

    fn added(new: u32, text: &str) -> Row {
        (DiffLineKind::Added, None, Some(new), text.to_owned())
    }

    fn texts<'a>(base: &'a str, head: &'a str) -> DiffTexts<'a> {
        DiffTexts {
            base: base.as_bytes(),
            head: head.as_bytes(),
        }
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
                added(5, "five"),
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
}
