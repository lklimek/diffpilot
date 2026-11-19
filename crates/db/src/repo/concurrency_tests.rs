//! Repository calls against a file-backed WAL database with a pool of several connections (as the server runs
//! it): a transaction that reads and then writes must wait for a concurrent writer, never fail.

use contracts::agent::JobKind;
use contracts::api::analyses::AnalysisState;
use contracts::api::feedback::{FeedbackTarget, FeedbackValue};
use contracts::api::review::ReviewState;
use contracts::ids::{FeedbackId, FlowId, QuestionId, StepId};
use contracts::job::result::{JobOutcome, JobResult};
use contracts::job::spec::JobPriority;
use serde_json::json;
use std::time::Duration;
use tempfile::TempDir;

use contracts::time::Timestamp;
use jiff::SignedDuration;

use super::Db;
use super::analyses::testing::{CREDENTIAL, job_spec, seed_analysis};
use super::documents::DocumentKind;
use super::feedback::{MemcanOutcome, NewFeedback};
use super::flow_work::NextFlow;
use super::jobs::{ClaimParams, JobStage, NewJob};
use super::questions::{NewQuestion, QuestionLimits};
use super::testing::file_backed;

fn at(second: i64) -> Timestamp {
    "2026-11-17T12:00:00Z"
        .parse::<Timestamp>()
        .unwrap_or_default()
        + SignedDuration::from_secs(second)
}

async fn file_db() -> (TempDir, Db) {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = file_backed(dir.path()).await.expect("file database");
    (dir, db)
}

async fn feedback_entry(db: &Db) -> (FeedbackId, contracts::ids::AnalysisId) {
    let analysis = seed_analysis(db).await.expect("seed");
    let feedback = db
        .feedback()
        .upsert(
            NewFeedback {
                id: FeedbackId::new_random(),
                analysis_id: analysis,
                user_id: "u1".to_owned(),
                target: FeedbackTarget::Step {
                    step_id: StepId("f1.1".to_owned()),
                },
                value: FeedbackValue::Overrated,
                note: None,
            },
            at(0),
        )
        .await
        .expect("feedback");
    (feedback.id, analysis)
}

fn failed(second: i64) -> MemcanOutcome {
    MemcanOutcome::Failed {
        retry_at: at(second + 30),
    }
}

