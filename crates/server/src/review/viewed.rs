//! The user's GitHub "Viewed" mark per file (Files view): read for the file list, written on toggle as the user.
//! A write is refused while the pull request's head is not the analysed head (the mark would apply to newer code).
//! The head can still move between that check and the write; GitHub then marks the file at the newer head, and it
//! clears marks of files later commits change. GitHub calls live in `github::viewed`.

use std::collections::{BTreeMap, HashSet};
use std::sync::{LazyLock, Mutex, PoisonError};

use contracts::api::analyses::AnalysisSummary;
use contracts::api::files::{FileViewedState, ViewedStatus};
use contracts::common::PrRef;
use contracts::ids::{AnalysisId, CommitSha};
use secrecy::SecretString;

use crate::auth::{UserTokenError, user_access_token, user_token_rejected};
use crate::github::graphql::{GraphqlError, GraphqlErrorType};
use crate::github::review::{self, PendingReviewError};
use crate::github::viewed::{self, ViewedReadError, ViewedWriteError};
use crate::pipeline::{PrDiffError, pr_diff};
use crate::state::AppState;

/// The Viewed state as the file list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewedRead {
    /// The user's state by path.
    pub files: BTreeMap<String, FileViewedState>,
    /// How completely it was read.
    pub status: ViewedStatus,
    /// The pull request's head when read.
    pub head: Option<CommitSha>,
}

