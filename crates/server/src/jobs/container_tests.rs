//! Container routing contracts, exercised without a provider or external services.
use super::*;
use contracts::api::questions::AskAnchor;
use contracts::ids::{QuestionId, UserId};
use contracts::job::result::{JobOutcome, JobResult, SessionOutcome, SessionReport};
use contracts::job::spec::{JobPriority, JobSpec};
use contracts::runtime::{AskPreflight, LastContext, PreflightState, SessionSeed, ThreadKind};
use db::repo::analyses::testing::{job_spec, seed_analysis, seed_payer};
use db::repo::container_sessions::{AnalyticThread, WorkingThread};
use db::repo::jobs::NewJob;
use db::repo::questions::{NewQuestion, QuestionLimits};
use futures::FutureExt;

struct Packs;
impl ContextPacks for Packs {
    fn pack<'a>(
        &'a self,
        _: AnalysisId,
        _: &'a JobStage,
    ) -> BoxFuture<'a, Result<Option<String>, ContextPackError>> {
        async { Ok(Some("private stored context".into())) }.boxed()
    }
}

async fn ask_job(db: &Db, user: &str) -> JobRecord {
    let analysis = seed_analysis(db).await.unwrap();
    seed_payer(db, user).await.unwrap();
    let question = QuestionId::new_random();
    db.questions()
        .create(
            NewQuestion {
                id: question,
                analysis_id: analysis,
                anchor: AskAnchor::Pr,
                parent_id: None,
                user_id: user.into(),
                text: "What changed?".into(),
            },
            QuestionLimits {
                per_analysis: 100,
                per_user: 100,
                user_window_start: Timestamp::now() - SignedDuration::from_hours(1),
            },
            Timestamp::now(),
        )
        .await
        .unwrap();
    let mut spec = job_spec(analysis, contracts::agent::JobKind::Ask);
    spec.payer = UserId(user.into());
    db.jobs()
        .enqueue(
            NewJob {
                spec,
                priority: JobPriority::Interactive,
                stage: JobStage::Ask(question),
                generation: 0,
            },
            Timestamp::now(),
        )
        .await
        .unwrap()
}

