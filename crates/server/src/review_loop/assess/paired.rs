//! Admission and method isolation for one frozen logical assessment pass.

use super::{
    AssessError, VERIFY_PROMPT_VERSION,
    frozen::{self, Frozen},
    prompt,
};
use crate::jobs::executor::{JobHandle, JobUpdate};
use crate::review_loop::{handin, observe::Snapshot};
use crate::state::AppState;
use contracts::agent::pr_data::{ItemIds, Manifest, PrData, ReviewScope};
use contracts::agent::verify_comments::VerifyCommentsInput;
use contracts::agent::{self, JobKind};
use contracts::api::pr::PrSummary;
use contracts::api::review_loop::CommentItemKind;
use contracts::assessment::*;
use contracts::common::LlmLanguage;
use contracts::ids::{JobId, PassId, UserId};
use contracts::job::result::{JobFailureKind, JobOutcome};
use contracts::job::spec::*;
use contracts::time::Timestamp;
use db::repo::assessments::{AssessmentError, MethodJob, MethodResult, NewAssessmentPass};
use db::repo::review_loop::{ItemRecord, NewPass, PassRecord};
use futures::StreamExt;
use serde_json::json;

const HOST_VERSION: &str = "13.7.0/71096d7/4.2/brief2";

pub(super) async fn admit(
    state: &AppState,
    pr: &PrSummary,
    review: &NewPass,
    native: &VerifyCommentsInput,
    items: &[ItemRecord],
    snapshot: Option<&Snapshot>,
    language: LlmLanguage,
) -> Result<super::PassQueued, AssessError> {
    let user = UserId(review.user_id.clone().ok_or(db::error::DbError::NotFound)?);
    let tenant = state
        .db
        .pull_requests()
        .get(review.pull_request_id)
        .await?
        .tenant_id;
    let now = Timestamp::now();
    let config = state.db.assessments().config(&user).await?;
    let mut data = snapshot
        .and_then(|s| s.pr_data.clone())
        .unwrap_or_else(|| fallback(pr, items));
    data.manifest.head = pr.head_sha.0.clone();
    data.manifest.base_sha = pr.base_sha.0.clone();
    data.scope.threads.retain(|thread| {
        review.sent.iter().any(|sent| {
            items.iter().any(|item| {
                item.id == sent.item
                    && snapshot
                        .and_then(|s| s.thread_ids.get(&item.stable_key))
                        .unwrap_or(&item.stable_key)
                        == thread
            })
        })
    });
    data.scope.sources.retain(|url| {
        review.sent.iter().any(|sent| {
            items.iter().any(|item| {
                item.id == sent.item
                    && item.url.as_ref() == Some(url)
                    && matches!(
                        item.kind,
                        CommentItemKind::ReviewBody | CommentItemKind::PrComment
                    )
            })
        })
    });
    if data.scope.threads.is_empty() && data.scope.sources.is_empty() {
        return Ok(super::PassQueued::Skipped);
    }
    if let Ok(mirror) =
        crate::github::mirror::ensure_commits(&state.config, &pr.repo, &[&pr.base_sha]).await
    {
        for path in ["CLAUDE.md", "AGENTS.md", ".claude/CLAUDE.md"] {
            if let Ok(Some(bytes)) =
                crate::github::mirror::read_blob(&mirror, &pr.base_sha, path).await
                && bytes.len() <= 64 * 1024
                && let Ok(text) = String::from_utf8(bytes)
            {
                data.conventions.insert(path.into(), text);
            }
        }
    }
    let authorities =
        super::imports::Authorities::read(state, &tenant, &user.0, &mut data.manifest).await?;
    let mut frozen = Frozen {
        data,
        authorities,
        own_reply_urls: Default::default(),
        own_reply_texts: Default::default(),
        written: snapshot.map(|s| s.written.clone()).unwrap_or_default(),
        items: items.to_vec(),
        sent: review.sent.clone(),
        native: native.clone(),
        standing: Default::default(),
        fingerprints: Default::default(),
        purpose_fingerprint: None,
    };
    frozen
        .ids(state, review.pull_request_id, pr, snapshot)
        .await?;
    let id = PassId::new_random();
    let hash = frozen.write(state, id).await?;
    let model = crate::model::select_model(
        JobKind::VerifyComments,
        &Default::default(),
        state.db.settings().llm_selection().await?.as_ref(),
    );
    let input = AssessmentInput {
        pr: native.pull_request.clone(),
        pull_request_id: review.pull_request_id,
        head: pr.head_sha.clone(),
        base: pr.base_sha.clone(),
        input_hash: hash,
        scope_hash: handin::hash(serde_json::to_vec(&frozen.data.scope)?),
        snapshot_ref: frozen::reference(id),
        model: model.clone(),
    };
    let methods = if config.paired(&input.pr, now) {
        vec![config.primary_method, config.primary_method.other()]
    } else {
        vec![config.primary_method]
    };
    let mut jobs = Vec::new();
    for method in methods {
        jobs.push(MethodJob {
            version: match method {
                AssessmentMethod::ClaudiusHost => HOST_VERSION.into(),
                AssessmentMethod::PromptV4 => VERIFY_PROMPT_VERSION.to_string(),
            },
            spec: JobSpec {
                job_id: JobId::new_random(),
                attempt: 0,
                kind: JobKind::VerifyComments,
                target: JobTarget::PullRequestComments(PullRequestCommentsTarget {
                    pull_request_id: review.pull_request_id,
                    pass_id: id,
                    repo: RepoCheckout {
                        clone_url: crate::github::oauth::endpoint(
                            &state.config.github.clone_url,
                            &format!("{}/{}.git", pr.repo.owner, pr.repo.name),
                        )
                        .to_string(),
                        base_sha: pr.base_sha.clone(),
                        head_sha: pr.head_sha.clone(),
                    },
                }),
                prompt: match method {
                    AssessmentMethod::ClaudiusHost => {
                        include_str!("../../../../../prompts/check_pr_comments.md").into()
                    }
                    AssessmentMethod::PromptV4 => prompt::render(native, pr, language)?,
                },
                output_schema: match method {
                    AssessmentMethod::ClaudiusHost => json!({}),
                    AssessmentMethod::PromptV4 => agent::output_schema(JobKind::VerifyComments)?,
                },
                assessment: Some(AssessmentRunRef {
                    pass_id: id,
                    method,
                }),
                pr_data: Some(frozen.data.clone()),
                model: model.clone(),
                limits: JobLimits::default(),
                payer: user.clone(),
                session: None,
                working_session: None,
                context_pack: None,
                flow_file: None,
                skills: if method == AssessmentMethod::ClaudiusHost {
                    vec!["check-pr-comments".into()]
                } else {
                    vec![]
                },
                access: Default::default(),
            },
        });
    }
    let request = NewAssessmentPass {
        id,
        request_key: id.to_string(),
        input,
        jobs,
    };
    match state
        .db
        .assessments()
        .admit_review(&tenant, &user, &request, review, now)
        .await
    {
        Ok(pass) => {
            super::follow(state, pass.id, None);
            Ok(super::PassQueued::Queued)
        }
        Err(error) => {
            tokio::fs::remove_file(state.config.data_dir.join(frozen::reference(id))).await?;
            match error {
                AssessmentError::Admission(AssessmentAdmissionFailure::AssessmentDailyLimit {
                    ..
                }) => Ok(super::PassQueued::DailyLimit),
                AssessmentError::Database(db::error::DbError::Conflict) => {
                    Ok(super::PassQueued::AlreadyRunning)
                }
                error => Err(error.into()),
            }
        }
    }
}

