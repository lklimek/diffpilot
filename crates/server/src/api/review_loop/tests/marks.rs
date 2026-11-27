use super::*;

const LIST: &str = "/api/v1/review-loop/not-a-review-point";
const MARK: &str = "not_a_review_point";
const L7: &str = "PRRT_fixture482_l7";

async fn mark(w: &World, key: &str) -> Value {
    w.decide_ok(key, decision(MARK)).await
}

async fn undo(w: &World, key: &str) -> (StatusCode, Value) {
    let item = w.item(key).await;
    w.post(
        &format!("{PR}/comments/items/C{}/undo", item["no"]),
        json!({"item_revision": item["revision"]}),
    )
    .await
}

async fn gate(w: &World, key: &str) {
    let item = w.item(key).await;
    let (status, body) = w
        .post(
            &format!("{PR}/comments/items/C{}/gates", item["no"]),
            json!({"gate": "G-DATA", "marked": true, "item_revision": item["revision"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

fn row(item: &Value, kind: &str, reason: Option<&str>) -> Value {
    let mut request = decision(kind);
    request["item_revision"] = item["revision"].clone();
    request["reason"] = json!(reason);
    request["item_no"] = item["no"].clone();
    request
}

async fn batch(w: &World, via: &str, rows: &[Value]) -> (StatusCode, Value) {
    w.post(
        &format!("{PR}/comments/decision-batches"),
        json!({"via": via, "rows": rows}),
    )
    .await
}

#[tokio::test]
async fn mark_has_no_effects_and_no_feed_entry() {
    let w = world().await;
    let item = w.item(L2).await;
    let requests = w.server.received_requests().await.unwrap().len();
    let (status, preview) = w
        .post(
            &format!("{PR}/comments/effects-preview"),
            json!({"rows": [row(&item, MARK, None)]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview[0]["effects"], json!([]));
    let mut request = decision(MARK);
    request["reply_text"] = json!("Never send this to GitHub.");
    request["comment"] = json!("Never send this to an agent.");
    let stored = w.decide_ok(L2, request).await;
    assert_eq!(stored["kind"], MARK);
    assert_eq!(stored["effects"], json!([]));
    assert!(stored["reply_text"].is_null());
    let rl = w.state.db.review_loop();
    assert!(rl.pr_decisions(w.pr_id().await).await.unwrap().is_empty());
    assert!(rl.open_feed(Timestamp::now()).await.unwrap().is_empty());
    assert_eq!(w.server.received_requests().await.unwrap().len(), requests);

    assert_eq!(undo(&w, L2).await.0, StatusCode::OK);
    w.decide_ok(L2, decision("fix_here")).await;
    propagation::link(&w, L6).await;
    mark(&w, L7).await;
    assert_eq!(w.item(L6).await["decisions"], json!([]));
}

#[tokio::test]
async fn marked_item_leaves_needs_you_and_table_waiting() {
    let w = world().await;
    w.assess(L2, recommend(DecisionKind::FixHere, "fix"), vec![])
        .await;
    gate(&w, L2).await;
    let before = w.get(&format!("{PR}/comments")).await;
    let item = w.item(L2).await;
    assert!(before["counts"]["needs_you_gates"].as_u64().unwrap() > 0);
    let mut request = decision(MARK);
    request["reason"] = json!("The same status is already shown above.");
    w.decide_ok(L2, request).await;
    let after = w.get(&format!("{PR}/comments")).await;
    for key in ["needs_you", "needs_you_gates", "undecided"] {
        assert_eq!(
            after["counts"][key].as_u64().unwrap() + 1,
            before["counts"][key].as_u64().unwrap(),
            "{key}"
        );
    }
    assert_eq!(after["counts"][MARK], 1);
    assert!(
        !after["queue"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| { entry["item_nos"].as_array().unwrap().contains(&item["no"]) })
    );
    let other = w.item(L6).await;
    let (status, body) = batch(&w, "table", &[row(&other, "fix_here", None)]).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

#[tokio::test]
async fn marked_item_not_sent_to_assessment() {
    let w = world().await;
    mark(&w, L2).await;
    let item = w.item(L2).await;
    let pr = w.pr_id().await;
    let rl = w.state.db.review_loop();
    let (status, body) = w
        .post(
            &format!("{PR}/comments/items/C{}/assess", item["no"]),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(rl.latest_pass(pr).await.unwrap().is_none());
    let (status, body) = w.post(&format!("{PR}/comments/assess"), json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let sent = rl.latest_pass(pr).await.unwrap().unwrap().sent;
    assert!(!sent.is_empty());
    assert!(sent.iter().all(|sent| json!(sent.item) != item["id"]));
}

#[tokio::test]
async fn gate_item_mark_requires_reason_and_single_row() {
    let w = world().await;
    gate(&w, L2).await;
    gate(&w, L6).await;
    let item = w.item(L2).await;
    let other = w.item(L6).await;
    for (via, request) in [
        ("table", vec![row(&item, MARK, Some("  "))]),
        (
            "table",
            vec![
                row(&item, MARK, Some("Status only.")),
                row(&other, "fix_here", None),
            ],
        ),
        ("group", vec![row(&item, MARK, Some("Status only."))]),
    ] {
        assert!(batch(&w, via, &request).await.0.is_client_error());
        assert!(w.item(L2).await[MARK].is_null());
        assert!(w.item(L6).await["decisions"].as_array().unwrap().is_empty());
    }
    let (status, body) = batch(&w, "table", &[row(&item, MARK, Some(" Status only. "))]).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body[0]["reason"], "Status only.");
}

#[tokio::test]
async fn bulk_never_marks() {
    let w = world().await;
    w.assess(
        L2,
        recommend(DecisionKind::FixHere, "normal recommendation"),
        vec![],
    )
    .await;
    let item = w.item(L2).await;
    let rest = w.get(&format!("{PR}/comments/decide-rest")).await;
    assert!(
        rest["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["row"]["decision"]["kind"] != MARK)
    );
    let (status, _) = batch(&w, "bulk", &[row(&item, MARK, None)]).await;
    assert!(status.is_client_error());
    assert!(w.item(L2).await[MARK].is_null());
    let (status, body) = batch(&w, "group", &[row(&item, MARK, None)]).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

#[tokio::test]
async fn primary_mark_marks_non_gate_followers() {
    let w = world().await;
    propagation::link(&w, L6).await;
    mark(&w, L7).await;
    propagation::link(&w, L7).await;
    mark(&w, L2).await;
    let primary = w.item(L2).await;
    let follower = w.item(L6).await;
    assert_eq!(follower[MARK]["follows"], primary["no"]);
    assert_eq!(follower[MARK]["by"], primary[MARK]["by"]);
    assert_eq!(follower["decisions"], json!([]));
    assert!(w.item(L7).await[MARK]["follows"].is_null());
}

#[tokio::test]
async fn gate_duplicate_never_follows_mark() {
    for gate_first in [true, false] {
        let w = world().await;
        propagation::link(&w, L6).await;
        if gate_first {
            gate(&w, L6).await;
        }
        mark(&w, L2).await;
        if !gate_first {
            assert!(w.item(L6).await[MARK]["follows"].is_number());
            gate(&w, L6).await;
        }
        assert!(w.item(L6).await[MARK].is_null());
    }
}

#[tokio::test]
async fn undo_primary_mark_undoes_follow_marks() {
    let w = world().await;
    propagation::link(&w, L6).await;
    mark(&w, L2).await;
    let follower = w.item(L6).await;
    assert!(follower[MARK]["follows"].is_number());
    let (status, body) = undo(&w, L2).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(w.item(L6).await[MARK].is_null());
    let log = w.get(&format!("{PR}/comments/log")).await;
    assert_eq!(log["marks"].as_array().unwrap().len(), 2);
    assert!(
        log["marks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|mark| { mark["undone_by"] == "lklimek" && mark["undone_at"].is_string() })
    );
}

#[tokio::test]
async fn later_decision_ends_mark() {
    let w = world().await;
    mark(&w, L2).await;
    let stored = w.decide_ok(L2, decision("fix_here")).await;
    assert!(!stored["effects"].as_array().unwrap().is_empty());
    assert!(w.item(L2).await[MARK].is_null());
    let log = w.get(&format!("{PR}/comments/log")).await;
    assert_eq!(log["marks"][0]["undone_by"], "lklimek");
    assert_eq!(log["marks"][0]["undone_at"], stored["decided_at"]);
}

#[tokio::test]
async fn undo_ends_mark() {
    let w = world().await;
    let before = w.item(L2).await;
    mark(&w, L2).await;
    let (status, body) = undo(&w, L2).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body[MARK].is_null());
    assert_eq!(body["state"], before["state"]);
    assert_eq!(
        body["revision"].as_u64().unwrap(),
        before["revision"].as_u64().unwrap() + 2
    );
    assert_eq!(undo(&w, L2).await.0, StatusCode::CONFLICT);
    let log = w.get(&format!("{PR}/comments/log")).await;
    assert_eq!(log["marks"][0]["by"], "lklimek");
    assert_eq!(log["marks"][0]["undone_by"], "lklimek");
    assert!(log["marks"][0]["at"].is_string());
    assert!(log["marks"][0]["undone_at"].is_string());
}

#[tokio::test]
async fn no_import_on_marked_item() {
    for text in [
        "Interesting observation.",
        "We will fix this in the current PR.",
    ] {
        let w = world().await;
        if text == "Interesting observation." {
            propagation::assess_recorded(&w, "fix_here", text, "lklimek", 1).await;
            assert_eq!(w.item(L2).await["import_proposal"]["kind"], "fix_here");
            mark(&w, L2).await;
        } else {
            // The already admitted assessment completes with binding evidence after the owner marks.
            propagation::assess_recorded_with_owner_decision(
                &w,
                "fix_here",
                text,
                "lklimek",
                1,
                Some(decision(MARK)),
            )
            .await;
        }
        let item = w.item(L2).await;
        assert!(item[MARK].is_object());
        assert!(item["imported_decision"].is_null());
        assert!(item["import_proposal"].is_null());
    }
}

#[tokio::test]
async fn marks_list_tenant_scoped_newest_first() {
    let mut w = world().await;
    mark(&w, L2).await;
    mark(&w, L6).await;
    let first = w.get(LIST).await;
    let items = first["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    for (listed, key) in items.iter().zip([L6, L2]) {
        let item = w.item(key).await;
        for field in ["no", "kind", "author", "author_kind", "title", "url"] {
            assert_eq!(listed[field], item[field]);
        }
        assert_eq!(listed["by"], "lklimek");
        assert_eq!(listed["status_comment"], false);
        assert_eq!(listed["pr"]["number"], 482);
    }
    assert!(first["next_cursor"].is_null());
    assert_eq!(
        w.call("GET", &format!("{LIST}?cursor=invalid"), None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(undo(&w, L2).await.0, StatusCode::OK);
    assert_eq!(w.get(LIST).await["items"].as_array().unwrap().len(), 1);
    db::repo::analyses::testing::seed_tenant(&w.state.db, "other")
        .await
        .unwrap();
    w.cookie = session_cookie(&w.state, "other").await;
    assert_eq!(w.get(LIST).await["items"], json!([]));
}

#[tokio::test]
async fn later_duplicate_follows_mark_until_undone() {
    let w = world().await;
    mark(&w, L2).await;
    propagation::link(&w, L6).await;
    let pr = w.pr_id().await;
    crate::review_loop::propagation::propagate(&w.state, DEFAULT_TENANT, pr)
        .await
        .unwrap();
    assert_eq!(w.item(L6).await[MARK]["follows"], w.item(L2).await["no"]);
    assert_eq!(undo(&w, L6).await.0, StatusCode::OK);
    crate::review_loop::propagation::propagate(&w.state, DEFAULT_TENANT, pr)
        .await
        .unwrap();
    assert!(w.item(L6).await[MARK].is_null());
    assert!(w.item(L2).await[MARK].is_object());
}

#[tokio::test]
async fn marked_bot_point_does_not_block_stale_review_dismissal() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let mut points: Vec<_> = w
        .items()
        .await
        .into_iter()
        .filter(|item| {
            item["author"] == "thepastaclaw"
                && item["state"] == "undecided"
                && item["status_comment"] != true
        })
        .collect();
    assert!(points.len() > 1);
    let marked = points.pop().unwrap();
    mark(&w, marked["stable_key"].as_str().unwrap()).await;
    let mut decisions = Vec::new();
    for point in &points {
        let mut request = row(point, "reject", Some("Out of scope."));
        request["resolve_ticked"] = json!(true);
        decisions.push(request);
    }
    let (status, body) = batch(&w, "table", &decisions).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    w.reread(&[]).await;
    let decision = w
        .decide_ok("pr:3:dismiss", decision("dismiss_stale_reviews"))
        .await;
    assert!(
        decision["effects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|effect| {
                effect["kind"] == "dismiss_review"
                    && effect["params"]["review_id"] == "PRR_fixture482_r5"
            })
    );
}

#[tokio::test]
async fn mark_cannot_hide_a_pr_question() {
    let w = world().await;
    let question = w.item("pr:3:merge").await;
    assert_eq!(question["kind"], "pr_question");
    let request = row(&question, MARK, None);
    let (status, body) = w
        .decide(question["no"].as_u64().unwrap(), request.clone())
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = batch(&w, "group", &[request]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(w.item("pr:3:merge").await[MARK].is_null());
}
