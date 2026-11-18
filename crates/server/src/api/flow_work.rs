//! Lazy flow controls: tenant/CSRF/head checks and serialised durable scheduling requests.
use crate::{
    api::analyses::{AnalysisApiError, ApiResult, failure, visible_analysis},
    auth::SessionCsrf,
    pipeline::{self, control, ingest},
    state::AppState,
};
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{post, put},
};
use contracts::{
    api::{
        analyses::{AnalysisSummary, BackgroundAnalysisRequest},
        events::AnalysisEvent,
    },
    ids::{AnalysisId, FlowId},
    routes::{REQUEST_FLOW, SET_BACKGROUND_ANALYSIS},
};
use db::error::DbError;

/// CSRF-protected scheduling routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(REQUEST_FLOW.path, post(request))
        .route(SET_BACKGROUND_ANALYSIS.path, put(background))
}
async fn request(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: Result<Path<(AnalysisId, String)>, PathRejection>,
) -> ApiResult<Json<AnalysisSummary>> {
    let Path((id, flow)) = path.map_err(failure)?;
    let summary = visible_analysis(&state, &session, id).await?;
    if summary.review_ready != Some(true) || !control::is_live(&summary.state) {
        return Err(failure(AnalysisApiError::StateConflict {
            action: "scheduled",
        }));
    }
    ingest::check_head(&state, &session, &summary)
        .await
        .map_err(failure)?;
    state
        .progress
        .update(id, || async {
            state
                .db
                .flow_work()
                .request(id, &FlowId(flow), chrono::Utc::now())
                .await?;
            Ok::<_, DbError>(AnalysisEvent::Analysis {
                summary: state.db.analyses().summary(id).await?,
            })
        })
        .await
        .map_err(failure)?;
    // The driver ends while no flow is admitted: the request starts one again.
    pipeline::start(&state, id);
    Ok(Json(visible_analysis(&state, &session, id).await?))
}
async fn background(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: Result<Path<AnalysisId>, PathRejection>,
    body: Result<Json<BackgroundAnalysisRequest>, JsonRejection>,
) -> ApiResult<Json<AnalysisSummary>> {
    let Path(id) = path.map_err(failure)?;
    let Json(body) = body.map_err(failure)?;
    let summary = visible_analysis(&state, &session, id).await?;
    if !control::is_live(&summary.state) {
        return Err(failure(AnalysisApiError::StateConflict {
            action: "scheduled",
        }));
    }
    if body.enabled {
        ingest::check_head(&state, &session, &summary)
            .await
            .map_err(failure)?;
    }
    state
        .progress
        .update(id, || async {
            state
                .db
                .flow_work()
                .set_background(id, body.enabled, chrono::Utc::now())
                .await?;
            Ok::<_, DbError>(AnalysisEvent::Analysis {
                summary: state.db.analyses().summary(id).await?,
            })
        })
        .await
        .map_err(failure)?;
    if body.enabled {
        pipeline::start(&state, id);
    }
    Ok(Json(visible_analysis(&state, &session, id).await?))
}
