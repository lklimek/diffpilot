//! Pull-request resolution and the reader's PR history.

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use contracts::api::pr::{PrHistory, PrHistoryQuery, ResolvePrRequest, ResolvePrResponse};
use contracts::routes::{LIST_MY_PRS, RESOLVE_PR};
use contracts::time::Timestamp;

use crate::api::analyses::{AnalysisApiError, ApiResult, failure};
use crate::auth::{Session, SessionCsrf};
use crate::github::rest::parse_pr_url;
use crate::pipeline::ingest;
use crate::state::AppState;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(RESOLVE_PR.path, post(resolve))
        .route(LIST_MY_PRS.path, get(history))
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

async fn history(
    State(state): State<AppState>,
    session: Session,
    query: Result<Query<PrHistoryQuery>, QueryRejection>,
) -> ApiResult<Json<PrHistory>> {
    let Query(query) = query.map_err(failure)?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(failure(AnalysisApiError::InvalidLimit));
    }
    let before = query
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor).ok_or_else(|| failure(AnalysisApiError::InvalidCursor)))
        .transpose()?;
    let mut rows = state
        .db
        .pull_requests()
        .history(&session.tenant_id, &session.user_id, before, limit + 1)
        .await
        .map_err(failure)?;
    let next_cursor = if rows.len() > limit as usize {
        rows.pop();
        rows.last()
            .map(|(id, row)| URL_SAFE_NO_PAD.encode(format!("{}|{id}", row.last_activity_at)))
    } else {
        None
    };
    Ok(Json(PrHistory {
        prs: rows.into_iter().map(|(_, row)| row).collect(),
        next_cursor,
    }))
}

fn decode_cursor(cursor: &str) -> Option<(Timestamp, i64)> {
    if cursor.len() > 80 {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(cursor).ok()?;
    let (at, id) = std::str::from_utf8(&bytes).ok()?.split_once('|')?;
    let id = id.parse::<i64>().ok().filter(|id| *id > 0)?;
    Some((at.parse().ok()?, id))
}
