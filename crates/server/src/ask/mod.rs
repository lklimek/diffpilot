//! Ask (brief §5 Ask): a reviewer's question about one step of a finished analysis becomes an `ask` job on
//! `JobExecutor` that resumes the analysis's agent session; a lost session falls back to the context pack built
//! at claim with the asked step and its earlier answers ([`context_pack`]). The answer is checked like every agent
//! output and stored as a `Question`; each state change is published on the analysis's event stream.
//!
//! One follower task per open question drives it to `answered` or `failed` (also after a server restart:
//! [`resume_unanswered`]). Ask jobs are claimed before batch jobs and never pre-empt a running job; the queue
//! runs one job per analysis, so a second question of the same analysis waits queued until the first ends.

mod job;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, PoisonError};

use chrono::{TimeDelta, Utc};
use contracts::api::analyses::{AnalysisState, AnalysisSummary};
use contracts::api::events::AnalysisEvent;
use contracts::api::questions::{AskQuestion, AskRequest};
use contracts::ids::{AnalysisId, QuestionId, StepId};
use db::error::DbError;
use db::repo::analyses::{AnalysisRecord, SessionState};
use db::repo::questions::{Asked, NewQuestion, QuestionLimits, QuestionUpdate};

pub use job::context_pack;

use crate::state::AppState;

/// Questions one analysis may hold (all users, every state).
pub const MAX_QUESTIONS_PER_ANALYSIS: u32 = 100;
/// Questions one user may ask per hour (all analyses).
pub const MAX_QUESTIONS_PER_USER_PER_HOUR: u32 = 30;
/// Longest answer shown, in characters; a longer one fails the question.
pub const MAX_ANSWER_CHARS: usize = 8_000;
/// Most evidence lines an answer may cite; more fail the question.
pub const MAX_EVIDENCE: usize = 50;

/// Questions with a follower task in this process.
static FOLLOWING: LazyLock<Mutex<HashSet<QuestionId>>> = LazyLock::new(Mutex::default);

/// A question was not accepted.
#[derive(Debug, thiserror::Error)]
pub enum AskError {
    /// The question is blank.
    #[error("the question is empty")]
    Empty,
    /// The question is longer than [`AskRequest::MAX_TEXT_CHARS`].
    #[error("the question is too long")]
    TooLong,
    /// Ask is available only for a finished analysis.
    #[error("the analysis has not finished")]
    NotFinished,
    /// A re-analysis replaced this analysis; its agent session is dropped.
    #[error("the analysis was replaced by a re-analysis")]
    Superseded,
    /// The step id is not a canonical step id of the analysis's Tour.
    #[error("the step is not part of the analysis")]
    UnknownStep,
    /// The analysis holds [`MAX_QUESTIONS_PER_ANALYSIS`] questions.
    #[error("the analysis holds the most questions allowed")]
    AnalysisLimit,
    /// The user asked [`MAX_QUESTIONS_PER_USER_PER_HOUR`] questions in the last hour.
    #[error("the user asked the most questions allowed per hour")]
    UserLimit,
    /// The database failed.
    #[error("the question could not be stored")]
    Db(#[from] DbError),
}

/// Stores the user's question about `step` of the finished `analysis` as `pending`, publishes it and starts its
/// follower, which submits the `ask` job.
pub async fn ask(
    state: &AppState,
    user_id: &str,
    analysis: &AnalysisSummary,
    step: StepId,
    request: AskRequest,
) -> Result<AskQuestion, AskError> {
    let text = request.text.trim();
    if text.is_empty() {
        return Err(AskError::Empty);
    }
    if text.chars().count() > AskRequest::MAX_TEXT_CHARS {
        return Err(AskError::TooLong);
    }
    if !matches!(analysis.state, AnalysisState::Finished { .. }) {
        return Err(AskError::NotFinished);
    }
    if superseded(&state.db.analyses().get(analysis.id).await?) {
        return Err(AskError::Superseded);
    }
    if job::stored_step(&state.db, analysis.id, &step)
        .await?
        .is_none()
    {
        return Err(AskError::UnknownStep);
    }
    let now = Utc::now();
    let question = NewQuestion {
        id: QuestionId::new_random(),
        analysis_id: analysis.id,
        step_id: step,
        user_id: user_id.to_owned(),
        text: text.to_owned(),
    };
    let limits = QuestionLimits {
        per_analysis: MAX_QUESTIONS_PER_ANALYSIS,
        per_user: MAX_QUESTIONS_PER_USER_PER_HOUR,
        user_window_start: now - TimeDelta::hours(1),
    };
    let db = &state.db;
    let event = state
        .progress
        .update(analysis.id, || async {
            match db.questions().create(question, limits, now).await? {
                Asked::Stored(record) => Ok(AnalysisEvent::Question {
                    question: record.question,
                }),
                Asked::AnalysisLimit => Err(AskError::AnalysisLimit),
                Asked::UserLimit => Err(AskError::UserLimit),
            }
        })
        .await?;
    let AnalysisEvent::Question { question } = event else {
        return Err(AskError::Db(DbError::Corrupt {
            column: "question event",
        }));
    };
    tracing::info!(analysis = %analysis.id, question = %question.id, step = %question.step_id,
        "question asked");
    start(state, question.id);
    Ok(question)
}

/// Starts the follower of `question` unless one runs already.
pub fn start(state: &AppState, question: QuestionId) {
    if !FOLLOWING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(question)
    {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        job::run(&state, question).await;
        FOLLOWING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&question);
    });
}

/// Starts a follower for every question left pending or answering (server start): each follows its job again,
/// or submits it if the server stopped before that, so no question stays open forever.
pub async fn resume_unanswered(state: &AppState) -> Result<usize, DbError> {
    let open = state.db.questions().unanswered(Utc::now()).await?;
    for record in &open {
        start(state, record.question.id);
    }
    if !open.is_empty() {
        tracing::info!(questions = open.len(), "following the open questions again");
    }
    Ok(open.len())
}

/// A finished analysis whose session was dropped was replaced by a re-analysis (only a cancel drops it otherwise,
/// and a finished analysis cannot be cancelled).
fn superseded(record: &AnalysisRecord) -> bool {
    matches!(
        record.session_state,
        SessionState::DropRequested | SessionState::Dropped
    )
}

/// Stores a state change of `question` and publishes it under the analysis's update lock.
async fn set(
    state: &AppState,
    analysis: AnalysisId,
    question: QuestionId,
    update: &QuestionUpdate,
) -> Result<AskQuestion, DbError> {
    let db = &state.db;
    let event = state
        .progress
        .update(analysis, || async {
            let now = Utc::now();
            db.questions().update(question, update, now).await?;
            let record = db.questions().get(question, now).await?;
            Ok::<_, DbError>(AnalysisEvent::Question {
                question: record.question,
            })
        })
        .await?;
    match event {
        AnalysisEvent::Question { question } => Ok(question),
        AnalysisEvent::Analysis { .. } | AnalysisEvent::Heartbeat { .. } => Err(DbError::Corrupt {
            column: "question event",
        }),
    }
}
