//! The Tour of a finished analysis.

use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::agent::plan::PlanOutput;
use contracts::agent::pr_context::PrContextOutput;
use contracts::api::analyses::AnalysisState;
use contracts::api::tour::Tour;
use contracts::ids::AnalysisId;
use contracts::routes::GET_TOUR;
use db::error::DbError;
use db::repo::documents::DocumentKind;

use crate::api::analyses::{AnalysisApiError, ApiResult, analysis_diff, failure, visible_analysis};
use crate::auth::Session;
use crate::pipeline::tour::project;
use crate::state::AppState;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new().route(GET_TOUR.path, get(tour))
}

async fn tour(
    State(state): State<AppState>,
    session: Session,
    id: Result<Path<AnalysisId>, PathRejection>,
) -> ApiResult<Json<Tour>> {
    let Path(id) = id.map_err(failure)?;
    let summary = visible_analysis(&state, &session, id).await?;
    if !matches!(summary.state, AnalysisState::Finished { .. }) {
        return Err(failure(AnalysisApiError::NotFinished));
    }
    let documents = state.db.documents();
    let plan: PlanOutput = documents
        .get(id, DocumentKind::Plan, "")
        .await
        .map_err(failure)?
        .ok_or_else(|| failure(DbError::NotFound))?;
    let pr_context: PrContextOutput = documents
        .get(id, DocumentKind::PrContext, "")
        .await
        .map_err(failure)?
        .ok_or_else(|| failure(DbError::NotFound))?;
    let flows: Vec<FlowAnalysisOutput> = documents
        .list(id, DocumentKind::Flow)
        .await
        .map_err(failure)?
        .into_iter()
        .map(|(_, flow)| flow)
        .collect();
    let (_, files) = analysis_diff(&state, &summary).await?;
    let tour = project(&summary, &plan, pr_context, flows, &files).map_err(failure)?;
    Ok(Json(tour))
}
