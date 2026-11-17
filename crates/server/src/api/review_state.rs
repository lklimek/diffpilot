//! Review state: ticks and last viewed block per user and analysis. Writes replace the whole state and are
//! checked against the analysis's Tour, so a stale step or block id is rejected; storage lives in `db`.

use std::borrow::Cow;
use std::collections::HashSet;

use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use contracts::api::error::ApiErrorCode;
use contracts::api::review::{BlockRef, QuietRowRef, ReviewState};
use contracts::api::tour::Tour;
use contracts::ids::AnalysisId;
use contracts::routes::GET_REVIEW_STATE;

use crate::api::analyses::{AnalysisApiError, visible_analysis};
use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::{Session, SessionCsrf};
use crate::pipeline::tour;
use crate::state::AppState;

/// A review-state request failed.
#[derive(Debug, thiserror::Error)]
pub enum ReviewStateApiError {
    /// Request, tenant check, Tour or database failed as for the other analysis routes.
    #[error("the review-state request failed")]
    Analysis(#[from] AnalysisApiError),
    /// A ticked step or the last viewed block is not part of the analysis's Tour.
    #[error("the review state names a block that is not part of the analysis")]
    StaleBlock,
}

impl ApiErrorSource for ReviewStateApiError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Analysis(error) => error.code(),
            Self::StaleBlock => ApiErrorCode::InvalidRequest,
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Analysis(error) => error.public_message(),
            Self::StaleBlock => Cow::Borrowed(
                "The review state names a step or block that is not part of this analysis. Reload the review.",
            ),
        }
    }
}

type StateResult<T> = Result<T, ApiFailure<ReviewStateApiError>>;

fn failure(error: impl Into<AnalysisApiError>) -> ApiFailure<ReviewStateApiError> {
    ApiFailure(ReviewStateApiError::Analysis(error.into()))
}

/// Routes of this module (`PUT_REVIEW_STATE` shares the path).
pub fn routes() -> Router<AppState> {
    Router::new().route(GET_REVIEW_STATE.path, get(read).put(replace))
}

async fn read(
    State(state): State<AppState>,
    session: Session,
    id: Result<Path<AnalysisId>, PathRejection>,
) -> StateResult<Json<ReviewState>> {
    let Path(id) = id.map_err(failure)?;
    visible_analysis(&state, &session, id)
        .await
        .map_err(|ApiFailure(error)| failure(error))?;
    let stored = state
        .db
        .review_states()
        .get(&session.user_id, id)
        .await
        .map_err(failure)?;
    Ok(Json(stored))
}

async fn replace(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    id: Result<Path<AnalysisId>, PathRejection>,
    body: Result<Json<ReviewState>, JsonRejection>,
) -> StateResult<StatusCode> {
    let Path(id) = id.map_err(failure)?;
    let Json(review) = body.map_err(failure)?;
    let summary = visible_analysis(&state, &session, id)
        .await
        .map_err(|ApiFailure(error)| failure(error))?;
    let tour = tour::load(&state, &summary).await.map_err(failure)?;
    let review =
        checked(&tour, review).ok_or_else(|| ApiFailure(ReviewStateApiError::StaleBlock))?;
    state
        .db
        .review_states()
        .put(&session.user_id, id, &review, Utc::now())
        .await
        .map_err(failure)?;
    Ok(StatusCode::NO_CONTENT)
}

/// The review state with repeated ticks removed, if every step and block it names is in the Tour.
fn checked(tour: &Tour, mut review: ReviewState) -> Option<ReviewState> {
    let steps: HashSet<&str> = tour
        .flows
        .iter()
        .flat_map(|flow| &flow.steps)
        .map(|step| step.id.0.as_str())
        .collect();
    let mut seen = HashSet::new();
    review.ticked_steps.retain(|step| seen.insert(step.clone()));
    if !review
        .ticked_steps
        .iter()
        .all(|step| steps.contains(step.0.as_str()))
    {
        return None;
    }
    let quiet = &tour.quiet;
    let known = match &review.last_viewed {
        None | Some(BlockRef::PrContext | BlockRef::PendingReview) => true,
        Some(BlockRef::FlowIntro { flow_id }) => tour.flows.iter().any(|flow| flow.id == *flow_id),
        Some(BlockRef::Step { step_id }) => steps.contains(step_id.0.as_str()),
        Some(BlockRef::Quiet { row }) => match row {
            QuietRowRef::Unexplained => !quiet.unexplained.is_empty(),
            QuietRowRef::Moves => !quiet.moves.is_empty(),
            QuietRowRef::NotAnalysed { category } => quiet
                .not_analysed
                .iter()
                .any(|section| section.category == *category),
        },
    };
    known.then_some(review)
}
