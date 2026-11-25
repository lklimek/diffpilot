use super::*;
use contracts::api::review_loop::{CommentSource, CommentSourceKind, SourceRole};

const THREAD_2: &str = "PRRT_fixture482_l2";
const THREAD_6: &str = "PRRT_fixture482_l6";
const THREAD_7: &str = "PRRT_fixture482_l7";

async fn link(w: &World, follower_key: &str) {
    let primary = w.item(L2).await;
    let follower = w.item(follower_key).await;
    let id = w.pr_id().await;
    let source = w
        .state
        .db
        .comment_sources()
        .put(
            DEFAULT_TENANT,
            id,
            &CommentSource {
                url: follower["url"].as_str().unwrap().into(),
                kind: CommentSourceKind::PrComment,
                author: "reviewer".into(),
                body_hash: hex::encode(crate::secrets::sha256(b"A duplicate point")),
                body: "A duplicate point".into(),
                role: SourceRole::Source,
                read_at: None,
            },
        )
        .await
        .unwrap();
    w.state
        .db
        .review_loop()
        .bind_source(
            serde_json::from_value(follower["id"].clone()).unwrap(),
            source,
            "A duplicate point",
            Some(serde_json::from_value(primary["id"].clone()).unwrap()),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn deciding_primary_decides_non_gate_duplicates_with_own_effects() {
    let w = world().await;
    link(&w, L6).await;
    w.decide_ok(L2, reject("This exceeds the scope.")).await;
    let follower = w.item(L6).await;
    let primary = w.item(L2).await;
    let followed = &follower["decisions"][0];
    assert_eq!(followed["via"], "follows_primary");
    assert_eq!(followed["follows"], primary["no"]);
    assert_eq!(followed["kind"], "reject");
    assert!(follower["decision_follows_primary"].as_bool().unwrap());
    assert!(
        followed["effects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|effect| {
                effect["kind"] == "resolve" && effect["params"]["thread_id"] == THREAD_6
            })
    );
    w.decide_ok(L2, decision("fix_here")).await;
    let follower = w.item(L6).await;
    assert_eq!(follower["decisions"][1]["supersedes"], followed["id"]);
    assert_eq!(follower["decisions"][0]["effects"][0]["state"], "cancelled");
}

#[tokio::test]
async fn gate_duplicate_never_follows() {
    for mark_later in [false, true] {
        let w = world().await;
        link(&w, L6).await;
        if mark_later {
            w.decide_ok(L2, reject("This exceeds the scope.")).await;
        }
        let follower = w.item(L6).await;
        let (status, body) = w
            .post(
                &format!("{PR}/comments/items/C{}/gates", follower["no"]),
                json!({"gate":"G-DATA", "marked":true, "item_revision":follower["revision"]}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if !mark_later {
            w.decide_ok(L2, reject("This exceeds the scope.")).await;
        }
        let follower = w.item(L6).await;
        assert!(
            !follower["decision_follows_primary"]
                .as_bool()
                .unwrap_or_default()
        );
        if mark_later {
            assert_eq!(follower["state"], "reopened");
            assert!(
                follower["decisions"][0]["effects"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|effect| { effect["state"] == "cancelled" })
            );
        } else {
            assert_eq!(follower["decisions"], json!([]));
        }
    }
}

#[tokio::test]
async fn individually_decided_follower_stops_following() {
    let w = world().await;
    link(&w, L6).await;
    w.decide_ok(L2, reject("This exceeds the scope.")).await;
    w.decide_ok(L6, decision("fix_here")).await;
    w.decide_ok(L2, reject("A revised reason.")).await;
    let follower = w.item(L6).await;
    assert_eq!(follower["decisions"].as_array().unwrap().len(), 2);
    assert_eq!(follower["decisions"][1]["via"], "question");
    assert!(
        !follower["decision_follows_primary"]
            .as_bool()
            .unwrap_or_default()
    );
}

#[tokio::test]
async fn undo_primary_undoes_followers() {
    let w = world().await;
    link(&w, L6).await;
    w.decide_ok(L2, reject("This exceeds the scope.")).await;
    let primary = w.item(L2).await;
    let (status, body) = w
        .post(
            &format!("{PR}/comments/items/C{}/undo", primary["no"]),
            json!({"item_revision":primary["revision"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let follower = w.item(L6).await;
    assert_eq!(follower["state"], "undecided");
    assert!(
        !follower["decision_follows_primary"]
            .as_bool()
            .unwrap_or_default()
    );
    assert!(follower["decisions"][0]["undone_at"].is_string());
}

#[tokio::test]
async fn imported_unseen_counts_since_last_visit() {
    let w = world().await;
    let uri = format!("{PR}/comments");
    assert_eq!(w.get(&uri).await["counts"]["imported_unseen"], 0);
    assess_recorded(&w, "reject", "We won't fix this issue.", "lklimek", 1).await;
    assert_eq!(
        w.get(PR).await["comments"]["counts"].get("imported_unseen"),
        None
    );
    let rl = w.state.db.review_loop();
    let pr = crate::github::review_loop::tests::pr482();
    let wanted = [(pr.repo, pr.number)];
    assert_eq!(
        rl.comment_tallies(DEFAULT_TENANT, "u1", &wanted)
            .await
            .unwrap()[0]
            .imported_unseen,
        1
    );
    assert_eq!(
        w.post(&format!("{uri}/visit"), json!({})).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        rl.comment_tallies(DEFAULT_TENANT, "u1", &wanted)
            .await
            .unwrap()[0]
            .imported_unseen,
        0
    );
    assert_eq!(w.get(&uri).await["counts"]["imported_unseen"], 1);
    w.decide_ok(L2, decision("fix_here")).await;
    assert_eq!(w.get(&uri).await["counts"]["imported_unseen"], 1);
    let filtered = w.get(&format!("{uri}/items?imported_unseen=true")).await;
    assert_eq!(filtered, json!([]));
    assert_eq!(
        w.post(&format!("{uri}/visit"), json!({})).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(w.get(&uri).await["counts"]["imported_unseen"], 0);
}

#[tokio::test]
async fn dashboard_and_comments_filter_exclude_inactive_imports() {
    for undo in [true, false] {
        let w = world().await;
        let uri = format!("{PR}/comments/items?imported_unseen=true");
        assess_recorded(&w, "reject", "We won't fix this issue.", "lklimek", 1).await;
        assert_eq!(w.get(&uri).await[0]["stable_key"], L2);
        let pr = crate::github::review_loop::tests::pr482();
        let wanted = [(pr.repo, pr.number)];
        let rl = w.state.db.review_loop();
        assert_eq!(
            rl.comment_tallies(DEFAULT_TENANT, "u1", &wanted)
                .await
                .unwrap()[0]
                .imported_unseen,
            1
        );
        if undo {
            let item = w.item(L2).await;
            let (status, body) = w
                .post(
                    &format!("{PR}/comments/items/C{}/undo", item["no"]),
                    json!({"item_revision":item["revision"]}),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
        } else {
            w.decide_ok(L2, decision("fix_here")).await;
        }
        assert_eq!(
            rl.comment_tallies(DEFAULT_TENANT, "u1", &wanted)
                .await
                .unwrap()[0]
                .imported_unseen,
            0,
            "undo: {undo}"
        );
        assert_eq!(w.get(&uri).await, json!([]));
    }
}

#[tokio::test]
async fn first_comments_visit_succeeds_before_the_overview() {
    let w = world().await;
    w.link_llm().await;
    let uri = format!("{PR}/comments");
    let (status, body) = w.post(&format!("{uri}/visit"), json!({})).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let pr = w.pr_id().await;
    assert!(
        w.state
            .db
            .review_loop()
            .latest_pass(pr)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(w.get(&uri).await["counts"]["imported_unseen"], 0);
}

#[tokio::test]
async fn later_linked_duplicate_follows() {
    let w = world().await;
    w.decide_ok(L2, reject("This exceeds the scope.")).await;
    link(&w, L6).await;
    for _ in 0..2 {
        crate::review_loop::propagation::propagate(&w.state, DEFAULT_TENANT, w.pr_id().await)
            .await
            .unwrap();
    }
    let follower = w.item(L6).await;
    assert_eq!(follower["decisions"].as_array().unwrap().len(), 1);
    assert_eq!(follower["decisions"][0]["via"], "follows_primary");
}

async fn assess_recorded(
    w: &World,
    kind: &str,
    text: &str,
    writer: &str,
    writer_id: u64,
) -> String {
    assess_recorded_with_owner_decision(w, kind, text, writer, writer_id, None).await
}

async fn assess_recorded_with_owner_decision(
    w: &World,
    kind: &str,
    text: &str,
    writer: &str,
    writer_id: u64,
    owner_decision: Option<Value>,
) -> String {
    let reply = "https://github.com/acme/node-client/pull/482#discussion_r9127";
    w.get(PR).await;
    let change = |answer: &mut Value| {
        let comments = &mut thread_node(answer, THREAD_2)["comments"];
        let root = &mut comments["nodes"][0];
        let root_login = root["author"]["login"].as_str().unwrap();
        let root_id = if root_login == writer { writer_id } else { 42 };
        root["author"]["databaseId"] = json!(root_id);
        let mut reply_node = root.clone();
        reply_node["databaseId"] = json!(9127);
        reply_node["url"] = json!(reply);
        reply_node["author"] = json!({"login":writer,"databaseId":writer_id,"__typename":"User"});
        reply_node["createdAt"] = json!("2026-11-25T00:00:00Z");
        reply_node["body"] = json!(text);
        comments["nodes"].as_array_mut().unwrap().push(reply_node);
        comments["totalCount"] = json!(comments["nodes"].as_array().unwrap().len());
    };
    w.reread(&[("review-threads.json", &change)]).await;
    w.link_llm().await;
    let pr = w.pr_id().await;
    let rl = w.state.db.review_loop();
    let item = rl
        .items(pr)
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.stable_key == L2)
        .unwrap();
    let payer = contracts::ids::UserId("u1".into());
    w.state
        .db
        .assessments()
        .set_config(
            DEFAULT_TENANT,
            &payer,
            &contracts::assessment::AssessmentConfig {
                primary_method: contracts::assessment::AssessmentMethod::ClaudiusHost,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let (status, body) = w.post(&format!("{PR}/comments/assess"), json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    if let Some(owner_decision) = owner_decision {
        w.decide_ok(L2, owner_decision).await;
    }
    let pass = rl.latest_pass(pr).await.unwrap().unwrap();
    let report = json!({"schema_version":"4.2.0","metadata":{"report_type":"comment_check",
    "input_mode":"host_supplied","pr_number":482,"commit":pass.head.0},
    "comment_assessments":[{
        "id":format!("CMT-{:03}", item.no), "source_kind":"inline", "thread_id":THREAD_2,
        "reviewer":item.author,"comment_url":item.url,"description":"Bound retries",
        "reasoning":"The loop has no upper bound.","assessment":"valid",
        "proposed_action":"Fix here","proposed_decision":{"kind":"fix_here"},
        "recorded_decision":{"kind":kind,"by":writer,"comment_url":reply,"quote":text}
    }]});
    {
        let now = Timestamp::now();
        let job = w
            .state
            .db
            .jobs()
            .claim(db::repo::jobs::ClaimParams {
                payer_ids: &["u1".into()],
                job_token_hash: b"fake",
                lease_until: now + SignedDuration::from_mins(5),
                now,
            })
            .await
            .unwrap()
            .unwrap();
        let run = job.spec.assessment.unwrap();
        let host = run.method == contracts::assessment::AssessmentMethod::ClaudiusHost;
        let result = contracts::job::result::JobResult {
            job_id: job.id,
            attempt: job.attempt,
            assessment: Some(Box::new(run)),
            report: host.then(|| Box::new(serde_json::from_value(report.clone()).unwrap())),
            outcome: contracts::job::result::JobOutcome::Succeeded {
                output: if host {
                    Value::Null
                } else {
                    json!({"purpose":"Retry calls","items":[],"answer":null})
                },
            },
            usage: None,
            session: None,
            skills_commit: None,
            llm_call: None,
        };
        w.state.db.jobs().complete(&result, now).await.unwrap();
    }
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while rl
            .live_passes()
            .await
            .unwrap()
            .iter()
            .any(|pass| pass.pull_request_id == pr)
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("assessment finished");
    reply.to_owned()
}

#[tokio::test]
async fn import_has_no_effects() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let url = assess_recorded(
        &w,
        "fix_here",
        "We will fix this in the current PR.",
        "lklimek",
        1,
    )
    .await;
    link(&w, L6).await;
    crate::review_loop::propagation::propagate(&w.state, DEFAULT_TENANT, w.pr_id().await)
        .await
        .unwrap();
    let primary = w.item(L2).await;
    let follower = w.item(L6).await;
    let imported = &primary["decisions"][0];
    assert_eq!(primary["state"], "decided");
    assert_eq!(primary["imported_decision"]["evidence_url"], url);
    assert_eq!(imported["imported"]["by_id"], 1);
    assert_eq!(imported["effects"], json!([]));
    assert_eq!(follower["decisions"][0]["via"], "follows_primary");
    assert_eq!(follower["decisions"][0]["effects"], json!([]));
    let repeated = |answer: &mut Value| {
        thread_node(answer, THREAD_2)["comments"]["nodes"][0]["url"] =
            json!("https://github.com/acme/node-client/pull/482#discussion_r9130");
        thread_node(answer, THREAD_6)["comments"]["nodes"][0]["url"] =
            json!("https://github.com/acme/node-client/pull/482#discussion_r9131");
    };
    w.reread(&[("review-threads.json", &repeated)]).await;
    for key in [L2, L6] {
        let item = w.item(key).await;
        let decisions = item["decisions"].as_array().unwrap();
        assert_eq!(decisions.len(), 1, "{key}: {decisions:?}");
        assert_eq!(decisions[0]["effects"], json!([]));
    }
    assert!(
        w.state
            .db
            .review_loop()
            .open_feed(Timestamp::now())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn owner_decision_supersedes_import() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    assess_recorded(&w, "reject", "We won't fix this issue.", "lklimek", 1).await;
    let first = w.item(L2).await["decisions"][0]["id"].clone();
    assert!(first.is_string());
    let second = w.decide_ok(L2, decision("fix_here")).await;
    assert_eq!(second["supersedes"], first);
    assert!(!second["effects"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn withdrawal_never_propagates_to_other_authors() {
    let w = world().await;
    let reviewer = w.item(L2).await["author"].as_str().unwrap().to_owned();
    assert_eq!(w.item(L6).await["author"], reviewer);
    assert_ne!(w.item(THREAD_7).await["author"], reviewer);
    assess_recorded(&w, "withdrawn", "I withdraw my finding.", &reviewer, 99).await;
    link(&w, L6).await;
    link(&w, THREAD_7).await;
    crate::review_loop::propagation::propagate(&w.state, DEFAULT_TENANT, w.pr_id().await)
        .await
        .unwrap();
    let primary = w.item(L2).await;
    assert_eq!(primary["decisions"][0]["via"], "imported_github");
    assert_eq!(primary["decisions"][0]["imported"]["by_id"], 99);
    assert_eq!(primary["decisions"][0]["effects"], json!([]));
    assert_eq!(w.item(L6).await["decisions"], json!([]));
    assert_eq!(w.item(THREAD_7).await["decisions"], json!([]));
}

#[tokio::test]
async fn import_without_cue_is_proposal() {
    let w = world().await;
    let evidence = assess_recorded(&w, "reject", "Interesting observation.", "lklimek", 1).await;
    let item = w.item(L2).await;
    assert_eq!(item["import_proposal"]["kind"], "reject");
    assert_eq!(item["import_proposal"]["evidence_url"], evidence);
    assert_eq!(item["decisions"], json!([]));
    assert_eq!(item["state"], "undecided");
    assert!(
        w.get(&format!("{PR}/comments")).await["queue"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| { entry["item_nos"].as_array().unwrap().contains(&item["no"]) })
    );
}

#[tokio::test]
async fn duplicate_artifacts_are_created_once_for_the_primary() {
    for (kind, effect, target) in [
        (
            "follow_up_pr",
            "follow_up_pr",
            json!({"type":"follow_up_pr","title":"Follow up"}),
        ),
        (
            "upstream_issue",
            "upstream_issue",
            json!({"type":"upstream_issue","url":null}),
        ),
        (
            "todo",
            "todo",
            json!({"type":"todo","title":"Track retry policy"}),
        ),
        ("fix_here", "fix", Value::Null),
    ] {
        let w = world().await;
        link(&w, L6).await;
        let mut request = decision(kind);
        request["target"] = target;
        let primary = w.decide_ok(L2, request).await;
        let primary_item = w.item(L2).await;
        let follower = w.item(L6).await;
        let work = primary["effects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["kind"] == effect)
            .unwrap();
        assert!(
            work["params"]["item_nos"]
                .as_array()
                .unwrap()
                .contains(&follower["no"]),
            "{kind}"
        );
        assert_eq!(
            work["params"]["text"],
            format!(
                "Review comments:\n{}\n{}",
                primary_item["url"].as_str().unwrap(),
                follower["url"].as_str().unwrap()
            ),
            "{kind}"
        );
        let effects = follower["decisions"][0]["effects"].as_array().unwrap();
        assert!(
            effects
                .iter()
                .all(|e| e["kind"] == "reply" || e["kind"] == "resolve")
        );
        assert!(
            effects
                .iter()
                .any(|e| e["kind"] == "reply" && e["state"] == "waiting")
        );
    }
}

#[tokio::test]
async fn duplicate_reply_waits_for_the_primary_outcome() {
    use contracts::api::review_loop::{EffectKind, ReportOutcome};
    use db::repo::review_loop::{ClaimOutcome, ReportOutcomeRecord, ReportRequest};

    for (kind, artefact, reference, outcome) in [
        (
            "follow_up_pr",
            EffectKind::FollowUpPr,
            "https://github.com/acme/node-client/pull/999",
            "https://github.com/acme/node-client/pull/999",
        ),
        (
            "fix_here",
            EffectKind::Fix,
            "aabbccd",
            "https://github.com/acme/node-client/commit/aabbccd",
        ),
    ] {
        let w = world().await;
        link(&w, L6).await;
        let primary = w.decide_ok(L2, decision(kind)).await;
        let primary_url = w.item(L2).await["url"].as_str().unwrap().to_owned();
        let follower = w.item(L6).await;
        let follower_id = serde_json::from_value(follower["decisions"][0]["id"].clone()).unwrap();
        let primary_id = serde_json::from_value(primary["id"].clone()).unwrap();
        let effect_id = serde_json::from_value(primary["effects"][0]["id"].clone()).unwrap();
        let (_, agent) = super::feed::agent(&w, &super::feed::EXECUTE).await;
        let rl = w.state.db.review_loop();
        let lease = SignedDuration::from_mins(5);
        let capabilities = [artefact, EffectKind::Reply, EffectKind::Resolve];
        assert_eq!(
            rl.claim(
                follower_id,
                &agent.token_id,
                &capabilities,
                lease,
                Timestamp::now()
            )
            .await
            .unwrap(),
            ClaimOutcome::NothingOpen
        );
        assert!(matches!(
            rl.claim(
                primary_id,
                &agent.token_id,
                &capabilities,
                lease,
                Timestamp::now()
            )
            .await
            .unwrap(),
            ClaimOutcome::Claimed(_)
        ));
        let request = ReportRequest {
            outcome: ReportOutcome::Done,
            reference: Some(reference.into()),
            message: None,
            idempotency_key: uuid::Uuid::new_v4().to_string(),
        };
        assert_eq!(
            rl.report(
                DEFAULT_TENANT,
                effect_id,
                &agent.token_id,
                &request,
                lease,
                Timestamp::now()
            )
            .await
            .unwrap(),
            ReportOutcomeRecord::Recorded
        );
        let follower = w.item(L6).await;
        let reply = &follower["decisions"][0]["effects"][0];
        assert_eq!(reply["state"], "open");
        assert_eq!(
            reply["params"]["text"],
            format!("Duplicate of {primary_url}\nTracked in {outcome}")
        );
    }
}

#[tokio::test]
async fn reopening_primary_reopens_its_followers() {
    let w = world().await;
    link(&w, L6).await;
    w.decide_ok(L2, reject("This exceeds the scope.")).await;
    let primary = w.item(L2).await;
    let (status, body) = w
        .post(
            &format!("{PR}/comments/items/C{}/reopen", primary["no"]),
            json!({"item_revision":primary["revision"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let follower = w.item(L6).await;
    assert_eq!(follower["state"], "reopened");
    assert_ne!(follower["decision_follows_primary"], true);
    assert!(
        follower["decisions"][0]["effects"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["state"] == "cancelled")
    );
}

#[tokio::test]
async fn owner_undo_fences_earlier_unapplied_imports() {
    let w = world().await;
    assess_recorded_with_owner_decision(
        &w,
        "reject",
        "We won't fix this issue.",
        "lklimek",
        1,
        Some(decision("fix_here")),
    )
    .await;
    let primary = w.item(L2).await;
    assert_eq!(primary["decisions"].as_array().unwrap().len(), 1);
    let pr = w.pr_id().await;
    assert!(
        w.state
            .db
            .review_loop()
            .items(pr)
            .await
            .unwrap()
            .into_iter()
            .find(|item| item.stable_key == L2)
            .and_then(|item| item.import_candidate)
            .is_some_and(|candidate| candidate.binding)
    );
    assert_eq!(
        w.post(
            &format!("{PR}/comments/items/C{}/undo", primary["no"]),
            json!({"item_revision":primary["revision"]})
        )
        .await
        .0,
        StatusCode::OK
    );
    w.decide_ok(L6, decision("fix_here")).await;
    crate::review_loop::propagation::propagate(&w.state, DEFAULT_TENANT, pr)
        .await
        .unwrap();
    let primary = w.item(L2).await;
    assert_eq!(primary["decisions"].as_array().unwrap().len(), 1);
    assert_eq!(primary["state"], "undecided");
}

#[tokio::test]
async fn claimed_duplicate_blocks_undo_and_supersede_with_its_number() {
    let w = world().await;
    link(&w, L6).await;
    w.decide_ok(L2, reject("This exceeds the scope.")).await;
    let primary = w.item(L2).await;
    let follower = w.item(L6).await;
    let (token, _) = super::feed::agent(&w, &super::feed::EXECUTE).await;
    let (claimed, _, _) = super::feed::with_token(
        &w,
        &token,
        "POST",
        &format!(
            "/api/v1/decision-feed/decisions/{}/claim",
            follower["decisions"][0]["id"].as_str().unwrap()
        ),
        Some(json!({"capabilities":["reply", "resolve"]})),
    )
    .await;
    assert_eq!(claimed, StatusCode::CREATED);
    let (undo, undo_error) = w
        .post(
            &format!("{PR}/comments/items/C{}/undo", primary["no"]),
            json!({"item_revision":primary["revision"]}),
        )
        .await;
    let mut request = reject("Revised scope.");
    request["item_revision"] = primary["revision"].clone();
    let (supersede, supersede_error) = w.decide(primary["no"].as_u64().unwrap(), request).await;
    assert_eq!(
        (undo, supersede),
        (StatusCode::CONFLICT, StatusCode::CONFLICT)
    );
    for error in [undo_error, supersede_error] {
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains(&format!("C{}", follower["no"])),
            "{error}"
        );
    }
}

#[tokio::test]
async fn new_primary_thread_text_reasks_its_followers() {
    let w = world().await;
    link(&w, L6).await;
    w.decide_ok(L2, reject("This exceeds the scope.")).await;
    let edited = |answer: &mut Value| {
        let body = &mut thread_node(answer, THREAD_2)["comments"]["nodes"][0]["body"];
        *body = json!(format!(
            "{}\nThis also affects retries after a reset.",
            body.as_str().unwrap()
        ));
    };
    w.reread(&[("review-threads.json", &edited)]).await;
    assert_eq!(w.item(L2).await["state"], "reopened");
    assert_eq!(w.item(L6).await["state"], "reopened");
}
