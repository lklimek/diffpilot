//! Line arithmetic for showing a file by line range: the window of a file, removed blocks at their deletion
//! markers, changed lines and the lines GitHub accepts line threads on, and where a comment on lines goes.
//! Pure functions over file bytes and hunks.

use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::api::code::DeletedBlock;
use contracts::api::comments::CommentTarget;
use contracts::common::{LineRange, LineSide};

use crate::diff::{FileDiff, LineSpan};

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
