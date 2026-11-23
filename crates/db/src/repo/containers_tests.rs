use super::*;

#[tokio::test]
async fn containers_schema_exists_on_fresh_database() {
    let db = Db::in_memory().await.unwrap();
    for table in ["analytic_threads", "comment_sources"] {
        let present: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?")
                .bind(table)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(present, 1, "{table}");
    }
}

#[tokio::test]
async fn container_sessions_fence_generation_and_compaction_acknowledgements() {
    use super::container_sessions::{AnalyticThread, WorkingThread};
    use contracts::{
        ids::UserId,
        runtime::{AskPreflight, LastContext, PreflightState, SessionSeed, ThreadKind},
    };
    let db = Db::in_memory().await.unwrap();
    analyses::testing::seed_analysis(&db).await.unwrap();
    let user = UserId("u1".into());
    let sessions = db.container_sessions();
    let analytic = AnalyticThread {
        thread_id: "analytic".into(),
        generation: 1,
        last_context: Some(LastContext {
            tokens: 90_000,
            generation: 1,
        }),
    };
    sessions
        .save_analytic(1, &user, 0, &analytic)
        .await
        .unwrap();
    assert_eq!(
        sessions.analytic(1, &user).await.unwrap(),
        Some(analytic.clone())
    );
    assert!(
        sessions
            .analytic(1, &UserId("someone-else".into()))
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        sessions.save_analytic(1, &user, 0, &analytic).await,
        Err(crate::error::DbError::Conflict)
    ));
    let pending = WorkingThread {
        thread_id: "working".into(),
        kind: ThreadKind::Ask,
        preflight: AskPreflight {
            seed: SessionSeed::Fork,
            generation: 1,
            state: PreflightState::Pending,
        },
    };
    sessions
        .seed_working(1, &user, None, &pending)
        .await
        .unwrap();
    assert_eq!(
        sessions.working(1, &user, ThreadKind::Ask).await.unwrap(),
        Some(pending.clone())
    );
    let mut stale = pending.clone();
    stale.thread_id = "lost-working".into();
    assert!(matches!(
        sessions.complete_preflight(1, &user, &stale).await,
        Err(crate::error::DbError::Conflict)
    ));
    sessions
        .complete_preflight(1, &user, &pending)
        .await
        .unwrap();
    assert_eq!(
        sessions
            .working(1, &user, ThreadKind::Ask)
            .await
            .unwrap()
            .unwrap()
            .preflight
            .state,
        PreflightState::Done
    );
    assert!(matches!(
        sessions.complete_preflight(1, &user, &pending).await,
        Err(crate::error::DbError::Conflict)
    ));
    let review = WorkingThread {
        thread_id: "review-working".into(),
        kind: ThreadKind::Verify,
        preflight: AskPreflight {
            seed: SessionSeed::ContextPack,
            generation: 1,
            state: PreflightState::NotNeeded,
        },
    };
    sessions
        .seed_working(1, &user, None, &review)
        .await
        .unwrap();
    assert_eq!(
        sessions
            .working(1, &user, ThreadKind::Verify)
            .await
            .unwrap(),
        Some(review.clone())
    );
    assert_eq!(
        sessions
            .working(1, &user, ThreadKind::Ask)
            .await
            .unwrap()
            .unwrap()
            .preflight
            .state,
        PreflightState::Done
    );
    let wrong_kind = WorkingThread {
        thread_id: "working".into(),
        ..review
    };
    assert!(
        sessions
            .seed_working(1, &user, Some("review-working"), &wrong_kind)
            .await
            .is_err()
    );
    sessions
        .seed_working(1, &user, Some("working"), &pending)
        .await
        .unwrap();
    assert_eq!(
        sessions
            .working(1, &user, ThreadKind::Ask)
            .await
            .unwrap()
            .unwrap()
            .preflight
            .state,
        PreflightState::Done
    );
    let next = WorkingThread {
        thread_id: "next".into(),
        preflight: AskPreflight {
            generation: 2,
            ..pending.preflight.clone()
        },
        ..pending.clone()
    };
    sessions
        .seed_working(1, &user, Some("working"), &next)
        .await
        .unwrap();
    assert!(matches!(
        sessions.complete_preflight(1, &user, &pending).await,
        Err(crate::error::DbError::Conflict)
    ));
    assert!(matches!(
        sessions
            .seed_working(1, &user, Some("working"), &pending)
            .await,
        Err(crate::error::DbError::Conflict)
    ));
}

