//! Comments in the user's GitHub pending review (ADR-006, threat model T11): list, create (idempotent), edit and
//! delete. Writes are serialised per (user, pull request) and refused when the pull request's head moved since
//! the analysis; a new comment is also checked after the write (GitHub places it on the current head, which may
//! have moved in between) and removed again when it is not on the analysed head. A target's lines are classified from the analysed diff: inside one hunk (with GitHub's context)
//! → line thread; elsewhere in a file of the diff → file-level thread whose body ends with a permalink to the
//! lines; a file outside the diff → not commentable. A comment on a changed line no annotation covers also
//! records `missed_by_llm` feedback.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use analysis::diff::FileDiff;
use analysis::lines::{CommentPosition, comment_position, line_window, missed_line};
use chrono::Utc;
use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::api::analyses::{AnalysisState, AnalysisSummary};
use contracts::api::comments::{
    CommentTarget, CreateCommentRequest, CreateCommentResponse, PendingComment, PendingReview,
};
use contracts::api::feedback::{FeedbackTarget, FeedbackValue};
use contracts::common::{LineSide, PrRef, RepoRef};
use contracts::ids::FeedbackId;
use db::error::DbError;
use db::repo::comments::{CommentRequestKey, CommentRequestStart};
use db::repo::documents::DocumentKind;
use db::repo::feedback::NewFeedback;
use secrecy::SecretString;
use tokio::sync::OwnedMutexGuard;
use url::Url;

use crate::auth::{UserTokenError, user_access_token, user_token_rejected};
use crate::github::app_token::{self, AppTokenError, Installation};
use crate::github::graphql::{GraphqlError, GraphqlErrorType};
use crate::github::mirror::{MirrorError, is_tree_path, read_blob};
use crate::github::review::{self, PendingReviewError, ReviewState, ThreadTarget};
use crate::pipeline::{PrDiffError, pr_diff};
use crate::secrets::sha256;
use crate::state::AppState;

/// GitHub's limit for a comment body, in characters.
pub const MAX_BODY_CHARS: usize = 65_536;

