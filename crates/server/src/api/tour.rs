//! The Tour of a review-ready analysis: the tenant check; loading lives in `pipeline::tour`.

use std::convert::Infallible;

use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use contracts::api::tour::Tour;
use contracts::ids::AnalysisId;
use contracts::routes::GET_TOUR;

use crate::api::analyses::{ApiResult, failure, visible_analysis};
use crate::auth::Session;
use crate::auth::access_tokens::github_read;
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
    let mut tour = tour::load(&state, &summary).await.map_err(failure)?;
    if let Some(links) = &mut tour.pr_context.facts.related_work_links
        && !links.is_empty()
    {
        let mut checked = links.clone();
        let Ok(verified) = github_read(&session, format!("tour-links:{id}"), async {
            if let Ok(token) = crate::auth::user_access_token(&state, &session.user_id).await {
                crate::github::related::verify(&state.config.github, &token, &mut checked).await;
            }
            Ok::<_, Infallible>(checked)
        })
        .await;
        *links = verified;
    }
    tour.pr_context.facts.memcan_status = Some(state.config.memcan.status());
    Ok(Json(tour))
}
