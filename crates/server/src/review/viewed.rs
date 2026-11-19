//! The user's GitHub "Viewed" mark per file (Files view): read for the file list, written on toggle as the user.
//! A write is refused while the pull request's head is not the analysed head (the mark would apply to newer code).
//! The head can still move between that check and the write; GitHub then marks the file at the newer head, and it
//! clears marks of files later commits change. GitHub calls live in `github::viewed`.

use std::collections::BTreeMap;
use std::convert::Infallible;

use contracts::api::analyses::AnalysisSummary;
use contracts::api::files::{FileViewedState, ViewedStatus};
use contracts::common::PrRef;
use contracts::ids::CommitSha;
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

/// A refusal is an answer that says GitHub did not do it: refused before execution, or a FORBIDDEN, NOT_FOUND or
/// UNPROCESSABLE error. Anything else leaves the outcome open.
fn classify(error: GraphqlError) -> ViewedError {
    let refused = matches!(error, GraphqlError::NotExecuted(_))
        || matches!(
            error.kind(),
            Some(
                GraphqlErrorType::Forbidden
                    | GraphqlErrorType::NotFound
                    | GraphqlErrorType::Unprocessable
            )
        );
    if refused {
        ViewedError::Refused(error)
    } else {
        ViewedError::Github(error)
    }
}

/// The user's Viewed marks of the analysed pull request; anything GitHub does not answer is "not read", never
/// an assumed state.
pub async fn read(state: &AppState, user_id: &str, summary: &AnalysisSummary) -> ViewedRead {
    let unread = ViewedRead {
        files: BTreeMap::new(),
        status: ViewedStatus::NotChecked,
        head: None,
    };
    let token = match user_access_token(state, user_id).await {
        Ok(token) => token,
        Err(error) => {
            tracing::debug!(analysis = %summary.id, ?error, "no user token; the Viewed marks are not read");
            return unread;
        }
    };
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
        Err(error) => {
            if let ViewedReadError::Graphql(GraphqlError::Unauthorized) = &error {
                user_token_rejected(state, user_id, &token).await;
            }
            tracing::debug!(analysis = %summary.id, ?error, "the Viewed marks could not be read from GitHub");
            unread
        }
    }
}

/// [`read`] as an infallible future, for `github_read`.
pub async fn read_cached(
    state: &AppState,
    user_id: &str,
    summary: &AnalysisSummary,
) -> Result<ViewedRead, Infallible> {
    Ok(read(state, user_id, summary).await)
}

/// Marks (`viewed`) or unmarks `path` on GitHub as the user, once the head is still the analysed one.
pub async fn set(
    state: &AppState,
    user_id: &str,
    summary: &AnalysisSummary,
    path: &str,
    viewed: bool,
) -> Result<(), ViewedError> {
    let (_, files) = pr_diff(state, summary).await?;
    if !files.iter().any(|file| file.path == path) {
        return Err(ViewedError::UnknownFile);
    }
    let token = user_access_token(state, user_id).await?;
    let result = write(state, &token, summary, path, viewed).await;
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
            tracing::info!(analysis = %summary.id, viewed, "Viewed mark changed on GitHub");
            Ok(())
        }
        Err(ViewedWriteError::Graphql(error)) => Err(classify(error)),
        Err(ViewedWriteError::NoPayload) => Err(ViewedError::NoPayload),
    }
}

#[cfg(test)]
mod tests;
