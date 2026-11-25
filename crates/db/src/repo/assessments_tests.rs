use super::assessments::{AssessmentError, MethodJob, MethodResult, NewAssessmentPass};
use super::*;
use contracts::assessment::*;
use contracts::gateway::{CredentialMode, CredentialState, CredentialStatus};
use contracts::ids::{PassId, UserId};
use contracts::job::result::Usage;
use contracts::job::spec::{JobTarget, PullRequestCommentsTarget};
use contracts::time::Timestamp;

fn at(value: &str) -> Timestamp {
    value.parse().unwrap()
}
fn user() -> UserId {
    UserId("u1".into())
}
async fn seed(db: &Db) {
    analyses::testing::seed_analysis(db).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO memberships(tenant_id,user_id,created_at) VALUES('default','u1','t')").execute(&db.pool).await.unwrap();
    db.llm_credentials()
        .put_status(
            "default",
            "u1",
            &CredentialStatus {
                telemetry: Default::default(),
                challenge: None,
                mode: CredentialMode::ChatgptPlan,
                state: CredentialState::Linked,
                relink_reason: None,
                label: None,
                plan: None,
                paused_until: None,
                last_call: None,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn assessment_review_admission_rolls_back_pass_jobs_and_quota_together() {
    let db = Db::in_memory().await.unwrap();
    seed(&db).await;
    db.review_loop()
        .ensure_pr("default", 1, at("2026-11-23T00:00:00Z"))
        .await
        .unwrap();
    let request = request("review", false);
    let review = review_loop::NewPass {
        pull_request_id: 1,
        user_id: Some("u1".into()),
        cause: review_loop::PassCause::Read,
        head: request.input.head.clone(),
        sent: vec![],
        question: None,
    };
    let mut activity = db.jobs().subscribe_activity(DEFAULT_TENANT);
    let other_tenant = db.jobs().subscribe_activity("other");
    let pass = db
        .assessments()
        .admit_review(
            "default",
            &user(),
            &request,
            &review,
            at("2026-11-23T00:00:00Z"),
        )
        .await
        .unwrap();
    assert!(
        activity.has_changed().unwrap(),
        "admitting queued jobs publishes"
    );
    activity.changed().await.unwrap();
    assert!(!other_tenant.has_changed().unwrap());
    let jobs = db.jobs().activity(DEFAULT_TENANT, "u1").await.unwrap().jobs;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].kind, contracts::agent::JobKind::VerifyComments);
    assert!(!jobs[0].running);
    assert_eq!(
        jobs[0].own.as_ref().unwrap().id,
        request.jobs[0].spec.job_id
    );
    let same = db
        .assessments()
        .admit_review(
            DEFAULT_TENANT,
            &user(),
            &request,
            &review,
            at("2026-11-23T00:00:00Z"),
        )
        .await
        .unwrap();
    assert_eq!(same.id, pass.id);
    assert!(
        !activity.has_changed().unwrap(),
        "an idempotent admission inserts no jobs"
    );
    assert_eq!(
        db.review_loop().pass(pass.id).await.unwrap().user_id,
        Some("u1".into())
    );
    let other = request_with_key("busy");
    assert!(
        db.assessments()
            .admit_review(
                "default",
                &user(),
                &other,
                &review,
                at("2026-11-23T00:00:00Z")
            )
            .await
            .is_err()
    );
    assert!(
        !activity.has_changed().unwrap(),
        "a rolled-back admission publishes nothing"
    );
    assert_eq!(
        db.assessments()
            .quota(&user(), at("2026-11-23T00:00:00Z"))
            .await
            .unwrap()
            .remaining,
        79
    );
    assert_eq!(db.review_loop().live_passes().await.unwrap().len(), 1);
}

fn request_with_key(key: &str) -> NewAssessmentPass {
    request(key, false)
}

#[tokio::test]
async fn assessment_cancelled_before_claim_is_terminal_without_an_attempt() {
    let db = Db::in_memory().await.unwrap();
    seed(&db).await;
    let request = request("cancel-unclaimed", false);
    let now = at("2026-11-23T12:00:00Z");
    let pass = db
        .assessments()
        .admit("default", &user(), &request, now)
        .await
        .unwrap();
    let run = request.jobs[0].spec.assessment.as_ref().unwrap();
    db.jobs()
        .request_cancel(request.jobs[0].spec.job_id, now)
        .await
        .unwrap();
    assert!(
        db.assessments()
            .finish_attempt(
                "default",
                &user(),
                run,
                0,
                &MethodResult {
                    usage_known: true,
                    reported_head: None,
                    status: AssessmentRunStatus::Cancelled,
                    usage: Usage::default(),
                    duration_ms: 0,
                    failure: None,
                    items: vec![]
                }
            )
            .await
            .unwrap()
    );
    assert_eq!(
        db.assessments()
            .runs("default", &user(), pass.id)
            .await
            .unwrap()[0]
            .status,
        AssessmentRunStatus::Cancelled
    );
    assert_eq!(
        db.assessments()
            .quota(&user(), now)
            .await
            .unwrap()
            .remaining,
        79
    );
}
fn request(key: &str, paired: bool) -> NewAssessmentPass {
    let id = PassId::new_random();
    let mut input = AssessmentInput {
        pr: "o/r#1".into(),
        pull_request_id: 1,
        head: contracts::ids::CommitSha("head".into()),
        base: contracts::ids::CommitSha("base".into()),
        input_hash: "a".repeat(64),
        scope_hash: "b".repeat(64),
        snapshot_ref: "frozen/immutable-1".into(),
        model: contracts::job::spec::ModelChoice {
            model: "fake".into(),
            reasoning_effort: contracts::job::spec::ReasoningEffort::High,
        },
    };
    let methods = if paired {
        vec![AssessmentMethod::ClaudiusHost, AssessmentMethod::PromptV4]
    } else {
        vec![AssessmentMethod::ClaudiusHost]
    };
    let jobs = methods
        .into_iter()
        .map(|method| {
            let mut spec = analyses::testing::job_spec(
                contracts::ids::AnalysisId::new_random(),
                contracts::agent::JobKind::VerifyComments,
            );
            spec.payer = user();
            spec.target = JobTarget::PullRequestComments(PullRequestCommentsTarget {
                pull_request_id: 1,
                pass_id: id,
                repo: contracts::job::spec::RepoCheckout {
                    clone_url: "file:///fake".into(),
                    base_sha: input.base.clone(),
                    head_sha: input.head.clone(),
                },
            });
            spec.assessment = Some(AssessmentRunRef {
                pass_id: id,
                method,
            });
            spec.model = input.model.clone();
            MethodJob {
                spec,
                version: if method == AssessmentMethod::ClaudiusHost {
                    "4.2.0".into()
                } else {
                    "4".into()
                },
            }
        })
        .collect();
    // No real provider or credential is used by these repository tests.
    input.model.model = "fake".into();
    NewAssessmentPass {
        id,
        request_key: key.into(),
        input,
        jobs,
    }
}

#[tokio::test]
async fn assessment_admission_is_atomic_and_idempotent_across_restart_and_midnight() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.sqlite");
    let db = Db::open(&path).await.unwrap();
    seed(&db).await;
    let now = at("2026-11-23T23:59:59Z");
    let missing = request("unlinked", false);
    db.llm_credentials().remove_status("u1").await.unwrap();
    assert!(matches!(
        db.assessments()
            .admit("default", &user(), &missing, now)
            .await,
        Err(AssessmentError::Admission(
            AssessmentAdmissionFailure::CredentialMissing
        ))
    ));
    assert_eq!(
        db.assessments()
            .quota(&user(), now)
            .await
            .unwrap()
            .remaining,
        80
    );
    seed(&db).await;
    for (state, reason, pause) in [
        (CredentialState::Linking, None, None),
        (
            CredentialState::RelinkNeeded,
            Some(contracts::gateway::RelinkReason::Quarantined),
            None,
        ),
        (
            CredentialState::Linked,
            None,
            Some(at("2026-11-24T00:01:00Z")),
        ),
    ] {
        db.llm_credentials()
            .put_status(
                "default",
                "u1",
                &CredentialStatus {
                    telemetry: Default::default(),
                    challenge: None,
                    mode: CredentialMode::ChatgptPlan,
                    state,
                    relink_reason: reason,
                    label: None,
                    plan: None,
                    paused_until: pause,
                    last_call: None,
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            db.assessments()
                .admit("default", &user(), &missing, now)
                .await,
            Err(AssessmentError::Admission(
                AssessmentAdmissionFailure::CredentialMissing
            ))
        ));
        assert_eq!(
            db.assessments()
                .quota(&user(), now)
                .await
                .unwrap()
                .remaining,
            80
        );
    }
    // Reset the paused credential explicitly; gateway status refreshes retain usage pauses.
    db.llm_credentials().remove_status("u1").await.unwrap();
    seed(&db).await;
    let mut config = AssessmentConfig::default();
    config.ab_prs.push("o/r#1".into());
    db.assessments()
        .set_config("default", &user(), &config)
        .await
        .unwrap();
    let mut broken = request("broken-pair", true);
    broken.jobs[1].spec.job_id = broken.jobs[0].spec.job_id;
    assert!(
        db.assessments()
            .admit("default", &user(), &broken, now)
            .await
            .is_err()
    );
    assert_eq!(
        db.assessments()
            .quota(&user(), now)
            .await
            .unwrap()
            .remaining,
        80
    );
    let pair = request("pair", true);
    let first = db
        .assessments()
        .admit("default", &user(), &pair, now)
        .await
        .unwrap();
    assert!(first.paired);
    assert_eq!(
        db.assessments()
            .runs("default", &user(), first.id)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        db.assessments()
            .quota(&user(), now)
            .await
            .unwrap()
            .remaining,
        79
    );
    // A configuration change and a different proposed ID never alter a retry's snapshot/day.
    db.assessments()
        .set_config("default", &user(), &AssessmentConfig::default())
        .await
        .unwrap();
    let retry = request("pair", false);
    assert_eq!(
        db.assessments()
            .admit("default", &user(), &retry, now)
            .await
            .unwrap(),
        first
    );
    let invalid = request("invalid", true); // Pair no longer selected.
    assert!(
        db.assessments()
            .admit("default", &user(), &invalid, now)
            .await
            .is_err()
    );
    assert_eq!(
        db.assessments()
            .quota(&user(), now)
            .await
            .unwrap()
            .remaining,
        79
    );
    drop(db);
    let db = Db::open(&path).await.unwrap();
    let midnight = at("2026-11-24T00:00:00Z");
    assert_eq!(
        db.assessments()
            .admit("default", &user(), &retry, midnight)
            .await
            .unwrap(),
        first
    );
    assert_eq!(
        db.assessments()
            .quota(&user(), midnight)
            .await
            .unwrap()
            .remaining,
        80
    );
    assert_eq!(
        db.assessments()
            .quota(&user(), now)
            .await
            .unwrap()
            .remaining,
        79
    );
    let second = db
        .assessments()
        .admit("default", &user(), &request("next", false), midnight)
        .await
        .unwrap();
    assert_eq!(second.day_utc, "2026-11-24");
    assert_eq!(
        db.assessments()
            .quota(&user(), midnight)
            .await
            .unwrap()
            .resets_at,
        at("2026-11-25T00:00:00Z")
    );
}

