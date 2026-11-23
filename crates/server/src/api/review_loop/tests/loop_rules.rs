//! Review-loop rules around decisions and what agents report: carried reply texts, the dismissal question's list,
//! effect order, a resolution nobody claimed, recommendations shown, and what diffpilot reads back after a report
//! (plan §2.1, §2.3, §2.5, §2.6, §3.4, §4.1).

use std::time::Duration;

use contracts::api::review_loop::{EffectKind, WatchState};
use contracts::ids::CommentItemId;

use super::feed::{EXECUTE, agent, with_token};
use super::*;
use crate::review_loop::seen;

const FEED: &str = "/api/v1/decision-feed";

fn claim_uri(decision: &Value) -> String {
    format!(
        "{FEED}/decisions/{}/claim",
        decision["id"].as_str().expect("id")
    )
}

fn report_uri(effect: &Value) -> String {
    format!(
        "{FEED}/effects/{}/reports",
        effect["id"].as_str().expect("id")
    )
}

fn report(outcome: &str, reference: Option<&str>) -> Value {
    json!({"outcome": outcome, "reference": reference, "message": null,
        "idempotency_key": uuid::Uuid::new_v4()})
}

/// Plan §2.5: a carried decision has the same effects, so the reply the owner approved is posted again, not the
/// default text.
#[tokio::test]
async fn carried_decision_keeps_the_approved_reply_text() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let approved =
        "Per maintainer decision, not changed: the ceiling is set by the caller, see #490.";
    let mut body = reject("The ceiling is the caller's.");
    body["reply_text"] = json!(approved);
    let first = w.decide_ok(L2, body).await;
    assert_eq!(first["reply_text"], approved);
    let new_thread: &dyn Fn(&mut Value) = &|answer| {
        let thread = thread_node(answer, "PRRT_fixture482_l2");
        thread["id"] = json!("PRRT_fixture482_l2b");
        thread["comments"]["nodes"][0]["url"] =
            json!("https://github.com/acme/node-client/pull/482#discussion_r3000000999");
    };
    w.reread(&[("review-threads.json", new_thread)]).await;
    let item = w.item(L2).await;
    let carried = &item["decisions"][1];
    assert_eq!(carried["via"], "carried", "{item}");
    assert_eq!(carried["reply_text"], approved);
    assert_eq!(carried["effects"][0]["params"]["text"], approved);
    assert_eq!(
        carried["effects"][0]["params"]["thread_id"],
        "PRRT_fixture482_l2b"
    );
}

