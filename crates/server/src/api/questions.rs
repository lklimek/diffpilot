//! Ask: list the questions of an analysis and ask a question about one of its steps or about lines of one of its
//! files (session, CSRF and tenant check; the work lives in `ask`). Answers arrive on the analysis's event stream.

use std::borrow::Cow;

use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use contracts::api::error::ApiErrorCode;
use contracts::api::questions::{AnchoredAskRequest, AskAnchor, AskQuestion, AskRequest};
use contracts::ids::{AnalysisId, StepId};
use contracts::routes::{ASK_ANCHORED, ASK_QUESTION, LIST_QUESTIONS};
use contracts::time::Timestamp;

use crate::api::analyses::{AnalysisApiError, visible_analysis};
use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::ask::{self, AskError, MAX_QUESTIONS_PER_ANALYSIS, MAX_QUESTIONS_PER_USER_PER_HOUR};
use crate::auth::{Session, SessionCsrf};
use crate::pipeline::PrDiffError;
use crate::state::AppState;

/// A questions request failed.
#[derive(Debug, thiserror::Error)]
pub enum QuestionsApiError {
    /// Malformed JSON body.
    #[error("the request body is not valid")]
    Json(#[from] JsonRejection),
    /// Malformed path.
    #[error("the path is not valid")]
    Path(#[from] PathRejection),
    /// The analysis is unknown or not visible, or the database failed.
    #[error("the analysis is not available")]
    Analysis(#[from] AnalysisApiError),
    /// The question was not accepted.
    #[error("the question was not accepted")]
    Ask(#[from] AskError),
}

impl ApiErrorSource for QuestionsApiError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Json(rejection) => rejection.code(),
            Self::Path(rejection) => rejection.code(),
            Self::Analysis(error) => error.code(),
            Self::Ask(
                AskError::Empty
                | AskError::TooLong
                | AskError::InvalidAnchor
                | AskError::FollowUpAnchor,
            ) => ApiErrorCode::InvalidRequest,
            Self::Ask(AskError::Diff(PrDiffError::NotPrepared)) => {
                ApiErrorCode::AnalysisNotFinished
            }
            Self::Ask(AskError::Diff(_) | AskError::Mirror(_) | AskError::Tour(_)) => {
                ApiErrorCode::Internal
            }
            Self::Ask(AskError::NotFinished) => ApiErrorCode::AnalysisNotFinished,
            Self::Ask(AskError::UnknownStep | AskError::UnknownFlow | AskError::UnknownParent) => {
                ApiErrorCode::NotFound
            }
            Self::Ask(AskError::Superseded | AskError::AnalysisLimit | AskError::UserLimit) => {
                ApiErrorCode::Conflict
            }
            Self::Ask(AskError::Db(error)) => error.code(),
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Json(rejection) => rejection.public_message(),
            Self::Path(rejection) => rejection.public_message(),
            Self::Analysis(error) => error.public_message(),
            Self::Ask(AskError::Empty) => Cow::Borrowed("Write a question first."),
            Self::Ask(AskError::TooLong) => Cow::Owned(format!(
                "The question is too long; write at most {} characters.",
                AskRequest::MAX_TEXT_CHARS
            )),
            Self::Ask(AskError::NotFinished) => {
                Cow::Borrowed("Questions can be asked once the analysis has finished.")
            }
            Self::Ask(AskError::Superseded) => {
                Cow::Borrowed("The pull request was analysed again; ask on the new analysis.")
            }
            Self::Ask(AskError::UnknownStep) => {
                Cow::Borrowed("The step is not part of this analysis. Reload the review.")
            }
            Self::Ask(AskError::UnknownFlow) => {
                Cow::Borrowed("The flow is not analysed in this analysis. Reload the review.")
            }
            Self::Ask(AskError::UnknownParent) => {
                Cow::Borrowed("The question to follow up was not found. Reload the review.")
            }
            Self::Ask(AskError::FollowUpAnchor) => Cow::Borrowed(
                "A follow-up is about the same place as the first question of its thread.",
            ),
            Self::Ask(AskError::InvalidAnchor) => Cow::Owned(format!(
                "Ask about lines of the step or of a file of this pull request, at most {} lines.",
                AskAnchor::MAX_LINES
            )),
            Self::Ask(AskError::Diff(PrDiffError::NotPrepared)) => {
                Cow::Borrowed("The analysis has not finished yet.")
            }
            Self::Ask(AskError::Diff(_) | AskError::Mirror(_) | AskError::Tour(_)) => {
                Cow::Borrowed("Something went wrong on the server. Try again later.")
            }
            Self::Ask(AskError::AnalysisLimit) => Cow::Owned(format!(
                "You have {MAX_QUESTIONS_PER_ANALYSIS} questions on this analysis, the most allowed."
            )),
            Self::Ask(AskError::UserLimit) => Cow::Owned(format!(
                "You asked {MAX_QUESTIONS_PER_USER_PER_HOUR} questions in the last hour, the most allowed. \
                 Ask again later."
            )),
            Self::Ask(AskError::Db(error)) => error.public_message(),
        }
    }
}

type QuestionsResult<T> = Result<T, ApiFailure<QuestionsApiError>>;

fn failure(error: impl Into<QuestionsApiError>) -> ApiFailure<QuestionsApiError> {
    ApiFailure(error.into())
}

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(LIST_QUESTIONS.path, get(list))
        .route(ASK_QUESTION.path, post(create))
        .route(ASK_ANCHORED.path, post(create_anchored))
}

