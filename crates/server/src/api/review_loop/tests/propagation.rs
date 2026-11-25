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
    assert_eq!(w.get(&uri).await["counts"]["imported_unseen"], 1);
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
        reply_node["createdAt"] = json!("2026-11-26T00:00:00Z");
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
    assert_ne!(w.item(THREAD_7).await["author"], reviewer);
    assess_recorded(&w, "withdrawn", "I withdraw my finding.", &reviewer, 42).await;
    link(&w, THREAD_7).await;
    crate::review_loop::propagation::propagate(&w.state, DEFAULT_TENANT, w.pr_id().await)
        .await
        .unwrap();
    assert_eq!(w.item(L2).await["decisions"][0]["via"], "imported_github");
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
