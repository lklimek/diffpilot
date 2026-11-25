//! Review-loop rules around decisions and what agents report: carried reply texts, the dismissal question's list,
//! effect order, a resolution nobody claimed, recommendations shown, and what diffpilot reads back after a report
//! (plan §2.1, §2.3, §2.5, §2.6, §3.4, §4.1).

use std::time::Duration;

use contracts::api::review_loop::{EffectKind, ItemState, WatchState};
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
    let reported = reported_reply_and_resolve(&w).await;
    serve(&w.server, &[("review-threads.json", &acted_on_l2)]).await;
    w.age_read().await;
    // Only the read the reports scheduled reads GitHub here: the database is polled, no page is loaded.
    wait_until_seen(
        &w,
        reported.item,
        seen::REPORT_READ_DELAY + Duration::from_secs(20),
    )
    .await;
    let kinds: Vec<EffectKind> = reported
        .reports
        .iter()
        .map(|(effect, _)| serde_json::from_value(effect["kind"].clone()).expect("kind"))
        .collect();
    assert_eq!(kinds, [EffectKind::Reply, EffectKind::Resolve]);
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
    w.request_pass().await;
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

fn queued(overview: &Value, no: &Value) -> bool {
    overview["queue"]
        .as_array()
        .expect("queue")
        .iter()
        .any(|entry| {
            entry["item_nos"]
                .as_array()
                .is_some_and(|nos| nos.contains(no))
        })
}

/// Plan §2.1: an item a complete read no longer produces stays listed with that fact (never as resolved on GitHub),
/// leaves the queue, "Decide the rest" and bulk decisions, and returns when a read produces it again; an incomplete
/// read marks nothing.
#[tokio::test]
async fn items_no_longer_read_leave_the_queue_and_stay_listed() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let no = w.item(L2).await["no"].clone();
    let removed: &dyn Fn(&mut Value) = &|answer| {
        answer["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"]
            .as_array_mut()
            .expect("threads")
            .retain(|thread| thread["id"] != "PRRT_fixture482_l2");
    };
    let partial: &dyn Fn(&mut Value) = &|answer| {
        removed(answer);
        answer["data"]["repository"]["pullRequest"]["reviewThreads"]["pageInfo"] =
            json!({"hasNextPage": true, "endCursor": null});
    };
    w.reread(&[("review-threads.json", partial)]).await;
    let kept = w.item(L2).await;
    assert!(kept.get("gone_at").is_none(), "an incomplete read: {kept}");

    w.reread(&[("review-threads.json", removed)]).await;
    let gone = w.item(L2).await;
    assert!(gone["gone_at"].is_string(), "{gone}");
    assert_eq!(gone["state"], "undecided");
    assert!(gone["outside"].is_null(), "{gone}");
    let overview = w.get(&format!("{PR}/comments")).await;
    assert!(!queued(&overview, &no), "{overview}");
    let rest = w.get(&format!("{PR}/comments/decide-rest")).await;
    let listed = |key: &str| {
        rest[key]
            .as_array()
            .expect("list")
            .iter()
            .any(|entry| entry["item_no"] == no || entry["row"]["item_no"] == no)
    };
    assert!(!listed("rows") && !listed("left_out"), "{rest}");
    let mut row = reject("Out of scope.");
    row["item_no"] = no.clone();
    row["item_revision"] = gone["revision"].clone();
    let (status, refused) = w
        .post(
            &format!("{PR}/comments/decision-batches"),
            json!({"via": "bulk", "rows": [row]}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("not in the latest read from GitHub")),
        "{refused}"
    );

    w.reread(&[]).await;
    let back = w.item(L2).await;
    assert!(back.get("gone_at").is_none(), "{back}");
    assert!(queued(&w.get(&format!("{PR}/comments")).await, &no));
}

/// r1's body finding; the fixture's r2 carries it, its latest round r5 does not list it (S13-1 R5).
const R1_BODY: &str = "review:PRR_fixture482_r1:1";