async fn list(
    State(state): State<AppState>,
    session: Session,
    id: Result<Path<AnalysisId>, PathRejection>,
) -> QuestionsResult<Json<Vec<AskQuestion>>> {
    let Path(id) = id.map_err(failure)?;
    let analysis = visible_analysis(&state, &session, id)
        .await
        .map_err(|ApiFailure(error)| failure(error))?;
    let questions = state
        .db
        .questions()
        .list_for_user(analysis.id, &session.user_id, Timestamp::now())
        .await
        .map_err(|error| failure(AnalysisApiError::Db(error)))?;
    Ok(Json(questions))
}

async fn create(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    ids: Result<Path<(AnalysisId, StepId)>, PathRejection>,
    body: Result<Json<AskRequest>, JsonRejection>,
) -> QuestionsResult<(StatusCode, Json<AskQuestion>)> {
    let Path((id, step)) = ids.map_err(failure)?;
    let analysis = visible_analysis(&state, &session, id)
        .await
        .map_err(|ApiFailure(error)| failure(error))?;
    let Json(request) = body.map_err(failure)?;
    let anchored = AnchoredAskRequest {
        text: request.text,
        anchor: AskAnchor::Step {
            step_id: step,
            lines: None,
        },
        parent_id: None,
    };
    asked(&state, &session.user_id, &analysis, anchored).await
}

async fn create_anchored(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    id: Result<Path<AnalysisId>, PathRejection>,
    body: Result<Json<AnchoredAskRequest>, JsonRejection>,
) -> QuestionsResult<(StatusCode, Json<AskQuestion>)> {
    let Path(id) = id.map_err(failure)?;
    let analysis = visible_analysis(&state, &session, id)
        .await
        .map_err(|ApiFailure(error)| failure(error))?;
    let Json(request) = body.map_err(failure)?;
    asked(&state, &session.user_id, &analysis, request).await
}

async fn asked(
    state: &AppState,
    user_id: &str,
    analysis: &contracts::api::analyses::AnalysisSummary,
    request: AnchoredAskRequest,
) -> QuestionsResult<(StatusCode, Json<AskQuestion>)> {
    let mut question = ask::ask(state, user_id, analysis, request)
        .await
        .map_err(failure)?;
    question.mine = Some(true);
    Ok((StatusCode::ACCEPTED, Json(question)))
}
