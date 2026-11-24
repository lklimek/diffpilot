//! Assessment settings and read-only payer-scoped comparison; no operation here starts work.

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::{Session, SessionCsrf};
use crate::{review_loop, state::AppState};
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use contracts::api::assessment::{
    AssessmentComparison, AssessmentSettings, AssessmentSettingsUpdate,
};
use contracts::api::error::ApiErrorCode;
use contracts::assessment::{AssessmentRunStatus, ComparisonItemStatus};
use contracts::common::{PrRef, RepoRef};
use contracts::ids::UserId;
use contracts::routes::*;
use contracts::time::Timestamp;
use db::error::DbError;
use std::borrow::Cow;

/// Safe handler failures; no model text becomes an HTTP error.
#[derive(Debug, thiserror::Error)]
pub enum AssessmentApiError {
    /// Invalid JSON.
    #[error(transparent)]
    Json(#[from] JsonRejection),
    /// Invalid path.
    #[error(transparent)]
    Path(#[from] PathRejection),
    /// Invalid cutoff.
    #[error(transparent)]
    Query(#[from] QueryRejection),
    /// Invalid selection.
    #[error("select at most 100 distinct owner/repo#number pull requests")]
    InvalidConfig,
    /// Storage failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// PR access failed.
    #[error(transparent)]
    Loop(#[from] review_loop::LoopError),
}

impl ApiErrorSource for AssessmentApiError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Json(e) => e.code(),
            Self::Path(e) => e.code(),
            Self::Query(e) => e.code(),
            Self::Db(e) => e.code(),
            Self::Loop(review_loop::LoopError::NotFound) => ApiErrorCode::NotFound,
            Self::Loop(_) => ApiErrorCode::Internal,
            Self::InvalidConfig => ApiErrorCode::InvalidRequest,
        }
    }
    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Json(e) => e.public_message(),
            Self::Path(e) => e.public_message(),
            Self::Query(e) => e.public_message(),
            Self::Db(e) => e.public_message(),
            Self::Loop(_) => Cow::Borrowed("The pull request was not found."),
            Self::InvalidConfig => {
                Cow::Borrowed("Select at most 100 distinct pull requests as owner/repo#number.")
            }
        }
    }
}

type Result<T> = std::result::Result<T, ApiFailure<AssessmentApiError>>;
type PrPath = std::result::Result<Path<(String, String, u32)>, PathRejection>;
fn fail(e: impl Into<AssessmentApiError>) -> ApiFailure<AssessmentApiError> {
    ApiFailure(e.into())
}

/// W1 settings and report contracts, authenticated by the existing route gates.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            GET_ASSESSMENT_SETTINGS.path,
            get(settings).put(set_settings),
        )
        .route(GET_ASSESSMENT_COMPARISON.path, get(comparison))
        .route(GET_COMMENT_SOURCES.path, get(sources))
}

async fn read_settings(state: &AppState, session: &Session) -> Result<AssessmentSettings> {
    let user = UserId(session.user_id.clone());
    let (config, revision) = state
        .db
        .assessments()
        .config_snapshot(&user)
        .await
        .map_err(fail)?;
    Ok(AssessmentSettings {
        config,
        revision,
        quota: state
            .db
            .assessments()
            .quota(&user, Timestamp::now())
            .await
            .map_err(fail)?,
    })
}

async fn settings(
    State(state): State<AppState>,
    session: Session,
) -> Result<Json<AssessmentSettings>> {
    Ok(Json(read_settings(&state, &session).await?))
}

async fn set_settings(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    body: std::result::Result<Json<AssessmentSettingsUpdate>, JsonRejection>,
) -> Result<Json<AssessmentSettings>> {
    let Json(update) = body.map_err(fail)?;
    let config = update.config;
    let mut seen = std::collections::HashSet::new();
    if config.ab_prs.len() > 100
        || config.ab_prs.iter().any(|pr| {
            let Some((repo, number)) = pr.rsplit_once('#') else {
                return true;
            };
            let Some((owner, name)) = repo.split_once('/') else {
                return true;
            };
            !crate::github::rest::is_valid_repo(&RepoRef {
                owner: owner.into(),
                name: name.into(),
            }) || number.parse::<u32>().ok().filter(|n| *n > 0).is_none()
                || !seen.insert(pr.to_lowercase())
        })
    {
        return Err(fail(AssessmentApiError::InvalidConfig));
    }
    state
        .db
        .assessments()
        .set_config_if_current(
            &session.tenant_id,
            &UserId(session.user_id.clone()),
            &config,
            &update.revision,
        )
        .await
        .map_err(fail)?;
    Ok(Json(read_settings(&state, &session).await?))
}

#[derive(Default, serde::Deserialize)]
struct Cutoff {
    cutoff: Option<Timestamp>,
}

async fn handle(
    state: &AppState,
    session: &Session,
    path: PrPath,
) -> Result<review_loop::PrHandle> {
    let Path((owner, name, number)) = path.map_err(fail)?;
    review_loop::find(
        state,
        &session.tenant_id,
        &PrRef {
            repo: RepoRef { owner, name },
            number,
        },
    )
    .await
    .map_err(fail)
}

async fn comparison(
    State(state): State<AppState>,
    session: Session,
    path: PrPath,
    query: std::result::Result<Query<Cutoff>, QueryRejection>,
) -> Result<Json<AssessmentComparison>> {
    let handle = handle(&state, &session, path).await?;
    let Query(query) = query.map_err(fail)?;
    let user = UserId(session.user_id.clone());
    let pass = state
        .db
        .assessments()
        .latest(&session.tenant_id, &user, handle.id)
        .await
        .map_err(fail)?
        .ok_or_else(|| fail(DbError::NotFound))?;
    let mut report = state
        .db
        .assessments()
        .comparison(
            &session.tenant_id,
            &user,
            pass.id,
            query.cutoff.unwrap_or_else(Timestamp::now),
        )
        .await
        .map_err(fail)?;
    complete_rows(&mut report);
    report.summarize();
    Ok(Json(report))
}

fn complete_rows(report: &mut AssessmentComparison) {
    let original = report.items.clone();
    for run in &report.runs {
        for row in &original {
            if row.status != ComparisonItemStatus::Matched || row.primary_item_id.is_none() {
                continue;
            }
            if report
                .items
                .iter()
                .any(|i| i.run == run.reference && i.comparison_key == row.comparison_key)
            {
                continue;
            }
            let mut missing = row.clone();
            missing.run = run.reference.clone();
            missing.method_version = run.version.clone();
            missing.local_id = String::new();
            missing.verdict = None;
            missing.native_verdict = None;
            missing.proposal = None;
            missing.owner_decision = None;
            missing.duplicate_of = None;
            missing.status = if matches!(
                run.status,
                AssessmentRunStatus::Failed | AssessmentRunStatus::Cancelled
            ) {
                ComparisonItemStatus::MethodFailed
            } else {
                ComparisonItemStatus::MissingRow
            };
            report.items.push(missing);
        }
    }
}

async fn sources(
    State(state): State<AppState>,
    session: Session,
    path: PrPath,
) -> Result<Json<Vec<contracts::api::review_loop::CommentSource>>> {
    let handle = handle(&state, &session, path).await?;
    Ok(Json(
        state
            .db
            .comment_sources()
            .list(&session.tenant_id, handle.id)
            .await
            .map_err(fail)?
            .into_iter()
            .map(|s| s.source)
            .collect(),
    ))
}
