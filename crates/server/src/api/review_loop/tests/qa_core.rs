//! Review-loop contract checks across decisions, carry-overs and the feed: a question the owner is asked again stays
//! asked until he decides, a dismissal acts on the reviews its question names, carry-overs follow `dedupe=` repeats
//! only, and effects that wait on a failed one are not offered (plan §2.3, §2.5, §2.6, §5.2; RQ3, RQ11).

use super::feed::{EXECUTE, agent, fix_here, with_token};
use super::*;

const FEED: &str = "/api/v1/decision-feed";
const L3: &str = "3ba0e7fbff87a8f0";
const L7: &str = "PRRT_fixture482_l7";

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

fn report(outcome: &str, reference: Option<&str>, message: Option<&str>) -> Value {
    json!({"outcome": outcome, "reference": reference, "message": message,
        "idempotency_key": uuid::Uuid::new_v4()})
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

/// RQ3, plan §2.5: an item asked again because the bot rewrote it waits for the owner's new decision; the earlier
/// decision's effects are no longer handed to agents, and an agent's claim does not take the question away.
#[tokio::test]
async fn reasked_item_stays_asked_while_agents_work_the_feed() {
    let w = world().await;
    let first = fix_here(&w).await;
    let edited: &dyn Fn(&mut Value) = &|answer| {
        let thread = thread_node(answer, "PRRT_fixture482_l2");
        thread["id"] = json!("PRRT_fixture482_l2b");
        let comment = &mut thread["comments"]["nodes"][0];
        comment["url"] =
            json!("https://github.com/acme/node-client/pull/482#discussion_r3000000998");
        let body = comment["body"]
            .as_str()
            .expect("body")
            .replace("doubles without a ceiling", "doubles without any ceiling");
        comment["body"] = json!(body);
    };
    w.reread(&[("review-threads.json", edited)]).await;
    let item = w.item(L2).await;
    assert_eq!(item["state"], "reopened", "{item}");

    let (token, _) = agent(&w, &EXECUTE).await;
    let (status, _, claim) = with_token(
        &w,
        &token,
        "POST",
        &claim_uri(&first),
        Some(json!({"capabilities": ["fix", "reply", "resolve"]})),
    )
    .await;
    let item = w.item(L2).await;
    let overview = w.get(&format!("{PR}/comments")).await;
    assert_eq!(
        item["state"], "reopened",
        "claim answered {status} {claim}; the asked-again item left the queue"
    );
    assert!(queued(&overview, &item["no"]), "{overview}");
}

/// Plan §3.2 "Reopen": a reopened item stays asked until the owner decides again, even when an agent reports on
/// an effect it had claimed before the reopen.
#[tokio::test]
async fn reopened_item_stays_asked_after_a_report_on_a_claimed_effect() {
    let w = world().await;
    let decision = fix_here(&w).await;
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
    let item = w.item(L2).await;
    let (status, reopened) = w
        .post(
            &format!("{PR}/comments/items/C{}/reopen", item["no"]),
            json!({"item_revision": item["revision"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reopened}");
    assert_eq!(reopened["state"], "reopened");

    let fix = &decision["effects"][0];
    assert_eq!(fix["kind"], "fix");
    let (status, _, body) = with_token(
        &w,
        &token,
        "POST",
        &report_uri(fix),
        Some(report("done", Some("0123456789abcdef"), None)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let item = w.item(L2).await;
    let overview = w.get(&format!("{PR}/comments")).await;
    assert_eq!(
        item["state"], "reopened",
        "the agent's report took the reopened question away"
    );
    assert!(queued(&overview, &item["no"]), "{overview}");
}

/// RQ11, plan §2.6: "Dismiss stale bot reviews?" names the bots whose every item is decided; deciding it dismisses
/// those reviews only, never the change request of a bot with undecided items.
#[tokio::test]
async fn dismissal_acts_only_on_the_reviews_its_question_names() {
    let w = world().await;
    let coderabbit_requests: &dyn Fn(&mut Value) = &|answer| {
        let nodes = answer["data"]["repository"]["pullRequest"]["reviews"]["nodes"]
            .as_array_mut()
            .expect("reviews");
        let review = nodes
            .iter_mut()
            .find(|review| review["id"] == "PRR_fixture482_c1")
            .expect("c1");
        review["state"] = json!("CHANGES_REQUESTED");
    };
    serve(&w.server, &[("reviews.json", coderabbit_requests)]).await;
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
    assert!(
        w.items()
            .await
            .iter()
            .any(|item| item["author"] == "coderabbitai" && item["state"] == "undecided"),
        "coderabbitai keeps undecided items"
    );
    w.reread(&[("reviews.json", coderabbit_requests)]).await;
    let dismiss = w.item("pr:3:dismiss").await;
    let listed = dismiss["body"].as_str().expect("body");
    assert!(listed.contains("thepastaclaw"), "{listed}");
    assert!(!listed.contains("coderabbitai"), "{listed}");
    let decision = w
        .decide_ok("pr:3:dismiss", decision("dismiss_stale_reviews"))
        .await;
    let dismissed: Vec<&Value> = decision["effects"]
        .as_array()
        .expect("effects")
        .iter()
        .map(|effect| &effect["params"]["review_id"])
        .collect();
    assert_eq!(
        dismissed,
        [&json!("PRR_fixture482_r5")],
        "only the review the question lists is dismissed"
    );
}

/// Plan §2.5, ADR-023 §8: carry-overs follow a `dedupe=` repeat of a decided finding. A human reviewer who
/// unresolves his own thread after diffpilot's decision is not answered and resolved again without the owner.
#[tokio::test]
async fn unresolved_human_thread_is_not_carried_automatically() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let first = w
        .decide_ok(L7, reject("Reads keep their own policy."))
        .await;
    assert_eq!(first["effects"][1]["kind"], "resolve");
    // An agent executes the decision: the resolution it makes is diffpilot's (plan §5.2: effects run under a claim).
    let (token, _) = agent(&w, &EXECUTE).await;
    let (status, _, claim) = with_token(
        &w,
        &token,
        "POST",
        &claim_uri(&first),
        Some(json!({"capabilities": ["reply", "resolve"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{claim}");
    let resolved: &dyn Fn(&mut Value) = &|answer| {
        let thread = thread_node(answer, L7);
        thread["isResolved"] = json!(true);
        thread["resolvedBy"] = json!({"login": "lklimek"});
    };
    w.reread(&[("review-threads.json", resolved)]).await;
    assert_ne!(
        w.item(L7).await["state"],
        "outside",
        "resolved per decision"
    );
    w.reread(&[]).await;
    let item = w.item(L7).await;
    let decisions = item["decisions"].as_array().expect("decisions");
    assert_eq!(
        decisions.len(),
        1,
        "a carried decision with a new reply and resolve was made for a human thread: {decisions:?}"
    );
    assert_eq!(item["state"], "reopened", "the owner is asked again");
}

/// RQ3, plan §2.5 (a): a bot that rewrites a decided finding in place makes the item asked again.
#[tokio::test]
async fn bot_rewrite_in_place_asks_again() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    w.decide_ok(L3, reject("Counted by the caller.")).await;
    let rewritten: &dyn Fn(&mut Value) = &|answer| {
        let comment = &mut thread_node(answer, "PRRT_fixture482_l3")["comments"]["nodes"][0];
        let body = comment["body"].as_str().expect("body").to_owned();
        comment["body"] = json!(format!(
            "{body}\n\nStill present at the new head; the count now includes retries."
        ));
        comment["lastEditedAt"] = json!("2026-11-13T09:00:00Z");
    };
    w.reread(&[("review-threads.json", rewritten)]).await;
    let item = w.item(L3).await;
    assert_eq!(item["github"]["edited_after_decision"], true, "{item}");
    assert_eq!(
        item["state"], "reopened",
        "the rewritten finding is not asked again: {item}"
    );
}

/// Plan §2.3: a reply after a fix waits for the fix; when the fix failed nothing else runs (D7 "save failed →
/// nothing else"), so the waiting reply and resolve are not offered to another agent.
#[tokio::test]
async fn effects_waiting_on_a_failed_fix_are_not_offered() {
    let w = world().await;
    let decision = fix_here(&w).await;
    let (a, _) = agent(&w, &EXECUTE).await;
    let (status, _, claim) = with_token(
        &w,
        &a,
        "POST",
        &claim_uri(&decision),
        Some(json!({"capabilities": ["fix", "reply", "resolve"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{claim}");
    let (fix, reply) = (&decision["effects"][0], &decision["effects"][1]);
    let (status, _, body) = with_token(
        &w,
        &a,
        "POST",
        &report_uri(fix),
        Some(report("failed", None, Some("the build breaks"))),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, _, body) = with_token(
        &w,
        &a,
        "POST",
        &report_uri(reply),
        Some(report("released", None, None)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (b, _) = agent(&w, &EXECUTE).await;
    let (_, _, feed) = with_token(
        &w,
        &b,
        "GET",
        &format!("{FEED}?capabilities=reply,resolve"),
        None,
    )
    .await;
    let offered = feed
        .to_string()
        .contains(decision["id"].as_str().expect("id"));
    let (status, _, claim) = with_token(
        &w,
        &b,
        "POST",
        &claim_uri(&decision),
        Some(json!({"capabilities": ["reply", "resolve"]})),
    )
    .await;
    assert!(
        !offered && status != StatusCode::CREATED,
        "offered {offered}, claim {status} {claim}: a reply-only agent may post \"fixed\" after the fix failed"
    );
}

/// RQ4, RQ5, plan §2.3: gate items are never decided in bulk. "Merge now; remaining items become follow-ups" (P1)
/// leaves a waiting gate item to its own decision instead of replying to and resolving its thread.
#[tokio::test]
async fn merge_now_leaves_gate_items_to_their_own_decision() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    w.assess(
        L2,
        recommend(DecisionKind::FixHere, "real bug"),
        vec![Gate::Funds],
    )
    .await;
    let gated = w.item(L2).await;
    assert_eq!(gated["gates"][0]["gate"], "G-FUNDS");
    let merge = w
        .decide_ok("pr:3:merge", decision("merge_with_follow_ups"))
        .await;
    let touched: Vec<&Value> = merge["effects"]
        .as_array()
        .expect("effects")
        .iter()
        .filter(|effect| effect["kind"] != "follow_up_pr")
        .filter(|effect| {
            effect["params"]["item_nos"]
                .as_array()
                .is_some_and(|nos| nos.contains(&gated["no"]))
        })
        .collect();
    assert!(
        touched.is_empty(),
        "the gate item C{} is replied to and resolved by the merge decision: {touched:?}",
        gated["no"]
    );
}

/// US17, plan §2.1: "Resolved on GitHub ..., not through diffpilot" is for resolutions outside diffpilot. A thread
/// resolved by the effects of the owner's "Merge now" decision is not reported as resolved outside diffpilot.
#[tokio::test]
async fn thread_resolved_by_the_merge_decision_is_not_outside() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let merge = w
        .decide_ok("pr:3:merge", decision("merge_with_follow_ups"))
        .await;
    let l2 = w.item(L2).await;
    assert!(
        merge["effects"]
            .as_array()
            .expect("effects")
            .iter()
            .any(|effect| {
                effect["kind"] == "resolve" && effect["params"]["thread_id"] == "PRRT_fixture482_l2"
            }),
        "{merge}"
    );
    // An agent executes the merge decision's effects (plan §5.2: under a claim).
    let (token, _) = agent(&w, &EXECUTE).await;
    let (status, _, claim) = with_token(
        &w,
        &token,
        "POST",
        &claim_uri(&merge),
        Some(json!({"capabilities": ["follow_up_pr", "reply", "pr_comment", "resolve"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{claim}");
    let resolved: &dyn Fn(&mut Value) = &|answer| {
        let thread = thread_node(answer, "PRRT_fixture482_l2");
        thread["isResolved"] = json!(true);
        thread["resolvedBy"] = json!({"login": "lklimek"});
    };
    w.reread(&[("review-threads.json", resolved)]).await;
    let after = w.item(L2).await;
    assert_eq!(l2["no"], after["no"]);
    assert_ne!(
        after["state"], "outside",
        "resolved per the merge decision, shown as resolved outside diffpilot: {}",
        after["outside"]
    );
}
