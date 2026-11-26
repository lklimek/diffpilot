//! Tenant-scoped list of items marked as not a review point.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use contracts::api::review_loop::{MarkedItems, MarkedItemsQuery};
use contracts::ids::DecisionId;
use contracts::routes::LIST_MARKED_ITEMS;
use contracts::time::Timestamp;

use super::review_loop::ReviewLoopApiError;
use crate::auth::Session;
use crate::review_loop::LoopError;
use crate::state::AppState;

pub(super) fn routes() -> Router<AppState> {
    Router::new().route(LIST_MARKED_ITEMS.path, get(list))
}

async fn list(
    State(state): State<AppState>,
    session: Session,
    query: Result<Query<MarkedItemsQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<MarkedItems>, ReviewLoopApiError> {
    let Query(query) = query?;
    let before = query.cursor.as_deref().map(parse_cursor).transpose()?;
    let page = state
        .db
        .review_loop()
        .marked_items(&session.tenant_id, before)
        .await
        .map_err(LoopError::from)?;
    Ok(Json(page))
}

fn parse_cursor(raw: &str) -> Result<(Timestamp, DecisionId), LoopError> {
    let (at, id) = raw.split_once('/').ok_or(LoopError::InvalidCursor)?;
    Ok((
        at.parse().map_err(|_| LoopError::InvalidCursor)?,
        DecisionId(id.parse().map_err(|_| LoopError::InvalidCursor)?),
    ))
}
