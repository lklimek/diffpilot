//! The Tour of a finished analysis: the tenant check; loading lives in `pipeline::tour`.

use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use contracts::api::tour::Tour;
use contracts::ids::AnalysisId;
use contracts::routes::GET_TOUR;

use crate::api::analyses::{ApiResult, failure, visible_analysis};
use crate::auth::Session;
use crate::pipeline::tour;
use crate::state::AppState;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new().route(GET_TOUR.path, get(read))
}

async fn read(
    State(state): State<AppState>,
    session: Session,
    id: Result<Path<AnalysisId>, PathRejection>,
) -> ApiResult<Json<Tour>> {
    let Path(id) = id.map_err(failure)?;
    let summary = visible_analysis(&state, &session, id).await?;
    let tour = tour::load(&state, &summary).await.map_err(failure)?;
    Ok(Json(tour))
}
