//! Analyses: create and read. Also the error type and helpers shared by the pull-request, tour and lines
//! routes. TODO(WP3-PIPE): cancel and retry.
//!
//! An analysis is created with the pull request's base tip; the pipeline then stores the merge base of base tip
//! and head (GitHub's three-dot comparison) before any job runs. The diff, `side=base` reads, prompts and
//! permalinks all use that stored base, so routes serving the diff answer `analysis_not_finished` until the
//! analysis is prepared.

use std::borrow::Cow;
use std::path::PathBuf;

use analysis::diff::{DiffError, FileDiff};
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use contracts::api::analyses::{AnalysisSummary, CreateAnalysisRequest};
use contracts::api::error::ApiErrorCode;
use contracts::common::PrRef;
use contracts::ids::AnalysisId;
use contracts::routes::{CREATE_ANALYSIS, GET_ANALYSIS};
use db::error::DbError;
use db::repo::analyses::NewAnalysis;

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::{Session, SessionCsrf, UserTokenError, user_access_token};
use crate::github::mirror::MirrorError;
use crate::github::rest::{self, GithubError, GithubPull};
use crate::pipeline::{self, PrDiffError, tour::TourError};
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
            Self::HeadMoved | Self::AnalysisExists => ApiErrorCode::Conflict,
            Self::NotFinished => ApiErrorCode::AnalysisNotFinished,
            Self::InvalidRange | Self::InvalidPath | Self::InvalidRepository => {
                ApiErrorCode::InvalidRequest
            }
            Self::RangeTooLarge => ApiErrorCode::RangeTooLarge,
            Self::FileNotFound | Self::Github(GithubError::NotFound) => ApiErrorCode::NotFound,
            Self::Github(GithubError::Unauthorized) => ApiErrorCode::Unauthenticated,
            Self::Github(_) => ApiErrorCode::GithubUnavailable,
            Self::Mirror(MirrorError::BlobTooLarge { .. }) => ApiErrorCode::NotRendered,
            Self::Token(error) => error.code(),
            Self::Db(error) => error.code(),
            _ if self.is_fetch_failure() => ApiErrorCode::GithubUnavailable,
            Self::Mirror(_) | Self::Diff(_) | Self::DiffTask(_) | Self::Tour(_) => {
                ApiErrorCode::Internal
            }
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
            Self::RangeTooLarge => "The line range is too large; ask for at most 2000 lines.",
            Self::InvalidPath => "The file path is not valid.",
            Self::InvalidRepository => "The repository or pull request number is not valid.",
            Self::FileNotFound => "The file is not in the analysed commit.",
            Self::Github(GithubError::NotFound) => "The pull request was not found on GitHub.",
            Self::Github(GithubError::Unauthorized) => {
                "Your GitHub sign-in has ended. Sign in again."
            }
            Self::Github(_) => GITHUB_ERROR,
            Self::Mirror(MirrorError::BlobTooLarge { .. }) => "The file is too large to show.",
            _ if self.is_fetch_failure() => GITHUB_ERROR,
            Self::Mirror(_) | Self::Diff(_) | Self::DiffTask(_) | Self::Tour(_) => SERVER_ERROR,
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
        .route(GET_ANALYSIS.path, get(read))
}

/// Fetches the pull request with the user's token and stores its repository and snapshot; returns it with its
/// row id. Private repositories and names outside GitHub's charset are refused.
pub(crate) async fn fetch_pull(
    state: &AppState,
    session: &Session,
    pr: &PrRef,
) -> ApiResult<(GithubPull, i64)> {
    if !rest::is_valid_repo(&pr.repo) || pr.number == 0 {
        return Err(failure(AnalysisApiError::InvalidRepository));
    }
    let token = user_access_token(state, &session.user_id)
        .await
        .map_err(failure)?;
    let pull = rest::get_pull(&state.config.github, &token, pr)
        .await
        .map_err(failure)?;
    if pull.private {
        return Err(failure(AnalysisApiError::PrivateRepository));
    }
    let repo = state
        .db
        .repos()
        .upsert(
            &session.tenant_id,
            &pull.summary.repo,
            Some(pull.repo_github_id),
        )
        .await
        .map_err(failure)?;
    let pr_id = state
        .db
        .pull_requests()
        .upsert(&session.tenant_id, repo.id, &pull.summary, Utc::now())
        .await
        .map_err(failure)?;
    Ok((pull, pr_id))
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
    state.db.analyses().summary(id).await.map_err(failure)
}

/// The mirror of the analysed repository (fetched again if missing) and the pull request's diff;
/// `analysis_not_finished` until the pipeline has stored the analysis base.
pub(crate) async fn analysis_diff(
    state: &AppState,
    summary: &AnalysisSummary,
) -> ApiResult<(PathBuf, Vec<FileDiff>)> {
    pipeline::pr_diff(state, summary).await.map_err(failure)
}

async fn create(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    body: Result<Json<CreateAnalysisRequest>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<AnalysisSummary>)> {
    let Json(request) = body.map_err(failure)?;
    let (pull, pr_id) = fetch_pull(&state, &session, &request.pr).await?;
    if pull.summary.head_sha != request.head_sha {
        return Err(failure(AnalysisApiError::HeadMoved));
    }
    let user = state
        .db
        .users()
        .get(&session.user_id)
        .await
        .map_err(failure)?;
    let created = state
        .db
        .analyses()
        .create(
            NewAnalysis {
                id: AnalysisId::new_random(),
                tenant_id: session.tenant_id.clone(),
                pull_request_id: pr_id,
                // The pipeline replaces the base tip with the merge base before any job runs.
                base_sha: pull.summary.base_sha.clone(),
                head_sha: pull.summary.head_sha.clone(),
                language: user.language,
                created_by: session.user_id.clone(),
            },
            Utc::now(),
        )
        .await;
    let record = match created {
        Ok(record) => record,
        Err(DbError::Conflict) => return Err(failure(AnalysisApiError::AnalysisExists)),
        Err(error) => return Err(failure(error)),
    };
    tracing::info!(
        analysis = %record.id,
        user_id = %session.user_id,
        repo = %format!("{}/{}", pull.summary.repo.owner, pull.summary.repo.name),
        number = pull.summary.number,
        "analysis requested"
    );
    pipeline::start(&state, record.id);
    let summary = state
        .db
        .analyses()
        .summary(record.id)
        .await
        .map_err(failure)?;
    Ok((StatusCode::CREATED, Json(summary)))
}

async fn read(
    State(state): State<AppState>,
    session: Session,
    id: Result<Path<AnalysisId>, PathRejection>,
) -> ApiResult<Json<AnalysisSummary>> {
    let Path(id) = id.map_err(failure)?;
    Ok(Json(visible_analysis(&state, &session, id).await?))
}
