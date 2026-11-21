//! The Dependencies view (R15, ADR-021): session or token read and tenant check; the map lives in
//! `pipeline::deps`.

use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use contracts::agent::plan::PlanOutput;
use contracts::api::deps::DependencyView;
use contracts::ids::AnalysisId;
use contracts::routes::GET_DEPENDENCIES;
use db::repo::documents::DocumentKind;

use crate::api::analyses::{ApiResult, failure, visible_analysis};
use crate::auth::Session;
use crate::pipeline::deps::{self, ProjectionInput};
use crate::pipeline::pr_diff;
use crate::state::AppState;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new().route(GET_DEPENDENCIES.path, get(view))
}

/// The Cargo map of the analysis's head with its facts, the plan's crate notes and the controlled owners.
async fn view(
    State(state): State<AppState>,
    session: Session,
    id: Result<Path<AnalysisId>, PathRejection>,
) -> ApiResult<Json<DependencyView>> {
    let Path(id) = id.map_err(failure)?;
    let summary = visible_analysis(&state, &session, id).await?;
    let (mirror, files) = pr_diff(&state, &summary).await.map_err(failure)?;
    let map = deps::ensure_for_read(&state, &summary, &mirror, &files)
        .await
        .map_err(failure)?;
    let plan: Option<PlanOutput> = state
        .db
        .documents()
        .get(id, DocumentKind::Plan, "")
        .await
        .map_err(failure)?;
    let controlled = state
        .db
        .settings()
        .controlled_owners(&session.tenant_id)
        .await
        .map_err(failure)?;
    Ok(Json(deps::project(&ProjectionInput {
        map: &map,
        pr_repo: &summary.pr.repo,
        files: &files,
        plan: plan.as_ref(),
        controlled: &controlled.owners,
    })))
}