fn fallback(pr: &PrSummary, items: &[ItemRecord]) -> PrData {
    PrData {
        manifest: Manifest {
            repo: format!("{}/{}", pr.repo.owner, pr.repo.name),
            pr: pr.number,
            head: pr.head_sha.0.clone(),
            base_sha: pr.base_sha.0.clone(),
            base_ref: "base".into(),
            title: pr.title.clone(),
            body: String::new(),
            pr_author: pr.author.clone(),
            maintainers: vec![],
            bots: vec![],
            delegates: vec![],
        },
        threads: json!({"threads":items.iter().filter(|i|i.kind==CommentItemKind::Thread).map(|i| json!({"id":i.stable_key,"isResolved":i.github.resolved.unwrap_or(false),"comments":[{"author":{"login":i.author},"body":i.body,"url":i.url.clone().unwrap_or_default()}]})).collect::<Vec<_>>()}),
        review_comments: vec![],
        reviews: vec![],
        comments: vec![],
        conventions: Default::default(),
        ids: ItemIds {
            next_id: 1,
            items: vec![],
        },
        scope: ReviewScope {
            threads: items
                .iter()
                .filter(|i| i.kind == CommentItemKind::Thread)
                .map(|i| i.stable_key.clone())
                .collect(),
            sources: vec![],
        },
    }
}

