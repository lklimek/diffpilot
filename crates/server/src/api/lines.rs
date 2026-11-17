//! File content by line range: request validation and the tenant check; reading lives in `pipeline::lines`.

use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use contracts::api::code::{LinesResponse, RangeQuery};
use contracts::ids::AnalysisId;
use contracts::routes::GET_LINES;

use crate::api::analyses::{AnalysisApiError, ApiResult, failure, visible_analysis};
use crate::auth::Session;
use crate::github::mirror::is_tree_path;
use crate::pipeline;
use crate::state::AppState;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new().route(GET_LINES.path, get(lines))
}

async fn lines(
    State(state): State<AppState>,
    session: Session,
    id: Result<Path<AnalysisId>, PathRejection>,
    query: Result<Query<RangeQuery>, QueryRejection>,
) -> ApiResult<Json<LinesResponse>> {
    let Path(id) = id.map_err(failure)?;
    let Query(query) = query.map_err(failure)?;
    check_range(&query).map_err(failure)?;
    let summary = visible_analysis(&state, &session, id).await?;
    let lines = pipeline::lines::read(&state, &summary, query)
        .await
        .map_err(failure)?;
    Ok(Json(lines))
}

/// Transport checks of a `lines` or `diff` query: `1 ≤ from ≤ to`, at most `RangeQuery::MAX_LINES` lines and a
/// plain repository-relative path.
pub(crate) fn check_range(query: &RangeQuery) -> Result<(), AnalysisApiError> {
    if query.from == 0 || query.from > query.to {
        return Err(AnalysisApiError::InvalidRange);
    }
    if query.to - query.from >= RangeQuery::MAX_LINES {
        return Err(AnalysisApiError::RangeTooLarge);
    }
    if !is_tree_path(&query.path) {
        return Err(AnalysisApiError::InvalidPath);
    }
    Ok(())
}
