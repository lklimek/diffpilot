//! Pull-request resolution.

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::routing::post;
use axum::{Json, Router};
use contracts::api::pr::{ResolvePrRequest, ResolvePrResponse};
use contracts::routes::RESOLVE_PR;

use crate::api::analyses::{AnalysisApiError, ApiResult, failure, fetch_pull};
use crate::auth::SessionCsrf;
use crate::github::rest::parse_pr_url;
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
    let (pull, pr_id) = fetch_pull(&state, &session, &pr).await?;
    let user = state
        .db
        .users()
        .get(&session.user_id)
        .await
        .map_err(failure)?;
    let latest = state
        .db
        .analyses()
        .latest_for_pr(pr_id, user.language)
        .await
        .map_err(failure)?
        .filter(|record| record.tenant_id == session.tenant_id);
    let (analysis, head_moved) = match latest {
        Some(record) => (
            Some(
                state
                    .db
                    .analyses()
                    .summary(record.id)
                    .await
                    .map_err(failure)?,
            ),
            record.head_sha != pull.summary.head_sha,
        ),
        None => (None, false),
    };
    Ok(Json(ResolvePrResponse {
        pr: pull.summary,
        // TODO(WP3-APPTOKEN): true when the GitHub App is installed on the repository (installation lookup with
        // the App JWT); comments are not part of the stage-2 slice.
        commenting_available: false,
        analysis,
        head_moved,
    }))
}
