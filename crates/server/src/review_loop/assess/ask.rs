//! Ask on a review-comment item (T-L4): with a review-ready analysis of the pull request's head the question goes
//! to the normal Ask session (`crate::ask`); without one it is stored for the pull request and answered by a
//! `verify_comments` question pass. Either way the chat is the reader's own, per (pull request, user).

use contracts::api::analyses::AnalysisState;
use contracts::api::questions::{AnchoredAskRequest, AskAnchor, AskQuestion, AskRequest, AskState};
use contracts::common::RepoRef;
use contracts::ids::{AnalysisId, QuestionId};
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::analyses::SessionState;
use db::repo::questions::{Asked, NewPrQuestion, PrQuestionLimits, QuestionUpdate};
use db::repo::review_loop::PassRecord;
use jiff::SignedDuration;

use super::{PassReason, request_pass};
use crate::ask::{self, AskError, MAX_QUESTIONS_PER_ANALYSIS, MAX_QUESTIONS_PER_USER_PER_HOUR};
use crate::auth::Session;
use crate::state::AppState;

/// Failure sentence of an answer that did not pass the checks; it is never shown.
pub(super) const UNFIT_ANSWER: &str =
    "The LLM's answer did not follow the answer rules, so it is not shown. Ask again.";
/// Failure sentence of a question whose job was cancelled.
pub(super) const CANCELLED: &str = "The question was cancelled before it was answered. Ask again.";
/// Failure sentence of a server-side failure.
pub(super) const SERVER_ERROR: &str =
    "The question could not be answered because of a server error. Ask again later.";
/// Failure sentence of a question whose comment is gone from the pull request.
pub(super) const ITEM_GONE: &str =
    "The comment is no longer part of the pull request, so the question was not answered.";

