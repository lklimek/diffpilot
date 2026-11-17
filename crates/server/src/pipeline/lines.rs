//! File content by line range for the lines route (brief §7: never whole files) and the classic diff of a range
//! for the diff route, read only from the pinned base or head tree of a prepared analysis, with the deterministic
//! gutter facts of the displayed lines: blob reads here, line arithmetic in `analysis::lines`.

use std::path::Path;

use analysis::diff::FileDiff;
use analysis::lines::{
    DiffTexts, DiffTooLarge, LineWindow, changed_lines_within, classic_diff, deleted_blocks,
    deletion_markers, line_window, thread_ranges,
};
use analysis::prepass::Prepass;
use contracts::api::analyses::AnalysisSummary;
use contracts::api::code::{CommentScope, DeletedBlock, DiffResponse, LinesResponse, RangeQuery};
use contracts::common::LineSide;
use contracts::ids::CommitSha;

use crate::github::mirror::{MirrorError, read_blob};
use crate::pipeline::prepass::{self, PrepassRunError};
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
    /// The pre-pass lists the file side as never rendered (generated or binary).
    #[error("the file is not rendered")]
    NotRendered,
    /// The pre-pass could not be computed or read.
    #[error("the pre-pass is not available")]
    Prepass(#[from] PrepassRunError),
}

/// Refuses a file side the pre-pass lists as never rendered (generated or binary, brief §7). The pre-pass is
/// computed and stored here when the driver has not stored it yet (it is deterministic). `mirror` and `files`
/// come from [`pr_diff`] of the same analysis. Returns the pre-pass.
pub async fn ensure_rendered(
    state: &AppState,
    summary: &AnalysisSummary,
    (mirror, files): (&Path, &[FileDiff]),
    side: LineSide,
    path: &str,
) -> Result<Prepass, LinesError> {
    let prepass = prepass::ensure(state, summary, mirror, files).await?;
    if prepass.is_not_rendered(side, path) {
        return Err(LinesError::NotRendered);
    }
    Ok(prepass)
}

/// The classic diff of a range could not be shown.
#[derive(Debug, thiserror::Error)]
pub enum DiffReadError {
    /// The file or the diff could not be read.
    #[error("the file or the diff could not be read")]
    Lines(#[from] LinesError),
    /// More removed or added lines than one response may hold.
    #[error("the diff of the range is too large")]
    TooLarge(#[from] DiffTooLarge),
    /// The file is binary: its change has no lines.
    #[error("the file is binary")]
    Binary,
    /// The diff task did not finish.
    #[error("the diff task did not finish")]
    Task(#[source] tokio::task::JoinError),
}

/// Lines `query.from..=query.to` (clamped to the file) of `query.path` on `query.side` of the analysis, with
/// changed lines, removed blocks and comment scope. The query is validated by the caller.
pub async fn read(
    state: &AppState,
    summary: &AnalysisSummary,
    query: RangeQuery,
) -> Result<LinesResponse, LinesError> {
    let (mirror, files) = pr_diff(state, summary).await?;
    let prepass =
        ensure_rendered(state, summary, (&mirror, &files), query.side, &query.path).await?;
    let commit = match query.side {
        LineSide::Head => &summary.pr.head_sha,
        LineSide::Base => &summary.pr.base_sha,
    };
    let text = read_blob(&mirror, commit, &query.path)
        .await?
        .ok_or(LinesError::FileNotFound)?;
    let LineWindow {
        total_lines,
        from,
        to,
        lines,
    } = line_window(&text, query.from, query.to);
    drop(text);

    let file = files
        .iter()
        .find(|file| file.path_on(query.side) == Some(query.path.as_str()));
    let changed = changed_lines_within(&files, &query.path, query.side, from, to);
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
    // Moved blocks are detected on head lines (their target); base ranges have none.
    let moved = match query.side {
        LineSide::Head => prepass.moves.moved_lines(&query.path, from, to),
        LineSide::Base => Vec::new(),
    };
    Ok(LinesResponse {
        path: query.path,
        side: query.side,
        from,
        to,
        total_lines,
        lines,
        changed,
        deleted,
        moved,
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

/// The classic diff of lines `query.from..=query.to` of `query.path` on `query.side`: the hunks overlapping the
/// range, with context, at most `RangeQuery::MAX_LINES` lines of the other side. The query is validated by the
/// caller.
pub async fn diff(
    state: &AppState,
    summary: &AnalysisSummary,
    query: RangeQuery,
) -> Result<DiffResponse, DiffReadError> {
    let (mirror, files) = pr_diff(state, summary).await.map_err(LinesError::from)?;
    let (side_commit, other_commit, other_side) = match query.side {
        LineSide::Head => (&summary.pr.head_sha, &summary.pr.base_sha, LineSide::Base),
        LineSide::Base => (&summary.pr.base_sha, &summary.pr.head_sha, LineSide::Head),
    };
    let text = read_blob(&mirror, side_commit, &query.path)
        .await
        .map_err(LinesError::from)?
        .ok_or(LinesError::FileNotFound)?;
    let Some(file) = files
        .iter()
        .find(|file| file.path_on(query.side) == Some(query.path.as_str()))
    else {
        return Ok(DiffResponse {
            path: query.path,
            side: query.side,
            hunks: Vec::new(),
        });
    };
    // Generated and binary file sides are never rendered, on either side of the diff.
    let prepass =
        ensure_rendered(state, summary, (&mirror, &files), query.side, &query.path).await?;
    if file
        .path_on(other_side)
        .is_some_and(|other| prepass.is_not_rendered(other_side, other))
    {
        return Err(LinesError::NotRendered.into());
    }
    if file.binary {
        return Err(DiffReadError::Binary);
    }
    let other = match file.path_on(other_side) {
        Some(path) => read_blob(&mirror, other_commit, path)
            .await
            .map_err(LinesError::from)?
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let (base, head) = match query.side {
        LineSide::Head => (other, text),
        LineSide::Base => (text, other),
    };
    // Indexing two whole blobs is CPU work: off the async threads, like the diff itself.
    let (file, side, from, to) = (file.clone(), query.side, query.from, query.to);
    let hunks = tokio::task::spawn_blocking(move || {
        let texts = DiffTexts {
            base: &base,
            head: &head,
        };
        classic_diff(&file, side, texts, from, to, RangeQuery::MAX_LINES)
    })
    .await
    .map_err(DiffReadError::Task)??;
    Ok(DiffResponse {
        path: query.path,
        side: query.side,
        hunks,
    })
}