/// The writer records a rejected MemCan write while another connection holds the write lock (a review-state PUT
/// in flight): the outcome waits for it and is stored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn feedback_outcome_waits_for_a_concurrent_writer() {
    let (_dir, db) = file_db().await;
    let (id, analysis) = feedback_entry(&db).await;
    let mut writer = super::write_tx(&db.pool).await.expect("other writer");
    sqlx::query("UPDATE analyses SET flows_done = flows_done WHERE id = ?")
        .bind(analysis.to_string())
        .execute(&mut *writer)
        .await
        .expect("write");
    let recording = tokio::spawn({
        let db = db.clone();
        async move {
            db.feedback()
                .record_memcan_outcome(id, 1, failed(1), at(1))
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    writer.commit().await.expect("commit");
    let recorded = recording.await.expect("task");
    assert!(
        recorded.is_ok(),
        "{recorded:?} (code {:?})",
        recorded
            .as_ref()
            .err()
            .and_then(|error| error.sqlite_code())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn feedback_outcomes_and_review_state_puts_run_concurrently() {
    let (_dir, db) = file_db().await;
    let (id, analysis) = feedback_entry(&db).await;
    for round in 0..40 {
        let state = ReviewState {
            ticked_steps: vec![StepId(format!("f1.{round}"))],
            last_viewed: None,
        };
        let (feedback, states) = (db.feedback(), db.review_states());
        let (outcome, put) = tokio::join!(
            feedback.record_memcan_outcome(id, 1, failed(round), at(round)),
            states.put("u1", analysis, &state, at(round)),
        );
        outcome.expect("outcome");
        put.expect("review state");
    }
}

async fn claimed_job(db: &Db, analysis: contracts::ids::AnalysisId, round: i64) -> JobResult {
    let job = db
        .jobs()
        .enqueue(
            NewJob {
                spec: job_spec(analysis, JobKind::AnalyseFlow),
                priority: JobPriority::Batch,
                stage: JobStage::Flow(FlowId(format!("f{round}"))),
                generation: 0,
            },
            at(round),
        )
        .await
        .expect("enqueue");
    let claimed = db
        .jobs()
        .claim(ClaimParams {
            credential_ids: &[CREDENTIAL.to_owned()],
            job_token_hash: b"hash",
            lease_until: at(round + 60),
            now: at(round),
        })
        .await
        .expect("claim")
        .expect("claimed");
    assert_eq!(claimed.id, job.id);
    JobResult {
        job_id: job.id,
        attempt: claimed.attempt,
        outcome: JobOutcome::Succeeded {
            output: json!({"flows": []}),
        },
        session: None,
        usage: None,
        skills_commit: None,
        llm_call: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn job_completion_and_retry_run_concurrently() {
    let (_dir, db) = file_db().await;
    let analysis = seed_analysis(&db).await.expect("seed");
    let failed = AnalysisState::Failed {
        reason: None,
        flow: None,
        message: "Planning failed.".to_owned(),
    };
    for round in 0..20 {
        let result = claimed_job(&db, analysis, round).await;
        db.analyses()
            .set_state(analysis, &failed, at(round))
            .await
            .expect("failed");
        let (jobs, analyses) = (db.jobs(), db.analyses());
        let (completed, retried) = tokio::join!(
            jobs.complete(&result, at(round)),
            analyses.retry(analysis, at(round)),
        );
        completed.expect("complete");
        assert!(retried.expect("retry"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn questions_and_job_completion_run_concurrently() {
    let (_dir, db) = file_db().await;
    let analysis = seed_analysis(&db).await.expect("seed");
    let limits = QuestionLimits {
        per_analysis: 100,
        per_user: 100,
        user_window_start: Timestamp::UNIX_EPOCH,
    };
    for round in 0..20 {
        let result = claimed_job(&db, analysis, round).await;
        let question = NewQuestion {
            id: QuestionId::new_random(),
            analysis_id: analysis,
            anchor: contracts::api::questions::AskAnchor::Step {
                step_id: StepId("f1.1".to_owned()),
                lines: None,
            },
            parent_id: None,
            user_id: "u1".to_owned(),
            text: format!("question {round}?"),
        };
        let (questions, jobs) = (db.questions(), db.jobs());
        let (asked, completed) = tokio::join!(
            questions.create(question, limits, at(round)),
            jobs.complete(&result, at(round)),
        );
        asked.expect("question");
        completed.expect("complete");
    }
}

/// Every connection of the pool runs with the same settings.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_pool_connection_has_the_settings() {
    let (_dir, db) = file_db().await;
    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(db.pool.acquire().await.expect("connection"));
    }
    for connection in &mut held {
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut **connection)
            .await
            .expect("journal_mode");
        let busy: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&mut **connection)
            .await
            .expect("busy_timeout");
        let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&mut **connection)
            .await
            .expect("synchronous");
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut **connection)
            .await
            .expect("foreign_keys");
        assert_eq!(
            (mode.as_str(), busy, synchronous, foreign_keys),
            ("wal", 5000, 2, 1),
            "journal_mode, busy_timeout ms, synchronous FULL, foreign_keys"
        );
    }
}

/// A user's Retry stored while the driver asks for the next flow is seen by that choice: it uses no automatic
/// retry of the flow.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flow_requested_while_the_driver_picks_it_keeps_its_automatic_retries() {
    let (_dir, db) = file_db().await;
    let analysis = seed_analysis(&db).await.expect("seed");
    let flow = FlowId("f1".to_owned());
    let plan = json!({ "flows": [{ "id": "f1", "importance_rank": 1 }], "unassigned": [] });
    db.documents()
        .put(analysis, DocumentKind::Plan, "", &plan, at(0))
        .await
        .expect("plan");
    db.flow_work()
        .fail(analysis, &flow, "Failed")
        .await
        .expect("fail");
    // The request's transaction is open (write lock held) when the driver picks.
    let mut request = super::write_tx(&db.pool).await.expect("request");
    sqlx::query("UPDATE flow_work SET requested_at = ? WHERE analysis_id = ?")
        .bind(super::jobs::ts(at(1)))
        .bind(analysis.to_string())
        .execute(&mut *request)
        .await
        .expect("requested");
    let driver = tokio::spawn({
        let db = db.clone();
        async move { db.flow_work().next(analysis).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    request.commit().await.expect("commit");
    assert_eq!(
        driver.await.expect("join").expect("next"),
        NextFlow::Run(flow.clone())
    );
    assert_eq!(
        super::flow_work::testing::retries(&db, analysis, &flow)
            .await
            .expect("retries"),
        (1, 0),
        "a new round, and no automatic retry used"
    );
}
