//! Classic diff of a file range: request validation (as for `lines`) and the tenant check; reading lives in
//! `pipeline::lines`, the diff arithmetic in `analysis::lines`.

use std::borrow::Cow;

use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use contracts::api::code::{DiffResponse, RangeQuery};
use contracts::api::error::ApiErrorCode;
use contracts::ids::AnalysisId;
use contracts::routes::GET_DIFF;

use crate::api::analyses::{AnalysisApiError, visible_analysis};
use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::api::lines::check_range;
use crate::auth::Session;
use crate::pipeline::lines::{self, DiffReadError};
use crate::state::AppState;

/// A diff request failed.
#[derive(Debug, thiserror::Error)]
pub enum DiffApiError {
    /// Request, tenant check or file read failed as for the `lines` route.
    #[error("the diff request failed")]
    Analysis(#[from] AnalysisApiError),
    /// The range's change has more removed or added lines than one response holds.
    #[error("the diff of the range is too large")]
    TooLarge,
    /// The file is binary.
    #[error("the file is binary")]
    Binary,
}

impl From<DiffReadError> for DiffApiError {
    fn from(error: DiffReadError) -> Self {
        match error {
            DiffReadError::Lines(error) => Self::Analysis(error.into()),
            DiffReadError::TooLarge(_) => Self::TooLarge,
            DiffReadError::Binary => Self::Binary,
        }
    }
}

impl ApiErrorSource for DiffApiError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Analysis(error) => error.code(),
            Self::TooLarge => ApiErrorCode::RangeTooLarge,
            Self::Binary => ApiErrorCode::NotRendered,
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Analysis(error) => error.public_message(),
            Self::TooLarge => Cow::Borrowed(
                "This change has more than 2000 removed or added lines, too many to show as a classic diff.",
            ),
            Self::Binary => Cow::Borrowed("The file is binary; its change has no lines to show."),
        }
    }
}

fn failure(error: impl Into<DiffApiError>) -> ApiFailure<DiffApiError> {
    ApiFailure(error.into())
}

fn analysis_failure(error: impl Into<AnalysisApiError>) -> ApiFailure<DiffApiError> {
    failure(DiffApiError::Analysis(error.into()))
}

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new().route(GET_DIFF.path, get(diff))
}

async fn diff(
    State(state): State<AppState>,
    session: Session,
    id: Result<Path<AnalysisId>, PathRejection>,
    query: Result<Query<RangeQuery>, QueryRejection>,
) -> Result<Json<DiffResponse>, ApiFailure<DiffApiError>> {
    let Path(id) = id.map_err(analysis_failure)?;
    let Query(query) = query.map_err(analysis_failure)?;
    check_range(&query).map_err(analysis_failure)?;
    let summary = visible_analysis(&state, &session, id)
        .await
        .map_err(|ApiFailure(error)| failure(error))?;
    let diff = lines::diff(&state, &summary, query)
        .await
        .map_err(failure)?;
    Ok(Json(diff))
}