/// The recorded reviews without round r5: r2, which carries r1's body finding, is the latest round.
fn without_r5(answer: &mut Value) {
    answer["data"]["repository"]["pullRequest"]["reviews"]["nodes"]
        .as_array_mut()
        .expect("reviews")
        .retain(|review| review["id"] != "PRR_fixture482_r5");
}

/// The recorded reviews plus round r6 after r5, a copy of r2: it carries r1's body finding again.
fn carried_again(answer: &mut Value) {
    let nodes = answer["data"]["repository"]["pullRequest"]["reviews"]["nodes"]
        .as_array_mut()
        .expect("reviews");
    let mut round = nodes
        .iter()
        .find(|review| review["id"] == "PRR_fixture482_r2")
        .cloned()
        .expect("r2");
    round["id"] = json!("PRR_fixture482_r6");
    round["submittedAt"] = json!("2026-11-12T12:00:00Z");
    nodes.push(round);
}

/// Plan §2.1 I2 (owner, 2026-11-23): a bot round that no longer lists an earlier body finding supersedes it: the
/// item is marked `gone_at`, leaves the queue and stops counting as open in its round, and returns as the same item
/// when a later round carries it again.
#[tokio::test]
async fn a_later_round_supersedes_an_earlier_body_finding() {
    let w = world().await;
    let latest_r2: &dyn Fn(&mut Value) = &without_r5;
    serve(&w.server, &[("reviews.json", latest_r2)]).await;
    let before = w.get(&format!("{PR}/comments")).await;
    let open = w.item(R1_BODY).await;
    assert!(open.get("gone_at").is_none(), "r2 carries it: {open}");
    let no = open["no"].clone();
    assert!(queued(&before, &no), "{before}");

    w.reread(&[]).await;
    let superseded = w.item(R1_BODY).await;
    assert!(
        superseded["gone_at"].is_string(),
        "r5 does not list it: {superseded}"
    );
    assert_eq!(superseded["state"], "undecided");
    let after = w.get(&format!("{PR}/comments")).await;
    assert!(!queued(&after, &no), "{after}");
    let round_one = |overview: &Value, key: &str| overview["rounds"][0][key].as_u64().expect(key);
    for key in ["still_open", "undecided"] {
        assert_eq!(
            round_one(&after, key) + 1,
            round_one(&before, key),
            "{key}: {after}"
        );
    }

    let latest_r6: &dyn Fn(&mut Value) = &carried_again;
    w.reread(&[("reviews.json", latest_r6)]).await;
    let back = w.item(R1_BODY).await;
    assert!(back.get("gone_at").is_none(), "r6 carries it again: {back}");
    assert_eq!(back["no"], no, "the same item");
}

