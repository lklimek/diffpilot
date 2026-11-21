//! The Files view: the file list with owners and Viewed marks, and setting a Viewed mark (session, CSRF and tenant
//! check; the projection lives in `pipeline::files`, the GitHub side in `review::viewed`).

use std::borrow::Cow;

use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use contracts::api::analyses::AnalysisSummary;
use contracts::api::error::ApiErrorCode;
use contracts::api::files::{FilesResponse, SetViewedRequest};
use contracts::ids::AnalysisId;
use contracts::routes::{GET_FILES, SET_FILE_VIEWED};

use crate::api::analyses::{AnalysisApiError, ApiResult, failure, visible_analysis};
use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::access_tokens::github_read;
use crate::auth::{Session, SessionCsrf};
use crate::pipeline::context::StoredResults;
use crate::pipeline::tour::{self, TourLoadError};
use crate::pipeline::{PrDiffError, files};
use crate::review::viewed::{self, ViewedError};
use crate::state::AppState;

/// A Viewed request failed.
#[derive(Debug, thiserror::Error)]
pub enum ViewedApiError {
    /// Malformed JSON body.
    #[error("the request body is not valid")]
    Json(#[from] JsonRejection),
    /// Malformed path.
    #[error("the path is not valid")]
    Path(#[from] PathRejection),
    /// The analysis is not visible.
    #[error("the analysis is not available")]
    Analysis(#[source] AnalysisApiError),
    /// The mark was not changed.
    #[error("the Viewed mark was not changed")]
    Viewed(#[from] ViewedError),
}

impl ApiErrorSource for ViewedApiError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Json(rejection) => rejection.code(),
            Self::Path(rejection) => rejection.code(),
            Self::Analysis(error) => error.code(),
            Self::Viewed(error) if error.unauthorized() => ApiErrorCode::Unauthenticated,
            Self::Viewed(ViewedError::UnknownFile) => ApiErrorCode::InvalidRequest,
            Self::Viewed(ViewedError::PrHeadChanged) => ApiErrorCode::PrHeadChanged,
            Self::Viewed(ViewedError::Busy) => ApiErrorCode::Conflict,
            Self::Viewed(ViewedError::Refused(_) | ViewedError::MissingPullRequest) => {
                ApiErrorCode::GithubRefused
            }
            Self::Viewed(
                ViewedError::Github(_) | ViewedError::NoPayload | ViewedError::HeadUnread(_),
            ) => ApiErrorCode::GithubUnavailable,
            Self::Viewed(ViewedError::Token(error)) => error.code(),
            Self::Viewed(ViewedError::Diff(PrDiffError::NotPrepared)) => {
                ApiErrorCode::AnalysisNotFinished
            }
            Self::Viewed(ViewedError::Diff(_)) => ApiErrorCode::Internal,
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        Cow::Borrowed(match self {
            Self::Json(rejection) => return rejection.public_message(),
            Self::Path(rejection) => return rejection.public_message(),
            Self::Analysis(error) => return error.public_message(),
            Self::Viewed(ViewedError::Token(error)) => return error.public_message(),
            Self::Viewed(error) if error.unauthorized() => {
                "Your GitHub sign-in has ended. Sign in again."
            }
            Self::Viewed(ViewedError::UnknownFile) => "The file is not part of this pull request.",
            Self::Viewed(ViewedError::PrHeadChanged) => {
                "The pull request changed since this analysis; marking Viewed would apply to newer code. Analyse it again, or mark it on GitHub."
            }
            Self::Viewed(ViewedError::Busy) => {
                "Your previous Viewed change is still being saved. Try again in a moment."
            }
            Self::Viewed(ViewedError::Refused(_) | ViewedError::MissingPullRequest) => {
                "Not saved on GitHub: GitHub refused the change. Your mark is unchanged; try again later or mark it on GitHub."
            }
            Self::Viewed(ViewedError::Github(_) | ViewedError::NoPayload) => {
                "GitHub did not confirm the change. Reload to see your mark, then try again."
            }
            Self::Viewed(ViewedError::HeadUnread(_)) => {
                "Could not read the pull request from GitHub, so nothing was sent. Your mark is unchanged; try again."
            }
            Self::Viewed(ViewedError::Diff(PrDiffError::NotPrepared)) => {
                "The analysis has not finished yet."
            }
            Self::Viewed(ViewedError::Diff(_)) => {
                "Something went wrong on the server. Try again later."
            }
        })
    }
}

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(GET_FILES.path, get(list))
        .route(SET_FILE_VIEWED.path, put(set_viewed))
}

/// The file list of the Tour's analysis with the user's Viewed marks (read from GitHub; null when not read).
async fn list(
    State(state): State<AppState>,
    session: Session,
    id: Result<Path<AnalysisId>, PathRejection>,
) -> ApiResult<Json<FilesResponse>> {
    let Path(id) = id.map_err(failure)?;
    let summary = visible_analysis(&state, &session, id).await?;
    tour::ready(&summary).map_err(failure)?;
    let viewed = github_read(
        &session,
        format!("files-viewed:{id}"),
        viewed::read(&state, &session.user_id, &summary),
    );
    let (listed, viewed) = tokio::join!(file_list(&state, &summary), viewed);
    let mut listed = listed.map_err(failure)?;
    let viewed = viewed.map_err(failure)?;
    for file in &mut listed {
        file.viewed = viewed.files.get(&file.path).copied();
    }
    Ok(Json(FilesResponse {
        files: listed,
        viewed_status: viewed.status,
        current_head: viewed.head,
    }))
}

async fn file_list(
    state: &AppState,
    summary: &AnalysisSummary,
) -> Result<Vec<contracts::api::files::PrFile>, TourLoadError> {
    let results = StoredResults::load(&state.db, summary.id).await?;
    tour::with_projection(state, summary, results, |projection, tour| {
        files::from_tour(projection, &tour)
    })
    .await
}

async fn set_viewed(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    id: Result<Path<AnalysisId>, PathRejection>,
    body: Result<Json<SetViewedRequest>, JsonRejection>,
) -> Result<StatusCode, ApiFailure<ViewedApiError>> {
    let Path(id) = id.map_err(|error| ApiFailure(error.into()))?;
    let Json(request) = body.map_err(|error| ApiFailure(error.into()))?;
    let summary = visible_analysis(&state, &session, id)
        .await
        .map_err(|ApiFailure(error)| ApiFailure(ViewedApiError::Analysis(error)))?;
    viewed::set(
        &state,
        &session.user_id,
        &summary,
        &request.path,
        request.viewed,
    )
    .await
    .map_err(|error| ApiFailure(error.into()))?;
    Ok(StatusCode::NO_CONTENT)
}