pub(super) async fn run(
    state: &AppState,
    record: &PassRecord,
    pass: AssessmentPass,
) -> Result<(), AssessError> {
    let tenant = state
        .db
        .pull_requests()
        .get(record.pull_request_id)
        .await?
        .tenant_id;
    let frozen = Frozen::read(state, &pass).await?;
    let mut runs = state
        .db
        .assessments()
        .runs(&tenant, &pass.payer, pass.id)
        .await?;
    runs.sort_by_key(|r| r.reference.method != pass.config.primary_method);
    for run in runs {
        if run.status == AssessmentRunStatus::Succeeded {
            if run.reference.method == pass.config.primary_method
                && let Err(error) = crate::review_loop::propagation::propagate(
                    state,
                    &tenant,
                    record.pull_request_id,
                )
                .await
            {
                tracing::warn!(
                    ?error,
                    pr = record.pull_request_id,
                    "review decisions could not propagate after resuming assessment"
                );
            }
            continue;
        }
        loop {
            let mut events = state.executor.events(&JobHandle { job_id: run.job_id });
            let result = loop {
                match events.next().await.transpose()? {
                    Some(JobUpdate::Finished { result, .. }) => break result,
                    Some(_) => continue,
                    None => return Err(AssessError::NoResult),
                }
            };
            let job = state.db.jobs().get(run.job_id).await?;
            if result.job_id != run.job_id
                || result.attempt != job.attempt
                || (result.assessment.as_deref() != Some(&run.reference)
                    && (result.assessment.is_some()
                        || matches!(result.outcome, JobOutcome::Succeeded { .. })))
            {
                return Err(AssessError::Snapshot);
            }
            let mut normalized = MethodResult {
                usage_known: result.usage.is_some(),
                reported_head: match run.reference.method {
                    AssessmentMethod::PromptV4 => Some(pass.input.head.clone()),
                    AssessmentMethod::ClaudiusHost => result
                        .report
                        .as_ref()
                        .map(|r| contracts::ids::CommitSha(r.metadata.commit.clone()))
                        .or_else(|| match &result.outcome {
                            JobOutcome::Succeeded { output } => output
                                .pointer("/metadata/commit")
                                .and_then(serde_json::Value::as_str)
                                .map(|s| contracts::ids::CommitSha(s.into())),
                            _ => None,
                        }),
                },
                status: AssessmentRunStatus::Succeeded,
                usage: result.usage.unwrap_or_default(),
                duration_ms: state
                    .db
                    .assessments()
                    .attempt_duration(job.id, job.attempt)
                    .await?,
                failure: None,
                items: vec![],
            };
            let (items, mut writes) = match &result.outcome {
                JobOutcome::Succeeded { output } => {
                    match super::adapter::normalize(&frozen, &pass, &run, &result, output.clone()) {
                        Ok(value) => value,
                        Err(failure) => {
                            normalized.status = AssessmentRunStatus::Failed;
                            normalized.failure = Some(failure);
                            (vec![], vec![])
                        }
                    }
                }
                JobOutcome::Failed { kind, .. } => {
                    normalized.status = AssessmentRunStatus::Failed;
                    normalized.failure = Some(*kind);
                    (vec![], vec![])
                }
                JobOutcome::Cancelled => {
                    normalized.status = AssessmentRunStatus::Cancelled;
                    (vec![], vec![])
                }
                JobOutcome::UsageLimited { .. } => {
                    normalized.status = AssessmentRunStatus::Failed;
                    normalized.failure = Some(JobFailureKind::ProviderFailed);
                    (vec![], vec![])
                }
            };
            if run.reference.method == AssessmentMethod::ClaudiusHost {
                let mut summary = state
                    .db
                    .pull_requests()
                    .get(record.pull_request_id)
                    .await?
                    .pr;
                summary.head_sha = pass.input.head.clone();
                summary.base_sha = pass.input.base.clone();
                let mut code = super::CodeReader::new(state, &summary);
                let purpose = frozen.purpose_fingerprint.clone().unwrap_or_else(|| {
                    super::purpose_hash(&frozen.native.title, frozen.native.description.as_deref())
                });
                for write in &mut writes {
                    if let Some(observed) = &write.observed {
                        write.fingerprint = super::fingerprint_fields(
                            super::FingerprintPoint {
                                body_hash: &observed.body_hash,
                                location: observed.location.as_ref(),
                                bot_status: observed.github.bot_status,
                            },
                            &pass.input.head,
                            code.at(observed.location.as_ref()).await.as_deref(),
                            &purpose,
                            None,
                        );
                    }
                }
            }
            normalized.items = items;
            let primary = run.reference.method == pass.config.primary_method;
            let accepted = state
                .db
                .assessments()
                .finish_with_primary(
                    &tenant,
                    &pass.payer,
                    &run.reference,
                    result.attempt,
                    &normalized,
                    if primary { &writes } else { &[] },
                )
                .await?;
            if primary
                && accepted
                && normalized.status == AssessmentRunStatus::Succeeded
                && let Err(error) = crate::review_loop::propagation::propagate(
                    state,
                    &tenant,
                    record.pull_request_id,
                )
                .await
            {
                tracing::warn!(
                    ?error,
                    pr = record.pull_request_id,
                    "review decisions could not propagate after assessment"
                );
            }
            if primary
                && accepted
                && run.reference.method == AssessmentMethod::PromptV4
                && let JobOutcome::Succeeded { output } = &result.outcome
                && let Some(purpose) = output["purpose"].as_str()
                && super::ingest::fit_text(purpose, 600).is_ok()
            {
                state
                    .db
                    .review_loop()
                    .set_purpose(record.pull_request_id, purpose)
                    .await?;
            }
            if normalized.status == AssessmentRunStatus::Failed
                && normalized.failure.is_some_and(JobFailureKind::retryable)
                && result.attempt < job.max_attempts
            {
                state
                    .db
                    .assessments()
                    .retry_method(&tenant, &pass.payer, &run.reference, result.attempt)
                    .await?;
                continue;
            }
            break;
        }
    }
    let primary_succeeded = state
        .db
        .assessments()
        .runs(&tenant, &pass.payer, pass.id)
        .await?
        .iter()
        .any(|r| {
            r.reference.method == pass.config.primary_method
                && r.status == AssessmentRunStatus::Succeeded
        });
    let made_progress =
        primary_succeeded && state.db.assessments().primary_made_progress(&pass).await?;
    state
        .db
        .review_loop()
        .finish_pass(pass.id, primary_succeeded, Timestamp::now())
        .await?;
    super::next(state, record, made_progress).await;
    Ok(())
}