/// Plan §2.1 I2: a superseded body finding keeps its decision, and the decision's open effects stay claimable.
#[tokio::test]
async fn a_superseded_body_finding_keeps_its_decision_and_open_effects() {
    let w = world().await;
    let latest_r2: &dyn Fn(&mut Value) = &without_r5;
    serve(&w.server, &[("reviews.json", latest_r2)]).await;
    w.get(&format!("{PR}/comments")).await;
    let decided = w.decide_ok(R1_BODY, decision("fix_here")).await;
    let kinds: Vec<&Value> = decided["effects"]
        .as_array()
        .expect("effects")
        .iter()
        .map(|effect| &effect["kind"])
        .collect();
    assert_eq!(kinds, [&json!("fix"), &json!("pr_comment")], "{decided}");
    assert_eq!(
        decided["effects"][1]["params"]["review_id"],
        "PRR_fixture482_r1"
    );

    w.reread(&[]).await;
    let item = w.item(R1_BODY).await;
    assert!(item["gone_at"].is_string(), "{item}");
    assert_eq!(item["state"], "executing", "{item}");
    assert_eq!(item["decisions"][0]["id"], decided["id"]);
    let (token, _) = agent(&w, &EXECUTE).await;
    let (status, _, page) = with_token(
        &w,
        &token,
        "GET",
        &format!("{FEED}?capabilities=fix,pr_comment&repo=acme/node-client"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(
        page["decisions"]
            .as_array()
            .expect("decisions")
            .iter()
            .any(|listed| listed["decision"]["id"] == decided["id"]),
        "{page}"
    );
    let capabilities = json!({"capabilities": ["fix", "pr_comment"]});
    let (status, _, claim) =
        with_token(&w, &token, "POST", &claim_uri(&decided), Some(capabilities)).await;
    assert_eq!(status, StatusCode::CREATED, "{claim}");
}

/// The recorded reviews with round r5 dismissed, as GitHub shows it after RQ11's dismissal or a maintainer's.
fn r5_dismissed(answer: &mut Value) {
    for review in answer["data"]["repository"]["pullRequest"]["reviews"]["nodes"]
        .as_array_mut()
        .expect("reviews")
    {
        if review["id"] == "PRR_fixture482_r5" {
            review["state"] = json!("DISMISSED");
        }
    }
}

/// Plan §2.1 I2 with RQ11: dismissing the latest round (P3 "Dismiss stale bot reviews", or a maintainer on GitHub)
/// does not reopen the earlier body findings that round superseded.
#[tokio::test]
async fn dismissing_the_latest_round_does_not_reopen_superseded_body_findings() {
    let w = world().await;
    let latest_r2: &dyn Fn(&mut Value) = &without_r5;
    serve(&w.server, &[("reviews.json", latest_r2)]).await;
    w.get(&format!("{PR}/comments")).await;
    let no = w.item(R1_BODY).await["no"].clone();
    w.reread(&[]).await;
    assert!(
        w.item(R1_BODY).await["gone_at"].is_string(),
        "r5 supersedes it"
    );

    let dismissed: &dyn Fn(&mut Value) = &r5_dismissed;
    w.reread(&[("reviews.json", dismissed)]).await;
    let after = w.get(&format!("{PR}/comments")).await;
    let item = w.item(R1_BODY).await;
    assert!(
        item["gone_at"].is_string() && !queued(&after, &no),
        "dismissing r5 brings r1's superseded finding back into the queue: {item}"
    );
}

/// Plan §2.1: an item asked again that a later round then supersedes waits for nobody, so the overview no longer
/// counts it as asked again.
#[tokio::test]
async fn a_superseded_item_asked_again_is_not_counted_as_asked_again() {
    let w = world().await;
    let latest_r2: &dyn Fn(&mut Value) = &without_r5;
    serve(&w.server, &[("reviews.json", latest_r2)]).await;
    w.get(&format!("{PR}/comments")).await;
    w.decide_ok(R1_BODY, decision("resolve")).await;
    let item = w.item(R1_BODY).await;
    let (status, reopened) = w
        .post(
            &format!("{PR}/comments/items/C{}/reopen", item["no"]),
            json!({"item_revision": item["revision"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reopened}");
    let before = w.get(&format!("{PR}/comments")).await;
    assert_eq!(before["counts"]["asked_again"], 1, "{before}");

    w.reread(&[]).await;
    assert!(w.item(R1_BODY).await["gone_at"].is_string());
    let after = w.get(&format!("{PR}/comments")).await;
    assert_eq!(after["counts"]["asked_again"], 0, "{after}");
}

/// The RC16 preview of a decision of `kind` on the item `key`: (the decision body, its `plan_digest`).
async fn previewed(w: &World, key: &str, kind: &str) -> (Value, Value) {
    let item = w.item(key).await;
    let mut body = decision(kind);
    body["item_revision"] = item["revision"].clone();
    let mut row = body.clone();
    row["item_no"] = item["no"].clone();
    let (status, preview) = w
        .post(
            &format!("{PR}/comments/effects-preview"),
            json!({"rows": [row]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let digest = preview[0]["plan_digest"].clone();
    assert!(digest.is_string(), "{preview}");
    (body, digest)
}

/// Thepastaclaw's items decided in one table batch, then read again: the question "Dismiss stale bot reviews?" waits.
async fn bot_items_decided(w: &World) {
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
}

/// RQ4, RQ5: "Merge now" carries the digest of the plan the owner confirmed; an item that joined its scope since
/// (here asked again) refuses the decision until the owner checks the new plan, which then includes it.
#[tokio::test]
async fn merge_now_is_refused_when_its_confirmed_plan_grew() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    w.decide_ok(L2, reject("The ceiling is the caller's."))
        .await;
    let (mut body, digest) = previewed(&w, "pr:3:merge", "merge_with_follow_ups").await;
    let l2 = w.item(L2).await;
    let (status, reopened) = w
        .post(
            &format!("{PR}/comments/items/C{}/reopen", l2["no"]),
            json!({"item_revision": l2["revision"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reopened}");
    body["plan_digest"] = digest;
    let merge = w.item("pr:3:merge").await;
    let (status, refused) = w
        .decide(merge["no"].as_u64().expect("no"), body.clone())
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "conflict", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("Check the plan again")),
        "{refused}"
    );
    let (_, digest) = previewed(&w, "pr:3:merge", "merge_with_follow_ups").await;
    body["plan_digest"] = digest;
    let (status, stored) = w.decide(merge["no"].as_u64().expect("no"), body).await;
    assert_eq!(status, StatusCode::CREATED, "{stored}");
    assert!(
        stored["effects"]
            .as_array()
            .expect("effects")
            .iter()
            .any(|effect| effect["params"]["item_nos"]
                .as_array()
                .is_some_and(|nos| nos.contains(&l2["no"]))),
        "{stored}"
    );
}

/// RQ4, RQ5: the digest binds everything "Merge now" would do, not only which items at which revisions: a stored
/// read that maps an item to its thread differently (no item revision changes) turns a PR comment into a thread
/// reply and resolution, which refuses the confirmed plan.
#[tokio::test]
async fn merge_now_is_refused_when_its_effects_changed_without_a_revision() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let (mut body, digest) = previewed(&w, "pr:3:merge", "merge_with_follow_ups").await;
    let id = w.pr_id().await;
    let rl = w.state.db.review_loop();
    let row = rl.pr_state(id).await.expect("state").expect("row");
    let mut snapshot = row.github_snapshot.expect("snapshot");
    let mapped = snapshot["thread_ids"]
        .as_object_mut()
        .expect("thread ids")
        .remove(L2);
    assert!(mapped.is_some(), "{snapshot}");
    rl.record_read(
        id,
        &ReadResult::Read {
            snapshot,
            round_count: row.round_count,
        },
        Timestamp::now(),
    )
    .await
    .expect("read stored");
    body["plan_digest"] = digest;
    let merge = w.item("pr:3:merge").await;
    let (status, refused) = w.decide(merge["no"].as_u64().expect("no"), body).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "conflict", "{refused}");
}

/// RQ11: the dismissal's digest covers the listed reviews and whether they are still stale, not the items' other
/// changes: an item of the listed bot finishing its effects (executing to done) leaves it valid.
#[tokio::test]
async fn dismissal_keeps_its_plan_through_harmless_changes() {
    let w = world().await;
    bot_items_decided(&w).await;
    let (mut body, digest) = previewed(&w, "pr:3:dismiss", "dismiss_stale_reviews").await;
    let rl = w.state.db.review_loop();
    let id = w.pr_id().await;
    let executing = rl
        .items(id)
        .await
        .expect("items")
        .into_iter()
        .find(|item| item.author == "thepastaclaw" && item.state == ItemState::Executing)
        .expect("an executing bot item");
    rl.set_item_state(
        executing.id,
        executing.revision,
        ItemState::Done,
        Timestamp::now(),
    )
    .await
    .expect("done");
    body["plan_digest"] = digest;
    let dismiss = w.item("pr:3:dismiss").await;
    let (status, stored) = w.decide(dismiss["no"].as_u64().expect("no"), body).await;
    assert_eq!(status, StatusCode::CREATED, "{stored}");
}

/// RQ11: a newer change request of a listed bot (empty, so it raises no item) makes the listed review no longer the
/// one that applies: the dismissal is refused instead of dismissing it.
#[tokio::test]
async fn dismissal_is_refused_after_a_newer_change_request() {
    let w = world().await;
    bot_items_decided(&w).await;
    let (mut body, digest) = previewed(&w, "pr:3:dismiss", "dismiss_stale_reviews").await;
    let newer: &dyn Fn(&mut Value) = &|answer| {
        let nodes = answer["data"]["repository"]["pullRequest"]["reviews"]["nodes"]
            .as_array_mut()
            .expect("reviews");
        let mut review = nodes
            .iter()
            .find(|review| review["id"] == "PRR_fixture482_r5")
            .expect("r5")
            .clone();
        review["id"] = json!("PRR_fixture482_r6");
        review["databaseId"] = json!(4_000_000_109_u64);
        review["submittedAt"] = json!("2026-11-12T11:00:00Z");
        review["body"] = json!("");
        review["url"] =
            json!("https://github.com/acme/node-client/pull/482#pullrequestreview-4000000109");
        nodes.push(review);
    };
    w.reread(&[("reviews.json", newer)]).await;
    body["plan_digest"] = digest;
    let dismiss = w.item("pr:3:dismiss").await;
    body["item_revision"] = dismiss["revision"].clone();
    let (status, refused) = w.decide(dismiss["no"].as_u64().expect("no"), body).await;
    assert_ne!(status, StatusCode::CREATED, "{refused}");
    assert!(
        w.item("pr:3:dismiss").await["decisions"]
            .as_array()
            .expect("decisions")
            .is_empty()
    );
}

const REPLY_URL: &str = "https://github.com/acme/node-client/pull/482#discussion_r3000000555";

/// L2 decided, its reply and resolve claimed by an agent and reported `done` (each report sent once).
struct Reported {
    item: CommentItemId,
    token: String,
    /// (effect, report body) of the reply and of the resolve.
    reports: Vec<(Value, Value)>,
}

async fn reported_reply_and_resolve(w: &World) -> Reported {
    w.get(&format!("{PR}/comments")).await;
    let decision = w
        .decide_ok(L2, reject("The ceiling is the caller's."))
        .await;
    let (token, _) = agent(w, &EXECUTE).await;
    let (status, _, claim) = with_token(
        w,
        &token,
        "POST",
        &claim_uri(&decision),
        Some(json!({"capabilities": ["reply", "resolve"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{claim}");
    let mut reports = Vec::new();
    for (effect, reference) in [
        (&decision["effects"][0], REPLY_URL),
        (&decision["effects"][1], "PRRT_fixture482_l2"),
    ] {
        let body = report("done", Some(reference));
        let (status, _, answer) =
            with_token(w, &token, "POST", &report_uri(effect), Some(body.clone())).await;
        assert_eq!(status, StatusCode::CREATED, "{answer}");
        reports.push((effect.clone(), body));
    }
    Reported {
        item: serde_json::from_value(w.item(L2).await["id"].clone()).expect("id"),
        token,
        reports,
    }
}

/// GitHub after the agent acted on L2: the reply at [`REPLY_URL`], the thread resolved.
fn acted_on_l2(answer: &mut Value) {
    let thread = thread_node(answer, "PRRT_fixture482_l2");
    thread["isResolved"] = json!(true);
    thread["resolvedBy"] = json!({"login": "lklimek"});
    let mut reply = thread["comments"]["nodes"][0].clone();
    reply["url"] = json!(REPLY_URL);
    reply["body"] = json!("Per maintainer decision, not changed.");
    reply["author"] = json!({"login": "lklimek", "__typename": "User"});
    thread["comments"]["nodes"]
        .as_array_mut()
        .expect("comments")
        .push(reply);
}

/// Polls until every effect of the item's decision is seen on GitHub, at most `within`.
async fn wait_until_seen(w: &World, item: CommentItemId, within: Duration) {
    let deadline = std::time::Instant::now() + within;
    loop {
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
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "not marked seen: {effects:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Plan §4.1: a read that ended moments before the reports' read makes that read too soon, not unnecessary; it is
/// read again once allowed, so what the agent did is still marked.
#[tokio::test]
async fn a_read_just_before_the_reports_read_does_not_drop_it() {
    let w = world().await;
    let reported = reported_reply_and_resolve(&w).await;
    tokio::time::sleep(seen::REPORT_READ_DELAY / 2).await;
    w.reread(&[]).await;
    serve(&w.server, &[("review-threads.json", &acted_on_l2)]).await;
    wait_until_seen(
        &w,
        reported.item,
        seen::REPORT_READ_DELAY * 2 + Duration::from_secs(15),
    )
    .await;
}

/// Plan §4.1: an agent repeating its `done` report (same idempotency key) schedules the read again too.
#[tokio::test]
async fn a_repeated_done_report_reads_github_again() {
    let w = world().await;
    let reported = reported_reply_and_resolve(&w).await;
    let reported_at = Timestamp::now();
    let id = w.pr_id().await;
    let deadline = std::time::Instant::now() + seen::REPORT_READ_DELAY + Duration::from_secs(15);
    while w
        .state
        .db
        .review_loop()
        .pr_state(id)
        .await
        .expect("state")
        .and_then(|row| row.read_at)
        .is_none_or(|at| at <= reported_at)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the reports' read did not run"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    serve(&w.server, &[("review-threads.json", &acted_on_l2)]).await;
    let (effect, body) = &reported.reports[1];
    let (status, _, answer) = with_token(
        &w,
        &reported.token,
        "POST",
        &report_uri(effect),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    wait_until_seen(
        &w,
        reported.item,
        seen::REPORT_READ_DELAY + Duration::from_secs(15),
    )
    .await;
}

/// Plan §2.3 (owner, 2026-11-23): the PR-level comment answering a review starts with a line naming it, a person by
/// an @-mention and a bot without one; the RC16 preview shows the text the decision stores and agents post.
#[tokio::test]
async fn pr_level_comments_name_the_review_they_answer() {
    let w = world().await;
    let latest_r2: &dyn Fn(&mut Value) = &without_r5;
    serve(&w.server, &[("reviews.json", latest_r2)]).await;
    w.get(&format!("{PR}/comments")).await;
    let review = "https://github.com/acme/node-client/pull/482#pullrequestreview-";
    for (key, line) in [
        (
            "review:PRR_fixture482_h1:1",
            format!("@bob, re [your review]({review}4000000105):"),
        ),
        (
            R1_BODY,
            format!("Re [thepastaclaw's review]({review}4000000101):"),
        ),
    ] {
        let item = w.item(key).await;
        let mut row = decision("fix_here");
        row["item_no"] = item["no"].clone();
        row["item_revision"] = item["revision"].clone();
        let (status, preview) = w
            .post(
                &format!("{PR}/comments/effects-preview"),
                json!({"rows": [row]}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        let previewed = &preview[0]["effects"][1]["params"]["text"];
        assert_eq!(
            previewed,
            &json!(format!(
                "{line}\nPer maintainer decision, this is fixed in this pull request."
            )),
            "{preview}"
        );
        let decided = w.decide_ok(key, decision("fix_here")).await;
        assert_eq!(decided["effects"][1]["kind"], "pr_comment");
        assert_eq!(&decided["effects"][1]["params"]["text"], previewed);
        assert_eq!(&decided["reply_text"], previewed);
    }
}