async fn analytic(db: &Db, tokens: Option<u64>) {
    db.container_sessions()
        .save_analytic(
            1,
            &UserId("u1".into()),
            0,
            &AnalyticThread {
                thread_id: "analytic-owner".into(),
                generation: 7,
                last_context: tokens.map(|tokens| LastContext {
                    tokens,
                    generation: 7,
                }),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn container_ask_never_forks_another_payers_history() {
    let db = Db::in_memory().await.unwrap();
    let job = ask_job(&db, "u2").await;
    analytic(&db, Some(90_000)).await;
    let spec = prepare_spec(&db, &Packs, &job).await.unwrap();
    assert!(
        spec.session.is_none(),
        "another payer uses only the private context pack"
    );
    let working = spec.working_session.unwrap();
    assert_eq!(working.kind, Some(ThreadKind::Ask));
    assert_eq!(
        working.preflight.unwrap(),
        AskPreflight {
            seed: SessionSeed::ContextPack,
            generation: 7,
            state: PreflightState::NotNeeded
        }
    );
    assert_eq!(spec.context_pack.as_deref(), Some("private stored context"));
}

#[tokio::test]
async fn container_first_ask_preflight_uses_context_not_billable_usage() {
    for (tokens, expected) in [
        (Some(80_000), PreflightState::NotNeeded),
        (Some(80_001), PreflightState::Pending),
        (None, PreflightState::Pending),
    ] {
        let db = Db::in_memory().await.unwrap();
        let job = ask_job(&db, "u1").await;
        analytic(&db, tokens).await;
        let spec = prepare_spec(&db, &Packs, &job).await.unwrap();
        assert_eq!(spec.session.unwrap().thread_id, "analytic-owner");
        let working = spec.working_session.unwrap();
        assert_eq!(working.analytic_generation, 7);
        assert_eq!(working.parent_context.map(|c| c.tokens), tokens);
        assert_eq!(working.preflight.unwrap().state, expected);
    }
}

#[tokio::test]
async fn container_ask_reuses_acknowledged_preflight_and_reforks_on_generation_change() {
    let db = Db::in_memory().await.unwrap();
    let job = ask_job(&db, "u1").await;
    analytic(&db, Some(100_000)).await;
    db.container_sessions()
        .seed_working(
            1,
            &job.spec.payer,
            None,
            &WorkingThread {
                thread_id: "private".into(),
                kind: ThreadKind::Ask,
                preflight: AskPreflight {
                    seed: SessionSeed::Fork,
                    generation: 7,
                    state: PreflightState::Done,
                },
            },
        )
        .await
        .unwrap();
    let spec = prepare_spec(&db, &Packs, &job).await.unwrap();
    assert_eq!(
        spec.working_session.as_ref().unwrap().thread_id.as_deref(),
        Some("private")
    );
    assert_eq!(
        spec.working_session.unwrap().preflight.unwrap().state,
        PreflightState::Done
    );
    db.analyses()
        .set_session(job.analysis_id.unwrap(), "advanced", None)
        .await
        .unwrap();
    let spec = prepare_spec(&db, &Packs, &job).await.unwrap();
    assert!(spec.working_session.unwrap().thread_id.is_none());
}

#[tokio::test]
async fn container_context_pack_working_thread_survives_without_an_analytic_thread() {
    let db = Db::in_memory().await.unwrap();
    let job = ask_job(&db, "u2").await;
    let preflight = AskPreflight {
        seed: SessionSeed::ContextPack,
        generation: 0,
        state: PreflightState::NotNeeded,
    };
    db.container_sessions()
        .seed_working(
            1,
            &job.spec.payer,
            None,
            &WorkingThread {
                thread_id: "private-pack".into(),
                kind: ThreadKind::Ask,
                preflight: preflight.clone(),
            },
        )
        .await
        .unwrap();
    let spec = prepare_spec(&db, &Packs, &job).await.unwrap();
    let working = spec.working_session.unwrap();
    assert_eq!(working.thread_id.as_deref(), Some("private-pack"));
    assert_eq!(working.preflight, Some(preflight));
}

fn result(spec: &JobSpec, session: SessionReport) -> JobResult {
    JobResult {
        job_id: spec.job_id,
        attempt: 1,
        assessment: None,
        report: None,
        outcome: JobOutcome::Succeeded {
            output: serde_json::json!({}),
        },
        session: Some(session),
        usage: None,
        skills_commit: None,
        llm_call: None,
    }
}

#[tokio::test]
async fn container_result_keeps_last_context_separate_from_generation_and_usage() {
    let db = Db::in_memory().await.unwrap();
    let analysis = seed_analysis(&db).await.unwrap();
    let spec = job_spec(analysis, contracts::agent::JobKind::Plan);
    db.jobs()
        .enqueue(
            NewJob {
                spec: spec.clone(),
                priority: JobPriority::Batch,
                stage: JobStage::Plan { batch: 1 },
                generation: 0,
            },
            Timestamp::now(),
        )
        .await
        .unwrap();
    db.jobs()
        .claim(ClaimParams {
            payer_ids: std::slice::from_ref(&spec.payer.0),
            job_token_hash: &[1],
            lease_until: lease_until(Timestamp::now()),
            now: Timestamp::now(),
        })
        .await
        .unwrap()
        .unwrap();
    db.jobs()
        .complete(
            &result(
                &spec,
                SessionReport {
                    thread_id: "analytic".into(),
                    outcome: SessionOutcome::Started,
                    last_context: Some(Box::new(LastContext {
                        tokens: 90_000,
                        generation: 0,
                    })),
                    preflight: None,
                },
            ),
            Timestamp::now(),
        )
        .await
        .unwrap();
    let thread = db
        .container_sessions()
        .analytic(1, &spec.payer)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        thread.last_context,
        Some(LastContext {
            tokens: 90_000,
            generation: thread.generation
        })
    );
}

async fn link(db: &Db, user: &str) {
    db.llm_credentials()
        .put_status(
            db::repo::DEFAULT_TENANT,
            user,
            &contracts::gateway::CredentialStatus {
                telemetry: Default::default(),
                challenge: None,
                mode: contracts::gateway::CredentialMode::ChatgptPlan,
                state: contracts::gateway::CredentialState::Linked,
                relink_reason: None,
                plan: None,
                label: None,
                paused_until: None,
                last_call: None,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn container_claim_uses_gateway_status_without_runner_credentials() {
    let db = Db::in_memory().await.unwrap();
    let job = ask_job(&db, "u1").await;
    let dir = tempfile::tempdir().unwrap();
    let github = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(wiremock::ResponseTemplate::new(404))
        .mount(&github)
        .await;
    let mut config = crate::config::tests::test_config();
    config.github = crate::github::app_token::tests::github_config(
        &github,
        crate::github::app_token::tests::write_key(dir.path()),
    );
    let request = ClaimRequest {
        runner_id: "fake".into(),
        credentials: vec![contracts::job::protocol::CredentialStatus {
            credential: contracts::job::spec::LlmCredentialRef {
                id: "default".into(),
                kind: contracts::job::spec::CredentialKind::CodexChatgpt,
            },
            usable: true,
        }],
        dropped_sessions: Vec::new(),
    };
    let config = Arc::new(config);
    let unlinked = claim(&db, &config, &Packs, &request, Timestamp::now())
        .await
        .unwrap();
    assert!(unlinked.claim.is_none());
    assert!(
        db.llm_credentials()
            .status("default")
            .await
            .unwrap()
            .is_none()
    );
    link(&db, "u1").await;
    let reply = claim(&db, &config, &Packs, &request, Timestamp::now())
        .await
        .unwrap();
    let claimed = reply.claim.unwrap();
    assert_eq!(claimed.spec.job_id, job.id);
    assert_eq!(claimed.spec.payer.0, "u1");
}

#[tokio::test]
async fn container_nonpayer_context_pack_is_invalidated_by_shared_analysis_progress() {
    let db = Db::in_memory().await.unwrap();
    let job = ask_job(&db, "u2").await;
    analytic(&db, Some(90_000)).await;
    db.container_sessions()
        .seed_working(
            1,
            &job.spec.payer,
            None,
            &WorkingThread {
                thread_id: "old-pack".into(),
                kind: ThreadKind::Ask,
                preflight: AskPreflight {
                    seed: SessionSeed::ContextPack,
                    generation: 6,
                    state: PreflightState::NotNeeded,
                },
            },
        )
        .await
        .unwrap();
    let spec = prepare_spec(&db, &Packs, &job).await.unwrap();
    assert!(spec.session.is_none());
    let working = spec.working_session.unwrap();
    assert_eq!(working.analytic_generation, 7);
    assert!(working.thread_id.is_none());
}

#[tokio::test]
async fn container_result_requires_first_ask_acknowledgement_bound_to_seed() {
    for (seed, state) in [
        (SessionSeed::Fork, PreflightState::Pending),
        (SessionSeed::Fork, PreflightState::NotNeeded),
        (SessionSeed::Fork, PreflightState::Done),
        (SessionSeed::ContextPack, PreflightState::NotNeeded),
    ] {
        let db = Db::in_memory().await.unwrap();
        let job = ask_job(&db, "u1").await;
        analytic(&db, Some(90_000)).await;
        let mut spec = prepare_spec(&db, &Packs, &job).await.unwrap();
        spec.attempt = 1;
        db.jobs()
            .claim(ClaimParams {
                payer_ids: &["u1".into()],
                job_token_hash: &[1],
                lease_until: lease_until(Timestamp::now()),
                now: Timestamp::now(),
            })
            .await
            .unwrap()
            .unwrap();
        db.jobs().save_session_route(&spec).await.unwrap();
        let preflight = AskPreflight {
            seed,
            generation: 7,
            state,
        };
        let outcome = db
            .jobs()
            .complete(
                &result(
                    &spec,
                    SessionReport {
                        thread_id: "fork".into(),
                        outcome: SessionOutcome::Started,
                        last_context: None,
                        preflight: Some(Box::new(preflight.clone())),
                    },
                ),
                Timestamp::now(),
            )
            .await;
        if seed == SessionSeed::Fork && state != PreflightState::Done {
            outcome.unwrap();
            let stored = db.jobs().get(spec.job_id).await.unwrap();
            assert_eq!(
                stored.state,
                contracts::job::state::JobState::Failed,
                "{state:?}"
            );
            let failure = stored.result.unwrap().outcome;
            assert_eq!(
                serde_json::to_value(&failure).unwrap()["kind"],
                "ask_compaction_unconfirmed"
            );
            let JobOutcome::Failed { kind, .. } = failure else {
                panic!("failure expected")
            };
            assert!(!kind.retryable());
            assert_eq!(
                crate::api::settings::plain_failure(Some(db::repo::settings::CallFailure::Failed(
                    kind
                ))),
                "The answer was discarded because the required context shortening was not confirmed."
            );
            assert!(
                db.container_sessions()
                    .working(1, &spec.payer, ThreadKind::Ask)
                    .await
                    .unwrap()
                    .is_none()
            );
        } else {
            outcome.unwrap();
            assert_eq!(
                db.container_sessions()
                    .working(1, &spec.payer, ThreadKind::Ask)
                    .await
                    .unwrap()
                    .unwrap()
                    .preflight,
                preflight
            );
        }
    }
}

#[tokio::test]
async fn container_admission_requires_linked_payer_and_pause_stays_payer_scoped() {
    let db = Db::in_memory().await.unwrap();
    let analysis = seed_analysis(&db).await.unwrap();
    let new_job = NewJob {
        spec: job_spec(analysis, contracts::agent::JobKind::Plan),
        priority: JobPriority::Batch,
        stage: JobStage::Plan { batch: 1 },
        generation: 0,
    };
    let refused = db
        .jobs()
        .enqueue_linked(new_job.clone(), Timestamp::now())
        .await
        .unwrap_err();
    assert_eq!(refused.to_string(), "Link your Codex account in Settings");
    assert!(
        db.jobs()
            .list_for_analysis(analysis, &[])
            .await
            .unwrap()
            .is_empty()
    );
    link(&db, "u1").await;
    db.jobs()
        .enqueue_linked(new_job, Timestamp::now())
        .await
        .unwrap();
    let until = Timestamp::now() + SignedDuration::from_hours(1);
    db.llm_credentials().pause("u1", until).await.unwrap();
    assert!(
        db.jobs()
            .claim(ClaimParams {
                payer_ids: &["u1".into()],
                job_token_hash: &[1],
                lease_until: lease_until(Timestamp::now()),
                now: Timestamp::now()
            })
            .await
            .unwrap()
            .is_none()
    );
    seed_payer(&db, "u2").await.unwrap();
    link(&db, "u2").await;
    let other = ask_job(&db, "u2").await;
    assert_eq!(
        db.jobs()
            .claim(ClaimParams {
                payer_ids: &["u1".into(), "u2".into()],
                job_token_hash: &[2],
                lease_until: lease_until(Timestamp::now()),
                now: Timestamp::now()
            })
            .await
            .unwrap()
            .unwrap()
            .id,
        other.id
    );
}

#[tokio::test]
async fn container_unknown_parent_context_can_be_recovered_below_compaction_threshold() {
    let db = Db::in_memory().await.unwrap();
    let job = ask_job(&db, "u1").await;
    analytic(&db, None).await;
    let mut spec = prepare_spec(&db, &Packs, &job).await.unwrap();
    spec.attempt = 1;
    assert_eq!(
        spec.working_session
            .as_ref()
            .unwrap()
            .preflight
            .as_ref()
            .unwrap()
            .state,
        PreflightState::Pending
    );
    db.jobs()
        .claim(ClaimParams {
            payer_ids: &["u1".into()],
            job_token_hash: &[1],
            lease_until: lease_until(Timestamp::now()),
            now: Timestamp::now(),
        })
        .await
        .unwrap()
        .unwrap();
    db.jobs().save_session_route(&spec).await.unwrap();
    let preflight = AskPreflight {
        seed: SessionSeed::Fork,
        generation: 7,
        state: PreflightState::NotNeeded,
    };
    db.jobs()
        .complete(
            &result(
                &spec,
                SessionReport {
                    thread_id: "recovered-small-fork".into(),
                    outcome: SessionOutcome::Started,
                    last_context: Some(Box::new(LastContext {
                        tokens: 50_000,
                        generation: 7,
                    })),
                    preflight: Some(Box::new(preflight.clone())),
                },
            ),
            Timestamp::now(),
        )
        .await
        .unwrap();
    assert_eq!(
        db.container_sessions()
            .working(1, &spec.payer, ThreadKind::Ask)
            .await
            .unwrap()
            .unwrap()
            .preflight,
        preflight
    );
}

#[tokio::test]
async fn container_missing_preflight_stores_answer_without_checkpoint() {
    let db = Db::in_memory().await.unwrap();
    let job = ask_job(&db, "u1").await;
    analytic(&db, None).await;
    let mut spec = prepare_spec(&db, &Packs, &job).await.unwrap();
    spec.attempt = 1;
    assert_eq!(
        spec.working_session
            .as_ref()
            .unwrap()
            .preflight
            .as_ref()
            .unwrap()
            .state,
        PreflightState::Pending
    );
    db.jobs()
        .claim(ClaimParams {
            payer_ids: &["u1".into()],
            job_token_hash: &[1],
            lease_until: lease_until(Timestamp::now()),
            now: Timestamp::now(),
        })
        .await
        .unwrap()
        .unwrap();
    db.jobs().save_session_route(&spec).await.unwrap();
    let mut answer = result(
        &spec,
        SessionReport {
            thread_id: "recovered-small-fork".into(),
            outcome: SessionOutcome::Started,
            last_context: Some(Box::new(LastContext {
                tokens: 50_000,
                generation: 7,
            })),
            preflight: None,
        },
    );
    answer.outcome = JobOutcome::Succeeded {
        output: serde_json::json!({"answer":"The retry is bounded.", "evidence":[]}),
    };
    db.jobs().complete(&answer, Timestamp::now()).await.unwrap();
    let stored = db.jobs().get(spec.job_id).await.unwrap();
    assert_eq!(stored.result.unwrap().outcome, answer.outcome);
    assert!(
        db.container_sessions()
            .working(1, &spec.payer, ThreadKind::Ask)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn container_required_compaction_without_acknowledgement_cannot_store_answer() {
    for checkpoint in [false, true] {
        let db = Db::in_memory().await.unwrap();
        let job = ask_job(&db, "u1").await;
        analytic(&db, Some(90_000)).await;
        let mut spec = prepare_spec(&db, &Packs, &job).await.unwrap();
        spec.attempt = 1;
        db.jobs()
            .claim(ClaimParams {
                payer_ids: &["u1".into()],
                job_token_hash: &[1],
                lease_until: lease_until(Timestamp::now()),
                now: Timestamp::now(),
            })
            .await
            .unwrap()
            .unwrap();
        db.jobs().save_session_route(&spec).await.unwrap();
        let mut answer = result(
            &spec,
            SessionReport {
                thread_id: "fork".into(),
                outcome: SessionOutcome::Started,
                last_context: None,
                preflight: None,
            },
        );
        if !checkpoint {
            answer.session = None;
        }
        db.jobs()
            .record_output(
                spec.job_id,
                1,
                &serde_json::json!({"answer":"must be discarded"}),
                lease_until(Timestamp::now()),
            )
            .await
            .unwrap();
        db.jobs().complete(&answer, Timestamp::now()).await.unwrap();
        let stored = db.jobs().get(spec.job_id).await.unwrap();
        assert_eq!(stored.state, contracts::job::state::JobState::Failed);
        assert_eq!(
            serde_json::to_value(stored.result.unwrap().outcome).unwrap()["kind"],
            "ask_compaction_unconfirmed"
        );
        assert!(stored.ready_output.is_none());
        assert!(
            db.container_sessions()
                .working(1, &spec.payer, ThreadKind::Ask)
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn container_checkpoint_metadata_mismatch_keeps_compacted_answer() {
    let db = Db::in_memory().await.unwrap();
    let job = ask_job(&db, "u1").await;
    analytic(&db, Some(90_000)).await;
    let mut spec = prepare_spec(&db, &Packs, &job).await.unwrap();
    spec.attempt = 1;
    db.jobs()
        .claim(ClaimParams {
            payer_ids: &["u1".into()],
            job_token_hash: &[1],
            lease_until: lease_until(Timestamp::now()),
            now: Timestamp::now(),
        })
        .await
        .unwrap()
        .unwrap();
    db.jobs().save_session_route(&spec).await.unwrap();
    let answer = result(
        &spec,
        SessionReport {
            thread_id: "fork".into(),
            outcome: SessionOutcome::Started,
            last_context: None,
            preflight: Some(Box::new(AskPreflight {
                seed: SessionSeed::Fork,
                generation: 6,
                state: PreflightState::Done,
            })),
        },
    );
    db.jobs().complete(&answer, Timestamp::now()).await.unwrap();
    let stored = db.jobs().get(spec.job_id).await.unwrap();
    assert_eq!(stored.state, contracts::job::state::JobState::Succeeded);
    assert_eq!(stored.result.unwrap().outcome, answer.outcome);
    assert!(
        db.container_sessions()
            .working(1, &spec.payer, ThreadKind::Ask)
            .await
            .unwrap()
            .is_none()
    );
}
