//! File content by line range for the lines route (brief §7: never whole files), read only from the pinned base
//! or head tree of a prepared analysis, with the deterministic gutter facts of the displayed lines: blob reads
//! here, line arithmetic in `analysis::lines`.

use std::path::Path;

use analysis::diff::FileDiff;
use analysis::lines::{
    changed_lines, deleted_blocks, deletion_markers, line_window, thread_ranges,
};
use contracts::api::analyses::AnalysisSummary;
use contracts::api::code::{CommentScope, DeletedBlock, LinesResponse, RangeQuery};
use contracts::common::LineSide;
use contracts::ids::CommitSha;

use crate::github::mirror::{MirrorError, read_blob};
use crate::pipeline::{PrDiffError, pr_diff};
use crate::state::AppState;

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
            removed_code(&mirror, &summary.pr.base_sha, file, from, to).await?
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

/// Removed code at every deletion marker between head lines `from - 1` and `to`, read from the base file; at
/// most `RangeQuery::MAX_LINES` removed lines in total.
async fn removed_code(
    mirror: &Path,
    base: &CommitSha,
    file: &FileDiff,
    from: u32,
    to: u32,
) -> Result<Vec<DeletedBlock>, LinesError> {
    let markers = deletion_markers(file, from, to);
    let Some(base_path) = file.path_on(LineSide::Base).filter(|_| !markers.is_empty()) else {
        return Ok(Vec::new());
    };
    let base_text = read_blob(mirror, base, base_path)
        .await?
        .unwrap_or_default();
    Ok(deleted_blocks(&markers, &base_text, RangeQuery::MAX_LINES))
}