/// A question about a comment item was not accepted.
#[derive(Debug, thiserror::Error)]
pub enum ItemAskError {
    /// The anchor is not a comment item.
    #[error("the question is not about a comment item")]
    NotAnItem,
    /// The pull request is unknown in the tenant.
    #[error("the pull request is not known")]
    UnknownPr,
    /// The item is not one of the pull request's.
    #[error("the comment item is not part of the pull request")]
    UnknownItem,
    /// The question was not accepted (text, limits, thread), also by the analysis's Ask.
    #[error("the question was not accepted")]
    Ask(#[from] AskError),
    /// The database failed.
    #[error("the question could not be stored")]
    Db(#[from] DbError),
}

/// Stores the reader's question about a comment item of `repo#number` and starts its answer: through the Ask
/// session of a review-ready analysis of the current head, else through a `verify_comments` question pass.
pub async fn ask_about_item(
    state: &AppState,
    session: &Session,
    repo: &RepoRef,
    number: u32,
    request: AnchoredAskRequest,
) -> Result<AskQuestion, ItemAskError> {
    let text = request.text.trim();
    if text.is_empty() {
        return Err(AskError::Empty.into());
    }
    if text.chars().count() > AskRequest::MAX_TEXT_CHARS {
        return Err(AskError::TooLong.into());
    }
    let AskAnchor::CommentItem { item } = request.anchor else {
        return Err(ItemAskError::NotAnItem);
    };
    let db = &state.db;
    let pr = db
        .pull_requests()
        .find(&session.tenant_id, repo, number)
        .await?
        .ok_or(ItemAskError::UnknownPr)?;
    match db.review_loop().item(item).await {
        Ok(record) if record.pull_request_id == pr => {}
        Ok(_) | Err(DbError::NotFound) => return Err(ItemAskError::UnknownItem),
        Err(error) => return Err(error.into()),
    }
    let now = Timestamp::now();
    let parent = match request.parent_id {
        Some(parent) => Some(db.questions().get(parent, now).await.map_err(
            |error| match error {
                DbError::NotFound => ItemAskError::Ask(AskError::UnknownParent),
                error => error.into(),
            },
        )?),
        None => None,
    };
    // A follow-up stays where its thread started; a new thread uses the analysis of the head when there is one.
    let analysis = match &parent {
        Some(parent) => parent.analysis_id,
        None => head_analysis(state, &session.user_id, pr).await?,
    };
    if let Some(analysis) = analysis {
        let summary = db
            .analyses()
            .summary_for_user(analysis, &session.user_id)
            .await?;
        let mut question = ask::ask(state, &session.user_id, &summary, request).await?;
        question.mine = Some(true);
        return Ok(question);
    }
    let parent_id = match parent {
        Some(parent) => {
            if parent.pull_request_id != Some(pr) || parent.user_id != session.user_id {
                return Err(AskError::UnknownParent.into());
            }
            if parent.question.anchor != (AskAnchor::CommentItem { item }) {
                return Err(AskError::FollowUpAnchor.into());
            }
            Some(parent.question.parent_id.unwrap_or(parent.question.id))
        }
        None => None,
    };
    let limits = PrQuestionLimits {
        per_pull_request: MAX_QUESTIONS_PER_ANALYSIS,
        per_user: MAX_QUESTIONS_PER_USER_PER_HOUR,
        user_window_start: now - SignedDuration::from_hours(1),
    };
    let question = NewPrQuestion {
        id: QuestionId::new_random(),
        pull_request_id: pr,
        item,
        parent_id,
        user_id: session.user_id.clone(),
        text: text.to_owned(),
    };
    let mut stored = match db.questions().create_for_pr(question, limits, now).await {
        Ok(Asked::Stored(record)) => record.question,
        Ok(Asked::AnalysisLimit) => return Err(AskError::AnalysisLimit.into()),
        Ok(Asked::UserLimit) => return Err(AskError::UserLimit.into()),
        Err(DbError::NotFound) => return Err(ItemAskError::UnknownItem),
        Err(error) => return Err(error.into()),
    };
    tracing::info!(pr, question = %stored.id, "question about a comment item asked without an analysis");
    // A busy pass takes the question when it ends; a failed request is retried by the next pass or at start.
    match request_pass(state, pr, PassReason::Ask).await {
        Ok(queued) => tracing::debug!(pr, ?queued, "question pass requested"),
        Err(error) => {
            tracing::warn!(pr, error = ?error, "the question pass could not be requested; the question waits")
        }
    }
    stored.mine = Some(true);
    Ok(stored)
}

/// The analysis of `pr` at its current head in the reader's language that accepts questions, if any.
async fn head_analysis(
    state: &AppState,
    user: &str,
    pr: i64,
) -> Result<Option<AnalysisId>, DbError> {
    let db = &state.db;
    let head = db.pull_requests().get(pr).await?.pr.head_sha;
    let language = db.users().get(user).await?.language;
    let Some(record) = db.analyses().latest_for_pr(pr, language).await? else {
        return Ok(None);
    };
    if record.head_sha != head
        || matches!(
            record.session_state,
            SessionState::DropRequested | SessionState::Dropped
        )
    {
        return Ok(None);
    }
    let summary = db.analyses().summary_for_user(record.id, user).await?;
    let ready = summary.review_ready == Some(true)
        || matches!(summary.state, AnalysisState::Finished { .. });
    Ok(ready.then_some(record.id))
}

/// The reader's questions about comment items of `repo#number`, with or without an analysis, oldest first.
pub async fn list_questions(
    state: &AppState,
    session: &Session,
    repo: &RepoRef,
    number: u32,
) -> Result<Vec<AskQuestion>, ItemAskError> {
    let pr = state
        .db
        .pull_requests()
        .find(&session.tenant_id, repo, number)
        .await?
        .ok_or(ItemAskError::UnknownPr)?;
    Ok(state
        .db
        .questions()
        .list_for_pull_request(pr, &session.user_id, Timestamp::now())
        .await?)
}

/// Fails a question with a plain sentence.
pub(super) async fn fail(
    state: &AppState,
    question: QuestionId,
    text: &str,
) -> Result<(), DbError> {
    state
        .db
        .questions()
        .update(
            question,
            &QuestionUpdate::Failed(text.to_owned()),
            Timestamp::now(),
        )
        .await
}

/// Fails the pass's question, if it has one that is still open.
pub(super) async fn fail_pass_question(
    state: &AppState,
    pass: &PassRecord,
    text: &str,
) -> Result<(), super::AssessError> {
    let Some(question) = pass.question else {
        return Ok(());
    };
    match fail(state, question, text).await {
        Ok(()) | Err(DbError::NotFound | DbError::Conflict) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Marks the pass's question as being answered.
pub(super) async fn answering(state: &AppState, pass: &PassRecord) -> Result<(), DbError> {
    set_open(state, pass, QuestionUpdate::Answering).await
}

/// Marks the pass's question as waiting again (usage limit).
pub(super) async fn pending(state: &AppState, pass: &PassRecord) -> Result<(), DbError> {
    set_open(state, pass, QuestionUpdate::Pending).await
}

async fn set_open(
    state: &AppState,
    pass: &PassRecord,
    update: QuestionUpdate,
) -> Result<(), DbError> {
    let Some(question) = pass.question else {
        return Ok(());
    };
    let now = Timestamp::now();
    let record = state.db.questions().get(question, now).await?;
    if record.question.state == update_state(&update) || record.answered_at.is_some() {
        return Ok(());
    }
    match state.db.questions().update(question, &update, now).await {
        Ok(()) | Err(DbError::Conflict) => Ok(()),
        Err(error) => Err(error),
    }
}

fn update_state(update: &QuestionUpdate) -> AskState {
    match update {
        QuestionUpdate::Pending => AskState::Pending,
        QuestionUpdate::Answering => AskState::Answering,
        QuestionUpdate::Provisional(_) | QuestionUpdate::Answered(_) => AskState::Answered,
        QuestionUpdate::Failed(_) => AskState::Failed,
    }
}
