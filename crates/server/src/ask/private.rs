//! Standalone comment Ask: payer-owned Verify session, private recovery pack, no assessment admission.
use contracts::agent::{self, JobKind};
use contracts::api::pr::PrSummary;
use contracts::api::questions::AskAnchor;
use contracts::ids::{JobId, QuestionId, UserId};
use contracts::job::access::AgentAccess;
use contracts::job::spec::{
    JobLimits, JobPriority, JobSpec, JobTarget, PullRequestAskTarget, RepoCheckout,
};
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::Db;
use db::repo::jobs::JobStage;
use db::repo::questions::QuestionRecord;

use super::job::FollowError;
use crate::github::oauth::endpoint;
use crate::jobs::claim::ContextPackError;
use crate::jobs::executor::{JobHandle, JobRequest};
use crate::model::{ModelContext, select_model_for_payer};
use crate::prompts::{self, ThreadQuestion};
use crate::review_loop::assess::verify_item;
use crate::state::AppState;

/// Submits a standalone private Ask with the payer's question and thread history.
pub(super) async fn submit(
    state: &AppState,
    pr: &PrSummary,
    record: &QuestionRecord,
) -> Result<JobHandle, FollowError> {
    let db = &state.db;
    let AskAnchor::CommentItem { item } = record.question.anchor else {
        return Err(DbError::Conflict.into());
    };
    let item = db.review_loop().item(item).await?;
    let earlier = db
        .questions()
        .thread_before(record.question.id, Timestamp::now())
        .await?;
    let thread: Vec<_> = earlier
        .iter()
        .map(|r| ThreadQuestion {
            question: &r.question.text,
            state: r.question.state,
            answer: r.question.answer.as_ref().map(|a| a.answer.as_str()),
        })
        .collect();
    let payer = UserId(record.user_id.clone());
    let spec = JobSpec {
        job_id: JobId::new_random(),
        attempt: 0,
        kind: JobKind::Ask,
        target: JobTarget::PullRequestAsk(PullRequestAskTarget {
            pull_request_id: record.pull_request_id.ok_or(DbError::NotFound)?,
            repo: RepoCheckout {
                clone_url: endpoint(
                    &state.config.github.clone_url,
                    &format!("{}/{}.git", pr.repo.owner, pr.repo.name),
                )
                .to_string(),
                base_sha: pr.base_sha.clone(),
                head_sha: pr.head_sha.clone(),
            },
        }),
        prompt: prompts::ask_comment(
            pr,
            db.users().get(&record.user_id).await?.language,
            &verify_item(&item),
            &record.question.text,
            &thread,
        )?,
        output_schema: agent::output_schema(JobKind::Ask)?,
        flow_file: None,
        assessment: None,
        pr_data: None,
        model: select_model_for_payer(db, &payer, JobKind::Ask, &ModelContext::default()).await?,
        limits: JobLimits::default(),
        payer,
        session: None,
        working_session: None,
        context_pack: None,
        skills: prompts::required_skills(),
        access: AgentAccess::default(),
    };
    Ok(state
        .executor
        .submit(JobRequest {
            spec,
            stage: JobStage::Ask(record.question.id),
            priority: JobPriority::Interactive,
            generation: 0,
            pr_repo: Some(pr.repo.clone()),
        })
        .await?)
}

/// Builds recovery context from public PR data and only the question owner's history.
pub(crate) async fn context_pack(
    db: &Db,
    pr: &PrSummary,
    question: QuestionId,
) -> Result<String, ContextPackError> {
    let record = db.questions().get(question, Timestamp::now()).await?;
    let AskAnchor::CommentItem { item } = record.question.anchor else {
        return Err(DbError::Conflict.into());
    };
    let item = db.review_loop().item(item).await?;
    if record.pull_request_id != Some(item.pull_request_id) {
        return Err(DbError::Conflict.into());
    }
    let earlier = db.questions().history_before(question).await?;
    Ok(prompts::comment_context(pr, &verify_item(&item), &earlier)?)
}
