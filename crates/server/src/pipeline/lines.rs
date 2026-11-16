//! File content by line range for the lines route (brief §7: never whole files), read only from the pinned base
//! or head tree of a prepared analysis, with the deterministic gutter facts of the displayed lines.

use std::path::Path;

use analysis::diff::{FileDiff, LineSpan};
use contracts::api::analyses::AnalysisSummary;
use contracts::api::code::{CommentScope, DeletedBlock, LinesResponse, RangeQuery};
use contracts::common::{LineRange, LineSide};
use contracts::ids::CommitSha;

use crate::github::mirror::{MirrorError, read_blob};
use crate::pipeline::tour::{changed_lines, ranges};
use crate::pipeline::{PrDiffError, pr_diff};
use crate::state::AppState;

/// Context lines GitHub shows around a change; lines inside them can carry a line thread (ADR-006).
const DIFF_CONTEXT: u32 = 3;

/// Lines could not be read.
#[derive(Debug, thiserror::Error)]
pub enum LinesError {
    /// The diff of the analysis is not available (or not yet: the analysis is not prepared).
    #[error("the diff of the analysis is not available")]
    Diff(#[from] PrDiffError),
    /// A file could not be read from the mirror.
    #[error("a file could not be read from the mirror")]
    Mirror(#[from] MirrorError),
    /// The path is not a file in the pinned tree of that side.
    #[error("the file is not in the analysed commit")]
    FileNotFound,
}

/// Lines `query.from..=query.to` (clamped to the file) of `query.path` on `query.side` of the analysis, with
/// changed lines, removed blocks and comment scope. The query is validated by the caller.
pub async fn read(
    state: &AppState,
    summary: &AnalysisSummary,
    query: RangeQuery,
) -> Result<LinesResponse, LinesError> {
    let (mirror, files) = pr_diff(state, summary).await?;
    let commit = match query.side {
        LineSide::Head => &summary.pr.head_sha,
        LineSide::Base => &summary.pr.base_sha,
    };
    let text = read_blob(&mirror, commit, &query.path)
        .await?
        .ok_or(LinesError::FileNotFound)?;
    let (total_lines, shown_lines) = line_window(&text, query.from, query.to);
    drop(text);
    let (from, to) = (query.from, query.to.min(total_lines));
    let shown = |line: &u32| (from..=to).contains(line);

    let file = files
        .iter()
        .find(|file| file.path_on(query.side) == Some(query.path.as_str()));
    let changed: Vec<u32> = changed_lines(&files, &query.path, query.side)
        .into_iter()
        .filter(shown)
        .collect();
    let deleted = match (query.side, file) {
        (LineSide::Head, Some(file)) => {
            deleted_blocks(&mirror, &summary.pr.base_sha, file, from, to).await?
        }
        _ => Vec::new(),
    };
    let comments = match file {
        Some(file) => CommentScope::InDiff {
            line_thread_ranges: thread_ranges(file, query.side, total_lines, from, to),
        },
        None => CommentScope::NotInDiff,
    };
    Ok(LinesResponse {
        path: query.path,
        side: query.side,
        from,
        to,
        total_lines,
        lines: shown_lines,
        changed,
        deleted,
        // TODO(WP3-PREPASS): moved blocks from the deterministic move detection.
        moved: Vec::new(),
        comments,
    })
}

/// The line count of `bytes` and its lines `from..=to` (1-based, `str::lines` rules); only the lines in the
/// window are decoded.
fn line_window(bytes: &[u8], from: u32, to: u32) -> (u32, Vec<String>) {
    let mut shown = Vec::new();
    if bytes.is_empty() {
        return (0, shown);
    }
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let mut total = 0u32;
    for line in body.split(|&byte| byte == b'\n') {
        total = total.saturating_add(1);
        if (from..=to).contains(&total) {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            shown.push(String::from_utf8_lossy(line).into_owned());
        }
    }
    (total, shown)
}

/// Removed code at every deletion marker between lines `from - 1` and `to` of the head side; at most
/// `RangeQuery::MAX_LINES` removed lines in total, in marker order (`lines.len() < total_lines` marks a
/// truncated block, read on with `side=base`).
async fn deleted_blocks(
    mirror: &Path,
    base: &CommitSha,
    file: &FileDiff,
    from: u32,
    to: u32,
) -> Result<Vec<DeletedBlock>, LinesError> {
    let markers: Vec<(u32, LineSpan)> = file
        .hunks
        .iter()
        .filter(|hunk| hunk.base.len > 0)
        .map(|hunk| {
            let after = if hunk.head.len == 0 {
                hunk.head.start
            } else {
                hunk.head.start.saturating_sub(1)
            };
            (after, hunk.base)
        })
        .filter(|(after, _)| *after + 1 >= from && *after <= to)
        .collect();
    let Some(base_path) = file.path_on(LineSide::Base).filter(|_| !markers.is_empty()) else {
        return Ok(Vec::new());
    };
    let base_text = read_blob(mirror, base, base_path)
        .await?
        .unwrap_or_default();
    let mut budget = RangeQuery::MAX_LINES;
    let mut blocks = Vec::with_capacity(markers.len());
    let mut wanted = Vec::with_capacity(markers.len());
    for (after_line, span) in markers {
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
    let body = base_text.strip_suffix(b"\n").unwrap_or(&base_text);
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
    Ok(blocks)
}

/// The diff hunks of `file` on `side` with GitHub's context lines, merged and clipped to `from..=to`.
fn thread_ranges(
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

#[cfg(test)]
mod tests {
    use analysis::diff::{FileStatus, Hunk};
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
}