/// Marking a file failed.
#[derive(Debug, thiserror::Error)]
pub enum ViewedError {
    /// The path is not a file of the analysed pull request.
    #[error("the path is not a file of the analysis")]
    UnknownFile,
    /// The pull request's head moved since the analysis.
    #[error("the pull request head moved since the analysis")]
    PrHeadChanged,
    /// GitHub refused the change or the read before it (e.g. the token may not do this); nothing was changed.
    #[error("GitHub refused to change the Viewed mark")]
    Refused(#[source] GraphqlError),
    /// GitHub did not answer as expected; the mark may or may not have changed.
    #[error("GitHub did not answer as expected")]
    Github(#[source] GraphqlError),
    /// GitHub answered the mutation without its result.
    #[error("GitHub answered without the mutation's result")]
    NoPayload,
    /// GitHub did not return the pull request.
    #[error("GitHub did not return the pull request")]
    MissingPullRequest,
    /// Another Viewed write of the user on this analysis is still running.
    #[error("another Viewed change is still being saved")]
    Busy,
    /// The user's GitHub token is not available.
    #[error("the user's GitHub token is not available")]
    Token(#[from] UserTokenError),
    /// The diff of the analysis is not available.
    #[error("the diff of the analysis is not available")]
    Diff(#[from] PrDiffError),
}

impl ViewedError {
    /// GitHub rejected the user token.
    pub fn unauthorized(&self) -> bool {
        matches!(
            self,
            Self::Github(GraphqlError::Unauthorized) | Self::Refused(GraphqlError::Unauthorized)
        )
    }
}

fn pr_of(summary: &AnalysisSummary) -> PrRef {
    PrRef {
        repo: summary.pr.repo.clone(),
        number: summary.pr.number,
    }
}

/// A refusal is an answer that proves the mark was not changed ([`GraphqlError::proves_not_applied`], the rule of
/// pending-review writes); anything else leaves the outcome open.
fn classify(error: GraphqlError) -> ViewedError {
    if error.proves_not_applied() {
        ViewedError::Refused(error)
    } else {
        ViewedError::Github(error)
    }
}

/// The user's Viewed marks of the analysed pull request; anything GitHub does not answer is "not read", never
/// an assumed state. An ended GitHub sign-in is an error: the user must sign in again.
pub async fn read(
    state: &AppState,
    user_id: &str,
    summary: &AnalysisSummary,
) -> Result<ViewedRead, UserTokenError> {
    let unread = ViewedRead {
        files: BTreeMap::new(),
        status: ViewedStatus::NotChecked,
        head: None,
    };
    let token = match user_access_token(state, user_id).await {
        Ok(token) => token,
        Err(UserTokenError::SignInRequired) => return Err(UserTokenError::SignInRequired),
        Err(error) => {
            tracing::debug!(analysis = %summary.id, ?error, "no user token; the Viewed marks are not read");
            return Ok(unread);
        }
    };
    Ok(
        match viewed::read(&state.config.github, &token, &pr_of(summary)).await {
            Ok(read) => ViewedRead {
                files: read.files,
                status: if read.incomplete {
                    ViewedStatus::Partial
                } else {
                    ViewedStatus::Checked
                },
                head: Some(CommitSha(read.head_oid)),
            },
            Err(ViewedReadError::Graphql(GraphqlError::Unauthorized)) => {
                user_token_rejected(state, user_id, &token).await;
                tracing::debug!(analysis = %summary.id, "GitHub rejected the user token; the Viewed marks are not read");
                unread
            }
            Err(error) if read_refused(&error) => {
                tracing::warn!(analysis = %summary.id, ?error,
                "GitHub refused to read the Viewed marks; the file list shows them as not read");
                unread
            }
            Err(error) => {
                tracing::debug!(analysis = %summary.id, ?error, "the Viewed marks could not be read from GitHub");
                unread
            }
        },
    )
}

/// GitHub answered the read with a refusal (permission or a missing pull request), not a passing failure.
fn read_refused(error: &ViewedReadError) -> bool {
    match error {
        ViewedReadError::MissingPullRequest => true,
        ViewedReadError::Graphql(error) => {
            matches!(
                error.kind(),
                Some(GraphqlErrorType::Forbidden | GraphqlErrorType::NotFound)
            ) || matches!(error, GraphqlError::Status(status) if status.is_client_error())
        }
    }
}

/// Viewed writes running, by (user, analysis).
static WRITING: LazyLock<Mutex<HashSet<(String, AnalysisId)>>> = LazyLock::new(Mutex::default);

/// A running Viewed write; dropping it lets the next one start.
struct Writing((String, AnalysisId));

impl Writing {
    fn start(user_id: &str, analysis: AnalysisId) -> Option<Self> {
        let key = (user_id.to_owned(), analysis);
        let started = WRITING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone());
        // Built only when started: dropping a guard removes its key.
        started.then(|| Self(key))
    }
}

impl Drop for Writing {
    fn drop(&mut self) {
        WRITING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

/// Marks (`viewed`) or unmarks `path` on GitHub as the user, once the head is still the analysed one.
pub async fn set(
    state: &AppState,
    user_id: &str,
    summary: &AnalysisSummary,
    path: &str,
    viewed: bool,
) -> Result<(), ViewedError> {
    let _writing = Writing::start(user_id, summary.id).ok_or(ViewedError::Busy)?;
    let (_, files) = pr_diff(state, summary).await?;
    if !files.iter().any(|file| file.path == path) {
        return Err(ViewedError::UnknownFile);
    }
    let token = user_access_token(state, user_id).await?;
    let result = write(state, &token, user_id, summary, path, viewed).await;
    if let Err(error) = &result
        && error.unauthorized()
    {
        user_token_rejected(state, user_id, &token).await;
    }
    if let Err(error) = &result {
        tracing::debug!(analysis = %summary.id, ?error, "the Viewed mark was not changed on GitHub");
    }
    result
}

async fn write(
    state: &AppState,
    token: &SecretString,
    user_id: &str,
    summary: &AnalysisSummary,
    path: &str,
    viewed: bool,
) -> Result<(), ViewedError> {
    let config = &state.config.github;
    let pull = match review::pull_request_head(config, token, &pr_of(summary)).await {
        Ok(pull) => pull,
        Err(PendingReviewError::Graphql(error)) => return Err(classify(error)),
        Err(_) => return Err(ViewedError::MissingPullRequest),
    };
    if pull.head_oid != summary.pr.head_sha.0 {
        return Err(ViewedError::PrHeadChanged);
    }
    match viewed::set(config, token, &pull.id, path, viewed).await {
        Ok(()) => {
            tracing::info!(analysis = %summary.id, user_id, path, viewed, "Viewed mark changed on GitHub");
            Ok(())
        }
        Err(ViewedWriteError::Graphql(error)) => Err(classify(error)),
        Err(ViewedWriteError::NoPayload) => Err(ViewedError::NoPayload),
    }
}

#[cfg(test)]
mod tests;
