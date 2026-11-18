//! Lazy flow controls: tenant/CSRF/head checks and serialised durable scheduling requests.
use crate::{
    api::analyses::{AnalysisApiError, ApiResult, failure, visible_analysis},
    auth::SessionCsrf,
    pipeline::{self, control, ingest, tour},
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
        analyses::{AnalysisState, AnalysisSummary, BackgroundAnalysisRequest},
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
    let flow = FlowId(flow);
    let summary = visible_analysis(&state, &session, id).await?;
    // A finished analysis takes a request too: a failed flow there (a stored flow today's checks reject, or a
    // re-run that was cancelled) is analysed again, and the analysis runs until it is.
    let open =
        control::is_live(&summary.state) || matches!(summary.state, AnalysisState::Finished { .. });
    if summary.review_ready != Some(true) || !open {
        return Err(failure(AnalysisApiError::StateConflict {
            action: "scheduled",
        }));
    }
    ingest::check_head(&state, &session, &summary)
        .await
        .map_err(failure)?;
    // Verified outside the write; only the verified version is discarded (a newer result stays).
    let rejected = tour::rejected_stored_flows(&state, &summary)
        .await
        .map_err(failure)?
        .into_iter()
        .find_map(|(rejected, verified)| (rejected == flow).then_some(verified));
    state
        .progress
        .update(id, || async {
            let now = chrono::Utc::now();
            let work = state.db.flow_work();
            let discarded = match &rejected {
                Some(verified) => work.request_again(id, &flow, verified, now).await?,
                None => false,
            };
            if discarded {
                // Feedback on the discarded steps was removed: its MemCan memories are deleted.
                crate::memcan::writer::nudge();
            } else {
                work.request(id, &flow, now).await?;
            }
            Ok::<_, DbError>(AnalysisEvent::Analysis {
                summary: state.db.analyses().summary(id).await?,
            })
        })
        .await
        .map_err(failure)?;
    let summary = visible_analysis(&state, &session, id).await?;
    // The driver ends while no flow is admitted: the request starts one again (a finished analysis has none).
    if control::is_live(&summary.state) {
        pipeline::start(&state, id);
    }
    Ok(Json(summary))
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
    // Analysing all remaining flows includes stored flows today's checks reject (verified outside the write).
    let mut rejected = Vec::new();
    if body.enabled {
        ingest::check_head(&state, &session, &summary)
            .await
            .map_err(failure)?;
        if summary.review_ready == Some(true) {
            rejected = tour::rejected_stored_flows(&state, &summary)
                .await
                .map_err(failure)?;
        }
    }
    state
        .progress
        .update(id, || async {
            let now = chrono::Utc::now();
            let work = state.db.flow_work();
            if body.enabled {
                work.analyse_remaining(id, &rejected, now).await?;
                if !rejected.is_empty() {
                    crate::memcan::writer::nudge();
                }
            } else {
                work.set_background(id, false, now).await?;
            }
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