#[tokio::test]
async fn container_credential_status_contains_no_auth_material() {
    use contracts::gateway::{CredentialState, CredentialStatus, RelinkReason};
    let db = Db::in_memory().await.unwrap();
    analyses::testing::seed_analysis(&db).await.unwrap();
    let status = CredentialStatus {
        mode: contracts::gateway::CredentialMode::ChatgptPlan,
        state: CredentialState::RelinkNeeded,
        relink_reason: Some(RelinkReason::Quarantined),
        label: None,
        plan: None,
        paused_until: None,
        last_call: None,
    };
    db.llm_credentials()
        .put_status("default", "u1", &status)
        .await
        .unwrap();
    assert_eq!(
        db.llm_credentials().status("u1").await.unwrap(),
        Some(status)
    );
    assert!(db.llm_credentials().remove_status("u1").await.unwrap());
    assert_eq!(db.llm_credentials().status("u1").await.unwrap(), None);
    assert!(!db.llm_credentials().remove_status("u1").await.unwrap());
}

#[tokio::test]
async fn container_source_versions_are_idempotent_and_read_once() {
    use contracts::api::review_loop::{CommentSource, CommentSourceKind, SourceRole};
    let db = Db::in_memory().await.unwrap();
    analyses::testing::seed_analysis(&db).await.unwrap();
    sqlx::query(
        "INSERT INTO comment_prs(pull_request_id,tenant_id,created_at) VALUES (1,'default','t')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let source = CommentSource {
        url: "https://github.com/o/r/pull/1#issuecomment-1".into(),
        kind: CommentSourceKind::PrComment,
        author: "author".into(),
        body: "A useful concern".into(),
        body_hash: "a".repeat(64),
        role: SourceRole::Source,
        read_at: None,
    };
    let id = db
        .comment_sources()
        .put("default", 1, &source)
        .await
        .unwrap();
    assert_eq!(
        db.comment_sources()
            .put("default", 1, &source)
            .await
            .unwrap(),
        id
    );
    let now = "2026-11-23T10:00:00Z".parse().unwrap();
    assert!(db.comment_sources().mark_read(id, now).await.unwrap());
    assert!(!db.comment_sources().mark_read(id, now).await.unwrap());
    let next = CommentSource {
        body_hash: "b".repeat(64),
        body: "An edited concern".into(),
        ..source
    };
    assert_ne!(
        db.comment_sources().put("default", 1, &next).await.unwrap(),
        id
    );
    let list = db.comment_sources().list("default", 1).await.unwrap();
    assert_eq!(list.len(), 2);
    assert!(list[0].source.read_at.is_some());
    assert!(list[1].source.read_at.is_none());
    assert!(
        db.comment_sources()
            .list("other", 1)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn containers_indexes_triggers_and_checks_enforce_shape_and_seed_invariants() {
    let db = Db::in_memory().await.unwrap();
    let analysis = analyses::testing::seed_analysis(&db).await.unwrap();
    sqlx::query(
        "INSERT INTO users(id,github_id,login,created_at,updated_at) VALUES ('u2',2,'u2','t','t')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    for (id, payer) in [("j1", "u1"), ("j2", "u2")] {
        sqlx::query("INSERT INTO jobs(id,tenant_id,analysis_id,payer_id,pull_request_id,kind,priority,state,spec,stage,created_at) VALUES (?,'default',?,?,1,'plan',1,'running','{}',?,'t')")
            .bind(id).bind(analysis.to_string()).bind(payer).bind(format!("plan:{id}")).execute(&db.pool).await.unwrap();
    }
    assert!(sqlx::query("INSERT INTO jobs(id,tenant_id,analysis_id,payer_id,pull_request_id,kind,priority,state,spec,stage,created_at) VALUES ('j3','default',?,'u1',1,'ask',0,'claimed','{}','ask:q','t')")
        .bind(analysis.to_string()).execute(&db.pool).await.is_err());
    assert!(
        sqlx::query("UPDATE jobs SET payer_id='u1' WHERE id='j2'")
            .execute(&db.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE jobs SET pull_request_id=2 WHERE id='j2'")
            .execute(&db.pool)
            .await
            .is_err()
    );
    for invalid in [
        "INSERT INTO analytic_threads(tenant_id,pull_request_id,user_id,generation,state) VALUES('default',1,'u1',-1,'none')",
        "INSERT INTO analytic_threads(tenant_id,pull_request_id,user_id,generation,state) VALUES('default',1,'u1',0,'active')",
        "INSERT INTO analytic_threads(tenant_id,pull_request_id,user_id,generation,state,last_context_tokens,last_context_generation) VALUES('default',1,'u1',0,'none',1,2)",
        "INSERT INTO analytic_threads(tenant_id,pull_request_id,user_id,generation,state,last_context_tokens) VALUES('default',1,'u1',0,'none',1)",
        "INSERT INTO working_sessions(tenant_id,pull_request_id,user_id,agent_thread_id,analytic_generation,seed_generation,preflight_state) VALUES('default',1,'u1','w',1,0,'pending')",
        "INSERT INTO working_sessions(tenant_id,pull_request_id,user_id,agent_thread_id,analytic_generation,seed_generation,thread_kind) VALUES('default',1,'u1','w',1,1,'analytic')",
        "INSERT INTO llm_credentials(user_id,tenant_id,state,relink_reason) VALUES('u1','default','linked','quarantined')",
        "INSERT INTO llm_credentials(user_id,tenant_id,state) VALUES('u1','default','relink_needed')",
        "INSERT INTO llm_credentials(user_id,tenant_id,state,relink_reason) VALUES('u1','default','relink_needed','raw_token')",
    ] {
        assert!(
            sqlx::raw_sql(invalid).execute(&db.pool).await.is_err(),
            "{invalid}"
        );
    }
    let foreign: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&db.pool)
        .await
        .unwrap();
    assert!(foreign.is_empty());
}

#[tokio::test]
async fn container_shapes_have_independent_slots_and_expected_indexes() {
    let db = Db::in_memory().await.unwrap();
    let analysis = analyses::testing::seed_analysis(&db).await.unwrap();
    let refresh = analyses::testing::seed_ranking_refresh(&db).await.unwrap();
    sqlx::query("INSERT INTO jobs(id,tenant_id,analysis_id,payer_id,pull_request_id,kind,priority,state,spec,stage,created_at) VALUES ('pr','default',?,'u1',1,'plan',1,'running','{}','plan:1','t')")
        .bind(analysis.to_string()).execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO jobs(id,tenant_id,refresh_id,payer_id,kind,priority,state,spec,stage,created_at) VALUES ('judge','default',?,'u1','judge_work',0,'claimed','{}','judge:1','t')")
        .bind(refresh.to_string()).execute(&db.pool).await.unwrap();
    assert!(sqlx::query("INSERT INTO jobs(id,tenant_id,refresh_id,payer_id,kind,priority,state,spec,stage,created_at) VALUES ('judge2','default',?,'u1','judge_work',0,'running','{}','judge:2','t')")
        .bind(refresh.to_string()).execute(&db.pool).await.is_err());
    for (kind, name) in [
        ("index", "jobs_one_running_per_pr_payer"),
        ("index", "jobs_one_running_per_judge_payer"),
        ("index", "comment_sources_by_pr"),
        ("index", "comment_items_by_source"),
        ("index", "comment_assessments_by_item"),
        ("trigger", "jobs_container_target_insert"),
        ("trigger", "jobs_container_target_update"),
        ("trigger", "comment_items_source_insert"),
        ("trigger", "comment_items_source_update"),
    ] {
        let found: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE type=? AND name=?")
                .bind(kind)
                .bind(name)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(found, 1, "{name}");
    }
    for old in [
        "jobs_one_running_per_credential",
        "jobs_one_running_per_pr_insert",
        "jobs_one_running_per_pr_update",
        "comment_reports",
    ] {
        let found: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE name=?")
            .bind(old)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(found, 0, "{old}");
    }
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('pull_requests')")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    for dropped in ["agent_thread_id", "analytic_generation", "session_state"] {
        assert!(!columns.iter().any(|c| c == dropped));
    }
}

#[tokio::test]
async fn container_point_bindings_stay_within_their_pull_request() {
    use contracts::ids::CommentItemId;
    let db = Db::in_memory().await.unwrap();
    analyses::testing::seed_analysis(&db).await.unwrap();
    let first = CommentItemId::new_random();
    let other = CommentItemId::new_random();
    sqlx::raw_sql("INSERT INTO pull_requests(id,tenant_id,repo_id,number,title,author,html_url,base_sha,head_sha,fetched_at) SELECT 2,tenant_id,repo_id,number+1,title,author,html_url,base_sha,head_sha,fetched_at FROM pull_requests WHERE id=1;
        INSERT INTO comment_prs(pull_request_id,tenant_id,created_at) VALUES(1,'default','t'),(2,'default','t');
        INSERT INTO comment_sources(id,tenant_id,pull_request_id,source_kind,url,author,body,body_hash,role) VALUES(1,'default',1,'review_body','url1','a','A grounded concern','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','source'),(2,'default',2,'review_body','url2','a','Another concern','bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb','source');")
        .execute(&db.pool).await.unwrap();
    for (id, pr) in [(first, 1), (other, 2)] {
        sqlx::query("INSERT INTO comment_items(id,tenant_id,pull_request_id,item_no,kind,stable_key,author,author_kind,title,body,body_hash,first_seen_head,github,state,created_at,updated_at) VALUES (?,'default',?,1,'review_body','point','a','human','Concern','A grounded concern','hash','head','{\"edited_after_decision\":false,\"replies\":0}','undecided','2026-11-23T00:00:00Z','2026-11-23T00:00:00Z')")
            .bind(id.to_string()).bind(pr).execute(&db.pool).await.unwrap();
    }
    db.review_loop()
        .bind_source(first, 1, "A grounded concern", None)
        .await
        .unwrap();
    let item = db.review_loop().item(first).await.unwrap();
    assert_eq!(item.source_id, Some(1));
    assert_eq!(item.quote.as_deref(), Some("A grounded concern"));
    assert!(
        db.review_loop()
            .bind_source(first, 2, "Another concern", None)
            .await
            .is_err()
    );
    assert!(
        db.review_loop()
            .bind_source(first, 1, "A grounded concern", Some(other))
            .await
            .is_err()
    );
    assert!(
        db.review_loop()
            .bind_source(first, 1, &"x".repeat(601), None)
            .await
            .is_err()
    );
    assert!(
        db.review_loop()
            .bind_source(first, 1, "A grounded concern", Some(first))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn assessment_storage_and_plan_only_constraints_exist() {
    let db = Db::in_memory().await.unwrap();
    analyses::testing::seed_analysis(&db).await.unwrap();
    for table in [
        "assessment_config",
        "assessment_passes",
        "assessment_method_runs",
        "assessment_comparison_items",
        "assessment_owner_decisions",
        "assessment_daily_usage",
    ] {
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?")
                .bind(table)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(count, 1, "{table}");
    }
    sqlx::query(
        "INSERT INTO llm_credentials(user_id,tenant_id,state) VALUES('u1','default','linked')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(
        sqlx::query("UPDATE llm_credentials SET mode='api_key'")
            .execute(&db.pool)
            .await
            .is_err()
    );
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('llm_credentials')")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    for forbidden in [
        "api_key",
        "access_token",
        "refresh_token",
        "base_url",
        "secret",
    ] {
        assert!(
            !columns.iter().any(|c| c.contains(forbidden)),
            "{forbidden}"
        );
    }
}

#[tokio::test]
async fn assessment_storage_defaults_and_invalid_values_are_constrained() {
    let db = Db::in_memory().await.unwrap();
    analyses::testing::seed_analysis(&db).await.unwrap();
    sqlx::query("INSERT INTO assessment_config(user_id) VALUES('u1')")
        .execute(&db.pool)
        .await
        .unwrap();
    let config: (String, String, Option<String>) = sqlx::query_as(
        "SELECT primary_method,ab_prs,ab_until FROM assessment_config WHERE user_id='u1'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(config, ("claudius_host".into(), "[]".into(), None));
    for statement in [
        "UPDATE assessment_config SET primary_method='other'",
        "UPDATE assessment_config SET ab_prs='{}'",
        "UPDATE assessment_config SET ab_prs='[1]'",
        "UPDATE assessment_config SET ab_until='tomorrow'",
        "INSERT INTO assessment_daily_usage(user_id,day_utc,passes) VALUES('u1','2026-11-23',81)",
        "INSERT INTO assessment_daily_usage(user_id,day_utc,passes) VALUES('u1','abcdefghij',1)",
    ] {
        assert!(
            sqlx::raw_sql(statement).execute(&db.pool).await.is_err(),
            "{statement}"
        );
    }
}

#[tokio::test]
async fn container_retry_assigns_the_acting_payer_without_changing_creator() {
    use contracts::{api::analyses::AnalysisState, gateway::*, time::Timestamp};
    let db = Db::in_memory().await.unwrap();
    let analysis = analyses::testing::seed_analysis(&db).await.unwrap();
    analyses::testing::seed_payer(&db, "u2").await.unwrap();
    db.analyses()
        .set_state(
            analysis,
            &AnalysisState::Failed {
                reason: None,
                flow: None,
                message: "failed".into(),
            },
            Timestamp::now(),
        )
        .await
        .unwrap();
    assert!(
        !db.analyses()
            .retry_as(analysis, "u2", Timestamp::now())
            .await
            .unwrap()
    );
    assert_eq!(db.analyses().get(analysis).await.unwrap().payer_id, "u1");
    db.llm_credentials()
        .put_status(
            DEFAULT_TENANT,
            "u2",
            &CredentialStatus {
                mode: CredentialMode::ChatgptPlan,
                state: CredentialState::Linked,
                relink_reason: None,
                plan: None,
                label: None,
                paused_until: None,
                last_call: None,
            },
        )
        .await
        .unwrap();
    let previous = super::container_sessions::AnalyticThread {
        thread_id: "earlier-head".into(),
        generation: 5,
        last_context: Some(contracts::runtime::LastContext {
            tokens: 10_000,
            generation: 5,
        }),
    };
    db.container_sessions()
        .save_analytic(1, &contracts::ids::UserId("u2".into()), 0, &previous)
        .await
        .unwrap();
    assert!(
        db.analyses()
            .retry_as(analysis, "u2", Timestamp::now())
            .await
            .unwrap()
    );
    let retried = db
        .container_sessions()
        .analytic(1, &contracts::ids::UserId("u2".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(
        retried.generation > previous.generation,
        "Retry invalidates a working seed before the next provider turn"
    );
    assert_eq!(retried.thread_id, previous.thread_id);
    assert_eq!(retried.last_context, None);
    let record = db.analyses().get(analysis).await.unwrap();
    assert_eq!(record.payer_id, "u2");
    assert_eq!(record.created_by, "u1");
    assert!(
        !db.analyses()
            .retry_as(analysis, "u1", Timestamp::now())
            .await
            .unwrap()
    );
    assert_eq!(db.analyses().get(analysis).await.unwrap().payer_id, "u2");
}

#[tokio::test]
async fn container_relink_failure_preserves_gateway_quarantine_reason() {
    use contracts::{
        agent::JobKind,
        gateway::*,
        job::{result::*, spec::JobPriority},
        time::Timestamp,
    };
    let db = Db::in_memory().await.unwrap();
    let analysis = analyses::testing::seed_analysis(&db).await.unwrap();
    let spec = analyses::testing::job_spec(analysis, JobKind::Plan);
    db.jobs()
        .enqueue(
            jobs::NewJob {
                spec: spec.clone(),
                priority: JobPriority::Batch,
                stage: jobs::JobStage::Plan { batch: 1 },
                generation: 0,
            },
            Timestamp::now(),
        )
        .await
        .unwrap();
    db.jobs()
        .claim(jobs::ClaimParams {
            payer_ids: &["u1".into()],
            job_token_hash: &[1],
            lease_until: Timestamp::now() + jiff::SignedDuration::from_mins(1),
            now: Timestamp::now(),
        })
        .await
        .unwrap()
        .unwrap();
    db.llm_credentials()
        .put_status(
            DEFAULT_TENANT,
            "u1",
            &CredentialStatus {
                mode: CredentialMode::ChatgptPlan,
                state: CredentialState::RelinkNeeded,
                relink_reason: Some(RelinkReason::Quarantined),
                plan: None,
                label: None,
                paused_until: None,
                last_call: None,
            },
        )
        .await
        .unwrap();
    db.jobs()
        .complete(
            &JobResult {
                job_id: spec.job_id,
                attempt: 1,
                assessment: None,
                report: None,
                outcome: JobOutcome::Failed {
                    kind: JobFailureKind::CredentialRelink,
                    message: "Relink required".into(),
                },
                session: None,
                usage: None,
                skills_commit: None,
                llm_call: None,
            },
            Timestamp::now(),
        )
        .await
        .unwrap();
    assert_eq!(
        db.llm_credentials()
            .status("u1")
            .await
            .unwrap()
            .unwrap()
            .relink_reason,
        Some(RelinkReason::Quarantined)
    );
}
