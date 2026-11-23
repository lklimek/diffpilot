//! Ask (brief §5 Ask): a reviewer's question about one accepted step (or about lines or a file, an analysed flow or
//! the whole PR; a follow-up continues a thread with its anchor) of a review-ready analysis becomes an `ask` job on
//! `JobExecutor` that resumes the user's private PR working session; a lost session falls back to the context pack built
//! at claim with the asked step and its earlier answers ([`context_pack`]). The answer is checked like every agent
//! output and stored as a `Question`; each state change is published on the analysis's event stream.
//!
//! One follower task per open question drives it to `answered` or `failed` (also after a server restart:
//! [`resume_unanswered`]). Ask jobs are claimed before batch jobs and never pre-empt a running job; the queue
//! runs one job per PR, so a second question of the same analysis waits queued until the first ends.

mod job;
#[cfg(test)]
mod tests;

use contracts::agent::ask::AskOutput;
use contracts::api::analyses::{AnalysisState, AnalysisSummary};
use contracts::api::events::AnalysisEvent;
use contracts::api::questions::{AnchoredAskRequest, AskAnchor, AskQuestion, AskRequest};
use contracts::api::tour::FlowAnalysisState;
use contracts::common::LineRange;
use contracts::ids::{AnalysisId, JobId, QuestionId};
use db::error::DbError;
use db::repo::analyses::{AnalysisRecord, SessionState};
use db::repo::questions::{Asked, NewQuestion, QuestionLimits, QuestionRecord, QuestionUpdate};
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, PoisonError};

use contracts::time::Timestamp;
use jiff::SignedDuration;

pub use job::context_pack;
pub(crate) use job::{CANCELLED, SERVER_ERROR, UNFIT_ANSWER};