/// A pending-review request failed.
#[derive(Debug, thiserror::Error)]
pub enum CommentError {
    /// The comment text is empty or too long, or the target lines are malformed.
    #[error("the comment request is not valid: {0}")]
    Invalid(&'static str),
    /// The comment is not in the user's pending review of this pull request (or was deleted on GitHub).
    #[error("the comment is not in the pending review")]
    NotFound,
    /// The GitHub App is not installed on the repository, so the user token may not write there.
    #[error("the GitHub App is not installed on the repository")]
    AppNotInstalled,
    /// The pull request's head moved since the analysis; line numbers would point to other code.
    #[error("the pull request head moved since the analysis")]
    PrHeadChanged,
    /// The head moved while the comment was written and the misplaced comment could not be deleted again: it
    /// may remain in the pending review.
    #[error("the head moved during the write and the comment could not be removed")]
    MisplacedCommentLeft(#[source] PendingReviewError),
    /// The head moved during the write and the misplaced comment was deleted again, but its review had been
    /// published meanwhile: the published comment was removed. Nothing of the attempt remains.
    #[error(
        "the head moved during the write; the misplaced comment had been published and was removed"
    )]
    MisplacedPublishedRemoved,
    /// The comment's review was published on GitHub meanwhile; the edit (or, with `deleted`, the deletion)
    /// applied to the published comment.
    #[error("the comment was published meanwhile")]
    PublishedMeanwhile {
        /// The change was a deletion.
        deleted: bool,
    },
    /// The target is not commentable (file outside the diff, line outside the file) or GitHub rejected it.
    #[error("the comment position is not commentable")]
    PositionRejected,
    /// The idempotency key belongs to another request, or its first attempt's outcome is unknown.
    #[error("the idempotency key belongs to another or an unfinished request")]
    KeyConflict,
    /// GitHub did not confirm a comment write; it may or may not exist.
    #[error("GitHub did not confirm the comment write")]
    OutcomeUnknown(#[source] PendingReviewError),
    /// Whether the GitHub App is installed could not be read.
    #[error("the GitHub App installation could not be read")]
    App(#[source] AppTokenError),
    /// The user's GitHub token is not available.
    #[error("the user's GitHub token is not available")]
    Token(#[from] UserTokenError),
    /// A GitHub call failed.
    #[error("a GitHub call failed")]
    Github(#[from] PendingReviewError),
    /// The diff of the analysis is not available.
    #[error("the diff of the analysis is not available")]
    Diff(#[from] PrDiffError),
    /// A file could not be read from the mirror.
    #[error("a file could not be read from the mirror")]
    Mirror(#[from] MirrorError),
    /// The database failed.
    #[error("the database failed")]
    Db(#[from] DbError),
}

/// Pending-review write locks by (user, repository, pull request); entries without holders are dropped.
static WRITE_LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Mutex::default);

/// Waits until no other write of `user_id` to `pr`'s pending review runs (GitHub creates a second pending
/// review for concurrent first comments, S3 run 80).
async fn lock_writes(user_id: &str, pr: &PrRef) -> OwnedMutexGuard<()> {
    let key = format!(
        "{user_id}\n{}/{}#{}",
        pr.repo.owner.to_ascii_lowercase(),
        pr.repo.name.to_ascii_lowercase(),
        pr.number
    );
    let lock = {
        let mut locks = WRITE_LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        Arc::clone(locks.entry(key).or_default())
    };
    lock.lock_owned().await
}

fn pr_of(summary: &AnalysisSummary) -> PrRef {
    PrRef {
        repo: summary.pr.repo.clone(),
        number: summary.pr.number,
    }
}

/// Whether the user token may write review comments to `repo`: the GitHub App must be installed there (S3
/// finding 10). An App that is unusable as configured makes commenting unavailable.
pub(crate) async fn commenting_available(
    state: &AppState,
    repo: &RepoRef,
) -> Result<bool, CommentError> {
    match app_token::installation(&state.config.github, repo, Utc::now()).await {
        Ok(Installation::Installed { .. }) => Ok(true),
        Ok(Installation::NotInstalled) => Ok(false),
        Err(error) if error.is_configuration() => {
            tracing::error!(%error, "GitHub App unusable; commenting is not available");
            Ok(false)
        }
        Err(error) => Err(CommentError::App(error)),
    }
}

async fn require_commenting(state: &AppState, repo: &RepoRef) -> Result<(), CommentError> {
    if commenting_available(state, repo).await? {
        Ok(())
    } else {
        Err(CommentError::AppNotInstalled)
    }
}

fn validate_body(body: &str) -> Result<(), CommentError> {
    if body.trim().is_empty() {
        return Err(CommentError::Invalid("empty body"));
    }
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(CommentError::Invalid("body too long"));
    }
    Ok(())
}

/// Passes `result` on; a GitHub 401 for the user's `token` makes its next use refresh the pair (ADR-007 addendum).
/// Never retries the call.
async fn note_rejected<T>(
    state: &AppState,
    user_id: &str,
    token: &SecretString,
    result: Result<T, CommentError>,
) -> Result<T, CommentError> {
    if let Err(CommentError::Github(PendingReviewError::Graphql(GraphqlError::Unauthorized))) =
        &result
    {
        user_token_rejected(state, user_id, token).await;
    }
    result
}

/// Maps a GitHub failure of a pending-review call whose outcome is known.
fn github_error(error: PendingReviewError) -> CommentError {
    match error {
        PendingReviewError::ThreadRejected => CommentError::PositionRejected,
        PendingReviewError::Graphql(ref graphql) => match graphql.kind() {
            Some(GraphqlErrorType::Forbidden) => CommentError::AppNotInstalled,
            Some(GraphqlErrorType::NotFound) => CommentError::NotFound,
            _ => CommentError::Github(error),
        },
        error => CommentError::Github(error),
    }
}

/// Maps a failure of a thread write. Only answers that prove nothing was written are definite: GitHub refused
/// the request before executing it (GraphQL errors without `data` or with `data: null`; HTTP 401, 4xx or a rate
/// limit without resolved data; a client that sent nothing), `FORBIDDEN` with the mutation field null, or
/// `thread: null` without errors. Anything else (no answer, 5xx, an unreadable answer, any error next to
/// resolved data, any other error of a failed mutation field) leaves the outcome unknown.
fn write_error(error: PendingReviewError) -> CommentError {
    match error {
        PendingReviewError::ThreadRejected => CommentError::PositionRejected,
        PendingReviewError::Graphql(
            GraphqlError::FieldFailed(GraphqlErrorType::Forbidden)
            | GraphqlError::NotExecuted(GraphqlErrorType::Forbidden),
        ) => CommentError::AppNotInstalled,
        PendingReviewError::Graphql(
            GraphqlError::NotExecuted(_)
            | GraphqlError::Client(_)
            | GraphqlError::Unauthorized
            | GraphqlError::RateLimited,
        ) => CommentError::Github(error),
        PendingReviewError::Graphql(GraphqlError::Status(status)) if status.is_client_error() => {
            CommentError::Github(error)
        }
        error => CommentError::OutcomeUnknown(error),
    }
}

/// `https://github.com/<o>/<r>/blob/<sha of side>/<path>#L<a>[-L<b>]`.
fn permalink(summary: &AnalysisSummary, target: &CommentTarget) -> String {
    let sha = match target.side {
        LineSide::Head => &summary.pr.head_sha,
        LineSide::Base => &summary.pr.base_sha,
    };
    let repo = &summary.pr.repo;
    let raw = format!(
        "https://github.com/{}/{}/blob/{}",
        repo.owner, repo.name, sha.0
    );
    let start = target.start_line.unwrap_or(target.line);
    let fragment = if start < target.line {
        format!("L{start}-L{}", target.line)
    } else {
        format!("L{}", target.line)
    };
    let Ok(mut url) = Url::parse(&raw) else {
        return format!("{raw}/{}#{fragment}", target.path);
    };
    if let Ok(mut segments) = url.path_segments_mut() {
        segments.extend(target.path.split('/'));
    }
    url.set_fragment(Some(&fragment));
    url.into()
}

/// Records `missed_by_llm` when the analysis is finished and the target has a changed line without annotation;
/// returns whether it was recorded. A failure is logged and never undoes the comment.
async fn record_missed(
    state: &AppState,
    user_id: &str,
    summary: &AnalysisSummary,
    files: &[FileDiff],
    target: &CommentTarget,
) -> bool {
    if !matches!(summary.state, AnalysisState::Finished { .. }) {
        return false;
    }
    let flows: Vec<FlowAnalysisOutput> = match state
        .db
        .documents()
        .list(summary.id, DocumentKind::Flow)
        .await
    {
        Ok(flows) => flows.into_iter().map(|(_, flow)| flow).collect(),
        Err(error) => {
            tracing::warn!(analysis = %summary.id, ?error,
                "flow analyses could not be read; missed_by_llm not recorded for the comment");
            return false;
        }
    };
    let Some(line) = missed_line(files, &flows, target) else {
        return false;
    };
    let feedback = NewFeedback {
        id: FeedbackId::new_random(),
        analysis_id: summary.id,
        user_id: user_id.to_owned(),
        target: FeedbackTarget::Line {
            path: target.path.clone(),
            side: target.side,
            line,
        },
        value: FeedbackValue::MissedByLlm,
        note: None,
    };
    match state.db.feedback().upsert(feedback, Utc::now()).await {
        Ok(_) => {
            crate::memcan::writer::nudge();
            true
        }
        Err(error) => {
            tracing::warn!(analysis = %summary.id, ?error,
                "missed_by_llm feedback could not be stored; the comment stays on GitHub");
            false
        }
    }
}

/// The user's pending comments on the analysed pull request, merged over all their pending reviews.
pub async fn list(
    state: &AppState,
    user_id: &str,
    summary: &AnalysisSummary,
) -> Result<PendingReview, CommentError> {
    let commenting_available = commenting_available(state, &summary.pr.repo).await?;
    let token = user_access_token(state, user_id).await?;
    let pending = review::pending_comments(&state.config.github, &token, &pr_of(summary))
        .await
        .map_err(github_error);
    let pending = note_rejected(state, user_id, &token, pending).await?;
    Ok(PendingReview {
        commenting_available,
        comments: pending.comments,
        incomplete: pending.incomplete,
    })
}

/// Adds the comment to the user's pending review. The same idempotency key with the same request returns the
/// first response without writing again.
pub async fn create(
    state: &AppState,
    user_id: &str,
    summary: &AnalysisSummary,
    request: &CreateCommentRequest,
) -> Result<CreateCommentResponse, CommentError> {
    validate_body(&request.body)?;
    let target = &request.target;
    if !is_tree_path(&target.path)
        || target.line == 0
        || target
            .start_line
            .is_some_and(|start| start == 0 || start > target.line)
    {
        return Err(CommentError::Invalid("target lines"));
    }
    let request_hash =
        sha256(&serde_json::to_vec(&(target, &request.body)).map_err(DbError::from)?);
    let idempotency_key = request.idempotency_key.to_string();
    let key = CommentRequestKey {
        user_id,
        idempotency_key: &idempotency_key,
    };
    let _write = lock_writes(user_id, &pr_of(summary)).await;
    let requests = state.db.comment_requests();
    match requests
        .begin(key, summary.id, &request_hash, Utc::now())
        .await?
    {
        CommentRequestStart::Started => {}
        CommentRequestStart::Done(response) => return Ok(response),
        CommentRequestStart::InProgress | CommentRequestStart::Mismatch => {
            return Err(CommentError::KeyConflict);
        }
    }
    match add_comment(state, user_id, summary, request).await {
        Ok(response) => {
            if let Err(error) = requests.complete(key, &response).await {
                tracing::error!(analysis = %summary.id, ?error,
                    "the comment was added but its idempotency record could not be completed");
            }
            Ok(response)
        }
        // A comment may exist on GitHub: the key stays busy so a retry cannot add a second one.
        Err(error @ (CommentError::OutcomeUnknown(_) | CommentError::MisplacedCommentLeft(_))) => {
            Err(error)
        }
        Err(error) => {
            if let Err(db_error) = requests.fail(key).await {
                tracing::error!(analysis = %summary.id, error = ?db_error,
                    "a failed comment request could not be released for a retry");
            }
            Err(error)
        }
    }
}

async fn add_comment(
    state: &AppState,
    user_id: &str,
    summary: &AnalysisSummary,
    request: &CreateCommentRequest,
) -> Result<CreateCommentResponse, CommentError> {
    let target = &request.target;
    require_commenting(state, &summary.pr.repo).await?;
    let token = user_access_token(state, user_id).await?;
    let (mirror, files) = pr_diff(state, summary).await?;
    let commit = match target.side {
        LineSide::Head => &summary.pr.head_sha,
        LineSide::Base => &summary.pr.base_sha,
    };
    let total_lines = match read_blob(&mirror, commit, &target.path).await? {
        Some(bytes) => line_window(&bytes, 1, 1).total_lines,
        None => 0,
    };
    let (thread, body) = match comment_position(&files, target, total_lines) {
        CommentPosition::NotCommentable => return Err(CommentError::PositionRejected),
        CommentPosition::Line {
            path,
            start_line,
            line,
        } => (
            ThreadTarget::Line {
                path,
                side: target.side,
                start_line,
                line,
            },
            request.body.clone(),
        ),
        CommentPosition::File { path } => (
            ThreadTarget::File { path },
            format!(
                "{}\n\n{}",
                request.body.trim_end(),
                permalink(summary, target)
            ),
        ),
    };
    validate_body(&body)?;
    let pr = pr_of(summary);
    let pull = note_rejected(
        state,
        user_id,
        &token,
        head_checked(state, &token, summary).await,
    )
    .await?;
    let added = review::add_thread(&state.config.github, &token, &pr, &pull, &thread, &body)
        .await
        .map_err(write_error);
    let added = note_rejected(state, user_id, &token, added).await?;
    let head = &summary.pr.head_sha.0;
    if added.commits.is_empty() || added.commits.iter().any(|oid| oid != head) {
        return Err(remove_misplaced(state, &token, summary, &added.comment.id).await);
    }
    let comment = added.comment;
    tracing::info!(analysis = %summary.id, user_id, kind = ?comment.kind,
        "comment added to the pending review");
    let missed_by_llm_recorded = record_missed(state, user_id, summary, &files, target).await;
    Ok(CreateCommentResponse {
        comment,
        missed_by_llm_recorded,
    })
}

/// The pull request's node id, after checking that its head is still the analysed head.
async fn head_checked(
    state: &AppState,
    token: &SecretString,
    summary: &AnalysisSummary,
) -> Result<String, CommentError> {
    let pull = review::pull_request_head(&state.config.github, token, &pr_of(summary))
        .await
        .map_err(github_error)?;
    if pull.head_oid != summary.pr.head_sha.0 {
        return Err(CommentError::PrHeadChanged);
    }
    Ok(pull.id)
}

/// Deletes a comment GitHub placed on another head than the analysed one (the head moved between the check and
/// the write); the answer is `PrHeadChanged`, or `MisplacedCommentLeft` when the deletion failed.
async fn remove_misplaced(
    state: &AppState,
    token: &SecretString,
    summary: &AnalysisSummary,
    comment_id: &str,
) -> CommentError {
    match review::delete_comment(&state.config.github, token, comment_id).await {
        Ok(ReviewState::Pending) => {
            tracing::warn!(analysis = %summary.id,
                "the head moved during the comment write; the misplaced comment was deleted again");
            CommentError::PrHeadChanged
        }
        Ok(ReviewState::Published) => {
            tracing::warn!(analysis = %summary.id,
                "the head moved during the comment write and the review was published meanwhile; the misplaced published comment was deleted");
            CommentError::MisplacedPublishedRemoved
        }
        Err(error) => {
            tracing::error!(analysis = %summary.id, ?error,
                "the head moved during the comment write and the misplaced comment could not be deleted");
            CommentError::MisplacedCommentLeft(error)
        }
    }
}

/// The user's pending comment `comment_id` on the analysed pull request, after the head check. A list cut at the
/// read cap is no proof of absence: the comment is then looked up by id.
async fn own_pending_comment(
    state: &AppState,
    token: &SecretString,
    summary: &AnalysisSummary,
    comment_id: &str,
) -> Result<PendingComment, CommentError> {
    let pending = review::pending_comments(&state.config.github, token, &pr_of(summary))
        .await
        .map_err(github_error)?;
    if pending.pull_request.head_oid != summary.pr.head_sha.0 {
        return Err(CommentError::PrHeadChanged);
    }
    let incomplete = pending.incomplete;
    if let Some(comment) = pending
        .comments
        .into_iter()
        .find(|comment| comment.id == comment_id)
    {
        return Ok(comment);
    }
    if !incomplete {
        return Err(CommentError::NotFound);
    }
    review::pending_comment_by_id(&state.config.github, token, &pr_of(summary), comment_id)
        .await
        .map_err(github_error)?
        .ok_or(CommentError::NotFound)
}

/// Replaces the text of the user's pending comment `comment_id` on the analysed pull request. GitHub has no
/// conditional edit: when the review was published after the check, the edit applied to the published comment
/// and the answer says so (`PublishedMeanwhile`).
pub async fn update(
    state: &AppState,
    user_id: &str,
    summary: &AnalysisSummary,
    comment_id: &str,
    body: &str,
) -> Result<PendingComment, CommentError> {
    validate_body(body)?;
    let _write = lock_writes(user_id, &pr_of(summary)).await;
    require_commenting(state, &summary.pr.repo).await?;
    let token = user_access_token(state, user_id).await?;
    let comment = own_pending_comment(state, &token, summary, comment_id).await;
    let comment = note_rejected(state, user_id, &token, comment).await?;
    let updated = review::update_comment(&state.config.github, &token, &comment.id, body)
        .await
        .map_err(github_error);
    let (body, review_state) = note_rejected(state, user_id, &token, updated).await?;
    if review_state == ReviewState::Published {
        tracing::warn!(analysis = %summary.id, user_id,
            "the review was published meanwhile; the edit applied to the published comment");
        return Err(CommentError::PublishedMeanwhile { deleted: false });
    }
    tracing::info!(analysis = %summary.id, user_id, "pending comment edited");
    Ok(PendingComment { body, ..comment })
}

/// Deletes the user's pending comment `comment_id` on the analysed pull request; like [`update`], a review
/// published after the check is reported (`PublishedMeanwhile`).
pub async fn delete(
    state: &AppState,
    user_id: &str,
    summary: &AnalysisSummary,
    comment_id: &str,
) -> Result<(), CommentError> {
    let _write = lock_writes(user_id, &pr_of(summary)).await;
    require_commenting(state, &summary.pr.repo).await?;
    let token = user_access_token(state, user_id).await?;
    let comment = own_pending_comment(state, &token, summary, comment_id).await;
    let comment = note_rejected(state, user_id, &token, comment).await?;
    let deleted = review::delete_comment(&state.config.github, &token, &comment.id)
        .await
        .map_err(github_error);
    let review_state = note_rejected(state, user_id, &token, deleted).await?;
    if review_state == ReviewState::Published {
        tracing::warn!(analysis = %summary.id, user_id,
            "the review was published meanwhile; the deletion applied to the published comment");
        return Err(CommentError::PublishedMeanwhile { deleted: true });
    }
    tracing::info!(analysis = %summary.id, user_id, "pending comment deleted");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_failures_are_unknown_unless_they_prove_nothing_was_written() {
        let status = |code| PendingReviewError::Graphql(GraphqlError::Status(code));
        let graphql = PendingReviewError::Graphql;
        for unknown in [
            status(reqwest::StatusCode::BAD_GATEWAY),
            graphql(GraphqlError::NoData),
            graphql(GraphqlError::Rejected(GraphqlErrorType::Other)),
            graphql(GraphqlError::Rejected(GraphqlErrorType::NotFound)),
            graphql(GraphqlError::Rejected(GraphqlErrorType::Unprocessable)),
            graphql(GraphqlError::Rejected(GraphqlErrorType::Forbidden)),
            graphql(GraphqlError::Rejected(GraphqlErrorType::RateLimited)),
            graphql(GraphqlError::FieldFailed(GraphqlErrorType::Other)),
            graphql(GraphqlError::FieldFailed(GraphqlErrorType::NotFound)),
            PendingReviewError::MissingPullRequest,
        ] {
            let mapped = write_error(unknown);
            assert!(
                matches!(mapped, CommentError::OutcomeUnknown(_)),
                "{mapped:?}"
            );
        }
        for refused in [
            graphql(GraphqlError::NotExecuted(GraphqlErrorType::Other)),
            graphql(GraphqlError::NotExecuted(GraphqlErrorType::Unprocessable)),
            graphql(GraphqlError::Unauthorized),
            graphql(GraphqlError::RateLimited),
            status(reqwest::StatusCode::BAD_REQUEST),
        ] {
            let mapped = write_error(refused);
            assert!(matches!(mapped, CommentError::Github(_)), "{mapped:?}");
        }
        assert!(matches!(
            write_error(graphql(GraphqlError::NotExecuted(
                GraphqlErrorType::Forbidden
            ))),
            CommentError::AppNotInstalled
        ));
        assert!(matches!(
            write_error(graphql(GraphqlError::FieldFailed(
                GraphqlErrorType::Forbidden
            ))),
            CommentError::AppNotInstalled
        ));
        assert!(matches!(
            write_error(PendingReviewError::ThreadRejected),
            CommentError::PositionRejected
        ));
        assert!(matches!(
            write_error(status(reqwest::StatusCode::FORBIDDEN)),
            CommentError::Github(_)
        ));
    }

    #[tokio::test]
    async fn writes_of_one_user_to_one_pull_request_are_serialised() {
        let pr = PrRef {
            repo: RepoRef {
                owner: "Acme".to_owned(),
                name: "Node-Client".to_owned(),
            },
            number: 482,
        };
        let held = lock_writes("u-serial", &pr).await;
        let mut same = pr.clone();
        same.repo.owner = "acme".to_owned();
        let waiting = tokio::spawn(async move { lock_writes("u-serial", &same).await });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished(), "same user and PR waits");
        let other_user = lock_writes("u-other", &pr).await;
        let mut other_pr = pr.clone();
        other_pr.number = 483;
        let other_number = lock_writes("u-serial", &other_pr).await;
        drop((other_user, other_number));
        drop(held);
        waiting.await.expect("lock acquired after release");
    }
}

/// Comment operations against a wiremock GitHub and the fixture repository of `acme/node-client#482`.
#[cfg(test)]
mod github_tests {
    use std::path::Path;
    use std::process::Command;

    use contracts::api::comments::CommentKind;
    use contracts::api::pr::PrSummary;
    use contracts::ids::{AnalysisId, CommitSha};
    use db::repo::DEFAULT_TENANT;
    use db::repo::analyses::NewAnalysis;
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use uuid::Uuid;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::github::review::{
        ADD_THREAD, COMMENT_BY_ID, DELETE_COMMENT, MAX_COMMENT_PAGES, PENDING_REVIEW,
        PULL_REQUEST_HEAD, REVIEW_COMMENTS_PAGE, UPDATE_COMMENT,
    };
    use crate::review::tests::github_state_with;

    const HEAD: &str = "52d2da140191a9d47becf6a446310be803c007fc";
    const CLIENT: &str = "src/rpc/client.rs";

    fn fixtures() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482")
    }

    fn recorded(name: &str) -> Value {
        let text =
            std::fs::read_to_string(fixtures().join("github/graphql").join(name)).expect("fixture");
        serde_json::from_str::<Value>(&text).expect("JSON")["response"].clone()
    }

    struct Fixture {
        state: AppState,
        summary: AnalysisSummary,
        server: MockServer,
        _dir: TempDir,
    }

    /// A finished analysis of the fixture PR (flow documents stored) with its mirror, for user `u1`.
    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("temp dir");
        let built = Command::new(fixtures().join("repo/build-repo.sh"))
            .arg(dir.path().join("remotes/acme/node-client.git"))
            .output()
            .expect("build-repo.sh");
        assert!(built.status.success(), "{built:?}");
        let server = MockServer::start().await;
        let (state, _) = github_state_with(&server, true, |config| {
            config.data_dir = dir.path().join("data");
            config.github.clone_url =
                Url::parse(&format!("file://{}", dir.path().join("remotes").display()))
                    .expect("url");
        })
        .await;
        let text = std::fs::read_to_string(fixtures().join("api-seed/default/pr-resolve.json"))
            .expect("seed");
        let pr: PrSummary = serde_json::from_value(
            serde_json::from_str::<Value>(&text).expect("JSON")["pr"].clone(),
        )
        .expect("pr");
        let repo = state
            .db
            .repos()
            .upsert(DEFAULT_TENANT, &pr.repo, None)
            .await
            .expect("repo");
        let pr_id = state
            .db
            .pull_requests()
            .upsert(DEFAULT_TENANT, repo.id, &pr, Utc::now())
            .await
            .expect("pr");
        let id = state
            .db
            .analyses()
            .create(
                NewAnalysis {
                    id: AnalysisId::new_random(),
                    tenant_id: DEFAULT_TENANT.to_owned(),
                    pull_request_id: pr_id,
                    base_sha: pr.base_sha.clone(),
                    head_sha: pr.head_sha.clone(),
                    language: Default::default(),
                    created_by: "u1".to_owned(),
                },
                Utc::now(),
            )
            .await
            .expect("analysis")
            .id;
        for n in 1..=5 {
            let text = std::fs::read_to_string(fixtures().join(format!("agent/flow-{n}.json")))
                .expect("flow");
            let flow: Value = serde_json::from_str(&text).expect("JSON");
            state
                .db
                .documents()
                .put(id, DocumentKind::Flow, &format!("f{n}"), &flow, Utc::now())
                .await
                .expect("flow document");
        }
        state
            .db
            .analyses()
            .set_state(id, &AnalysisState::Finished { total: 5 }, Utc::now())
            .await
            .expect("finished");
        let summary = state.db.analyses().summary(id).await.expect("summary");
        assert_eq!(summary.pr.head_sha, CommitSha(HEAD.to_owned()));
        Fixture {
            state,
            summary,
            server,
            _dir: dir,
        }
    }

    impl Fixture {
        async fn mock(&self, request: Value, response: ResponseTemplate, calls: u64) {
            Mock::given(method("POST"))
                .and(path("/api/graphql"))
                .and(body_partial_json(request))
                .respond_with(response)
                .expect(calls)
                .mount(&self.server)
                .await;
        }

        async fn head_is(&self, head: &str, calls: u64) {
            let response = json!({"data": {"repository": {"pullRequest": {"id": "PR_fixture482", "headRefOid": head}}}});
            self.mock(
                json!({"query": PULL_REQUEST_HEAD}),
                ResponseTemplate::new(200).set_body_json(response),
                calls,
            )
            .await;
        }

        async fn add_thread_answers(&self, input: Value, response: ResponseTemplate, calls: u64) {
            self.mock(
                json!({"query": ADD_THREAD, "variables": {"in": input}}),
                response,
                calls,
            )
            .await;
        }

        async fn pending_is(&self, response: Value, calls: u64) {
            self.mock(
                json!({"query": PENDING_REVIEW}),
                ResponseTemplate::new(200).set_body_json(response),
                calls,
            )
            .await;
        }

        async fn create(
            &self,
            key: Uuid,
            side: LineSide,
            start_line: Option<u32>,
            line: u32,
            path: &str,
            body: &str,
        ) -> Result<CreateCommentResponse, CommentError> {
            let request = CreateCommentRequest {
                idempotency_key: key,
                target: CommentTarget {
                    path: path.to_owned(),
                    side,
                    start_line,
                    line,
                },
                body: body.to_owned(),
            };
            create(&self.state, "u1", &self.summary, &request).await
        }

        async fn graphql_bodies(&self) -> Vec<Value> {
            self.server
                .received_requests()
                .await
                .unwrap_or_default()
                .iter()
                .filter(|request| request.url.path() == "/api/graphql")
                .map(|request| serde_json::from_slice(&request.body).expect("JSON body"))
                .collect()
        }
    }

    fn line_ok() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(recorded("add-thread-line.json"))
    }

    fn pending_one() -> Value {
        let mut response = recorded("pending-list-one.json");
        let pull = &mut response["data"]["repository"]["pullRequest"];
        let none = json!({"hasNextPage": false, "endCursor": null});
        pull["reviewThreads"]["pageInfo"] = none.clone();
        pull["reviews"]["pageInfo"] = none.clone();
        for review in pull["reviews"]["nodes"].as_array_mut().expect("reviews") {
            review["comments"]["pageInfo"] = none.clone();
            for comment in review["comments"]["nodes"]
                .as_array_mut()
                .expect("comments")
            {
                comment["replyTo"] = Value::Null;
            }
        }
        response
    }

    #[tokio::test]
    async fn second_comment_reuses_the_pending_review() {
        let f = fixture().await;
        f.head_is(HEAD, 2).await;
        f.add_thread_answers(
            json!({"pullRequestId": "PR_fixture482", "line": 96}),
            line_ok(),
            1,
        )
        .await;
        f.add_thread_answers(
            json!({"pullRequestId": "PR_fixture482", "line": 97}),
            line_ok(),
            1,
        )
        .await;
        f.create(Uuid::new_v4(), LineSide::Head, None, 96, CLIENT, "first")
            .await
            .expect("first");
        f.create(Uuid::new_v4(), LineSide::Head, None, 97, CLIENT, "second")
            .await
            .expect("second");
        let mutations: Vec<String> = f
            .graphql_bodies()
            .await
            .iter()
            .filter_map(|body| body["query"].as_str().map(str::to_owned))
            .filter(|query| query.starts_with("mutation"))
            .collect();
        assert_eq!(
            mutations,
            [ADD_THREAD, ADD_THREAD],
            "both comments go to the pending review by pull request id; no review is created"
        );
    }

    #[tokio::test]
    async fn double_submit_with_one_key_creates_one_comment() {
        let f = fixture().await;
        f.head_is(HEAD, 1).await;
        f.add_thread_answers(json!({"line": 96, "side": "RIGHT"}), line_ok(), 1)
            .await;
        let key = Uuid::new_v4();
        let (first, second) = tokio::join!(
            f.create(key, LineSide::Head, None, 96, CLIENT, "body"),
            f.create(key, LineSide::Head, None, 96, CLIENT, "body"),
        );
        let first = first.expect("first");
        assert_eq!(second.expect("second"), first);
        let third = f
            .create(key, LineSide::Head, None, 96, CLIENT, "body")
            .await
            .expect("third");
        assert_eq!(third, first);
        assert_eq!(first.comment.id, "PRRC_fixture482_c1");
        assert_eq!(first.comment.kind, CommentKind::Line);
    }

    #[tokio::test]
    async fn key_reused_for_another_comment_conflicts() {
        let f = fixture().await;
        f.head_is(HEAD, 1).await;
        f.add_thread_answers(json!({"line": 96}), line_ok(), 1)
            .await;
        let key = Uuid::new_v4();
        f.create(key, LineSide::Head, None, 96, CLIENT, "body")
            .await
            .expect("first");
        let other = f
            .create(key, LineSide::Head, None, 96, CLIENT, "other")
            .await;
        assert!(matches!(other, Err(CommentError::KeyConflict)), "{other:?}");
    }

    #[tokio::test]
    async fn outside_diff_comment_is_file_level_on_github() {
        let f = fixture().await;
        f.head_is(HEAD, 1).await;
        let link =
            format!("https://github.com/acme/node-client/blob/{HEAD}/src/rpc/client.rs#L60-L62");
        f.add_thread_answers(
            json!({"pullRequestId": "PR_fixture482", "path": CLIENT, "subjectType": "FILE",
                   "body": format!("Look here.\n\n{link}")}),
            ResponseTemplate::new(200).set_body_json(recorded("add-thread-file.json")),
            1,
        )
        .await;
        let response = f
            .create(
                Uuid::new_v4(),
                LineSide::Head,
                Some(60),
                62,
                CLIENT,
                "Look here.\n",
            )
            .await
            .expect("file comment");
        assert_eq!(response.comment.kind, CommentKind::File);
        assert_eq!(
            (response.comment.start_line, response.comment.line),
            (Some(60), Some(62))
        );
        assert!(!response.missed_by_llm_recorded, "no changed line");
        let sent = &f.graphql_bodies().await;
        assert!(
            sent.iter()
                .all(|body| body["variables"]["in"].get("line").is_none()),
            "{sent:?}"
        );
    }

    #[tokio::test]
    async fn removed_line_comment_is_a_left_line_thread() {
        let f = fixture().await;
        f.head_is(HEAD, 1).await;
        f.add_thread_answers(
            json!({"path": CLIENT, "line": 53, "side": "LEFT"}),
            ResponseTemplate::new(200).set_body_json(recorded("add-thread-left-removed-line.json")),
            1,
        )
        .await;
        f.create(Uuid::new_v4(), LineSide::Base, None, 53, CLIENT, "gone?")
            .await
            .expect("left thread");
    }

    #[tokio::test]
    async fn file_outside_the_diff_is_rejected_without_calling_github() {
        let f = fixture().await;
        let result = f
            .create(
                Uuid::new_v4(),
                LineSide::Head,
                None,
                3,
                "src/rpc/transport.rs",
                "b",
            )
            .await;
        assert!(
            matches!(result, Err(CommentError::PositionRejected)),
            "{result:?}"
        );
        assert!(f.graphql_bodies().await.is_empty());
    }

    #[tokio::test]
    async fn thread_null_is_position_rejected_and_the_key_can_retry() {
        let f = fixture().await;
        f.head_is(HEAD, 2).await;
        f.add_thread_answers(
            json!({"line": 96}),
            ResponseTemplate::new(200)
                .set_body_json(recorded("add-thread-null-line-outside-diff.json")),
            2,
        )
        .await;
        let key = Uuid::new_v4();
        for _ in 0..2 {
            let result = f.create(key, LineSide::Head, None, 96, CLIENT, "b").await;
            assert!(
                matches!(result, Err(CommentError::PositionRejected)),
                "{result:?}"
            );
        }
    }

    #[tokio::test]
    async fn moved_head_is_rejected_before_writing() {
        let f = fixture().await;
        f.head_is(&"0".repeat(40), 1).await;
        f.add_thread_answers(json!({}), line_ok(), 0).await;
        let result = f
            .create(Uuid::new_v4(), LineSide::Head, None, 96, CLIENT, "b")
            .await;
        assert!(
            matches!(result, Err(CommentError::PrHeadChanged)),
            "{result:?}"
        );
        let mut moved = pending_one();
        moved["data"]["repository"]["pullRequest"]["headRefOid"] = json!("0".repeat(40));
        f.pending_is(moved, 2).await;
        f.mock(json!({"query": UPDATE_COMMENT}), line_ok(), 0).await;
        f.mock(json!({"query": DELETE_COMMENT}), line_ok(), 0).await;
        let edit = update(&f.state, "u1", &f.summary, "PRRC_fixture482_c1", "x").await;
        assert!(matches!(edit, Err(CommentError::PrHeadChanged)), "{edit:?}");
        let removal = delete(&f.state, "u1", &f.summary, "PRRC_fixture482_c1").await;
        assert!(
            matches!(removal, Err(CommentError::PrHeadChanged)),
            "{removal:?}"
        );
    }

    #[tokio::test]
    async fn forbidden_before_the_mutation_ran_is_app_not_installed_and_frees_the_key() {
        let f = fixture().await;
        f.head_is(HEAD, 2).await;
        f.add_thread_answers(
            json!({}),
            ResponseTemplate::new(200).set_body_json(json!({"data": {"addPullRequestReviewThread": null},
                "errors": [{"type": "FORBIDDEN", "message": "Resource not accessible by integration"}]})),
            2,
        )
        .await;
        let key = Uuid::new_v4();
        for _ in 0..2 {
            let result = f.create(key, LineSide::Head, None, 96, CLIENT, "b").await;
            assert!(
                matches!(result, Err(CommentError::AppNotInstalled)),
                "{result:?}"
            );
        }
    }

    #[tokio::test]
    async fn forbidden_next_to_resolved_data_keeps_the_key_busy() {
        let f = fixture().await;
        f.head_is(HEAD, 1).await;
        let mut answer = recorded("add-thread-line.json");
        answer["errors"] =
            json!([{"type": "FORBIDDEN", "message": "field not accessible", "path": ["x"]}]);
        f.add_thread_answers(
            json!({}),
            ResponseTemplate::new(200).set_body_json(answer),
            1,
        )
        .await;
        let key = Uuid::new_v4();
        let result = f.create(key, LineSide::Head, None, 96, CLIENT, "b").await;
        assert!(
            matches!(result, Err(CommentError::OutcomeUnknown(_))),
            "{result:?}"
        );
        let retry = f.create(key, LineSide::Head, None, 96, CLIENT, "b").await;
        assert!(matches!(retry, Err(CommentError::KeyConflict)), "{retry:?}");
    }

    #[tokio::test]
    async fn unconfirmed_write_keeps_the_key_busy() {
        let f = fixture().await;
        f.head_is(HEAD, 1).await;
        f.add_thread_answers(json!({}), ResponseTemplate::new(502), 1)
            .await;
        let key = Uuid::new_v4();
        let result = f.create(key, LineSide::Head, None, 96, CLIENT, "b").await;
        assert!(
            matches!(result, Err(CommentError::OutcomeUnknown(_))),
            "{result:?}"
        );
        let retry = f.create(key, LineSide::Head, None, 96, CLIENT, "b").await;
        assert!(matches!(retry, Err(CommentError::KeyConflict)), "{retry:?}");
    }

    /// The recorded line-thread answer with the comment placed on `commit`.
    fn line_on(commit: &str) -> ResponseTemplate {
        let mut response = recorded("add-thread-line.json");
        response["data"]["addPullRequestReviewThread"]["thread"]["comments"]["nodes"][0]["commit"] =
            json!({"oid": commit});
        ResponseTemplate::new(200).set_body_json(response)
    }

    #[tokio::test]
    async fn head_moved_during_the_write_deletes_the_comment_again() {
        let f = fixture().await;
        // The preflight sees the analysed head; the author pushes before the mutation runs.
        f.head_is(HEAD, 2).await;
        f.add_thread_answers(json!({"line": 96}), line_on(&"1".repeat(40)), 2)
            .await;
        f.mock(
            json!({"query": DELETE_COMMENT, "variables": {"in": {"id": "PRRC_fixture482_c1"}}}),
            ResponseTemplate::new(200).set_body_json(recorded("delete-comment.json")),
            2,
        )
        .await;
        let key = Uuid::new_v4();
        for _ in 0..2 {
            let result = f.create(key, LineSide::Head, None, 96, CLIENT, "b").await;
            assert!(
                matches!(result, Err(CommentError::PrHeadChanged)),
                "removed again, so the key is free for a retry: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn misplaced_comment_that_cannot_be_deleted_is_reported_and_keeps_the_key() {
        let f = fixture().await;
        f.head_is(HEAD, 1).await;
        f.add_thread_answers(json!({"line": 96}), line_on(&"1".repeat(40)), 1)
            .await;
        f.mock(
            json!({"query": DELETE_COMMENT}),
            ResponseTemplate::new(502),
            1,
        )
        .await;
        let key = Uuid::new_v4();
        let result = f.create(key, LineSide::Head, None, 96, CLIENT, "b").await;
        assert!(
            matches!(result, Err(CommentError::MisplacedCommentLeft(_))),
            "{result:?}"
        );
        let retry = f.create(key, LineSide::Head, None, 96, CLIENT, "b").await;
        assert!(matches!(retry, Err(CommentError::KeyConflict)), "{retry:?}");
    }

    #[tokio::test]
    async fn misplaced_comment_in_a_review_published_meanwhile_is_reported_as_removed() {
        let f = fixture().await;
        f.head_is(HEAD, 2).await;
        f.add_thread_answers(json!({"line": 96}), line_on(&"1".repeat(40)), 2)
            .await;
        f.mock(
            json!({"query": DELETE_COMMENT, "variables": {"in": {"id": "PRRC_fixture482_c1"}}}),
            ResponseTemplate::new(200).set_body_json(json!({"data": {"deletePullRequestReviewComment":
                {"pullRequestReview": {"state": "COMMENTED"}, "pullRequestReviewComment": {"id": "PRRC_fixture482_c1"}}}})),
            2,
        )
        .await;
        let key = Uuid::new_v4();
        for _ in 0..2 {
            let result = f.create(key, LineSide::Head, None, 96, CLIENT, "b").await;
            assert!(
                matches!(result, Err(CommentError::MisplacedPublishedRemoved)),
                "nothing of the attempt remains, so the key is free: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn execution_error_next_to_data_keeps_the_key_busy() {
        let f = fixture().await;
        f.head_is(HEAD, 1).await;
        let mut answer = recorded("add-thread-line.json");
        answer["errors"] =
            json!([{"message": "Something went wrong resolving a field", "path": ["x"]}]);
        f.add_thread_answers(
            json!({}),
            ResponseTemplate::new(200).set_body_json(answer),
            1,
        )
        .await;
        let key = Uuid::new_v4();
        let result = f.create(key, LineSide::Head, None, 96, CLIENT, "b").await;
        assert!(
            matches!(result, Err(CommentError::OutcomeUnknown(_))),
            "{result:?}"
        );
        let retry = f.create(key, LineSide::Head, None, 96, CLIENT, "b").await;
        assert!(matches!(retry, Err(CommentError::KeyConflict)), "{retry:?}");
    }

    #[tokio::test]
    async fn refusal_before_execution_frees_the_key() {
        let f = fixture().await;
        f.head_is(HEAD, 2).await;
        f.add_thread_answers(
            json!({}),
            ResponseTemplate::new(200).set_body_json(json!({"errors": [{"message":
                "Argument 'input' on Field 'addPullRequestReviewThread' has an invalid value"}]})),
            2,
        )
        .await;
        let key = Uuid::new_v4();
        for _ in 0..2 {
            let result = f.create(key, LineSide::Head, None, 96, CLIENT, "b").await;
            assert!(matches!(result, Err(CommentError::Github(_))), "{result:?}");
        }
    }

    #[tokio::test]
    async fn edit_of_a_comment_published_meanwhile_is_reported() {
        let f = fixture().await;
        f.pending_is(pending_one(), 1).await;
        f.mock(
            json!({"query": UPDATE_COMMENT}),
            ResponseTemplate::new(200).set_body_json(
                json!({"data": {"updatePullRequestReviewComment":
                {"pullRequestReviewComment": {"id": "PRRC_fixture482_c1", "body": "edited",
                    "state": "SUBMITTED", "pullRequestReview": {"state": "COMMENTED"}}}}}),
            ),
            1,
        )
        .await;
        let edit = update(&f.state, "u1", &f.summary, "PRRC_fixture482_c1", "edited").await;
        assert!(
            matches!(
                edit,
                Err(CommentError::PublishedMeanwhile { deleted: false })
            ),
            "{edit:?}"
        );
    }

    /// A GitHub 401 on the write itself (after reads that succeeded) marks the user token expired; nothing is
    /// retried.
    #[tokio::test]
    async fn github_401_on_a_write_marks_the_user_token_expired() {
        let f = fixture().await;
        let token_expired = || async {
            let stored = f
                .state
                .db
                .user_tokens()
                .get("u1")
                .await
                .expect("lookup")
                .expect("pair");
            let expired = stored.tokens.access_expires_at <= Utc::now();
            let lock = crate::auth::lock_user("u1").await;
            let grant = crate::github::oauth::TokenGrant {
                access_token: SecretString::from(crate::review::tests::USER_TOKEN),
                expires_in: 8 * 3600,
                refresh_token: SecretString::from("ghr_review_refresh_u1"),
                refresh_token_expires_in: 180 * 86_400,
            };
            crate::auth::store_sign_in_tokens(&f.state, &lock, DEFAULT_TENANT, &grant, Utc::now())
                .await
                .expect("fresh pair again");
            expired
        };
        f.head_is(HEAD, 1).await;
        f.add_thread_answers(json!({}), ResponseTemplate::new(401), 1)
            .await;
        let created = f
            .create(Uuid::new_v4(), LineSide::Head, None, 96, CLIENT, "b")
            .await;
        assert!(
            matches!(created, Err(CommentError::Github(_))),
            "{created:?}"
        );
        assert!(token_expired().await, "add thread");

        f.pending_is(pending_one(), 2).await;
        for query in [UPDATE_COMMENT, DELETE_COMMENT] {
            f.mock(json!({"query": query}), ResponseTemplate::new(401), 1)
                .await;
        }
        let edit = update(&f.state, "u1", &f.summary, "PRRC_fixture482_c1", "edited").await;
        assert!(matches!(edit, Err(CommentError::Github(_))), "{edit:?}");
        assert!(token_expired().await, "update comment");
        let removal = delete(&f.state, "u1", &f.summary, "PRRC_fixture482_c1").await;
        assert!(
            matches!(removal, Err(CommentError::Github(_))),
            "{removal:?}"
        );
        assert!(token_expired().await, "delete comment");
    }

    #[tokio::test]
    async fn delete_of_a_comment_published_meanwhile_is_reported() {
        let f = fixture().await;
        f.pending_is(pending_one(), 1).await;
        f.mock(
            json!({"query": DELETE_COMMENT}),
            ResponseTemplate::new(200).set_body_json(json!({"data": {"deletePullRequestReviewComment":
                {"pullRequestReview": {"state": "COMMENTED"}, "pullRequestReviewComment": {"id": "PRRC_fixture482_c1"}}}})),
            1,
        )
        .await;
        let removal = delete(&f.state, "u1", &f.summary, "PRRC_fixture482_c1").await;
        assert!(
            matches!(
                removal,
                Err(CommentError::PublishedMeanwhile { deleted: true })
            ),
            "{removal:?}"
        );
    }

    #[tokio::test]
    async fn comment_beyond_the_listing_cap_is_found_by_id() {
        let f = fixture().await;
        let comments = |from: u32| -> Vec<Value> {
            (from..from + 100)
                .map(|n| {
                    json!({"id": format!("C{n}"), "path": CLIENT, "line": 96, "startLine": null,
                    "subjectType": "LINE", "body": "x", "replyTo": null})
                })
                .collect()
        };
        let more = json!({"hasNextPage": true, "endCursor": "next"});
        let none = json!({"hasNextPage": false, "endCursor": null});
        f.pending_is(
            json!({"data": {"repository": {"pullRequest": {"id": "PR_fixture482", "headRefOid": HEAD,
                "reviews": {"pageInfo": none, "nodes": [{"id": "R1", "viewerDidAuthor": true,
                    "comments": {"pageInfo": more, "nodes": comments(0)}}]},
                "reviewThreads": {"pageInfo": none, "nodes": []}}}}}),
            1,
        )
        .await;
        f.mock(
            json!({"query": REVIEW_COMMENTS_PAGE}),
            ResponseTemplate::new(200).set_body_json(
                json!({"data": {"node": {"comments": {"pageInfo": more, "nodes": comments(100)}}}}),
            ),
            u64::try_from(MAX_COMMENT_PAGES - 1).expect("pages"),
        )
        .await;
        f.mock(
            json!({"query": COMMENT_BY_ID, "variables": {"id": "C5000"}}),
            ResponseTemplate::new(200).set_body_json(json!({"data": {"node": {
                "__typename": "PullRequestReviewComment", "id": "C5000", "path": CLIENT, "line": 96,
                "startLine": null, "subjectType": "LINE", "body": "late", "state": "PENDING", "replyTo": null,
                "pullRequestReview": {"state": "PENDING", "viewerDidAuthor": true},
                "pullRequest": {"number": 482, "repository": {"name": "node-client", "owner": {"login": "acme"}}}}}})),
            1,
        )
        .await;
        f.mock(
            json!({"query": DELETE_COMMENT, "variables": {"in": {"id": "C5000"}}}),
            ResponseTemplate::new(200).set_body_json(recorded("delete-comment.json")),
            1,
        )
        .await;
        delete(&f.state, "u1", &f.summary, "C5000")
            .await
            .expect("a truncated list is no proof that the comment is missing");
    }

    #[tokio::test]
    async fn edit_and_delete_reach_github() {
        let f = fixture().await;
        f.pending_is(pending_one(), 2).await;
        f.mock(
            json!({"query": UPDATE_COMMENT, "variables": {"in": {"pullRequestReviewCommentId": "PRRC_fixture482_c1",
                "body": "edited"}}}),
            ResponseTemplate::new(200).set_body_json(json!({"data": {"updatePullRequestReviewComment":
                {"pullRequestReviewComment": {"id": "PRRC_fixture482_c1", "body": "edited",
                    "state": "PENDING", "pullRequestReview": {"state": "PENDING"}}}}})),
            1,
        )
        .await;
        f.mock(
            json!({"query": DELETE_COMMENT, "variables": {"in": {"id": "PRRC_fixture482_c1"}}}),
            ResponseTemplate::new(200).set_body_json(recorded("delete-comment.json")),
            1,
        )
        .await;
        let edited = update(&f.state, "u1", &f.summary, "PRRC_fixture482_c1", "edited")
            .await
            .expect("edit");
        assert_eq!(edited.body, "edited");
        assert_eq!(edited.thread_id, "PRRT_fixture482_c1");
        assert_eq!((edited.side, edited.line), (Some(LineSide::Head), Some(96)));
        delete(&f.state, "u1", &f.summary, "PRRC_fixture482_c1")
            .await
            .expect("delete");
    }

    #[tokio::test]
    async fn a_comment_outside_the_users_pending_review_is_not_touched() {
        let f = fixture().await;
        f.pending_is(pending_one(), 2).await;
        f.mock(json!({"query": UPDATE_COMMENT}), line_ok(), 0).await;
        f.mock(json!({"query": DELETE_COMMENT}), line_ok(), 0).await;
        let edit = update(&f.state, "u1", &f.summary, "PRRC_someone_else", "x").await;
        assert!(matches!(edit, Err(CommentError::NotFound)), "{edit:?}");
        let removal = delete(&f.state, "u1", &f.summary, "PRRC_someone_else").await;
        assert!(
            matches!(removal, Err(CommentError::NotFound)),
            "{removal:?}"
        );
    }

    #[tokio::test]
    async fn comment_deleted_on_github_meanwhile_is_not_found() {
        let f = fixture().await;
        f.pending_is(pending_one(), 1).await;
        f.mock(
            json!({"query": DELETE_COMMENT}),
            ResponseTemplate::new(200).set_body_json(recorded("delete-comment-not-found.json")),
            1,
        )
        .await;
        let removal = delete(&f.state, "u1", &f.summary, "PRRC_fixture482_c1").await;
        assert!(
            matches!(removal, Err(CommentError::NotFound)),
            "{removal:?}"
        );
    }

    #[tokio::test]
    async fn annotated_or_unchanged_lines_record_no_missed_by_llm() {
        let f = fixture().await;
        f.head_is(HEAD, 1).await;
        f.add_thread_answers(json!({"line": 97}), line_ok(), 1)
            .await;
        let response = f
            .create(
                Uuid::new_v4(),
                LineSide::Head,
                None,
                97,
                CLIENT,
                "annotated line",
            )
            .await
            .expect("comment");
        assert!(!response.missed_by_llm_recorded);
        let feedback = f.state.db.feedback().list(f.summary.id, "u1").await;
        assert!(
            !matches!(feedback, Ok(entries) if !entries.is_empty()),
            "no feedback for an annotated line"
        );
    }

    /// Hard case 2: a comment on the unannotated changed line 96 records `missed_by_llm`.
    #[tokio::test]
    async fn comment_on_unannotated_changed_line_records_missed_by_llm() {
        let f = fixture().await;
        f.head_is(HEAD, 1).await;
        f.add_thread_answers(json!({"line": 96}), line_ok(), 1)
            .await;
        let response = f
            .create(Uuid::new_v4(), LineSide::Head, None, 96, CLIENT, "missed")
            .await
            .expect("comment");
        assert!(response.missed_by_llm_recorded);
        let feedback = f
            .state
            .db
            .feedback()
            .list(f.summary.id, "u1")
            .await
            .expect("feedback");
        assert_eq!(feedback.len(), 1);
        assert_eq!(feedback[0].value, FeedbackValue::MissedByLlm);
        assert_eq!(
            feedback[0].memory,
            contracts::api::feedback::FeedbackMemory::Pending
        );
        assert_eq!(
            feedback[0].target,
            FeedbackTarget::Line {
                path: CLIENT.to_owned(),
                side: LineSide::Head,
                line: 96
            }
        );
    }
}
