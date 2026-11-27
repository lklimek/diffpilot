use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use contracts::agent::JobKind;
use contracts::api::access_tokens::TokenScope;
use contracts::common::{LlmLanguage, RepoRef};
use contracts::ids::{AnalysisId, JobId, UserId};
use contracts::job::result::{JobOutcome, JobResult, LlmCallOutcome, LlmCallStatus};
use contracts::job::spec::{JobPriority, JobTarget};
use contracts::job::state::JobState;
use contracts::time::Timestamp;
use db::repo::DEFAULT_TENANT;
use db::repo::analyses::NewAnalysis;
use db::repo::analyses::testing::{
    job_spec, judge_spec, seed_analysis, seed_ranking_refresh, seed_tenant,
};
use db::repo::jobs::{ClaimParams, JobStage, NewJob};
use http_body_util::BodyExt;
use jiff::SignedDuration;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::api::router;
use crate::auth::access_tokens::stored_token;
use crate::state::{AppState, tests::test_state};
use crate::test_support::{other_user_cookie, session_cookie};

async fn setup() -> (AppState, AnalysisId, String, String) {
    let state = test_state().await;
    let analysis = seed_analysis(&state.db).await.unwrap();
    let reader = session_cookie(&state, DEFAULT_TENANT).await;
    let peer = other_user_cookie(&state).await;
    (state, analysis, reader, peer)
}

async fn enqueue(
    state: &AppState,
    analysis: AnalysisId,
    pr: i64,
    payer: &str,
    batch: u32,
) -> JobId {
    let mut spec = job_spec(analysis, JobKind::Plan);
    spec.payer = UserId(payer.to_owned());
    if let JobTarget::PullRequest(target) = &mut spec.target {
        target.pull_request_id = pr;
    }
    state
        .db
        .jobs()
        .enqueue(
            NewJob {
                spec,
                stage: JobStage::Plan { batch },
                priority: JobPriority::Interactive,
                generation: 0,
            },
            Timestamp::now(),
        )
        .await
        .unwrap()
        .id
}

