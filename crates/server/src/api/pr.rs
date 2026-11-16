//! Pull-request resolution: URL parsing; fetching and lookup live in `pipeline::ingest`.

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::routing::post;
use axum::{Json, Router};
use contracts::api::pr::{ResolvePrRequest, ResolvePrResponse};
use contracts::routes::RESOLVE_PR;

use crate::api::analyses::{AnalysisApiError, ApiResult, failure};
use crate::auth::SessionCsrf;
use crate::github::rest::parse_pr_url;
use crate::pipeline::ingest;
use crate::state::AppState;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new().route(RESOLVE_PR.path, post(resolve))
}

async fn resolve(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    body: Result<Json<ResolvePrRequest>, JsonRejection>,
) -> ApiResult<Json<ResolvePrResponse>> {
    let Json(request) = body.map_err(failure)?;
    let pr = parse_pr_url(&request.url).ok_or_else(|| failure(AnalysisApiError::NotAPrUrl))?;
    let resolved = ingest::resolve(&state, &session, &pr)
        .await
        .map_err(failure)?;
    Ok(Json(resolved))
}
