//! Pull-request ingest: REST metadata read with the user's token, the stored repository and pull-request snapshot,
//! and the creation of an analysis. The analysis is stored at once with the pull request's base tip; the driver
//! prepares it (fetch, merge base) before any job.

use chrono::Utc;
use contracts::api::analyses::{AnalysisSummary, CreateAnalysisRequest};
use contracts::api::pr::ResolvePrResponse;
use contracts::common::PrRef;
use contracts::ids::AnalysisId;
use db::error::DbError;
use db::repo::analyses::NewAnalysis;

use crate::auth::{Session, UserTokenError, user_access_token, user_token_rejected};
use crate::github::rest::{self, GithubError, GithubPull};
use crate::pipeline;
use crate::review::comments::commenting_available;
use crate::state::AppState;

/// A pull request could not be resolved or analysed.
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
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
    /// The user's GitHub token is not available.
    #[error("the user's GitHub token is not available")]
    Token(#[from] UserTokenError),
    /// A GitHub call failed.
    #[error("a GitHub call failed")]
    Github(#[from] GithubError),
    /// The database failed.
    #[error("the database failed")]
    Db(#[from] DbError),
}

/// Fetches the pull request with the user's token and stores its repository and snapshot; returns it with its
/// row id. Private repositories and names outside GitHub's charset are refused.
async fn fetch_pull(
    state: &AppState,
    session: &Session,
    pr: &PrRef,
) -> Result<(GithubPull, i64), IngestError> {
    if !rest::is_valid_repo(&pr.repo) || pr.number == 0 {
        return Err(IngestError::InvalidRepository);
    }
    let token = user_access_token(state, &session.user_id).await?;
    let pull = match rest::get_pull(&state.config.github, &token, pr).await {
        Ok(pull) => pull,
        Err(GithubError::Unauthorized) => {
            user_token_rejected(state, &session.user_id, &token).await;
            return Err(GithubError::Unauthorized.into());
        }
        Err(error) => return Err(error.into()),
    };
    if pull.private {
        return Err(IngestError::PrivateRepository);
    }
    let repo = state
        .db
        .repos()
        .upsert(
            &session.tenant_id,
            &pull.summary.repo,
            Some(pull.repo_github_id),
        )
        .await?;
    let pr_id = state
        .db
        .pull_requests()
        .upsert(&session.tenant_id, repo.id, &pull.summary, Utc::now())
        .await?;
    Ok((pull, pr_id))
}

/// The pull request with the tenant's latest analysis of it in the user's language.
pub async fn resolve(
    state: &AppState,
    session: &Session,
    pr: &PrRef,
) -> Result<ResolvePrResponse, IngestError> {
    let (pull, pr_id) = fetch_pull(state, session, pr).await?;
    let user = state.db.users().get(&session.user_id).await?;
    let latest = state
        .db
        .analyses()
        .latest_for_pr(pr_id, user.language)
        .await?
        .filter(|record| record.tenant_id == session.tenant_id);
    let (analysis, head_moved) = match latest {
        Some(record) => (
            Some(
                state
                    .db
                    .analyses()
                    .summary_for_user(record.id, &session.user_id)
                    .await?,
            ),
            record.head_sha != pull.summary.head_sha,
        ),
        None => (None, false),
    };
    let commenting_available = commenting_available(state, &pull.summary.repo)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(
                ?error,
                "GitHub App installation not readable; commenting shown as unavailable"
            );
            false
        });
    Ok(ResolvePrResponse {
        pr: pull.summary,
        commenting_available,
        analysis,
        head_moved,
    })
}

/// Stores a `planning` analysis of the requested head and starts its driver; returns at once.
pub async fn create(
    state: &AppState,
    session: &Session,
    request: &CreateAnalysisRequest,
) -> Result<AnalysisSummary, IngestError> {
    let (pull, pr_id) = fetch_pull(state, session, &request.pr).await?;
    if pull.summary.head_sha != request.head_sha {
        return Err(IngestError::HeadMoved);
    }
    let user = state.db.users().get(&session.user_id).await?;
    let superseded = state
        .db
        .analyses()
        .latest_for_pr(pr_id, user.language)
        .await?
        .filter(|old| old.head_sha != pull.summary.head_sha);
    let created = state
        .db
        .analyses()
        .create(
            NewAnalysis {
                id: AnalysisId::new_random(),
                tenant_id: session.tenant_id.clone(),
                pull_request_id: pr_id,
                // The driver replaces the base tip with the merge base before any job runs.
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
        Err(DbError::Conflict) => return Err(IngestError::AnalysisExists),
        Err(error) => return Err(error.into()),
    };
    // A re-analysis on a moved head starts a new agent session; the old analysis's session files are deleted.
    if let Some(old) = superseded {
        state.db.analyses().request_session_drop(old.id).await?;
        tracing::debug!(analysis = %record.id, superseded = %old.id, "agent session of the superseded analysis dropped");
        // Its open Ask jobs are cancelled; their questions fail with a plain sentence.
        if let Err(error) = state.executor.cancel_analysis(old.id).await {
            tracing::error!(superseded = %old.id, ?error, "could not cancel the jobs of the superseded analysis");
        }
    }
    tracing::info!(
        analysis = %record.id,
        user_id = %session.user_id,
        repo = %format!("{}/{}", pull.summary.repo.owner, pull.summary.repo.name),
        number = pull.summary.number,
        "analysis requested"
    );
    pipeline::start(state, record.id);
    Ok(state
        .db
        .analyses()
        .summary_for_user(record.id, &session.user_id)
        .await?)
}