async fn claim(state: &AppState, payer: &str, job: JobId) {
    let now = Timestamp::now();
    let record = state
        .db
        .jobs()
        .claim(ClaimParams {
            payer_ids: &[payer.to_owned()],
            job_token_hash: b"fake-hash",
            lease_until: now + SignedDuration::from_mins(5),
            now,
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.id, job);
}

async fn read(state: &AppState, cookie: &str, path: &str) -> Body {
    let response = router(state.clone())
        .oneshot(
            Request::get(path)
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.into_body()
}

async fn activity(state: &AppState, cookie: &str) -> Value {
    let body = read(state, cookie, "/api/v1/llm/activity").await;
    serde_json::from_slice(&body.collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn other_users_jobs_have_only_kind() {
    let (state, analysis, reader, peer) = setup().await;
    let running = enqueue(&state, analysis, 1, "u2", 1).await;
    claim(&state, "u2", running).await;
    let queued = enqueue(&state, analysis, 1, "u2", 2).await;
    let snapshot = activity(&state, &reader).await;
    for (list, real_id) in [("running", running), ("queued", queued)] {
        let jobs = snapshot[list].as_array().unwrap();
        assert_eq!(jobs.len(), 1);
        let opaque = jobs[0]["id"].as_str().unwrap();
        assert!(opaque.parse::<usize>().is_ok());
        assert_ne!(opaque, real_id.to_string());
        assert_eq!(
            jobs[0],
            json!({"owner":"other", "id":opaque, "kind":"analysis"})
        );
    }
    assert_ne!(snapshot["running"][0]["id"], snapshot["queued"][0]["id"]);

    let own = activity(&state, &peer).await;
    assert_eq!(own["running"][0]["id"], running.to_string());
    assert_eq!(
        own["running"][0]["subject"]["href"],
        format!("/pr/acme/node-client/482/analyses/{analysis}/progress")
    );
    assert!(own["running"][0]["since"].is_string());
    assert!(own["queued"][0]["queued_at"].is_string());
    assert_eq!(own["queued"][0]["waits_for"], json!({"kind":"runner"}));
    assert_eq!(
        state.db.jobs().get(queued).await.unwrap().state,
        JobState::Queued
    );
}

#[tokio::test]
async fn other_users_pause_end_not_exposed() {
    let (state, analysis, reader, peer) = setup().await;
    enqueue(&state, analysis, 1, "u2", 1).await;
    let now = Timestamp::from_second(Timestamp::now().as_second()).unwrap();
    let until = now + SignedDuration::from_hours(1);
    state.db.llm_credentials().pause("u2", until).await.unwrap();
    state
        .db
        .llm_credentials()
        .record_call(
            "u2",
            &LlmCallOutcome {
                at: now,
                status: LlmCallStatus::Working,
            },
        )
        .await
        .unwrap();

    let other = activity(&state, &reader).await;
    assert_eq!(other["queued"].as_array().unwrap().len(), 1);
    assert!(other["paused_until"].is_null());
    assert!(other["last_call"].is_null());
    let own = activity(&state, &peer).await;
    assert_eq!(own["paused_until"], json!(until));
    assert_eq!(own["last_call"]["status"]["state"], "working");
}

pub(super) async fn claim_foreign_analysis(state: &AppState, payer: &str) {
    let db = &state.db;
    let tenant = "other";
    seed_tenant(db, tenant).await.unwrap();
    let pr = db.pull_requests().get(1).await.unwrap().pr;
    let repo = db
        .repos()
        .upsert(
            tenant,
            &RepoRef {
                owner: "other-owner".to_owned(),
                name: pr.repo.name.clone(),
            },
            None,
        )
        .await
        .unwrap();
    let now = Timestamp::now();
    let pr_id = db
        .pull_requests()
        .upsert(tenant, repo.id, &pr, now)
        .await
        .unwrap();
    let foreign = db
        .analyses()
        .create(
            NewAnalysis {
                id: AnalysisId::new_random(),
                tenant_id: tenant.to_owned(),
                pull_request_id: pr_id,
                base_sha: pr.base_sha,
                head_sha: pr.head_sha,
                language: LlmLanguage::En,
                created_by: "u1".to_owned(),
            },
            now,
        )
        .await
        .unwrap()
        .id;
    let hidden = enqueue(state, foreign, pr_id, payer, 1).await;
    claim(state, payer, hidden).await;
}

#[tokio::test]
async fn other_tenants_jobs_not_listed() {
    let (state, analysis, reader, _) = setup().await;
    let own = enqueue(&state, analysis, 1, "u1", 1).await;

    claim_foreign_analysis(&state, "u2").await;

    let snapshot = activity(&state, &reader).await;
    assert_eq!(snapshot["running"], json!([]));
    assert_eq!(snapshot["queued"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["queued"][0]["id"], own.to_string());
    state
        .db
        .jobs()
        .request_cancel(own, Timestamp::now())
        .await
        .unwrap();
    assert_eq!(activity(&state, &reader).await["queued"], json!([]));
}

#[tokio::test]
async fn waits_for_usage_limit_until_own_only() {
    let (state, analysis, reader, peer) = setup().await;
    let job = enqueue(&state, analysis, 1, "u1", 1).await;
    claim(&state, "u1", job).await;
    let now = Timestamp::from_second(Timestamp::now().as_second()).unwrap();
    let until = now + SignedDuration::from_hours(1);
    state
        .db
        .jobs()
        .complete(
            &JobResult {
                assessment: None,
                report: None,
                job_id: job,
                attempt: 1,
                outcome: JobOutcome::UsageLimited {
                    resets_at: Some(until),
                },
                session: None,
                usage: None,
                skills_commit: None,
                llm_call: None,
            },
            now,
        )
        .await
        .unwrap();
    let queued = enqueue(&state, analysis, 1, "u1", 2).await;

    let snapshot = activity(&state, &reader).await;
    assert_eq!(snapshot["running"], json!([]));
    assert_eq!(snapshot["queued"].as_array().unwrap().len(), 2);
    for entry in snapshot["queued"].as_array().unwrap() {
        assert_eq!(
            entry["waits_for"],
            json!({"kind":"usage_limit", "until":until})
        );
    }
    assert_eq!(
        state.db.jobs().get(job).await.unwrap().state,
        JobState::Paused
    );
    assert_eq!(
        state.db.jobs().get(queued).await.unwrap().state,
        JobState::Queued
    );
    let other = activity(&state, &peer).await;
    assert_eq!(other["queued"].as_array().unwrap().len(), 2);
    assert!(
        other["queued"]
            .as_array()
            .unwrap()
            .iter()
            .all(|job| job.get("waits_for").is_none())
    );
}

#[tokio::test]
async fn ranking_reports_analysis_only_when_it_occupies_the_payers_slots() {
    let (state, analysis, reader, _) = setup().await;
    let refresh = seed_ranking_refresh(&state.db).await.unwrap();
    let mut spec = judge_spec(refresh);
    spec.payer = UserId("u1".to_owned());
    state
        .db
        .jobs()
        .enqueue(
            NewJob {
                spec,
                stage: JobStage::Judge { refresh, batch: 1 },
                priority: JobPriority::Batch,
                generation: 0,
            },
            Timestamp::now(),
        )
        .await
        .unwrap();

    assert_eq!(
        activity(&state, &reader).await["queued"][0]["waits_for"],
        json!({"kind":"runner"}),
        "a ranking without an active analysis waits for a runner"
    );
    claim_foreign_analysis(&state, "u2").await;
    assert_eq!(
        activity(&state, &reader).await["queued"][0]["waits_for"],
        json!({"kind":"runner"}),
        "an active analysis in another tenant does not affect the wait"
    );

    state
        .db
        .settings()
        .set_llm_job_slots("u2", std::num::NonZeroU16::new(2).unwrap())
        .await
        .unwrap();
    let peer_job = enqueue(&state, analysis, 1, "u2", 2).await;
    claim(&state, "u2", peer_job).await;
    assert_eq!(
        activity(&state, &reader).await["queued"][0]["waits_for"],
        json!({"kind":"runner"}),
        "another payer in the same tenant uses its own slots"
    );

    let job = enqueue(&state, analysis, 1, "u1", 1).await;
    claim(&state, "u1", job).await;
    let snapshot = activity(&state, &reader).await;
    let ranking = &snapshot["queued"][0];
    assert_eq!(ranking["kind"], "ranking");
    assert_eq!(ranking["subject"], json!({"label":"My work","href":"/"}));
    assert_eq!(ranking["waits_for"], json!({"kind":"analysis_running"}));
    state
        .db
        .settings()
        .set_llm_job_slots("u1", std::num::NonZeroU16::new(2).unwrap())
        .await
        .unwrap();
    assert_eq!(
        activity(&state, &reader).await["queued"][0]["waits_for"],
        json!({"kind":"runner"}),
        "a free payer slot leaves the ranking eligible"
    );
}

#[tokio::test]
async fn activity_stream_starts_with_the_current_snapshot_on_every_connection() {
    let (state, analysis, reader, _) = setup().await;
    let mut first = read(&state, &reader, "/api/v1/events").await;
    assert_eq!(
        next_activity(&mut first).await["activity"]["queued"],
        json!([])
    );
    drop(first);

    let job = enqueue(&state, analysis, 1, "u1", 1).await;
    let mut reconnected = read(&state, &reader, "/api/v1/events").await;
    let snapshot = next_activity(&mut reconnected).await;
    assert_eq!(snapshot["activity"], activity(&state, &reader).await);
    assert_eq!(snapshot["activity"]["queued"][0]["id"], job.to_string());
}

#[tokio::test]
async fn activity_event_on_job_state_change_throttled() {
    let (state, analysis, _, _) = setup().await;
    let (token, _) = stored_token(
        &state.db,
        "u1",
        vec![TokenScope::Read],
        Timestamp::now() + SignedDuration::from_hours(1),
    )
    .await;
    let app = router(state.clone());
    let unauthorized = app
        .clone()
        .oneshot(
            Request::get("/api/v1/llm/activity")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let read_as_token = |path: &str| {
        app.clone().oneshot(
            Request::get(path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
    };
    let get = read_as_token("/api/v1/llm/activity").await.unwrap();
    assert_eq!(get.status(), StatusCode::OK);
    let stream = read_as_token("/api/v1/events").await.unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    let mut events = stream.into_body();
    assert_eq!(
        next_activity(&mut events).await["activity"]["queued"],
        json!([])
    );
    let connected_at = tokio::time::Instant::now();
    let job = enqueue(&state, analysis, 1, "u2", 1).await;
    let first = next_activity(&mut events).await;
    assert!(connected_at.elapsed() >= Duration::from_millis(950));
    assert_eq!(
        first["activity"]["queued"][0],
        json!({"owner":"other","id":"0","kind":"analysis"})
    );

    let sent_at = tokio::time::Instant::now();
    claim(&state, "u2", job).await;
    let cancelled = enqueue(&state, analysis, 1, "u2", 2).await;
    state
        .db
        .jobs()
        .request_cancel(cancelled, Timestamp::now())
        .await
        .unwrap();
    let second = next_activity(&mut events).await;
    assert!(sent_at.elapsed() >= Duration::from_millis(950));
    assert_eq!(
        second["activity"]["running"][0],
        json!({"owner":"other","id":"0","kind":"analysis"})
    );
    assert_eq!(second["activity"]["queued"], json!([]));

    state
        .db
        .jobs()
        .record_output(
            job,
            1,
            &json!({}),
            Timestamp::now() + SignedDuration::from_mins(5),
        )
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(1150), events.frame())
            .await
            .is_err(),
        "a peer's transition within the same list leaves the visible snapshot unchanged"
    );
}

async fn next_activity(events: &mut Body) -> Value {
    let frame = tokio::time::timeout(Duration::from_secs(3), events.frame())
        .await
        .expect("activity event deadline")
        .unwrap()
        .unwrap();
    let bytes = frame.into_data().unwrap();
    let wire = std::str::from_utf8(&bytes).unwrap();
    assert!(wire.starts_with("event: llm_activity\n"), "{wire}");
    let data = wire
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap();
    let event: Value = serde_json::from_str(data).unwrap();
    assert_eq!(event["event"], "llm_activity");
    event
}