use crate::github::mirror::{MirrorError, is_tree_path, read_blob};
use crate::pipeline::tour::{self, TourLoadError};
use crate::pipeline::{PrDiffError, pr_diff};
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
    /// Ask requires accepted planning and PR context.
    #[error("the analysis has not finished")]
    NotFinished,
    /// A re-analysis replaced this analysis; its agent session is dropped.
    #[error("the analysis was replaced by a re-analysis")]
    Superseded,
    /// The step id is not a canonical step id of the analysis's Tour.
    #[error("the step is not part of the analysis")]
    UnknownStep,
    /// The flow has no stored analysis.
    #[error("the flow is not analysed in the analysis")]
    UnknownFlow,
    /// The review-comment item is not one of the analysed pull request's.
    #[error("the comment item is not part of the pull request")]
    UnknownItem,
    /// The parent is not a question of this user on this analysis.
    #[error("the question to follow up is not one of the user's on the analysis")]
    UnknownParent,
    /// A follow-up's anchor differs from its thread's.
    #[error("a follow-up is not about its thread's anchor")]
    FollowUpAnchor,
    /// The anchor's lines are not lines of the step or of a file of the pull request, or too many.
    #[error("the anchor is not lines of the analysis")]
    InvalidAnchor,
    /// The diff of the analysis is not available.
    #[error("the diff of the analysis is not available")]
    Diff(#[from] PrDiffError),
    /// The Tour (which flows are analysed) could not be built from the stored documents or the pre-pass.
    #[error("the Tour of the analysis is not available")]
    Tour(#[source] TourLoadError),
    /// A file could not be read from the mirror.
    #[error("a file could not be read from the mirror")]
    Mirror(#[from] MirrorError),
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

/// As on the Tour route: not finished, a missing document or a busy database, and an unprepared diff keep their own
/// answers; the rest is a server error.
impl From<TourLoadError> for AskError {
    fn from(error: TourLoadError) -> Self {
        match error {
            TourLoadError::NotFinished => Self::NotFinished,
            TourLoadError::Db(error) => Self::Db(error),
            TourLoadError::Diff(error) => Self::Diff(error),
            other @ (TourLoadError::Prepass(_) | TourLoadError::Tour(_)) => Self::Tour(other),
        }
    }
}

/// Stores the user's question about `request.anchor` (a step of the review-ready `analysis`, lines or a file, an
/// analysed flow or the whole PR; a follow-up of `request.parent_id`'s thread) as `pending`, publishes it and starts
/// its follower, which submits the `ask` job.
pub async fn ask(
    state: &AppState,
    user_id: &str,
    analysis: &AnalysisSummary,
    request: AnchoredAskRequest,
) -> Result<AskQuestion, AskError> {
    let text = question_text(&request.text)?;
    if analysis.review_ready != Some(true)
        && !matches!(analysis.state, AnalysisState::Finished { .. })
    {
        return Err(AskError::NotFinished);
    }
    if superseded(&state.db.analyses().get(analysis.id).await?) {
        return Err(AskError::Superseded);
    }
    let now = Timestamp::now();
    let limits = QuestionLimits {
        per_analysis: MAX_QUESTIONS_PER_ANALYSIS,
        per_user: MAX_QUESTIONS_PER_USER_PER_HOUR,
        user_window_start: now - SignedDuration::from_hours(1),
    };
    // Refused requests do no diff or file work.
    match state
        .db
        .questions()
        .limit_reached(analysis.id, user_id, limits)
        .await?
    {
        Some(Asked::AnalysisLimit) => return Err(AskError::AnalysisLimit),
        Some(_) => return Err(AskError::UserLimit),
        None => {}
    }
    let parent_id = match request.parent_id {
        Some(parent) => {
            Some(thread_of(state, user_id, analysis, parent, &request.anchor, now).await?)
        }
        None => None,
    };
    check_anchor(state, analysis, &request.anchor).await?;
    let question = NewQuestion {
        id: QuestionId::new_random(),
        analysis_id: analysis.id,
        anchor: request.anchor,
        parent_id,
        user_id: user_id.to_owned(),
        text: text.to_owned(),
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
    tracing::info!(analysis = %analysis.id, question = %question.id, step = ?question.step_id,
        "question asked");
    start(state, question.id);
    Ok(question)
}

/// The question text without surrounding space: not empty, at most [`AskRequest::MAX_TEXT_CHARS`].
pub(crate) fn question_text(text: &str) -> Result<&str, AskError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(AskError::Empty);
    }
    if text.chars().count() > AskRequest::MAX_TEXT_CHARS {
        return Err(AskError::TooLong);
    }
    Ok(text)
}

/// The first question of `parent`'s thread (one level: a follow-up's parent is its thread's first question); not
/// found unless `parent` is this user's question in the asker's scope (`in_scope`: its analysis or pull request), and
/// refused unless `anchor` is the thread's.
pub(crate) fn thread_root(
    parent: &QuestionRecord,
    user_id: &str,
    in_scope: bool,
    anchor: &AskAnchor,
) -> Result<QuestionId, AskError> {
    if !in_scope || parent.user_id != user_id {
        return Err(AskError::UnknownParent);
    }
    if parent.question.anchor != *anchor {
        return Err(AskError::FollowUpAnchor);
    }
    Ok(parent.question.parent_id.unwrap_or(parent.question.id))
}

/// [`thread_root`] of `parent` on `analysis`.
async fn thread_of(
    state: &AppState,
    user_id: &str,
    analysis: &AnalysisSummary,
    parent: QuestionId,
    anchor: &AskAnchor,
    now: Timestamp,
) -> Result<QuestionId, AskError> {
    let record = match state.db.questions().get(parent, now).await {
        Ok(record) => record,
        Err(DbError::NotFound) => return Err(AskError::UnknownParent),
        Err(error) => return Err(error.into()),
    };
    thread_root(
        &record,
        user_id,
        record.analysis_id == Some(analysis.id),
        anchor,
    )
}

/// A step anchor names a stored step (and lines inside it); a lines anchor names a file of the diff on its side and
/// at most [`AskAnchor::MAX_LINES`] lines of it; a flow anchor names a flow the Tour shows analysed; the PR always
/// exists; a comment-item anchor names an item of the analysed pull request.
async fn check_anchor(
    state: &AppState,
    analysis: &AnalysisSummary,
    anchor: &AskAnchor,
) -> Result<(), AskError> {
    let valid = |range: &LineRange| {
        range.start_line >= 1
            && range.start_line <= range.end_line
            && range.end_line - range.start_line < AskAnchor::MAX_LINES
    };
    match anchor {
        AskAnchor::Flow { flow_id } => {
            // As the reviewer sees it: a stored analysis that fails today's checks is shown failed.
            let tour = tour::load(state, analysis).await?;
            let analysed = tour.flows.iter().any(|flow| {
                flow.id == *flow_id
                    && flow
                        .analysis_status
                        .as_ref()
                        .is_some_and(|status| status.state == FlowAnalysisState::Analysed)
            });
            if !analysed {
                return Err(AskError::UnknownFlow);
            }
        }
        AskAnchor::Pr => {}
        AskAnchor::CommentItem { item } => {
            let pull_request = state.db.analyses().get(analysis.id).await?.pull_request_id;
            match state.db.review_loop().item(*item).await {
                Ok(record) if record.pull_request_id == pull_request => {}
                Ok(_) | Err(DbError::NotFound) => return Err(AskError::UnknownItem),
                Err(error) => return Err(error.into()),
            }
        }
        AskAnchor::Step { step_id, lines } => {
            let Some((_, step)) = job::stored_step(&state.db, analysis.id, step_id).await? else {
                return Err(AskError::UnknownStep);
            };
            let inside = |range: &LineRange| {
                valid(range)
                    && step.start_line <= range.start_line
                    && range.end_line <= step.end_line
            };
            if lines.as_ref().is_some_and(|range| !inside(range)) {
                return Err(AskError::InvalidAnchor);
            }
        }
        AskAnchor::Lines { path, side, range } => {
            if !is_tree_path(path) || range.as_ref().is_some_and(|range| !valid(range)) {
                return Err(AskError::InvalidAnchor);
            }
            let (mirror, files) = pr_diff(state, analysis).await?;
            if !files
                .iter()
                .any(|file| file.path_on(*side) == Some(path.as_str()))
            {
                return Err(AskError::InvalidAnchor);
            }
            if let Some(range) = range {
                let commit = match side {
                    contracts::common::LineSide::Head => &analysis.pr.head_sha,
                    contracts::common::LineSide::Base => &analysis.pr.base_sha,
                };
                // A file too large to show has no line selection; a question can name the whole file.
                let total = match read_blob(&mirror, commit, path).await {
                    Ok(bytes) => bytes.map_or(0, |bytes| {
                        analysis::lines::line_window(&bytes, 1, 0).total_lines
                    }),
                    Err(MirrorError::BlobTooLarge { .. }) => return Err(AskError::InvalidAnchor),
                    Err(error) => return Err(error.into()),
                };
                if range.end_line > total {
                    return Err(AskError::InvalidAnchor);
                }
            }
        }
    }
    Ok(())
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
    let open = state.db.questions().unanswered(Timestamp::now()).await?;
    for record in &open {
        start(state, record.question.id);
    }
    if !open.is_empty() {
        tracing::info!(questions = open.len(), "following the open questions again");
    }
    Ok(open.len())
}

/// A PR whose session was explicitly removed cannot accept new questions.
fn superseded(record: &AnalysisRecord) -> bool {
    matches!(
        record.session_state,
        SessionState::DropRequested | SessionState::Dropped
    )
}

/// Stores and publishes a ready answer of `job` unless the question was cancelled meanwhile (analysis cancelled,
/// PR session removed, or the job flagged): checked under the analysis's update lock, the one Cancel writes
/// under, so no answer appears after it. `None` when it is not shown.
async fn publish_provisional(
    state: &AppState,
    analysis: AnalysisId,
    question: QuestionId,
    job: JobId,
    answer: AskOutput,
) -> Result<Option<AskQuestion>, DbError> {
    let db = &state.db;
    let event = state
        .progress
        .update(analysis, || async {
            let record = db.analyses().get(analysis).await?;
            let job_cancelled = match db.jobs().get(job).await {
                Ok(job) => job.cancel_requested,
                // An executor that keeps its jobs elsewhere: the analysis decides alone.
                Err(DbError::NotFound) => false,
                Err(error) => return Err(Some(error)),
            };
            if matches!(record.state, AnalysisState::Cancelled)
                || superseded(&record)
                || job_cancelled
            {
                return Err(None);
            }
            let now = Timestamp::now();
            db.questions()
                .update(question, &QuestionUpdate::Provisional(answer), now)
                .await?;
            let record = db.questions().get(question, now).await?;
            Ok(AnalysisEvent::Question {
                question: record.question,
            })
        })
        .await;
    match event {
        Ok(AnalysisEvent::Question { question }) => Ok(Some(question)),
        Ok(AnalysisEvent::Analysis { .. } | AnalysisEvent::Heartbeat { .. }) => {
            Err(DbError::Corrupt {
                column: "question event",
            })
        }
        Err(None) => Ok(None),
        Err(Some(error)) => Err(error),
    }
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
            let now = Timestamp::now();
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