#[tokio::test]
async fn assessment_latest_orders_utc_instants_across_fractional_second_forms() {
    let db = Db::in_memory().await.unwrap();
    seed(&db).await;
    db.assessments()
        .admit(
            "default",
            &user(),
            &request("whole-second", false),
            at("2026-11-23T12:00:00Z"),
        )
        .await
        .unwrap();
    let newer = db
        .assessments()
        .admit(
            "default",
            &user(),
            &request("fractional-second", false),
            at("2026-11-23T12:00:00.100Z"),
        )
        .await
        .unwrap();
    assert_eq!(
        db.assessments()
            .latest("default", &user(), 1)
            .await
            .unwrap()
            .unwrap()
            .id,
        newer.id
    );
    assert!(
        db.assessments()
            .latest("other", &user(), 1)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assessment_quota_race_admits_exactly_eighty_and_deduplicates_keys() {
    let dir = tempfile::tempdir().unwrap();
    let db = super::testing::file_backed(dir.path()).await.unwrap();
    seed(&db).await;
    let now = at("2026-11-23T12:00:00Z");
    let shared = request("same", false);
    let (left, right) = tokio::join!(
        async {
            db.assessments()
                .admit("default", &user(), &shared, now)
                .await
        },
        async {
            db.assessments()
                .admit("default", &user(), &shared, now)
                .await
        }
    );
    assert_eq!(left.unwrap(), right.unwrap());
    for i in 1..79 {
        db.assessments()
            .admit(
                "default",
                &user(),
                &request(&format!("key-{i}"), false),
                now,
            )
            .await
            .unwrap();
    }
    let a = request("last-a", false);
    let b = request("last-b", false);
    let (left, right) = tokio::join!(
        async { db.assessments().admit("default", &user(), &a, now).await },
        async { db.assessments().admit("default", &user(), &b, now).await }
    );
    let outcomes = [left, right];
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(outcomes.iter().any(|r| matches!(r,Err(AssessmentError::Admission(AssessmentAdmissionFailure::AssessmentDailyLimit{resets_at})) if *resets_at==at("2026-11-24T00:00:00Z"))));
    assert_eq!(
        db.assessments()
            .quota(&user(), now)
            .await
            .unwrap()
            .remaining,
        0
    );
    let passes: i64 = sqlx::query_scalar("SELECT count(*) FROM assessment_passes")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!((passes, jobs), (80, 80));
}

fn item(run: AssessmentRunRef, key: &str) -> AssessmentComparisonItem {
    AssessmentComparisonItem {
        recorded_decision: None,
        comparison_key: key.into(),
        local_id: "local-1".into(),
        run,
        method_version: "4.2.0".into(),
        source: AssessmentSource::Inline {
            thread_id: "T1".into(),
            version: "a".repeat(64),
        },
        primary_item_id: None,
        primary_item_hash: None,
        status: ComparisonItemStatus::Matched,
        duplicate_of: None,
        verdict: Some(AssessmentVerdict::Valid),
        native_verdict: Some("valid".into()),
        proposal: None,
        owner_decision: None,
    }
}

async fn claim_assessment(db: &Db, now: Timestamp) -> jobs::JobRecord {
    db.jobs()
        .claim(jobs::ClaimParams {
            payer_ids: &[user().0],
            job_token_hash: b"fake-attempt-token",
            lease_until: now + jiff::SignedDuration::from_secs(60),
            now,
        })
        .await
        .unwrap()
        .unwrap()
}

async fn started_assessment() -> (Db, jobs::JobRecord, AssessmentRunRef, MethodResult) {
    let db = Db::in_memory().await.unwrap();
    seed(&db).await;
    let req = request("lease-fence", false);
    db.assessments()
        .admit("default", &user(), &req, at("2026-11-23T12:00:00Z"))
        .await
        .unwrap();
    let job = claim_assessment(&db, at("2026-11-23T12:00:00Z")).await;
    let run = req.jobs[0].spec.assessment.clone().unwrap();
    db.assessments()
        .begin_attempt("default", &user(), &run, job.attempt)
        .await
        .unwrap();
    let result = MethodResult {
        usage_known: true,
        reported_head: None,
        status: AssessmentRunStatus::Succeeded,
        usage: Usage {
            input_tokens: 7,
            ..Default::default()
        },
        duration_ms: 5,
        failure: None,
        items: vec![item(run.clone(), "current")],
    };
    (db, job, run, result)
}

#[tokio::test]
async fn assessment_unknown_usage_can_be_completed_later_without_double_duration() {
    let (db, job, run, mut result) = started_assessment().await;
    result.usage = Usage::default();
    result.usage_known = false;
    db.assessments()
        .finish_attempt("default", &user(), &run, job.attempt, &result)
        .await
        .unwrap();
    assert!(
        !db.assessments()
            .runs("default", &user(), run.pass_id)
            .await
            .unwrap()[0]
            .usage_complete
    );
    let usage = Usage {
        input_tokens: 17,
        requests: Some(2),
        ..Default::default()
    };
    assert!(
        db.assessments()
            .record_attempt_usage("default", &user(), &run, job.attempt, usage, 5)
            .await
            .unwrap()
    );
    assert!(
        !db.assessments()
            .record_attempt_usage("default", &user(), &run, job.attempt, usage, 5)
            .await
            .unwrap()
    );
    let complete = db
        .assessments()
        .runs("default", &user(), run.pass_id)
        .await
        .unwrap()
        .remove(0);
    assert!(complete.usage_complete);
    assert_eq!(complete.usage, usage);
    assert_eq!(complete.duration_ms, 5);
}

async fn assert_late_assessment_rejected(db: &Db, run: &AssessmentRunRef, result: &MethodResult) {
    assert!(matches!(
        db.assessments()
            .finish_attempt("default", &user(), run, 1, result)
            .await,
        Err(crate::error::DbError::Conflict)
    ));
    let runs = db
        .assessments()
        .runs("default", &user(), run.pass_id)
        .await
        .unwrap();
    assert_eq!(runs[0].status, AssessmentRunStatus::Running);
    assert_eq!(runs[0].accepted_attempt, None);
    assert_eq!(
        runs[0].usage,
        Usage {
            requests: Some(0),
            ..Default::default()
        }
    );
    assert_eq!(runs[0].duration_ms, 0);
    assert!(
        db.assessments()
            .comparison("default", &user(), run.pass_id, at("2026-11-23T12:02:00Z"))
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn assessment_rejects_expired_or_released_attempt_before_reclaim() {
    for release in [false, true] {
        for exhausted in [false, true] {
            let (db, job, run, result) = started_assessment().await;
            if exhausted {
                let mut tx = write_tx(&db.pool).await.unwrap();
                sqlx::query("UPDATE jobs SET max_attempts=1 WHERE id=?")
                    .bind(job.id.to_string())
                    .execute(&mut *tx)
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
            }
            let now = at("2026-11-23T12:02:00Z");
            if release {
                assert!(
                    db.jobs()
                        .release_claim(job.id, 1, now)
                        .await
                        .unwrap()
                        .is_some()
                );
            } else {
                assert_eq!(db.jobs().expire_leases(now).await.unwrap().len(), 1);
            }
            assert_late_assessment_rejected(&db, &run, &result).await;
            assert!(
                db.assessments()
                    .record_attempt_usage(
                        "default",
                        &user(),
                        &run,
                        1,
                        result.usage,
                        result.duration_ms
                    )
                    .await
                    .unwrap()
            );
            if !exhausted {
                let replacement = claim_assessment(&db, now).await;
                assert_eq!((replacement.id, replacement.attempt), (job.id, 2));
                db.assessments()
                    .begin_attempt("default", &user(), &run, 2)
                    .await
                    .unwrap();
                assert!(
                    db.assessments()
                        .finish_attempt("default", &user(), &run, 2, &result)
                        .await
                        .unwrap()
                );
                let runs = db
                    .assessments()
                    .runs("default", &user(), run.pass_id)
                    .await
                    .unwrap();
                assert_eq!(runs[0].accepted_attempt, Some(2));
                assert_eq!(runs[0].usage.input_tokens, 14);
            }
        }
    }
}

#[tokio::test]
async fn assessment_rejects_expired_attempt_after_reclaim() {
    let (db, job, run, result) = started_assessment().await;
    db.jobs()
        .expire_leases(at("2026-11-23T12:02:00Z"))
        .await
        .unwrap();
    let replacement = claim_assessment(&db, at("2026-11-23T12:02:00Z")).await;
    assert_eq!((replacement.id, replacement.attempt), (job.id, 2));
    assert_late_assessment_rejected(&db, &run, &result).await;
    db.assessments()
        .begin_attempt("default", &user(), &run, 2)
        .await
        .unwrap();
    assert!(
        db.assessments()
            .finish_attempt("default", &user(), &run, 2, &result)
            .await
            .unwrap()
    );
    assert_eq!(
        db.assessments()
            .runs("default", &user(), run.pass_id)
            .await
            .unwrap()[0]
            .accepted_attempt,
        Some(2)
    );
}

#[tokio::test]
async fn assessment_rejects_cancel_requested_and_cancelled_attempts() {
    for state in ["requested", "expired", "completed"] {
        let (db, job, run, result) = started_assessment().await;
        let now = at("2026-11-23T12:02:00Z");
        db.jobs().request_cancel(job.id, now).await.unwrap();
        match state {
            "expired" => {
                db.jobs().expire_leases(now).await.unwrap();
            }
            "completed" => {
                db.jobs()
                    .complete(
                        &jobs::server_result(
                            job.id,
                            1,
                            contracts::job::result::JobOutcome::Succeeded {
                                output: serde_json::json!({}),
                            },
                        ),
                        now,
                    )
                    .await
                    .unwrap();
            }
            _ => {}
        }
        assert_late_assessment_rejected(&db, &run, &result).await;
        assert!(
            db.assessments()
                .record_attempt_usage(
                    "default",
                    &user(),
                    &run,
                    1,
                    result.usage,
                    result.duration_ms
                )
                .await
                .unwrap()
        );
    }
}

#[tokio::test]
async fn assessment_cancelled_job_can_finalize_without_output() {
    use contracts::job::result::JobOutcome;
    let (db, job, run, mut result) = started_assessment().await;
    let now = at("2026-11-23T12:00:30Z");
    db.jobs().request_cancel(job.id, now).await.unwrap();
    db.jobs()
        .complete(&jobs::server_result(job.id, 1, JobOutcome::Cancelled), now)
        .await
        .unwrap();
    result.status = AssessmentRunStatus::Cancelled;
    assert!(matches!(
        db.assessments()
            .finish_attempt("default", &user(), &run, 1, &result)
            .await,
        Err(crate::error::DbError::Conflict)
    ));
    result.items.clear();
    assert_terminal_assessment_recorded(&db, &run, 1, &result, now).await;
}

#[tokio::test]
async fn assessment_exhausted_lease_can_finalize_without_output() {
    use contracts::job::result::JobFailureKind;
    let (db, job, run, mut result) = started_assessment().await;
    let mut now = at("2026-11-23T12:02:00Z");
    for attempt in 1..=3 {
        assert_eq!(db.jobs().expire_leases(now).await.unwrap().len(), 1);
        if attempt < 3 {
            let replacement = claim_assessment(&db, now).await;
            assert_eq!((replacement.id, replacement.attempt), (job.id, attempt + 1));
            db.assessments()
                .begin_attempt("default", &user(), &run, attempt + 1)
                .await
                .unwrap();
            now += jiff::SignedDuration::from_secs(120);
        }
    }
    assert_eq!(
        db.jobs().get(job.id).await.unwrap().state,
        contracts::job::state::JobState::Failed
    );
    result.items.clear();
    result.status = AssessmentRunStatus::Cancelled;
    assert!(matches!(
        db.assessments()
            .finish_attempt("default", &user(), &run, 3, &result)
            .await,
        Err(crate::error::DbError::Conflict)
    ));
    result.status = AssessmentRunStatus::Failed;
    result.failure = Some(JobFailureKind::Internal);
    assert!(matches!(
        db.assessments()
            .finish_attempt("default", &user(), &run, 2, &result)
            .await,
        Err(crate::error::DbError::Conflict)
    ));
    assert_terminal_assessment_recorded(&db, &run, 3, &result, now).await;
}

async fn assert_terminal_assessment_recorded(
    db: &Db,
    run: &AssessmentRunRef,
    attempt: u32,
    result: &MethodResult,
    now: Timestamp,
) {
    assert!(
        db.assessments()
            .finish_attempt("default", &user(), run, attempt, result)
            .await
            .unwrap()
    );
    assert!(
        !db.assessments()
            .finish_attempt("default", &user(), run, attempt, result)
            .await
            .unwrap()
    );
    let comparison = db
        .assessments()
        .comparison("default", &user(), run.pass_id, now)
        .await
        .unwrap();
    assert!(comparison.items.is_empty());
    let stored = &comparison.runs[0];
    assert_eq!(stored.status, result.status);
    assert_eq!(stored.accepted_attempt, None);
    assert_eq!(stored.failure, result.failure);
    assert_eq!(stored.usage.input_tokens, result.usage.input_tokens);
    assert_eq!(stored.duration_ms, result.duration_ms);
    assert_eq!(
        db.assessments()
            .quota(&user(), now)
            .await
            .unwrap()
            .remaining,
        79
    );
}

#[tokio::test]
async fn assessment_accepts_completed_attempt_without_replacing_failed_job_outcome() {
    use contracts::job::result::{JobFailureKind, JobOutcome};
    for failed in [false, true] {
        let (db, job, run, mut result) = started_assessment().await;
        let outcome = if failed {
            JobOutcome::Failed {
                kind: JobFailureKind::WorkerLost,
                message: "fake failure".into(),
            }
        } else {
            JobOutcome::Succeeded {
                output: serde_json::json!({}),
            }
        };
        db.jobs()
            .complete(
                &jobs::server_result(job.id, 1, outcome),
                at("2026-11-23T12:00:30Z"),
            )
            .await
            .unwrap();
        if failed {
            assert_late_assessment_rejected(&db, &run, &result).await;
            result.status = AssessmentRunStatus::Failed;
            result.failure = Some(JobFailureKind::WorkerLost);
            result.items.clear();
        }
        assert!(
            db.assessments()
                .finish_attempt("default", &user(), &run, 1, &result)
                .await
                .unwrap()
        );
        assert!(
            !db.assessments()
                .finish_attempt("default", &user(), &run, 1, &result)
                .await
                .unwrap()
        );
    }
}

#[tokio::test]
async fn assessment_results_fence_attempts_and_keep_secondary_comparison_only() {
    let db = Db::in_memory().await.unwrap();
    seed(&db).await;
    let now = at("2026-11-23T12:00:00Z");
    let config = AssessmentConfig {
        ab_prs: vec!["o/r#1".into()],
        ..Default::default()
    };
    db.assessments()
        .set_config("default", &user(), &config)
        .await
        .unwrap();
    let req = request("pair", true);
    let pass = db
        .assessments()
        .admit("default", &user(), &req, now)
        .await
        .unwrap();
    let run = AssessmentRunRef {
        pass_id: pass.id,
        method: AssessmentMethod::PromptV4,
    };
    let method_job = &req.jobs[1].spec;
    sqlx::query("UPDATE jobs SET attempt=1,state='running' WHERE id=?")
        .bind(method_job.job_id.to_string())
        .execute(&db.pool)
        .await
        .unwrap();
    db.assessments()
        .begin_attempt("default", &user(), &run, 1)
        .await
        .unwrap();
    let failure = MethodResult {
        usage_known: true,
        reported_head: None,
        status: AssessmentRunStatus::Failed,
        usage: Usage {
            input_tokens: 10,
            cached_input_tokens: 2,
            output_tokens: 1,
            requests: Some(1),
        },
        duration_ms: 5,
        failure: Some(contracts::job::result::JobFailureKind::WorkerLost),
        items: vec![],
    };
    assert!(
        db.assessments()
            .finish_attempt("default", &user(), &run, 1, &failure)
            .await
            .unwrap()
    );
    assert!(
        !db.assessments()
            .finish_attempt("default", &user(), &run, 1, &failure)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE jobs SET state='failed' WHERE id=?")
        .bind(method_job.job_id.to_string())
        .execute(&db.pool)
        .await
        .unwrap();
    let activity = db.jobs().subscribe_activity(DEFAULT_TENANT);
    db.assessments()
        .retry_method("default", &user(), &run, 1)
        .await
        .unwrap();
    assert!(
        activity.has_changed().unwrap(),
        "assessment requeue publishes"
    );
    assert_eq!(
        db.assessments()
            .quota(&user(), now)
            .await
            .unwrap()
            .remaining,
        79
    );
    sqlx::query("UPDATE jobs SET attempt=2,state='running' WHERE id=?")
        .bind(method_job.job_id.to_string())
        .execute(&db.pool)
        .await
        .unwrap();
    db.assessments()
        .begin_attempt("default", &user(), &run, 2)
        .await
        .unwrap();
    assert!(
        db.assessments()
            .finish_attempt("default", &user(), &run, 1, &failure)
            .await
            .is_err()
    );
    let mut row = item(run.clone(), "key");
    row.method_version = "4".into();
    let success = MethodResult {
        usage_known: true,
        reported_head: None,
        status: AssessmentRunStatus::Succeeded,
        items: vec![row],
        failure: None,
        ..failure.clone()
    };
    let foreign = UserId("someone-else".into());
    assert!(
        db.assessments()
            .finish_attempt("default", &foreign, &run, 2, &success)
            .await
            .is_err()
    );
    assert!(
        db.assessments()
            .finish_attempt("default", &user(), &run, 2, &success)
            .await
            .unwrap()
    );
    assert!(
        !db.assessments()
            .finish_attempt("default", &user(), &run, 2, &success)
            .await
            .unwrap()
    );
    let runs = db
        .assessments()
        .runs("default", &user(), pass.id)
        .await
        .unwrap();
    let secondary = runs
        .iter()
        .find(|r| r.reference.method == AssessmentMethod::PromptV4)
        .unwrap();
    assert_eq!(secondary.accepted_attempt, Some(2));
    assert_eq!(secondary.usage.input_tokens, 20);
    assert_eq!(secondary.usage.requests, Some(2));
    assert_eq!(secondary.duration_ms, 10);
    assert_eq!(
        runs.iter()
            .find(|r| r.reference.method == AssessmentMethod::ClaudiusHost)
            .unwrap()
            .status,
        AssessmentRunStatus::Queued
    );
    let items = db
        .assessments()
        .comparison("default", &user(), pass.id, now)
        .await
        .unwrap();
    assert_eq!(items.items.len(), 1);
    let primary: i64 = sqlx::query_scalar("SELECT count(*) FROM comment_items")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(primary, 0);
    assert!(
        db.assessments()
            .begin_attempt("default", &user(), &run, 3)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE assessment_passes SET head='moved' WHERE id=?")
            .bind(pass.id.to_string())
            .execute(&db.pool)
            .await
            .is_err()
    );
}

async fn seed_item(db: &Db) -> contracts::ids::CommentItemId {
    let id = contracts::ids::CommentItemId::new_random();
    sqlx::query(
        "INSERT INTO comment_prs(pull_request_id,tenant_id,created_at) VALUES(1,'default','t')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO comment_items(id,tenant_id,pull_request_id,item_no,kind,stable_key,author,author_kind,title,body,body_hash,first_seen_head,github,state,created_at,updated_at) VALUES(?,'default',1,1,'thread','T1','reviewer','human','Concern','Body',?,'head','{}','undecided','t','t')")
        .bind(id.to_string()).bind("a".repeat(64)).execute(&db.pool).await.unwrap();
    id
}
async fn decision(
    db: &Db,
    item: contracts::ids::CommentItemId,
    previous: Option<contracts::ids::DecisionId>,
    head: &str,
    hash: &str,
    at: Timestamp,
) -> contracts::ids::DecisionId {
    let id = contracts::ids::DecisionId::new_random();
    sqlx::query("INSERT INTO comment_decisions(id,tenant_id,item_id,user_id,kind,resolve_ticked,via,head,body_hash,decided_at,supersedes,source_version) VALUES(?,'default',?,'u1',?,0,'table',?,?,?,?,(SELECT s.body_hash FROM comment_items i JOIN comment_sources s ON s.id=i.source_id WHERE i.id=?))")
        .bind(id.to_string()).bind(item.to_string()).bind(if previous.is_some() {"reject"} else {"fix_here"}).bind(head).bind(hash).bind(at.to_string()).bind(previous.map(|v|v.to_string())).bind(item.to_string()).execute(&db.pool).await.unwrap();
    id
}

#[tokio::test]
async fn assessment_owner_history_uses_actual_decisions_at_cutoff_and_rejects_other_snapshots() {
    let db = Db::in_memory().await.unwrap();
    seed(&db).await;
    let now = at("2026-11-23T12:00:00Z");
    let item_id = seed_item(&db).await;
    db.assessments()
        .set_config(
            "default",
            &user(),
            &AssessmentConfig {
                ab_prs: vec!["o/r#1".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let req = request("primary", true);
    let pass = db
        .assessments()
        .admit("default", &user(), &req, now)
        .await
        .unwrap();
    let run = AssessmentRunRef {
        pass_id: pass.id,
        method: AssessmentMethod::ClaudiusHost,
    };
    sqlx::query("UPDATE jobs SET attempt=1,state='running' WHERE id=?")
        .bind(req.jobs[0].spec.job_id.to_string())
        .execute(&db.pool)
        .await
        .unwrap();
    db.assessments()
        .begin_attempt("default", &user(), &run, 1)
        .await
        .unwrap();
    let mut row = item(run.clone(), "key");
    row.primary_item_id = Some(item_id);
    row.primary_item_hash = Some("a".repeat(64));
    let output = MethodResult {
        usage_known: true,
        reported_head: None,
        status: AssessmentRunStatus::Succeeded,
        usage: Usage::default(),
        duration_ms: 0,
        failure: None,
        items: vec![row],
    };
    db.assessments()
        .finish_attempt("default", &user(), &run, 1, &output)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET state='succeeded' WHERE id=?")
        .bind(req.jobs[0].spec.job_id.to_string())
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET attempt=1,state='running' WHERE id=?")
        .bind(req.jobs[1].spec.job_id.to_string())
        .execute(&db.pool)
        .await
        .unwrap();
    let secondary = AssessmentRunRef {
        pass_id: pass.id,
        method: AssessmentMethod::PromptV4,
    };
    db.assessments()
        .begin_attempt("default", &user(), &secondary, 1)
        .await
        .unwrap();
    let mut unmatched = item(secondary, "key");
    unmatched.status = ComparisonItemStatus::Unmatched;
    unmatched.method_version = "4".into();
    db.assessments()
        .finish_attempt(
            "default",
            &user(),
            &unmatched.run,
            1,
            &MethodResult {
                usage_known: true,
                reported_head: None,
                items: vec![unmatched.clone()],
                ..output.clone()
            },
        )
        .await
        .unwrap();
    let early = at("2026-11-23T12:01:00Z");
    let late = at("2026-11-23T12:02:00Z");
    let first = decision(&db, item_id, None, "head", &"a".repeat(64), early).await;
    db.assessments()
        .record_owner_decision("default", &user(), pass.id, "key", first)
        .await
        .unwrap();
    db.assessments()
        .record_owner_decision("default", &user(), pass.id, "key", first)
        .await
        .unwrap();
    let second = decision(&db, item_id, Some(first), "head", &"a".repeat(64), late).await;
    db.assessments()
        .record_owner_decision("default", &user(), pass.id, "key", second)
        .await
        .unwrap();
    let before = db
        .assessments()
        .comparison("default", &user(), pass.id, now)
        .await
        .unwrap();
    assert!(before.items[0].owner_decision.is_none());
    let older = db
        .assessments()
        .comparison("default", &user(), pass.id, early)
        .await
        .unwrap();
    let latest = db
        .assessments()
        .comparison("default", &user(), pass.id, late)
        .await
        .unwrap();
    assert_eq!(older.items[0].owner_decision.as_ref().unwrap().id, first);
    assert_eq!(latest.items[0].owner_decision.as_ref().unwrap().revision, 2);
    assert_eq!(latest.items[0].owner_decision.as_ref().unwrap().id, second);
    assert!(
        latest.items[1].owner_decision.is_none(),
        "unmatched secondary is unscored"
    );
    let changed_head = decision(&db, item_id, Some(second), "moved", &"a".repeat(64), late).await;
    assert!(
        db.assessments()
            .record_owner_decision("default", &user(), pass.id, "key", changed_head)
            .await
            .is_err()
    );
    let changed_source = decision(
        &db,
        item_id,
        Some(changed_head),
        "head",
        &"b".repeat(64),
        late,
    )
    .await;
    assert!(
        db.assessments()
            .record_owner_decision("default", &user(), pass.id, "key", changed_source)
            .await
            .is_err()
    );
    assert!(
        db.assessments()
            .record_owner_decision("default", &UserId("other".into()), pass.id, "key", first)
            .await
            .is_err()
    );
    assert!(
        db.assessments()
            .comparison("other", &user(), pass.id, late)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE assessment_owner_decisions SET kind='resolve'")
            .execute(&db.pool)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn assessment_foreign_keys_prevent_cross_payer_pass_and_secondary_primary_attribution() {
    let db = Db::in_memory().await.unwrap();
    seed(&db).await;
    let now = at("2026-11-23T12:00:00Z");
    let item_id = seed_item(&db).await;
    let config = AssessmentConfig {
        ab_prs: vec!["o/r#1".into()],
        ..Default::default()
    };
    db.assessments()
        .set_config("default", &user(), &config)
        .await
        .unwrap();
    let a = request("a", true);
    let b = request("b", true);
    db.assessments()
        .admit("default", &user(), &a, now)
        .await
        .unwrap();
    db.assessments()
        .admit("default", &user(), &b, now)
        .await
        .unwrap();
    for (pass, job, method, payer) in [
        (a.id, a.jobs[1].spec.job_id, "prompt_v4", "u1"),
        (a.id, b.jobs[0].spec.job_id, "claudius_host", "u1"),
        (a.id, a.jobs[0].spec.job_id, "claudius_host", "other"),
    ] {
        assert!(sqlx::query("INSERT INTO comment_assessments(tenant_id,item_id,job_id,pass_id,method,payer_id,head,fingerprint,body,prompt_version,assessed_at) VALUES('default',?,?,?,?,?,'head','hash','{\"validity\":\"valid\"}',4,'t')")
            .bind(item_id.to_string()).bind(job.to_string()).bind(pass.to_string()).bind(method).bind(payer).execute(&db.pool).await.is_err());
    }
    assert!(
        sqlx::query("UPDATE assessment_method_runs SET payer_id='other' WHERE pass_id=?")
            .bind(a.id.to_string())
            .execute(&db.pool)
            .await
            .is_err()
    );
    let rows: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&db.pool)
        .await
        .unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn assessment_body_owner_choice_binds_point_hash_and_source_version_separately() {
    let db = Db::in_memory().await.unwrap();
    seed(&db).await;
    let now = at("2026-11-23T12:00:00Z");
    let item_id = seed_item(&db).await;
    sqlx::query("INSERT INTO comment_sources(id,tenant_id,pull_request_id,source_kind,url,author,body,body_hash,role) VALUES(1,'default',1,'review_body','https://fake.invalid/review','reviewer','Full source',?,'source')").bind("b".repeat(64)).execute(&db.pool).await.unwrap();
    sqlx::query("UPDATE comment_items SET source_id=1 WHERE id=?")
        .bind(item_id.to_string())
        .execute(&db.pool)
        .await
        .unwrap();
    let req = request("body", false);
    let pass = db
        .assessments()
        .admit("default", &user(), &req, now)
        .await
        .unwrap();
    let run = AssessmentRunRef {
        pass_id: pass.id,
        method: AssessmentMethod::ClaudiusHost,
    };
    sqlx::query("UPDATE jobs SET attempt=1,state='running' WHERE id=?")
        .bind(req.jobs[0].spec.job_id.to_string())
        .execute(&db.pool)
        .await
        .unwrap();
    db.assessments()
        .begin_attempt("default", &user(), &run, 1)
        .await
        .unwrap();
    let mut row = item(run.clone(), "body");
    row.primary_item_id = Some(item_id);
    row.primary_item_hash = Some("a".repeat(64));
    row.source = AssessmentSource::Body {
        source_kind: contracts::api::review_loop::CommentSourceKind::ReviewBody,
        url: "https://fake.invalid/review".into(),
        version: "b".repeat(64),
        quote: "Full source".into(),
    };
    db.assessments()
        .finish_attempt(
            "default",
            &user(),
            &run,
            1,
            &MethodResult {
                usage_known: true,
                reported_head: None,
                status: AssessmentRunStatus::Succeeded,
                usage: Usage::default(),
                duration_ms: 0,
                failure: None,
                items: vec![row],
            },
        )
        .await
        .unwrap();
    let first = decision(&db, item_id, None, "head", &"a".repeat(64), now).await;
    db.assessments()
        .record_owner_decision("default", &user(), pass.id, "body", first)
        .await
        .unwrap();
    sqlx::query("UPDATE comment_sources SET body_hash=? WHERE id=1")
        .bind("c".repeat(64))
        .execute(&db.pool)
        .await
        .unwrap();
    let changed = decision(&db, item_id, Some(first), "head", &"a".repeat(64), now).await;
    assert!(
        db.assessments()
            .record_owner_decision("default", &user(), pass.id, "body", changed)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn assessment_repository_attributes_primary_and_refuses_secondary_writes() {
    let db = Db::in_memory().await.unwrap();
    seed(&db).await;
    let now = at("2026-11-23T12:00:00Z");
    let item_id = seed_item(&db).await;
    db.assessments()
        .set_config(
            "default",
            &user(),
            &AssessmentConfig {
                ab_prs: vec!["o/r#1".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let req = request("attribution", true);
    db.assessments()
        .admit("default", &user(), &req, now)
        .await
        .unwrap();
    let assessment = |job| {
        serde_json::from_value::<contracts::api::review_loop::CommentAssessment>(serde_json::json!({"job":job,"head":"head","assessed_at":now,"validity":"valid","evidence":[],"category":"correctness","necessity":{"verdict":"in","reason":"Relevant"},"gates":[],"elsewhere":[],"reply_drafts":[],"recommendation":{"kind":"fix_here","reason":"Relevant","reason_class":"real bug"},"prompt_version":4})).unwrap()
    };
    assert!(
        db.review_loop()
            .add_assessment(
                "default",
                item_id,
                &assessment(req.jobs[1].spec.job_id),
                "fp"
            )
            .await
            .is_err()
    );
    let run = req.jobs[0].spec.assessment.as_ref().unwrap();
    sqlx::query("UPDATE jobs SET attempt=1,state='running' WHERE id=?")
        .bind(req.jobs[0].spec.job_id.to_string())
        .execute(&db.pool)
        .await
        .unwrap();
    db.assessments()
        .begin_attempt("default", &user(), run, 1)
        .await
        .unwrap();
    db.assessments()
        .finish_attempt(
            "default",
            &user(),
            run,
            1,
            &MethodResult {
                usage_known: true,
                reported_head: None,
                status: AssessmentRunStatus::Succeeded,
                usage: Usage::default(),
                duration_ms: 0,
                failure: None,
                items: vec![],
            },
        )
        .await
        .unwrap();
    db.review_loop()
        .add_assessment(
            "default",
            item_id,
            &assessment(req.jobs[0].spec.job_id),
            "fp",
        )
        .await
        .unwrap();
    let stored: (String, String, String) =
        sqlx::query_as("SELECT pass_id,method,payer_id FROM comment_assessments WHERE item_id=?")
            .bind(item_id.to_string())
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        stored,
        (req.id.to_string(), "claudius_host".into(), "u1".into())
    );
}

#[tokio::test]
async fn assessment_late_metering_counts_lost_attempts_once_without_overwriting_output() {
    let db = Db::in_memory().await.unwrap();
    seed(&db).await;
    let now = at("2026-11-23T12:00:00Z");
    let req = request("late-usage", false);
    db.assessments()
        .admit("default", &user(), &req, now)
        .await
        .unwrap();
    let run = req.jobs[0].spec.assessment.as_ref().unwrap();
    let job = req.jobs[0].spec.job_id;
    // The first attempt lost its lease before posting a result; the second succeeds.
    sqlx::query("UPDATE jobs SET attempt=2,state='running' WHERE id=?")
        .bind(job.to_string())
        .execute(&db.pool)
        .await
        .unwrap();
    db.assessments()
        .begin_attempt("default", &user(), run, 2)
        .await
        .unwrap();
    let usage = Usage {
        input_tokens: 10,
        cached_input_tokens: 2,
        output_tokens: 1,
        requests: Some(1),
    };
    let result = MethodResult {
        usage_known: true,
        reported_head: None,
        status: AssessmentRunStatus::Succeeded,
        usage,
        duration_ms: 5,
        failure: None,
        items: vec![item(run.clone(), "key")],
    };
    db.assessments()
        .finish_attempt("default", &user(), run, 2, &result)
        .await
        .unwrap();
    assert!(
        db.assessments()
            .record_attempt_usage("default", &user(), run, 1, usage, 5)
            .await
            .unwrap()
    );
    assert!(
        !db.assessments()
            .record_attempt_usage("default", &user(), run, 1, usage, 5)
            .await
            .unwrap()
    );
    assert!(
        !db.assessments()
            .record_attempt_usage("default", &user(), run, 2, usage, 5)
            .await
            .unwrap()
    );
    assert!(
        db.assessments()
            .record_attempt_usage("default", &user(), run, 3, usage, 5)
            .await
            .is_err()
    );
    assert!(
        db.assessments()
            .record_attempt_usage("default", &UserId("other".into()), run, 1, usage, 5)
            .await
            .is_err()
    );
    let runs = db
        .assessments()
        .runs("default", &user(), req.id)
        .await
        .unwrap();
    let telemetry = db
        .jobs()
        .payer_telemetry(&user(), Timestamp::now())
        .await
        .unwrap();
    assert_eq!(telemetry.tokens_today.unwrap().input_tokens, 20);
    sqlx::query("UPDATE jobs SET finished_at=?,result=? WHERE id=?")
        .bind(Timestamp::now().to_rfc3339_offset())
        .bind(serde_json::json!({"usage":usage}).to_string())
        .bind(job.to_string())
        .execute(&db.pool)
        .await
        .unwrap();
    let telemetry = db
        .jobs()
        .payer_telemetry(&user(), Timestamp::now())
        .await
        .unwrap();
    assert_eq!(
        telemetry.tokens_today.unwrap().input_tokens,
        20,
        "terminal result is already represented by its attempt meter"
    );
    assert_eq!(telemetry.tokens_today.unwrap().jobs, 1);
    assert_eq!(runs[0].usage.input_tokens, 20);
    assert_eq!(runs[0].usage.requests, Some(2));
    assert_eq!(runs[0].duration_ms, 10);
    assert_eq!(runs[0].accepted_attempt, Some(2));
    assert_eq!(runs[0].status, AssessmentRunStatus::Succeeded);
    assert_eq!(
        db.assessments()
            .comparison("default", &user(), req.id, now)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

#[tokio::test]
async fn assessment_user_configuration_and_quota_are_shared_across_tenant_memberships() {
    let db = Db::in_memory().await.unwrap();
    seed(&db).await;
    let now = at("2026-11-23T12:00:00Z");
    sqlx::raw_sql("INSERT INTO tenants(id,name,created_at) VALUES('other','Other','t'); INSERT INTO memberships(tenant_id,user_id,created_at) VALUES('other','u1','t'); INSERT INTO repos(id,tenant_id,owner,name) VALUES(2,'other','other','repo'); INSERT INTO pull_requests(id,tenant_id,repo_id,number,title,author,html_url,base_sha,head_sha,fetched_at) VALUES(2,'other',2,1,'Title','author','url','base','head','t');").execute(&db.pool).await.unwrap();
    let first = request("tenant-one", false);
    db.assessments()
        .admit("default", &user(), &first, now)
        .await
        .unwrap();
    let mut second = request("tenant-two", false);
    second.input.pull_request_id = 2;
    second.input.pr = "other/repo#1".into();
    if let JobTarget::PullRequestComments(target) = &mut second.jobs[0].spec.target {
        target.pull_request_id = 2;
    }
    db.assessments()
        .admit("other", &user(), &second, now)
        .await
        .unwrap();
    assert_eq!(
        db.assessments()
            .quota(&user(), now)
            .await
            .unwrap()
            .remaining,
        78
    );
    let config = AssessmentConfig {
        primary_method: AssessmentMethod::PromptV4,
        ..Default::default()
    };
    db.assessments()
        .set_config("other", &user(), &config)
        .await
        .unwrap();
    assert_eq!(db.assessments().config(&user()).await.unwrap(), config);
    assert!(
        db.assessments()
            .comparison("other", &user(), first.id, now)
            .await
            .is_err()
    );
    assert!(
        db.assessments()
            .comparison("default", &user(), second.id, now)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn assessment_stale_canonical_leaves_duplicate_missing_without_aborting_valid_rows() {
    use super::assessment_primary::PrimaryPoint;
    let (db, job, run, mut result) = started_assessment().await;
    let canonical = seed_item(&db).await;
    let mut tx = write_tx(&db.pool).await.unwrap();
    sqlx::query("UPDATE comment_prs SET next_item_no=2 WHERE pull_request_id=1")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut seen = super::review_loop::ObservedItem {
        kind: contracts::api::review_loop::CommentItemKind::Thread,
        stable_key: "T2".into(),
        author: "reviewer".into(),
        author_kind: contracts::api::review_loop::AuthorKind::Human,
        url: None,
        location: None,
        title: "Duplicate".into(),
        body: "Duplicate concern".into(),
        body_hash: "d".repeat(64),
        head: contracts::ids::CommitSha("head".into()),
        round: None,
        bot_severity: None,
        github: contracts::api::review_loop::ItemGithubFacts {
            resolved: Some(false),
            resolved_by: None,
            outdated: Some(false),
            bot_status: None,
            edited_after_decision: false,
            replies: 0,
            marker: None,
        },
        outside: None,
    };
    let duplicate = match db
        .review_loop()
        .upsert_item(
            "default",
            1,
            &seen,
            contracts::api::review_loop::ItemState::Undecided,
            at("2026-11-23T12:00:00Z"),
        )
        .await
        .unwrap()
    {
        super::review_loop::UpsertOutcome::New(i) => i,
        _ => panic!("new"),
    };
    seen.stable_key = "T3".into();
    let valid = match db
        .review_loop()
        .upsert_item(
            "default",
            1,
            &seen,
            contracts::api::review_loop::ItemState::Undecided,
            at("2026-11-23T12:00:00Z"),
        )
        .await
        .unwrap()
    {
        super::review_loop::UpsertOutcome::New(i) => i,
        _ => panic!("new"),
    };
    let point = |key: &str, id, hash: String, duplicate| PrimaryPoint {
        carry: None,
        key: key.into(),
        existing: Some((id, hash)),
        observed: None,
        preferred_no: 1,
        source: None,
        duplicate,
        assessment: None,
        fingerprint: "fp".into(),
    };
    let writes = vec![
        point("canonical", canonical, "changed".into(), None),
        point("valid", valid.id, valid.body_hash, None),
        point(
            "duplicate",
            duplicate.id,
            duplicate.body_hash,
            Some("canonical".into()),
        ),
    ];
    result.items = vec![
        item(run.clone(), "canonical"),
        item(run.clone(), "valid"),
        item(run.clone(), "duplicate"),
    ];
    result.items[2].status = ComparisonItemStatus::Duplicate;
    result.items[2].duplicate_of = Some("canonical".into());
    result.items[2].verdict = None;
    assert!(
        db.assessments()
            .finish_with_primary("default", &user(), &run, job.attempt, &result, &writes)
            .await
            .unwrap()
    );
    let report = db
        .assessments()
        .comparison("default", &user(), run.pass_id, at("2026-11-23T12:00:00Z"))
        .await
        .unwrap();
    let row = report
        .items
        .iter()
        .find(|r| r.comparison_key == "duplicate")
        .unwrap();
    assert_eq!(row.status, ComparisonItemStatus::MissingRow);
    assert!(row.duplicate_of.is_none());
    assert!(
        report
            .items
            .iter()
            .any(|r| r.comparison_key == "valid" && r.primary_item_id == Some(valid.id))
    );
}

#[tokio::test]
async fn assessment_primary_preserves_items_gone_during_the_pass() {
    use super::assessment_primary::PrimaryPoint;
    for gone_at in ["2026-11-23T12:00:00Z", "2026-11-23T12:01:00Z"] {
        let (db, job, run, result) = started_assessment().await;
        let id = seed_item(&db).await;
        db.review_loop()
            .mark_gone(1, &[], at(gone_at))
            .await
            .unwrap();
        let point = PrimaryPoint {
            carry: None,
            key: "current".into(),
            existing: Some((id, "a".repeat(64))),
            observed: None,
            preferred_no: 1,
            source: None,
            duplicate: None,
            assessment: None,
            fingerprint: "fp".into(),
        };
        assert!(
            db.assessments()
                .finish_with_primary("default", &user(), &run, job.attempt, &result, &[point])
                .await
                .unwrap()
        );
        let mut tx = read_tx(&db.pool).await.unwrap();
        let stored: Option<String> =
            sqlx::query_scalar("SELECT gone_at FROM comment_items WHERE id=?")
                .bind(id.to_string())
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(stored.map(|value| at(&value)), Some(at(gone_at)));
    }
}

#[tokio::test]
async fn configuration_revision_rejects_stale_tabs_without_losing_selections() {
    let db = Db::in_memory().await.unwrap();
    seed(&db).await;
    let initial = db.assessments().config_snapshot(&user()).await.unwrap();
    let mut first = initial.0.clone();
    first.ab_prs.push("acme/a#1".into());
    db.assessments()
        .set_config_if_current("default", &user(), &first, &initial.1)
        .await
        .unwrap();
    let mut second = initial.0;
    second.ab_prs.push("acme/b#2".into());
    assert!(matches!(
        db.assessments()
            .set_config_if_current("default", &user(), &second, &initial.1)
            .await,
        Err(crate::error::DbError::Conflict)
    ));
    assert_eq!(db.assessments().config(&user()).await.unwrap(), first);
}

#[tokio::test]
async fn payer_telemetry_uses_utc_boundaries_isolates_payers_and_lists_claimed_containers() {
    let db = Db::in_memory().await.unwrap();
    seed(&db).await;
    let now = at("2026-11-24T12:00:00Z");
    let empty = db.jobs().payer_telemetry(&user(), now).await.unwrap();
    assert!(empty.tokens_today.is_none());
    assert_eq!(empty.live_containers, Some(vec![]));
    use super::jobs::{JobStage, NewJob};
    use contracts::agent::JobKind;
    let analysis = analyses::testing::seed_analysis(&db).await.unwrap();
    for (index, finished, input) in [
        (1, "2026-11-24T00:00:00Z", Some(100)),
        (2, "2026-11-23T23:59:59Z", Some(200)),
        (3, "2026-10-31T23:59:59Z", Some(400)),
        (4, "2026-11-24T12:00:01Z", Some(800)),
        (5, "2026-11-24T01:00:00Z", None),
    ] {
        let mut spec = analyses::testing::job_spec(analysis, JobKind::Plan);
        spec.payer = user();
        let job = db
            .jobs()
            .enqueue(
                NewJob {
                    spec,
                    priority: contracts::job::spec::JobPriority::Batch,
                    stage: JobStage::Plan { batch: index },
                    generation: 0,
                },
                now,
            )
            .await
            .unwrap();
        let result = input.map(|input| serde_json::json!({"usage":{"input_tokens": input,"cached_input_tokens":input/2,"output_tokens":10}})).unwrap_or(serde_json::json!({"usage":null}));
        sqlx::query("UPDATE jobs SET state='succeeded',finished_at=?,result=? WHERE id=?")
            .bind(finished)
            .bind(result.to_string())
            .bind(job.id.to_string())
            .execute(&db.pool)
            .await
            .unwrap();
    }
    let mut spec = analyses::testing::job_spec(analysis, JobKind::Plan);
    spec.payer = user();
    let active = db
        .jobs()
        .enqueue(
            NewJob {
                spec,
                priority: contracts::job::spec::JobPriority::Batch,
                stage: JobStage::Plan { batch: 9 },
                generation: 0,
            },
            now,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET state='claimed' WHERE id=?")
        .bind(active.id.to_string())
        .execute(&db.pool)
        .await
        .unwrap();
    let t = db.jobs().payer_telemetry(&user(), now).await.unwrap();
    assert_eq!(t.tokens_today.unwrap().input_tokens, 100);
    assert_eq!(t.tokens_month.unwrap().input_tokens, 300);
    assert_eq!(t.live_containers.unwrap().len(), 1);
    let other = db
        .jobs()
        .payer_telemetry(&UserId("other".into()), now)
        .await
        .unwrap();
    assert!(other.tokens_today.is_none());
    assert_eq!(other.live_containers, Some(vec![]));
}
