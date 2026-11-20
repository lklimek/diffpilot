//! Analyses: create, list the reader's own, read, cancel and retry (request parsing and the tenant check; the
//! work lives in `pipeline`). Also the error type and helpers shared by the pull-request, tour and lines routes.
//!
//! An analysis is created with the pull request's base tip; the pipeline then stores the merge base of base tip
//! and head (GitHub's three-dot comparison) before any job runs. The diff, `side=base` reads, prompts and
//! permalinks all use that stored base, so routes serving the diff answer `analysis_not_finished` until the
//! analysis is prepared.

use std::borrow::Cow;

use analysis::diff::DiffError;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use contracts::api::analyses::{
    AnalysisSummary, CreateAnalysisRequest, MyAnalyses, MyAnalysesQuery,
};
use contracts::api::error::ApiErrorCode;
use contracts::ids::AnalysisId;
use contracts::routes::{
    CANCEL_ANALYSIS, CREATE_ANALYSIS, GET_ANALYSIS, LIST_MY_ANALYSES, PAUSE_ANALYSIS,
    RESUME_ANALYSIS, RETRY_ANALYSIS,
};
use db::error::DbError;

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::{Session, SessionCsrf, UserTokenError};
use crate::github::mirror::MirrorError;
use crate::github::rest::GithubError;
use crate::jobs::executor::ExecutorError;
use crate::pipeline::PrDiffError;
use crate::pipeline::control::{self, ControlError};
use crate::pipeline::ingest::{self, IngestError};
use crate::pipeline::lines::LinesError;
use crate::pipeline::prepass::PrepassRunError;
use crate::pipeline::tour::{TourError, TourLoadError};
use crate::state::AppState;