/// RQ11: the dismissal acts on the reviews its question lists while they are still stale; a listed bot whose item
/// is asked again makes the decision fail instead of dismissing its review.
#[tokio::test]
async fn dismissal_is_refused_when_a_listed_review_is_no_longer_stale() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let rows: Vec<Value> = w
        .items()
        .await
        .into_iter()
        .filter(|item| item["author"] == "thepastaclaw" && item["state"] != "outside")
        .map(|item| {
            let mut row = reject("Per the maintainers, out of scope.");
            row["item_no"] = item["no"].clone();
            row["item_revision"] = item["revision"].clone();
            row
        })
        .collect();
    let (status, body) = w
        .post(
            &format!("{PR}/comments/decision-batches"),
            json!({"via": "table", "rows": rows}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    w.reread(&[]).await;
    let dismiss = w.item("pr:3:dismiss").await;
    assert!(
        dismiss["body"]
            .as_str()
            .expect("body")
            .contains("thepastaclaw"),
        "{dismiss}"
    );
    let l2 = w.item(L2).await;
    let (status, reopened) = w
        .post(
            &format!("{PR}/comments/items/C{}/reopen", l2["no"]),
            json!({"item_revision": l2["revision"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reopened}");
    let mut body = decision("dismiss_stale_reviews");
    body["item_revision"] = dismiss["revision"].clone();
    let (status, refused) = w.decide(dismiss["no"].as_u64().expect("no"), body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("reviews this question lists changed")),
        "{refused}"
    );
    w.decide_ok("pr:3:dismiss", decision("keep_reviews")).await;
}

/// Plan §2.3: an effect with `after` waits for that one; a `done` report before it is refused.
#[tokio::test]
async fn done_before_the_awaited_effect_is_refused() {
    let w = world().await;
    let decision = super::feed::fix_here(&w).await;
    let (token, _) = agent(&w, &EXECUTE).await;
    let (status, _, claim) = with_token(
        &w,
        &token,
        "POST",
        &claim_uri(&decision),
        Some(json!({"capabilities": ["fix", "reply", "resolve"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{claim}");
    let reply = &decision["effects"][1];
    assert_eq!(reply["after"], 1);
    let (status, _, refused) = with_token(
        &w,
        &token,
        "POST",
        &report_uri(reply),
        Some(report(
            "done",
            Some("https://github.com/acme/node-client/pull/482#discussion_r1"),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    let (status, _, body) = with_token(
        &w,
        &token,
        "POST",
        &report_uri(&decision["effects"][0]),
        Some(report("done", Some("0123456789abcdef"))),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, _, body) = with_token(
        &w,
        &token,
        "POST",
        &report_uri(reply),
        Some(report(
            "done",
            Some("https://github.com/acme/node-client/pull/482#discussion_r1"),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// US17, plan §2.1: a thread resolved on GitHub while the decision's resolve waits unclaimed was resolved outside
/// diffpilot; the planned resolve is no resolution.
#[tokio::test]
async fn resolution_before_any_agent_claims_is_outside() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    w.decide_ok(L2, decision("fix_here")).await;
    let resolved: &dyn Fn(&mut Value) = &|answer| {
        let thread = thread_node(answer, "PRRT_fixture482_l2");
        thread["isResolved"] = json!(true);
        thread["resolvedBy"] = json!({"login": "alice"});
    };
    w.reread(&[("review-threads.json", resolved)]).await;
    let item = w.item(L2).await;
    assert_eq!(item["state"], "outside", "{item}");
    assert_eq!(item["outside"]["by"], "alice");
}

/// Plan §2.4: an assessment changing the recommendation changes the item, so a decision prepared on the earlier card
/// is refused as changed and the recommendation stored with a decision is the one shown; an unchanged one does not.
#[tokio::test]
async fn a_new_recommendation_changes_the_item() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let before = w.item(L2).await["revision"].as_u64().expect("revision");
    w.assess(L2, recommend(DecisionKind::FixHere, "real bug"), vec![])
        .await;
    let first = w.item(L2).await["revision"].as_u64().expect("revision");
    assert_eq!(first, before + 1, "a first recommendation is new");
    w.assess(L2, recommend(DecisionKind::FixHere, "real bug"), vec![])
        .await;
    assert_eq!(
        w.item(L2).await["revision"].as_u64(),
        Some(first),
        "the same recommendation again changes nothing"
    );
    let mut stale = decision("fix_here");
    stale["item_revision"] = json!(first);
    w.assess(L2, recommend(DecisionKind::Reject, "pre-existing"), vec![])
        .await;
    let no = w.item(L2).await["no"].as_u64().expect("no");
    let (status, body) = w.decide(no, stale).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "item_changed");
    let stored = w.decide_ok(L2, decision("fix_here")).await;
    assert_eq!(stored["recommendation"]["kind"], "reject");
}

/// Plan §4.1, §3.4: after an agent's `done` report diffpilot reads the pull request again and records each done
/// effect it finds on GitHub (fact badge); the report path schedules that read.
#[tokio::test]
async fn read_after_a_report_marks_what_github_shows() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let decision = w
        .decide_ok(L2, reject("The ceiling is the caller's."))
        .await;
    let (token, _) = agent(&w, &EXECUTE).await;
    let (status, _, claim) = with_token(
        &w,
        &token,
        "POST",
        &claim_uri(&decision),
        Some(json!({"capabilities": ["reply", "resolve"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{claim}");
    let reply_url = "https://github.com/acme/node-client/pull/482#discussion_r3000000555";
    for (effect, reference) in [
        (&decision["effects"][0], reply_url),
        (&decision["effects"][1], "PRRT_fixture482_l2"),
    ] {
        let (status, _, body) = with_token(
            &w,
            &token,
            "POST",
            &report_uri(effect),
            Some(report("done", Some(reference))),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let acted: &dyn Fn(&mut Value) = &move |answer| {
        let thread = thread_node(answer, "PRRT_fixture482_l2");
        thread["isResolved"] = json!(true);
        thread["resolvedBy"] = json!({"login": "lklimek"});
        let mut reply = thread["comments"]["nodes"][0].clone();
        reply["url"] = json!(reply_url);
        reply["body"] = json!("Per maintainer decision, not changed.");
        reply["author"] = json!({"login": "lklimek", "__typename": "User"});
        let nodes = thread["comments"]["nodes"]
            .as_array_mut()
            .expect("comments");
        nodes.push(reply);
    };
    let item: CommentItemId = serde_json::from_value(w.item(L2).await["id"].clone()).expect("id");
    serve(&w.server, &[("review-threads.json", acted)]).await;
    w.age_read().await;
    // Only the read the reports scheduled reads GitHub here: the database is polled, no page is loaded.
    let deadline = std::time::Instant::now() + seen::REPORT_READ_DELAY + Duration::from_secs(20);
    let effects = loop {
        let effects = w
            .state
            .db
            .review_loop()
            .item_decisions(item)
            .await
            .expect("decisions")[0]
            .decision
            .effects
            .clone();
        if effects
            .iter()
            .all(|effect| effect.seen_on_github_at.is_some())
        {
            break effects;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "not marked seen: {effects:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(
        effects.iter().map(|effect| effect.kind).collect::<Vec<_>>(),
        [EffectKind::Reply, EffectKind::Resolve]
    );
    assert_ne!(
        w.item(L2).await["state"],
        "outside",
        "resolved by the agent diffpilot handed it to"
    );
}

/// Plan §2.8, §4.2: while today's limit holds the reader's automatic passes back, an item not assessed says so and
/// the progress counts the items it holds back.
#[tokio::test]
async fn items_the_daily_limit_holds_back_say_so() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let rl = w.state.db.review_loop();
    for pass in rl.live_passes().await.expect("passes") {
        rl.finish_pass(pass.id, false, Timestamp::now())
            .await
            .expect("finish");
    }
    let pr = w.pr_id().await;
    rl.watch(DEFAULT_TENANT, "u1", pr, 10, Timestamp::now())
        .await
        .expect("watch");
    let before = w.item(L2).await;
    assert_eq!(before["assessment_state"]["reason"], "failed", "{before}");
    rl.set_watch_state("u1", pr, WatchState::LimitReached, Timestamp::now())
        .await
        .expect("limit");
    assert_eq!(
        w.item(L2).await["assessment_state"],
        json!({"state": "not_assessed", "reason": "daily_limit"})
    );
    let page = w.get(PR).await;
    assert!(
        page["comments"]["assessment"]["limited"]
            .as_u64()
            .is_some_and(|limited| limited > 0),
        "{page}"
    );
}

/// RC9, RC17: an item asked again is decided on its own; a bulk batch with it is refused, as "Decide the rest" leaves
/// it out, while the decision table still takes it.
#[tokio::test]
async fn bulk_batch_refuses_an_item_asked_again() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    w.decide_ok(L2, decision("fix_here")).await;
    let l2 = w.item(L2).await;
    let (status, reopened) = w
        .post(
            &format!("{PR}/comments/items/C{}/reopen", l2["no"]),
            json!({"item_revision": l2["revision"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reopened}");
    let mut row = reject("The ceiling is the caller's.");
    row["item_no"] = reopened["no"].clone();
    row["item_revision"] = reopened["revision"].clone();
    let (status, refused) = w
        .post(
            &format!("{PR}/comments/decision-batches"),
            json!({"via": "bulk", "rows": [row.clone()]}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "invalid_request");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("asked again")),
        "{refused}"
    );
    assert_eq!(w.item(L2).await["state"], "reopened");
    let (status, stored) = w
        .post(
            &format!("{PR}/comments/decision-batches"),
            json!({"via": "table", "rows": [row]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{stored}");
}
