//! Read-only projection of LLM queue activity for HTTP and user SSE.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use contracts::agent::JobKind;
use contracts::api::llm::{JobSubject, LlmActivity, LlmJob, LlmJobKind, WaitReason};
use contracts::routes::GET_LLM_ACTIVITY;
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::jobs::OwnJob;

use super::error::ApiFailure;
use super::settings::llm_status;
use crate::auth::Session;
use crate::github::rest::is_valid_repo;
use crate::review_loop::notify::comments_path;
use crate::state::AppState;

pub(super) fn routes() -> Router<AppState> {
    Router::new().route(GET_LLM_ACTIVITY.path, get(read_activity))
}

async fn read_activity(
    State(state): State<AppState>,
    session: Session,
) -> Result<Json<LlmActivity>, ApiFailure<DbError>> {
    Ok(Json(snapshot(&state, &session).await?))
}

pub(super) async fn snapshot(state: &AppState, reader: &Session) -> Result<LlmActivity, DbError> {
    let activity = state
        .db
        .jobs()
        .activity(&reader.tenant_id, &reader.user_id)
        .await?;
    let now = Timestamp::now();
    let paused_until = activity.paused_until.filter(|until| *until > now);
    let mut snapshot = LlmActivity {
        running: Vec::new(),
        queued: Vec::new(),
        paused_until,
        last_call: llm_status(activity.last_call, activity.unusable_since),
    };
    let mut other_index = 0;
    for job in activity.jobs {
        let bucket = if job.running {
            &mut snapshot.running
        } else {
            &mut snapshot.queued
        };
        let kind = match job.kind {
            JobKind::Plan | JobKind::PrContext => LlmJobKind::Analysis,
            JobKind::AnalyseFlow => LlmJobKind::FlowAnalysis,
            JobKind::JudgeWork => LlmJobKind::Ranking,
            JobKind::VerifyComments => LlmJobKind::CommentAssessment,
            JobKind::Ask => LlmJobKind::Ask,
        };
        match job.own {
            Some(own) => {
                let resumes_at = own
                    .resumes_at
                    .filter(|until| *until > now)
                    .max(paused_until);
                let waits_for = (!job.running).then_some(match resumes_at {
                    Some(until) => WaitReason::UsageLimit { until },
                    None if own.analysis_running => WaitReason::AnalysisRunning,
                    None => WaitReason::Runner,
                });
                bucket.push(LlmJob::Own {
                    id: own.id,
                    kind,
                    subject: subject(kind, &own),
                    since: own.claimed_at,
                    queued_at: own.queued_at,
                    waits_for,
                });
            }
            None => {
                bucket.push(LlmJob::Other {
                    id: other_index.to_string(),
                    kind,
                });
                other_index += 1;
            }
        }
    }
    Ok(snapshot)
}

fn subject(kind: LlmJobKind, job: &OwnJob) -> Option<JobSubject> {
    if kind == LlmJobKind::Ranking {
        return Some(JobSubject {
            label: "My work".to_owned(),
            href: "/".to_owned(),
        });
    }
    let pr = job
        .pr
        .as_ref()
        .filter(|pr| is_valid_repo(&pr.repo) && pr.number > 0)?;
    let label = format!("{}/{}#{}", pr.repo.owner, pr.repo.name, pr.number);
    let href = match (kind, job.analysis) {
        (LlmJobKind::Ask, Some(analysis)) => format!("/analyses/{analysis}"),
        (LlmJobKind::CommentAssessment | LlmJobKind::Ask, _) => comments_path(pr),
        _ => format!("/pr/{}/{}/{}", pr.repo.owner, pr.repo.name, pr.number),
    };
    Some(JobSubject { label, href })
}