/// A pull-request, analysis, tour or lines request failed.
#[derive(Debug, thiserror::Error)]
pub enum AnalysisApiError {
    /// Malformed JSON body.
    #[error("the request body is not valid")]
    Json(#[from] JsonRejection),
    /// Malformed query.
    #[error("the query is not valid")]
    Query(#[from] QueryRejection),
    /// Malformed path.
    #[error("the path is not valid")]
    Path(#[from] PathRejection),
    /// Not a GitHub pull-request URL.
    #[error("the URL is not a GitHub pull request URL")]
    NotAPrUrl,
    /// The repository owner, name or number is not a valid GitHub name.
    #[error("the repository name or pull request number is not valid")]
    InvalidRepository,
    /// v1 analyses public repositories only.
    #[error("the repository is private")]
    PrivateRepository,
    /// The pull request's head moved since the user resolved it.
    #[error("the pull request head moved")]
    HeadMoved,
    /// A live analysis exists for this head and language.
    #[error("an analysis of this head already exists")]
    AnalysisExists,
    /// The Tour exists only for finished analyses; the diff only once the analysis base is stored.
    #[error("the analysis has not finished")]
    NotFinished,
    /// `limit` outside 1 to [`MyAnalysesQuery::MAX_LIMIT`].
    #[error("the limit is not valid")]
    InvalidLimit,
    /// A line range is malformed (`from` 0 or after `to`).
    #[error("the line range is not valid")]
    InvalidRange,
    /// A line range is longer than `RangeQuery::MAX_LINES`.
    #[error("the line range is too large")]
    RangeTooLarge,
    /// A path is not a plain repository-relative path.
    #[error("the path is not a repository path")]
    InvalidPath,
    /// The path is not a file in the pinned tree of that side.
    #[error("the file is not in the analysed commit")]
    FileNotFound,
    /// The pre-pass could not be computed.
    #[error("the pre-pass is not available")]
    Prepass(#[source] PrepassRunError),
    /// The file side is generated or binary: never rendered.
    #[error("the file is not rendered")]
    NotRendered,
    /// Cancel or retry does not apply in the analysis's state.
    #[error("the analysis cannot be {action} in its current state")]
    StateConflict {
        /// What was asked.
        action: &'static str,
    },
    /// The jobs of the analysis could not be cancelled.
    #[error("the jobs of the analysis could not be cancelled")]
    Executor(#[source] ExecutorError),
    /// The user's GitHub token is not available.
    #[error("the user's GitHub token is not available")]
    Token(#[from] UserTokenError),
    /// A GitHub call failed.
    #[error("a GitHub call failed")]
    Github(#[from] GithubError),
    /// The repository mirror failed.
    #[error("the repository mirror failed")]
    Mirror(#[from] MirrorError),
    /// The diff could not be computed.
    #[error("the diff could not be computed")]
    Diff(#[from] DiffError),
    /// The diff task did not finish.
    #[error("the diff task did not finish")]
    DiffTask(#[source] tokio::task::JoinError),
    /// The stored documents do not form a Tour.
    #[error("the Tour could not be built")]
    Tour(#[from] TourError),
    /// The database failed.
    #[error("the database failed")]
    Db(#[from] DbError),
}

const SERVER_ERROR: &str = "Something went wrong on the server. Try again later.";
const GITHUB_ERROR: &str = "GitHub did not answer as expected. Try again later.";

impl From<PrDiffError> for AnalysisApiError {
    fn from(error: PrDiffError) -> Self {
        match error {
            PrDiffError::Mirror(error) => Self::Mirror(error),
            PrDiffError::Diff(error) => Self::Diff(error),
            PrDiffError::Task(error) => Self::DiffTask(error),
            PrDiffError::NotPrepared => Self::NotFinished,
        }
    }
}

impl From<crate::pipeline::deps::DepsRunError> for AnalysisApiError {
    fn from(error: crate::pipeline::deps::DepsRunError) -> Self {
        match error {
            crate::pipeline::deps::DepsRunError::Mirror(error) => Self::Mirror(error),
            crate::pipeline::deps::DepsRunError::Db(error) => Self::Db(error),
        }
    }
}

impl From<IngestError> for AnalysisApiError {
    fn from(error: IngestError) -> Self {
        match error {
            IngestError::InvalidRepository => Self::InvalidRepository,
            IngestError::PrivateRepository => Self::PrivateRepository,
            IngestError::HeadMoved => Self::HeadMoved,
            IngestError::AnalysisExists => Self::AnalysisExists,
            IngestError::Token(error) => Self::Token(error),
            IngestError::Github(error) => Self::Github(error),
            IngestError::Db(error) => Self::Db(error),
        }
    }
}

impl From<ControlError> for AnalysisApiError {
    fn from(error: ControlError) -> Self {
        match error {
            ControlError::Conflict { action } => Self::StateConflict { action },
            ControlError::Db(error) => Self::Db(error),
            ControlError::Executor(error) => Self::Executor(error),
        }
    }
}

impl From<TourLoadError> for AnalysisApiError {
    fn from(error: TourLoadError) -> Self {
        match error {
            TourLoadError::NotFinished => Self::NotFinished,
            TourLoadError::Db(error) => Self::Db(error),
            TourLoadError::Diff(error) => error.into(),
            TourLoadError::Tour(error) => Self::Tour(error),
            TourLoadError::Prepass(error) => Self::Prepass(error),
        }
    }
}

impl From<LinesError> for AnalysisApiError {
    fn from(error: LinesError) -> Self {
        match error {
            LinesError::Diff(error) => error.into(),
            LinesError::Mirror(error) => Self::Mirror(error),
            LinesError::FileNotFound => Self::FileNotFound,
            LinesError::NotRendered => Self::NotRendered,
            LinesError::Prepass(error) => Self::Prepass(error),
        }
    }
}

impl AnalysisApiError {
    fn is_fetch_failure(&self) -> bool {
        matches!(
            self,
            Self::Mirror(MirrorError::Git {
                operation: "fetch",
                ..
            })
        )
    }
}

impl ApiErrorSource for AnalysisApiError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Json(rejection) => rejection.code(),
            Self::Query(rejection) => rejection.code(),
            Self::Path(rejection) => rejection.code(),
            Self::NotAPrUrl => ApiErrorCode::NotAPrUrl,
            Self::PrivateRepository => ApiErrorCode::PrivateRepository,
            Self::HeadMoved | Self::AnalysisExists | Self::StateConflict { .. } => {
                ApiErrorCode::Conflict
            }
            Self::NotRendered => ApiErrorCode::NotRendered,
            Self::NotFinished => ApiErrorCode::AnalysisNotFinished,
            Self::InvalidRange
            | Self::InvalidPath
            | Self::InvalidRepository
            | Self::InvalidLimit => ApiErrorCode::InvalidRequest,
            Self::RangeTooLarge => ApiErrorCode::RangeTooLarge,
            Self::FileNotFound | Self::Github(GithubError::NotFound) => ApiErrorCode::NotFound,
            Self::Github(GithubError::Unauthorized) => ApiErrorCode::Unauthenticated,
            Self::Github(_) => ApiErrorCode::GithubUnavailable,
            Self::Mirror(MirrorError::BlobTooLarge { .. }) => ApiErrorCode::NotRendered,
            Self::Token(error) => error.code(),
            Self::Db(error) => error.code(),
            _ if self.is_fetch_failure() => ApiErrorCode::GithubUnavailable,
            Self::Mirror(_)
            | Self::Diff(_)
            | Self::DiffTask(_)
            | Self::Tour(_)
            | Self::Executor(_)
            | Self::Prepass(_) => ApiErrorCode::Internal,
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        Cow::Borrowed(match self {
            Self::Json(rejection) => return rejection.public_message(),
            Self::Query(rejection) => return rejection.public_message(),
            Self::Path(rejection) => return rejection.public_message(),
            Self::Token(error) => return error.public_message(),
            Self::Db(error) => return error.public_message(),
            Self::NotAPrUrl => {
                "Paste the address of a GitHub pull request, like https://github.com/owner/repo/pull/123."
            }
            Self::PrivateRepository => "Only public repositories can be analysed.",
            Self::HeadMoved => "The pull request changed meanwhile. Open it again.",
            Self::AnalysisExists => {
                "This pull request head is already analysed. Open the existing analysis."
            }
            Self::NotFinished => "The analysis has not finished yet.",
            Self::InvalidRange => "The line range is not valid.",
            Self::InvalidLimit => "The limit must be between 1 and 100.",
            Self::RangeTooLarge => "The line range is too large; ask for at most 2000 lines.",
            Self::InvalidPath => "The file path is not valid.",
            Self::InvalidRepository => "The repository or pull request number is not valid.",
            Self::FileNotFound => "The file is not in the analysed commit.",
            Self::NotRendered => {
                "This file is generated or binary; it is not shown and was not given to the agent."
            }
            Self::StateConflict { action: "retried" } => {
                "Only a failed analysis can be retried. Reload to see its current state."
            }
            Self::StateConflict { action: "resumed" } => {
                "Only an analysis paused by a user can be resumed. Reload to see its current state."
            }
            Self::StateConflict { .. } => {
                "The analysis has already ended. Reload to see its current state."
            }
            Self::Github(GithubError::NotFound) => "The pull request was not found on GitHub.",
            Self::Github(GithubError::Unauthorized) => {
                "Your GitHub sign-in has ended. Sign in again."
            }
            Self::Github(_) => GITHUB_ERROR,
            Self::Mirror(MirrorError::BlobTooLarge { .. }) => "The file is too large to show.",
            _ if self.is_fetch_failure() => GITHUB_ERROR,
            Self::Mirror(_)
            | Self::Diff(_)
            | Self::DiffTask(_)
            | Self::Tour(_)
            | Self::Executor(_)
            | Self::Prepass(_) => SERVER_ERROR,
        })
    }
}

/// Handler result of this family of routes.
pub(crate) type ApiResult<T> = Result<T, ApiFailure<AnalysisApiError>>;

/// Wraps any error of this family into the handler error.
pub(crate) fn failure(error: impl Into<AnalysisApiError>) -> ApiFailure<AnalysisApiError> {
    ApiFailure(error.into())
}

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(CREATE_ANALYSIS.path, post(create))
        .route(LIST_MY_ANALYSES.path, get(list_mine))
        .route(GET_ANALYSIS.path, get(read))
        .route(CANCEL_ANALYSIS.path, post(cancel))
        .route(RETRY_ANALYSIS.path, post(retry))
        .route(PAUSE_ANALYSIS.path, post(pause))
        .route(RESUME_ANALYSIS.path, post(resume))
}

/// The analysis if the session's tenant may see it; `not_found` otherwise (no existence leak).
pub(crate) async fn visible_analysis(
    state: &AppState,
    session: &Session,
    id: AnalysisId,
) -> ApiResult<AnalysisSummary> {
    let record = state.db.analyses().get(id).await.map_err(failure)?;
    if record.tenant_id != session.tenant_id {
        return Err(failure(DbError::NotFound));
    }
    state
        .db
        .analyses()
        .summary_for_user(id, &session.user_id)
        .await
        .map_err(failure)
}

async fn create(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    body: Result<Json<CreateAnalysisRequest>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<AnalysisSummary>)> {
    let Json(request) = body.map_err(failure)?;
    let summary = ingest::create(&state, &session, &request)
        .await
        .map_err(failure)?;
    Ok((StatusCode::CREATED, Json(summary)))
}

/// The reader's own analyses: those this user requested (`created_by`), in the session's tenant. Nothing of
/// another user's analyses is listed, whatever their repository.
async fn list_mine(
    State(state): State<AppState>,
    session: Session,
    query: Result<Query<MyAnalysesQuery>, QueryRejection>,
) -> ApiResult<Json<MyAnalyses>> {
    let Query(query) = query.map_err(failure)?;
    let limit = query.limit.unwrap_or(MyAnalysesQuery::DEFAULT_LIMIT);
    if !(1..=MyAnalysesQuery::MAX_LIMIT).contains(&limit) {
        return Err(failure(AnalysisApiError::InvalidLimit));
    }
    let analyses = state
        .db
        .analyses()
        .mine(&session.tenant_id, &session.user_id, limit)
        .await
        .map_err(failure)?;
    Ok(Json(MyAnalyses { analyses }))
}

async fn read(
    State(state): State<AppState>,
    session: Session,
    id: Result<Path<AnalysisId>, PathRejection>,
) -> ApiResult<Json<AnalysisSummary>> {
    let Path(id) = id.map_err(failure)?;
    Ok(Json(visible_analysis(&state, &session, id).await?))
}

async fn cancel(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    id: Result<Path<AnalysisId>, PathRejection>,
) -> ApiResult<Json<AnalysisSummary>> {
    let Path(id) = id.map_err(failure)?;
    visible_analysis(&state, &session, id).await?;
    let summary = control::cancel(&state, id).await.map_err(failure)?;
    Ok(Json(visible_analysis(&state, &session, summary.id).await?))
}

async fn retry(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    id: Result<Path<AnalysisId>, PathRejection>,
) -> ApiResult<Json<AnalysisSummary>> {
    let Path(id) = id.map_err(failure)?;
    visible_analysis(&state, &session, id).await?;
    let summary = control::retry(&state, id).await.map_err(failure)?;
    Ok(Json(visible_analysis(&state, &session, summary.id).await?))
}

async fn pause(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    id: Result<Path<AnalysisId>, PathRejection>,
) -> ApiResult<Json<AnalysisSummary>> {
    let Path(id) = id.map_err(failure)?;
    visible_analysis(&state, &session, id).await?;
    let user = state
        .db
        .users()
        .get(&session.user_id)
        .await
        .map_err(failure)?;
    control::pause(&state, id, &user.profile.login)
        .await
        .map_err(failure)?;
    Ok(Json(visible_analysis(&state, &session, id).await?))
}

async fn resume(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    id: Result<Path<AnalysisId>, PathRejection>,
) -> ApiResult<Json<AnalysisSummary>> {
    let Path(id) = id.map_err(failure)?;
    let summary = visible_analysis(&state, &session, id).await?;
    ingest::check_head(&state, &session, &summary)
        .await
        .map_err(failure)?;
    control::resume(&state, id).await.map_err(failure)?;
    Ok(Json(visible_analysis(&state, &session, id).await?))
}

#[cfg(test)]
mod tests;
