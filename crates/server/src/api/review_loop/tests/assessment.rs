use super::*;

#[tokio::test]
async fn assessment_settings_default_and_changes_never_schedule_work() {
    let w = world().await;
    let settings = w.get("/api/v1/settings/comment-assessment").await;
    assert_eq!(settings["config"]["primary_method"], "claudius_host");
    assert_eq!(settings["config"]["ab_prs"], json!([]));
    assert_eq!(settings["quota"]["remaining"], 80);
    let (status,updated)=w.call("PUT","/api/v1/settings/comment-assessment",Some(json!({"revision":settings["revision"],"config":{"primary_method":"prompt_v4","ab_prs":["acme/node-client#482"],"ab_until":null}}))).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["config"]["primary_method"], "prompt_v4");
    assert_eq!(updated["quota"]["remaining"], 80);
    assert!(
        w.state
            .db
            .review_loop()
            .live_passes()
            .await
            .unwrap()
            .is_empty()
    );
    let (status, _) = w
        .call(
            "PUT",
            "/api/v1/settings/comment-assessment",
            Some(json!({"ab_prs":["../../../other"]})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn assessment_comparison_is_payer_scoped_and_reads_do_not_fetch_github() {
    let w = world().await;
    w.server.reset().await;
    let (status, _) = w
        .call("GET", &format!("{PR}/comments/assessment-comparison"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(w.server.received_requests().await.unwrap().is_empty());
}

async fn seed_comparison(w: &World, payer: &str, paired: bool) -> contracts::ids::PassId {
    use contracts::assessment::*;
    use contracts::ids::{AnalysisId, PassId, UserId};
    use contracts::job::spec::{JobTarget, PullRequestCommentsTarget, RepoCheckout};
    use db::repo::assessments::{MethodJob, MethodResult, NewAssessmentPass};
    let pr_ref = crate::github::review_loop::tests::pr482();
    let pr = contracts::api::pr::PrSummary {
        repo: pr_ref.repo,
        number: pr_ref.number,
        title: "Retry calls".into(),
        author: "alice".into(),
        html_url: "https://github.com/acme/node-client/pull/482".into(),
        base_sha: contracts::ids::CommitSha("a".repeat(40)),
        head_sha: contracts::ids::CommitSha("b".repeat(40)),
    };
    let repo = w
        .state
        .db
        .repos()
        .upsert(DEFAULT_TENANT, &pr.repo, None)
        .await
        .unwrap();
    let pr_id = w
        .state
        .db
        .pull_requests()
        .upsert(DEFAULT_TENANT, repo.id, &pr, Timestamp::now())
        .await
        .unwrap();
    w.state
        .db
        .review_loop()
        .ensure_pr(DEFAULT_TENANT, pr_id, Timestamp::now())
        .await
        .unwrap();
    let id = PassId::new_random();
    let user = UserId(payer.into());
    if paired {
        w.state
            .db
            .assessments()
            .set_config(
                DEFAULT_TENANT,
                &user,
                &AssessmentConfig {
                    ab_prs: vec!["acme/node-client#482".into()],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    let mut spec = db::repo::analyses::testing::job_spec(
        AnalysisId::new_random(),
        contracts::agent::JobKind::VerifyComments,
    );
    spec.payer = user.clone();
    spec.target = JobTarget::PullRequestComments(PullRequestCommentsTarget {
        pull_request_id: pr_id,
        pass_id: id,
        repo: RepoCheckout {
            clone_url: "file:///fake".into(),
            base_sha: pr.base_sha.clone(),
            head_sha: pr.head_sha.clone(),
        },
    });
    spec.assessment = Some(AssessmentRunRef {
        pass_id: id,
        method: AssessmentMethod::ClaudiusHost,
    });
    let mut jobs = vec![MethodJob {
        spec: spec.clone(),
        version: "4.2".into(),
    }];
    if paired {
        spec.job_id = contracts::ids::JobId::new_random();
        spec.assessment.as_mut().unwrap().method = AssessmentMethod::PromptV4;
        jobs.push(MethodJob {
            spec: spec.clone(),
            version: "4".into(),
        });
    }
    let input = AssessmentInput {
        pr: "acme/node-client#482".into(),
        pull_request_id: pr_id,
        head: pr.head_sha,
        base: pr.base_sha,
        input_hash: "a".repeat(64),
        scope_hash: "b".repeat(64),
        snapshot_ref: "fake/frozen".into(),
        model: spec.model,
    };
    w.state
        .db
        .assessments()
        .admit(
            DEFAULT_TENANT,
            &user,
            &NewAssessmentPass {
                id,
                request_key: id.to_string(),
                input,
                jobs,
            },
            Timestamp::now(),
        )
        .await
        .unwrap();
    for _ in 0..if paired { 2 } else { 1 } {
        let now = Timestamp::now();
        let job = w
            .state
            .db
            .jobs()
            .claim(db::repo::jobs::ClaimParams {
                payer_ids: &[payer.into()],
                job_token_hash: b"fake",
                lease_until: now + SignedDuration::from_mins(5),
                now,
            })
            .await
            .unwrap()
            .unwrap();
        let run = job.spec.assessment.unwrap();
        let mut rows = vec![];
        if run.method == AssessmentMethod::PromptV4 {
            let base = AssessmentComparisonItem {
                recorded_decision: None,
                comparison_key: "extra".into(),
                local_id: "extra".into(),
                run: run.clone(),
                method_version: "4".into(),
                source: AssessmentSource::Inline {
                    thread_id: "T1".into(),
                    version: "v1".into(),
                },
                primary_item_id: None,
                primary_item_hash: None,
                status: ComparisonItemStatus::Unmatched,
                duplicate_of: None,
                verdict: Some(AssessmentVerdict::Valid),
                native_verdict: Some("valid".into()),
                proposal: None,
                owner_decision: None,
            };
            rows.push(base.clone());
            rows.push(AssessmentComparisonItem {
                comparison_key: "duplicate".into(),
                local_id: "duplicate".into(),
                status: ComparisonItemStatus::Duplicate,
                duplicate_of: Some("extra".into()),
                verdict: None,
                ..base
            });
        }
        w.state
            .db
            .assessments()
            .finish_attempt(
                DEFAULT_TENANT,
                &user,
                &run,
                job.attempt,
                &MethodResult {
                    usage_known: true,
                    reported_head: None,
                    status: AssessmentRunStatus::Succeeded,
                    usage: Default::default(),
                    duration_ms: 0,
                    failure: None,
                    items: rows,
                },
            )
            .await
            .unwrap();
        let result = contracts::job::result::JobResult {
            job_id: job.id,
            attempt: job.attempt,
            assessment: Some(Box::new(run)),
            report: None,
            outcome: contracts::job::result::JobOutcome::Succeeded {
                output: Value::Null,
            },
            usage: None,
            session: None,
            skills_commit: None,
            llm_call: None,
        };
        w.state.db.jobs().complete(&result, now).await.unwrap();
    }
    id
}

#[tokio::test]
async fn assessment_comparison_separates_two_payers_and_keeps_unmatched_unscored() {
    let mut w = world().await;
    let other = crate::test_support::other_user_cookie(&w.state).await;
    let first = seed_comparison(&w, "u1", true).await;
    w.server.reset().await;
    let report = w.get(&format!("{PR}/comments/assessment-comparison")).await;
    assert_eq!(report["pass"]["id"], first.to_string());
    assert_eq!(
        report["items"].as_array().unwrap().len(),
        2,
        "unmatched/duplicate rows must not create primary missing rows"
    );
    assert_eq!(report["summary"]["methods"].as_array().unwrap().len(), 2);
    assert_eq!(report["summary"]["pending_owner"], 0);
    assert_eq!(report["summary"]["verdict_compared"], 0);
    for score in report["summary"]["methods"].as_array().unwrap() {
        assert_eq!(score["missing"], 0);
        assert_eq!(score["owner_decided"], 0);
        assert_eq!(score["assessed"], 0);
        assert_eq!(
            score["unmatched"],
            if score["method"] == "prompt_v4" { 1 } else { 0 }
        );
    }
    let first_cookie = std::mem::replace(&mut w.cookie, other);
    assert_eq!(
        w.call("GET", &format!("{PR}/comments/assessment-comparison"), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let second = seed_comparison(&w, "u2", false).await;
    assert_eq!(
        w.get(&format!("{PR}/comments/assessment-comparison")).await["pass"]["id"],
        second.to_string()
    );
    w.cookie = first_cookie;
    assert_eq!(
        w.get(&format!("{PR}/comments/assessment-comparison")).await["pass"]["id"],
        first.to_string()
    );
    assert!(w.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn assessment_sources_include_context_and_explicit_zero_point_reads() {
    use contracts::api::review_loop::{CommentSource, CommentSourceKind, SourceRole};
    let w = world().await;
    seed_comparison(&w, "u1", false).await;
    let id = w.pr_id().await;
    for (suffix, role, read_at) in [
        (1, SourceRole::Context, None),
        (2, SourceRole::Source, Some(Timestamp::now())),
    ] {
        let source = CommentSource {
            url: format!("https://github.com/acme/node-client/pull/482#issuecomment-{suffix}"),
            kind: CommentSourceKind::PrComment,
            author: "owner".into(),
            body: "Progress update".into(),
            body_hash: "c".repeat(64),
            role,
            read_at: None,
        };
        let key = w
            .state
            .db
            .comment_sources()
            .put(DEFAULT_TENANT, id, &source)
            .await
            .unwrap();
        if let Some(at) = read_at {
            w.state
                .db
                .comment_sources()
                .mark_read(key, at)
                .await
                .unwrap();
        }
    }
    w.server.reset().await;
    let sources = w.get(&format!("{PR}/comments/sources")).await;
    assert_eq!(sources.as_array().unwrap().len(), 2);
    assert_eq!(sources[0]["role"], "context");
    assert!(sources[0]["read_at"].is_null());
    assert!(sources[1]["read_at"].is_string());
    assert!(w.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn assess_again_snapshot_failure_returns_an_error_without_charging_quota() {
    use contracts::api::review_loop::{AuthorKind, CommentItemKind, ItemGithubFacts, ItemState};
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("not-a-directory");
    std::fs::write(&blocked, "fixture").unwrap();
    let w = world_with(|state| {
        let config = std::sync::Arc::get_mut(&mut state.config).unwrap();
        config.data_dir = blocked;
        config.github.clone_url = url::Url::parse("file:///nonexistent-diffpilot-fixture").unwrap();
    })
    .await;
    seed_comparison(&w, "u1", false).await;
    let id = w.pr_id().await;
    let item = db::repo::review_loop::ObservedItem {
        kind: CommentItemKind::Thread,
        stable_key: "T1".into(),
        author: "reviewer".into(),
        author_kind: AuthorKind::Human,
        url: None,
        location: None,
        title: "Bound retries".into(),
        body: "Bound the retry loop.".into(),
        body_hash: "c".repeat(64),
        head: contracts::ids::CommitSha("b".repeat(40)),
        round: None,
        bot_severity: None,
        github: ItemGithubFacts {
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
    let item = match w
        .state
        .db
        .review_loop()
        .upsert_item(
            DEFAULT_TENANT,
            id,
            &item,
            ItemState::Undecided,
            Timestamp::now(),
        )
        .await
        .unwrap()
    {
        db::repo::review_loop::UpsertOutcome::New(item) => item,
        _ => panic!("new"),
    };
    let quota = w
        .state
        .db
        .assessments()
        .quota(&contracts::ids::UserId("u1".into()), Timestamp::now())
        .await
        .unwrap();
    let (status, body) = w
        .post(
            &format!("{PR}/comments/items/C{}/assess", item.no),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(
        w.state
            .db
            .assessments()
            .quota(&contracts::ids::UserId("u1".into()), Timestamp::now())
            .await
            .unwrap(),
        quota
    );
}

#[tokio::test]
async fn assessment_settings_stale_tab_returns_conflict_and_preserves_the_first_write() {
    let w = world().await;
    let initial = w.get("/api/v1/settings/comment-assessment").await;
    let update = json!({"revision":initial["revision"],"config":{"primary_method":"claudius_host","ab_prs":["acme/a#1"],"ab_until":null}});
    assert_eq!(
        w.call("PUT", "/api/v1/settings/comment-assessment", Some(update))
            .await
            .0,
        StatusCode::OK
    );
    let stale = json!({"revision":initial["revision"],"config":{"primary_method":"prompt_v4","ab_prs":["acme/b#2"],"ab_until":null}});
    assert_eq!(
        w.call("PUT", "/api/v1/settings/comment-assessment", Some(stale))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        w.get("/api/v1/settings/comment-assessment").await["config"]["ab_prs"],
        json!(["acme/a#1"])
    );
}

#[tokio::test]
async fn codex_status_enriches_gateway_observations_without_credentials_or_auth_posts() {
    let gateway = MockServer::start().await;
    let observed = json!({"mode":"chatgpt_plan","state":"linked","relink_reason":null,"label":"payer@example.invalid","plan":"plus","paused_until":null,"last_call":null,
        "telemetry":{"linked_at":"2026-11-24T00:00:00Z","primary":{"used_percent":12.5,"resets_at":"2026-11-25T00:00:00Z"},"secondary":null,"tokens_today":null,"tokens_month":null,"live_containers":null}});
    Mock::given(method("GET"))
        .and(path("/admin/credentials/u1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&observed))
        .mount(&gateway)
        .await;
    Mock::given(method("GET"))
        .and(path("/admin/credentials/u1/link"))
        .respond_with(ResponseTemplate::new(200).set_body_json(observed))
        .mount(&gateway)
        .await;
    let w = world_with(|state| {
        std::sync::Arc::get_mut(&mut state.config)
            .unwrap()
            .codex_gateway = Some(
            crate::gateway::client::Admin::new(
                Url::parse(&gateway.uri()).unwrap(),
                SecretString::from("fake-admin-scope"),
            )
            .unwrap(),
        );
    })
    .await;
    for route in ["/api/v1/settings/codex", "/api/v1/settings/codex/link"] {
        let status = w.get(route).await;
        assert_eq!(status["telemetry"]["primary"]["used_percent"], 12.5);
        assert_eq!(status["telemetry"]["live_containers"], json!([]));
        assert_eq!(status["telemetry"]["tokens_today"], Value::Null);
        assert_eq!(status["telemetry"]["tokens_month"], Value::Null);
        assert!(!status.to_string().contains("fake-admin-scope"));
        for key in ["access_token", "refresh_token", "id_token", "api_key"] {
            assert!(!status.to_string().contains(key));
        }
    }
    assert!(
        gateway
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
}

#[tokio::test]
async fn comments_reads_omit_uncomputed_assessment_progress_and_counts() {
    let w = world().await;
    let page = w.get(PR).await;
    let overview = w.get(&format!("{PR}/comments")).await;
    for (comments, running) in [(&page["comments"], false), (&overview, true)] {
        let progress = &comments["assessment"];
        assert_eq!(progress["running"], running);
        assert!(progress.get("in_pass").is_none());
        assert!(progress.get("waiting").is_none());
        for key in ["needs_you", "needs_you_gates", "imported_unseen"] {
            assert!(comments["counts"].get(key).is_none(), "{key}");
        }
    }
}

#[tokio::test]
async fn fixture_status_sources_are_omitted_and_new_prose_sources_explain_review_counts() {
    let w = world().await;
    let page = w.get(PR).await;
    assert!(
        w.state
            .db
            .review_loop()
            .live_passes()
            .await
            .unwrap()
            .is_empty(),
        "opening the PR page only reads GitHub"
    );
    let sources = w.get(&format!("{PR}/comments/sources")).await;
    for source in sources.as_array().unwrap() {
        assert!(
            !["#issuecomment-5000000101", "#issuecomment-5000000102"]
                .iter()
                .any(|suffix| source["url"].as_str().unwrap().ends_with(suffix)),
            "{source}"
        );
    }
    let items = w.items().await;
    assert_eq!(
        items.len(),
        15,
        "8 threads, legacy body findings, three newly eligible prose sources and merge question"
    );
    assert_eq!(page["comments"]["counts"]["undecided"], 13);
    assert_eq!(page["comments"]["assessment"]["total"], 12);
    w.get(&format!("{PR}/comments")).await;
    let pending = w.state.db.review_loop().live_passes().await.unwrap();
    assert_eq!(
        pending[0].sent.len(),
        crate::review_loop::assess::MAX_ITEMS_PER_PASS,
        "the first batch obeys the item cap; progress counts all pending comments"
    );
}
