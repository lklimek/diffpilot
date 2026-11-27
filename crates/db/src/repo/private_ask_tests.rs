//! Private PR Ask has no analysis/pass dependency and survives reopening its database.
use super::*;
use contracts::agent::JobKind;
use contracts::ids::{QuestionId, UserId};
use contracts::job::spec::JobTarget;
use contracts::time::Timestamp;
use jobs::{ClaimParams, JobStage, NewJob};

async fn seed(db: &Db) -> QuestionId {
    analyses::testing::seed_analysis(db).await.unwrap();
    analyses::testing::seed_payer(db, "u2").await.unwrap();
    let mut tx = write_tx(&db.pool).await.unwrap();
    sqlx::raw_sql("DELETE FROM analyses;
        INSERT INTO comment_prs(pull_request_id,tenant_id,created_at) VALUES(1,'default','t');
        INSERT INTO comment_items(id,tenant_id,pull_request_id,item_no,kind,stable_key,author,author_kind,title,body,body_hash,first_seen_head,github,state,created_at,updated_at)
        VALUES('11111111-1111-4111-8111-111111111111','default',1,1,'thread','thread','bot','bot','Retry','Check retry','hash','h','{}','undecided','t','t');")
        .execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let id = QuestionId::new_random();
    db.questions()
        .create_private_for_pr(
            questions::NewPrQuestion {
                id,
                pull_request_id: 1,
                item: serde_json::from_value(serde_json::json!(
                    "11111111-1111-4111-8111-111111111111"
                ))
                .unwrap(),
                parent_id: None,
                user_id: "u1".into(),
                text: "Private question".into(),
            },
            questions::PrQuestionLimits {
                per_pull_request: 100,
                per_user: 30,
                user_window_start: Timestamp::now(),
            },
            Timestamp::now(),
        )
        .await
        .unwrap();
    id
}

fn job(question: QuestionId, payer: &str) -> NewJob {
    let mut spec =
        analyses::testing::job_spec(contracts::ids::AnalysisId::new_random(), JobKind::Ask);
    spec.target = serde_json::from_value::<JobTarget>(serde_json::json!({
        "type":"pull_request_ask", "pull_request_id":1,
        "repo":{"clone_url":"file:///fake", "base_sha":"b", "head_sha":"h"}
    }))
    .expect("standalone private PR Ask target");
    spec.payer = UserId(payer.into());
    NewJob {
        spec,
        stage: JobStage::Ask(question),
        priority: contracts::job::spec::JobPriority::Interactive,
        generation: 0,
    }
}

#[tokio::test]
async fn private_ask_without_analysis_is_owned_idempotent_and_durable_without_quota() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("private.sqlite");
    let db = Db::open(&path).await.unwrap();
    let question = seed(&db).await;
    let now: Timestamp = "2026-11-23T12:00:00Z".parse().unwrap();
    let mut tx = write_tx(&db.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO assessment_daily_usage(user_id,day_utc,passes) VALUES('u1','2026-11-23',80)",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let before = db
        .assessments()
        .quota(&UserId("u1".into()), now)
        .await
        .unwrap();
    assert_eq!(before.remaining, 0);
    assert!(
        db.jobs()
            .enqueue(job(question, "u2"), Timestamp::now())
            .await
            .is_err(),
        "another payer cannot queue this question"
    );
    let queued = db
        .jobs()
        .enqueue(job(question, "u1"), Timestamp::now())
        .await
        .unwrap();
    assert!(
        db.jobs()
            .enqueue(job(question, "u1"), Timestamp::now())
            .await
            .is_err(),
        "submit is idempotent per private question"
    );
    assert_eq!(
        db.jobs()
            .current_for_private_question(question, &UserId("u1".into()), 0)
            .await
            .unwrap()
            .unwrap()
            .id,
        queued.id
    );
    assert!(
        db.jobs()
            .current_for_private_question(question, &UserId("u2".into()), 0)
            .await
            .unwrap()
            .is_none()
    );
    db.questions().set_job(question, queued.id).await.unwrap();
    assert!(
        db.questions()
            .list_for_pull_request(1, None, "u2", Timestamp::now())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        db.assessments()
            .quota(&UserId("u1".into()), now)
            .await
            .unwrap(),
        before
    );
    assert!(db.review_loop().live_passes().await.unwrap().is_empty());
    drop(db);
    let db = Db::open(&path).await.unwrap();
    assert_eq!(
        db.questions()
            .unanswered(Timestamp::now())
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        db.jobs()
            .get(queued.id)
            .await
            .unwrap()
            .spec
            .target
            .analysis_id(),
        None
    );
    let claim = db
        .jobs()
        .claim(ClaimParams {
            payer_ids: &["u1".into()],
            job_token_hash: b"fake",
            lease_until: Timestamp::now() + jiff::SignedDuration::from_mins(1),
            now: Timestamp::now(),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.id, queued.id);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM analyses")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM assessment_passes")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn private_ask_checkpoint_preserves_verify_kind_and_payer_across_restart() {
    use contracts::job::result::{JobOutcome, JobResult, SessionOutcome, SessionReport};
    use contracts::job::spec::WorkingSession;
    use contracts::runtime::{AskPreflight, PreflightState, SessionSeed, ThreadKind};
    for kind in [ThreadKind::Ask, ThreadKind::Verify] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.sqlite");
        let db = Db::open(&path).await.unwrap();
        let question = seed(&db).await;
        assert!(
            db.questions()
                .next_unsent_for_pr(1, Timestamp::now())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            db.questions()
                .pull_requests_with_unsent()
                .await
                .unwrap()
                .is_empty()
        );
        db.jobs()
            .enqueue(job(question, "u1"), Timestamp::now())
            .await
            .unwrap();
        let claimed = db
            .jobs()
            .claim(ClaimParams {
                payer_ids: &["u1".into()],
                job_token_hash: b"fake",
                lease_until: Timestamp::now() + jiff::SignedDuration::from_mins(1),
                now: Timestamp::now(),
            })
            .await
            .unwrap()
            .unwrap();
        let preflight = AskPreflight {
            seed: SessionSeed::ContextPack,
            generation: 0,
            state: PreflightState::NotNeeded,
        };
        let mut spec = claimed.spec;
        spec.working_session = Some(WorkingSession {
            kind: Some(kind),
            preflight: Some(preflight.clone()),
            parent_context: None,
            user_key: "a".repeat(64),
            analytic_generation: 0,
            thread_id: None,
        });
        db.jobs().save_session_route(&spec).await.unwrap();
        let result = JobResult {
            job_id: claimed.id,
            attempt: claimed.attempt,
            outcome: JobOutcome::Succeeded {
                output: serde_json::json!({}),
            },
            session: Some(SessionReport {
                thread_id: "private-durable".into(),
                outcome: SessionOutcome::Resumed,
                preflight: Some(Box::new(preflight)),
                last_context: None,
            }),
            assessment: None,
            report: None,
            usage: None,
            llm_call: None,
            skills_commit: None,
        };
        db.jobs().complete(&result, Timestamp::now()).await.unwrap();
        drop(db);
        let db = Db::open(&path).await.unwrap();
        assert!(matches!(
            db.jobs()
                .get(result.job_id)
                .await
                .unwrap()
                .result
                .unwrap()
                .outcome,
            JobOutcome::Succeeded { .. }
        ));
        if kind == ThreadKind::Ask {
            assert!(
                db.container_sessions()
                    .working(1, &UserId("u1".into()), ThreadKind::Verify)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                db.container_sessions()
                    .working(1, &UserId("u1".into()), ThreadKind::Ask)
                    .await
                    .unwrap()
                    .is_none()
            );
            continue;
        }
        let working = db
            .container_sessions()
            .working(1, &UserId("u1".into()), ThreadKind::Verify)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(working.thread_id, "private-durable");
        assert!(
            db.container_sessions()
                .working(1, &UserId("u1".into()), ThreadKind::Ask)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            db.container_sessions()
                .working(1, &UserId("u2".into()), ThreadKind::Verify)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            db.container_sessions()
                .analytic(1, &UserId("u1".into()))
                .await
                .unwrap()
                .is_none()
        );
    }
}
