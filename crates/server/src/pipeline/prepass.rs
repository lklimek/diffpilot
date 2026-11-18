//! The deterministic pre-pass of an analysis (brief §5 Pipeline 2): base paths and file texts read from the mirror
//! through the shared bounded reader, `analysis::prepass` over them, stored as the `prepass` document.

use std::collections::BTreeMap;
use std::path::Path;

use analysis::blob::{Blob, Limits};
use analysis::diff::FileDiff;
use analysis::prepass::{self, Prepass, PrepassError};
use chrono::Utc;
use contracts::api::analyses::AnalysisSummary;
use contracts::common::LineSide;
use contracts::ids::AnalysisId;
use db::error::DbError;
use db::repo::documents::DocumentKind;

use crate::github::mirror::{self, MAX_BLOB_BYTES, MirrorError, READ_TIMEOUT};
use crate::state::AppState;

/// Most bytes of file text one pre-pass reads; files beyond it are classified without their text.
pub const MAX_PREPASS_BYTES: u64 = 128 * 1024 * 1024;

/// The pre-pass could not run.
#[derive(Debug, thiserror::Error)]
pub enum PrepassRunError {
    /// The mirror could not be read.
    #[error("the repository files could not be read")]
    Mirror(#[from] MirrorError),
    /// The pre-pass rules failed.
    #[error("the pre-pass failed")]
    Prepass(#[from] PrepassError),
    /// The blocking pre-pass task did not finish.
    #[error("the pre-pass task did not finish")]
    Task(#[source] tokio::task::JoinError),
    /// The database failed.
    #[error("the pre-pass could not be stored")]
    Db(#[from] DbError),
}

/// The stored pre-pass of `analysis`, if computed. A document stored by an earlier build in another shape counts as
/// missing: the pre-pass is deterministic, so [`ensure`] computes it again from the stored diff.
pub async fn stored(state: &AppState, analysis: AnalysisId) -> Result<Option<Prepass>, DbError> {
    match state
        .db
        .documents()
        .get(analysis, DocumentKind::Prepass, "")
        .await
    {
        Err(DbError::Json(error)) => {
            tracing::info!(%analysis, %error, "stored pre-pass has an older shape; it is computed again");
            Ok(None)
        }
        stored => stored,
    }
}

/// The stored pre-pass, computed and stored first when missing (restart-safe: the pre-pass is deterministic).
pub async fn ensure(
    state: &AppState,
    summary: &AnalysisSummary,
    mirror: &Path,
    files: &[FileDiff],
) -> Result<Prepass, PrepassRunError> {
    if let Some(prepass) = stored(state, summary.id).await? {
        return Ok(prepass);
    }
    let prepass = compute(summary, mirror, files).await?;
    store(state, summary.id, &prepass).await?;
    Ok(prepass)
}

/// [`ensure`] for a read request: a failed store (e.g. the database is busy) is logged and the computed pre-pass
/// served, since the next read computes the same one again.
pub async fn ensure_for_read(
    state: &AppState,
    summary: &AnalysisSummary,
    mirror: &Path,
    files: &[FileDiff],
) -> Result<Prepass, PrepassRunError> {
    if let Some(prepass) = stored(state, summary.id).await? {
        return Ok(prepass);
    }
    let prepass = compute(summary, mirror, files).await?;
    if let Err(error) = store(state, summary.id, &prepass).await {
        tracing::warn!(
            analysis = %summary.id,
            sqlite_code = ?error.sqlite_code(),
            ?error,
            "the pre-pass computed for a read could not be stored; the next read computes it again"
        );
    }
    Ok(prepass)
}

async fn store(state: &AppState, analysis: AnalysisId, prepass: &Prepass) -> Result<(), DbError> {
    state
        .db
        .documents()
        .put(analysis, DocumentKind::Prepass, "", prepass, Utc::now())
        .await?;
    tracing::debug!(%analysis, hunks = prepass.hunks.len(), "pre-pass stored");
    Ok(())
}

async fn compute(
    summary: &AnalysisSummary,
    mirror: &Path,
    files: &[FileDiff],
) -> Result<Prepass, PrepassRunError> {
    let (base, head) = (&summary.pr.base_sha, &summary.pr.head_sha);
    let base_paths = mirror::tree_paths(mirror, base).await?;
    let wanted = prepass::files_to_read(files, &base_paths);
    let requests = wanted
        .iter()
        .map(|(side, path)| {
            let commit = match side {
                LineSide::Base => base.clone(),
                LineSide::Head => head.clone(),
            };
            (commit, path.clone())
        })
        .collect();
    let limits = Limits {
        max_blob_bytes: MAX_BLOB_BYTES,
        total_bytes: MAX_PREPASS_BYTES,
        timeout: READ_TIMEOUT,
    };
    let blobs = mirror::read_blobs(mirror, requests, limits).await?;
    let mut texts: BTreeMap<(LineSide, String), String> = BTreeMap::new();
    let mut unread = 0usize;
    let mut too_large = Vec::new();
    for ((side, path), blob) in wanted.into_iter().zip(blobs) {
        match blob {
            Blob::Content(bytes) => {
                if let Ok(text) = String::from_utf8(bytes.to_vec()) {
                    texts.insert((side, path), text);
                }
            }
            Blob::TooLarge { .. } => {
                unread += 1;
                too_large.push(prepass::NotRendered {
                    file: path,
                    side,
                    reason: prepass::NotRenderedReason::TooLarge,
                });
            }
            Blob::OverBudget { .. } => unread += 1,
            Blob::Missing => {}
        }
    }
    if unread > 0 {
        tracing::warn!(analysis = %summary.id, files = unread,
            "pre-pass read limit reached; these files are classified by path only");
    }
    let (repo, files) = (summary.pr.repo.clone(), files.to_vec());
    tokio::task::spawn_blocking(move || {
        let mut result = prepass::prepass(&repo, &files, &base_paths, &|side, path| {
            texts.get(&(side, path.to_owned())).map(String::as_str)
        })?;
        result.not_rendered.extend(too_large);
        Ok::<_, PrepassError>(result)
    })
    .await
    .map_err(PrepassRunError::Task)?
    .map_err(PrepassRunError::from)
}
