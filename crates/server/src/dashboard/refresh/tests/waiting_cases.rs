//! Owner captures of 2026-11-23, replayed through fact assembly and the dashboard read path.

use std::collections::BTreeMap;

use super::plugin::{awaiting_ids, stored, with_plugin, write};
use super::*;
use crate::dashboard::plugin::{PluginHost, Timing};
use crate::dashboard::refresh::facts::{PrContext, pr};
use crate::dashboard::score::{CiEffective, MilestoneContext, OwnedChange, ScoreRules};
use crate::dashboard::snapshot::{self, Snapshot};
use crate::github::work::{ItemRef, PrCandidate, PullDetail};
use crate::test_support::pr_hygiene::FakePrHygiene;

async fn captured(input: &str) -> (World, RefreshId, Snapshot) {
    let fixture: Value = serde_json::from_str(input).expect("fixture");
    let detail: PullDetail = serde_json::from_value(fixture["detail"].clone()).expect("detail");
    let mut w = world().await;
    w.refreshed().await;
    let (id, mut snapshot) = stored(&w).await;
    snapshot.settings.linked_logins.push("lklimek".into());
    let rules = ScoreRules::new(&snapshot.own_login, &snapshot.settings);
    let repo = fixture["hygiene"]["repo"].as_str().expect("repository");
    let (owner, name) = repo.split_once('/').expect("owner/name");
    let candidate = PrCandidate {
        updated_at: Some(detail.updated_at),
        title: detail.title.clone(),
        url: detail.url.clone(),
        relations: BTreeMap::new(),
    };
    let stored = pr(
        &rules,
        &ItemRef {
            repo: super::repo(owner, name),
            number: detail.number,
        },
        &detail,
        PrContext {
            candidate: &candidate,
            blockers: None,
            owned: OwnedChange::NoPolicy,
            milestones: MilestoneContext::default(),
        },
    );
    assert_eq!(
        stored.facts.ci,
        CiEffective::Green,
        "policy and bot checks are ignored (K12)"
    );
    snapshot.pull_requests = vec![stored];
    write(&w, id, &snapshot).await;
    // Keep captured event times; only the external snapshot time follows the test world's clock.
    let fake = FakePrHygiene::start("2026-11-18T08:59:00Z", vec![fixture["hygiene"].clone()]).await;
    with_plugin(&mut w, &fake, Timing::default());
    w.state.plugins.poll_once(w.state.dashboard.now()).await;
    (w, id, snapshot)
}

#[tokio::test]
async fn rust_dashcore_1336_waits_on_the_reviewers_named_by_hygiene() {
    let (mut w, _, snapshot) = captured(include_str!(
        "fixtures/waiting-cases/rust-dashcore-1336.json"
    ))
    .await;
    let facts = &snapshot.pull_requests[0].facts;
    assert_eq!(
        facts.review_decision,
        Some(crate::dashboard::score::ReviewDecision::ChangesRequested)
    );
    assert!(facts.changes_requested_by.contains(&"ZocoLini".into()));
    assert_eq!(facts.unresolved_threads, 1);
    let summary = w.summary(ALICE).await;
    let awaiting = &summary.snapshot.expect("snapshot").awaiting;
    assert_eq!(awaiting.len(), 1);
    assert_eq!(
        awaiting[0].waiting_on,
        ["QuantumExplorer", "ZocoLini", "xdustinface"]
    );
    assert!(matches!(awaiting[0].reason, AwaitingReason::Plugin { .. }));
    assert!(
        w.items("")
            .await
            .items
            .iter()
            .all(|i| i.number != Some(1336))
    );
    let rules = ScoreRules::new(&snapshot.own_login, &snapshot.settings);
    let stale = w
        .state
        .plugins
        .overlay(w.state.dashboard.now() + SignedDuration::from_hours(2));
    assert!(crate::dashboard::snapshot::waiting(&rules, facts, &stale).is_none());
    // Without statements, the judge uses the GitHub-only path as well.
    w.state.plugins = PluginHost::new(Vec::new(), Timing::default());
    assert!(awaiting_ids(&w.summary(ALICE).await).is_empty());
}

async fn configuration_error_stays_ranked(input: &str) {
    let (w, _, snapshot) = captured(input).await;
    let number = snapshot.pull_requests[0].facts.number;
    assert!(
        snapshot.pull_requests[0]
            .facts
            .review_requested_from_others
            .is_empty()
    );
    assert!(
        awaiting_ids(&w.summary(ALICE).await).is_empty(),
        "a configuration error names no waiting reviewer"
    );
    assert!(
        w.items("")
            .await
            .items
            .iter()
            .any(|i| i.number == Some(number))
    );
}

#[tokio::test]
async fn dash_evo_tool_1230_configuration_error_stays_ranked() {
    configuration_error_stays_ranked(include_str!(
        "fixtures/waiting-cases/dash-evo-tool-1230.json"
    ))
    .await;
}

#[tokio::test]
async fn dash_evo_tool_1231_configuration_error_stays_ranked() {
    configuration_error_stays_ranked(include_str!(
        "fixtures/waiting-cases/dash-evo-tool-1231.json"
    ))
    .await;
}

#[tokio::test]
async fn a_reviewers_statement_keeps_author_actions_and_merge_actions_in_the_ranking() {
    let (w, id, snapshot) = captured(include_str!(
        "fixtures/waiting-cases/rust-dashcore-1336.json"
    ))
    .await;
    for blocked in 0..6 {
        let mut snapshot = snapshot.clone();
        let facts = &mut snapshot.pull_requests[0].facts;
        match blocked {
            0 => facts.ci = CiEffective::Red,
            1 => facts.ci = CiEffective::Pending,
            2 => facts.mergeable = crate::dashboard::score::Mergeable::Conflicting,
            3 => facts.awaiting_our_reply = true,
            4 => facts.review_pending_for_us = true,
            _ => facts.is_draft = true,
        }
        write(&w, id, &snapshot).await;
        assert!(
            awaiting_ids(&w.summary(ALICE).await).is_empty(),
            "author action {blocked}"
        );
    }
    let mut snapshot = snapshot.clone();
    let facts = &mut snapshot.pull_requests[0].facts;
    facts.labels.clear();
    facts.review_decision = Some(crate::dashboard::score::ReviewDecision::Approved);
    facts.changes_requested_by.clear();
    facts.unresolved_threads = 0;
    facts.reviews_truncated = false;
    facts.merge_state = crate::dashboard::score::MergeState::Clean;
    write(&w, id, &snapshot).await;
    assert!(
        awaiting_ids(&w.summary(ALICE).await).is_empty(),
        "merging is the user's move even under a plug-in hold"
    );
}

fn review_blocked(reviews: Vec<Value>, decision: &str) -> String {
    let mut fixture: Value = serde_json::from_str(include_str!(
        "fixtures/waiting-cases/rust-dashcore-1336.json"
    ))
    .expect("fixture");
    let detail = &mut fixture["detail"];
    detail["reviewDecision"] = json!(decision);
    detail["reviews"] = json!({"totalCount": reviews.len(), "nodes": reviews});
    detail["reviewRequests"] = json!({"nodes": []});
    detail["reviewThreads"] = json!({"nodes": []});
    serde_json::to_string(&fixture).expect("fixture JSON")
}

fn change_request(login: &str, typename: &str, at: &str) -> Value {
    json!({"author": {"login": login, "__typename": typename},
        "state": "CHANGES_REQUESTED", "submittedAt": at, "commit": {"oid": "old-head"}})
}

async fn blocked_snapshot(reviews: Vec<Value>, decision: &str) -> (World, Snapshot) {
    let (w, _, mut snapshot) = captured(&review_blocked(reviews, decision)).await;
    snapshot.issues.clear();
    snapshot.todos.clear();
    snapshot.mentions.clear();
    (w, snapshot)
}

#[tokio::test]
async fn reviewer_statement_must_not_clear_an_active_bot_request() {
    for (login, typename, human, later_head, decision) in [
        ("thepastaclaw", "User", false, None, "CHANGES_REQUESTED"),
        ("thepastaclaw", "User", false, Some("old-head"), "APPROVED"),
        (
            "thepastaclaw",
            "User",
            false,
            Some("new-head"),
            "CHANGES_REQUESTED",
        ),
        ("review-helper", "Bot", true, None, "CHANGES_REQUESTED"),
    ] {
        let mut reviews = vec![change_request(login, typename, "2026-11-18T08:58:00Z")];
        if human {
            reviews.push(change_request("ZocoLini", "User", "2026-11-18T08:58:30Z"));
        }
        if let Some(head) = later_head {
            reviews.push(json!({"author": {"login": login, "__typename": typename},
                "state": "COMMENTED", "submittedAt": "2026-11-18T08:58:45Z",
                "commit": {"oid": head}}));
        }
        let (w, snapshot) = blocked_snapshot(reviews, decision).await;
        let without = snapshot::build(
            &snapshot,
            &Default::default(),
            None,
            &Default::default(),
            None,
        );
        assert_eq!(without.items.len(), 1, "control without a statement");
        let view = snapshot::build(
            &snapshot,
            &Default::default(),
            None,
            &w.state.plugins.overlay(w.state.dashboard.now()),
            None,
        );
        assert_eq!(
            view.items.len(),
            1,
            "active bot {login}, later head {later_head:?}, {decision}"
        );
        assert!(view.awaiting.is_empty());
    }
}

#[tokio::test]
async fn a_statement_observed_before_a_new_human_objection_must_not_hide_it() {
    let (w, snapshot) = blocked_snapshot(
        vec![change_request("ZocoLini", "User", "2026-11-18T08:59:30Z")],
        "CHANGES_REQUESTED",
    )
    .await;
    let view = snapshot::build(
        &snapshot,
        &Default::default(),
        None,
        &w.state.plugins.overlay(w.state.dashboard.now()),
        None,
    );
    assert_eq!(
        view.items.len(),
        1,
        "the statement predates the unanswered objection by 30s"
    );
    assert!(view.awaiting.is_empty());
}

#[tokio::test]
async fn future_source_clock_must_not_hide_an_unanswered_github_objection() {
    let (mut w, snapshot) = blocked_snapshot(
        vec![change_request("ZocoLini", "User", "2026-11-18T08:59:30Z")],
        "CHANGES_REQUESTED",
    )
    .await;
    let fixture: Value =
        serde_json::from_str(&review_blocked(Vec::new(), "CHANGES_REQUESTED")).expect("fixture");
    // Captured at 08:59 with a clock 90s ahead; GitHub's objection follows 30s later.
    let fake = FakePrHygiene::start("2026-11-18T09:00:30Z", vec![fixture["hygiene"].clone()]).await;
    with_plugin(&mut w, &fake, Timing::default());
    w.state.plugins.poll_once(w.state.dashboard.now()).await;
    let view = snapshot::build(
        &snapshot,
        &Default::default(),
        None,
        &w.state.plugins.overlay(w.state.dashboard.now()),
        None,
    );
    assert_eq!(view.items.len(), 1, "a future clock cannot prove coverage");
    assert!(view.awaiting.is_empty());
}

#[tokio::test]
async fn a_human_request_with_unknown_submission_time_is_not_proved_covered() {
    let mut review = change_request("ZocoLini", "User", "2026-11-18T08:58:30Z");
    review["submittedAt"] = Value::Null;
    for reviews in [
        vec![review.clone()],
        vec![
            review,
            change_request("xdustinface", "User", "2026-11-18T08:58:30Z"),
        ],
    ] {
        let (w, snapshot) = blocked_snapshot(reviews, "CHANGES_REQUESTED").await;
        assert!(snapshot.pull_requests[0].facts.changes_requested_by_human);
        assert!(
            snapshot.pull_requests[0]
                .facts
                .changes_requested_human_at
                .is_none()
        );
        let view = snapshot::build(
            &snapshot,
            &Default::default(),
            None,
            &w.state.plugins.overlay(w.state.dashboard.now()),
            None,
        );
        assert_eq!(
            view.items.len(),
            1,
            "an undated objection still needs a response"
        );
        assert!(view.awaiting.is_empty());
    }
}

#[tokio::test]
async fn reviewer_statement_needs_a_newer_observed_github_version() {
    for (updated, waits) in [
        (Value::Null, false),
        (json!("not a timestamp"), false),
        (json!("2026-11-18T08:58:00Z"), false),
        (json!("2026-11-18T08:58:30Z"), false),
        (json!("2026-11-18T08:58:31Z"), true),
        (json!("2026-11-18T09:00:01Z"), false),
    ] {
        let (mut w, mut snapshot) = blocked_snapshot(
            vec![change_request("ZocoLini", "User", "2026-11-18T08:58:30Z")],
            "CHANGES_REQUESTED",
        )
        .await;
        snapshot.pull_requests[0].facts.updated_at =
            "2026-11-18T09:00:00Z".parse().expect("GitHub time");
        let mut fixture: Value =
            serde_json::from_str(&review_blocked(Vec::new(), "CHANGES_REQUESTED"))
                .expect("fixture");
        fixture["hygiene"]["updated_at"] = updated.clone();
        let fake =
            FakePrHygiene::start("2026-11-18T08:59:00Z", vec![fixture["hygiene"].clone()]).await;
        with_plugin(&mut w, &fake, Timing::default());
        w.state.plugins.poll_once(w.state.dashboard.now()).await;
        let view = snapshot::build(
            &snapshot,
            &Default::default(),
            None,
            &w.state.plugins.overlay(w.state.dashboard.now()),
            None,
        );
        assert_eq!(
            view.awaiting.len(),
            usize::from(waits),
            "source GitHub time {updated}"
        );
        assert_eq!(view.items.len(), usize::from(!waits));
    }
}

#[tokio::test]
async fn reviewer_statement_only_covers_human_requests_older_than_its_data() {
    for (at, waits) in [
        ("2026-11-18T08:58:30Z", true),
        ("2026-11-18T08:59:00Z", false),
    ] {
        let (w, snapshot) = blocked_snapshot(
            vec![change_request("ZocoLini", "User", at)],
            "CHANGES_REQUESTED",
        )
        .await;
        let view = snapshot::build(
            &snapshot,
            &Default::default(),
            None,
            &w.state.plugins.overlay(w.state.dashboard.now()),
            None,
        );
        assert_eq!(view.awaiting.len(), usize::from(waits), "request at {at}");
        assert_eq!(view.items.len(), usize::from(!waits));
    }
}

#[tokio::test]
async fn reviewer_statement_can_apply_after_a_bot_request_is_superseded() {
    let (w, snapshot) = blocked_snapshot(
        vec![
            change_request("review-helper", "Bot", "2026-11-18T08:58:00Z"),
            json!({"author": {"login": "review-helper", "__typename": "Bot"},
                "state": "COMMENTED", "submittedAt": "2026-11-18T08:58:45Z",
                "commit": {"oid": "new-head"}}),
        ],
        "APPROVED",
    )
    .await;
    let view = snapshot::build(
        &snapshot,
        &Default::default(),
        None,
        &w.state.plugins.overlay(w.state.dashboard.now()),
        None,
    );
    assert!(
        snapshot.pull_requests[0]
            .facts
            .changes_requested_by
            .is_empty()
    );
    assert!(view.items.is_empty());
    assert_eq!(view.awaiting.len(), 1);
}

#[tokio::test]
async fn reviewer_statement_keeps_requests_with_unknown_bot_types_ranked() {
    let (w, snapshot) = blocked_snapshot(
        vec![change_request("ZocoLini", "User", "2026-11-18T08:58:30Z")],
        "CHANGES_REQUESTED",
    )
    .await;
    let mut document = serde_json::to_value(snapshot).expect("snapshot");
    document["pull_requests"][0]["facts"]
        .as_object_mut()
        .expect("facts")
        .remove("changes_requested_by_bot");
    let snapshot: Snapshot = serde_json::from_value(document).expect("older snapshot");
    let view = snapshot::build(
        &snapshot,
        &Default::default(),
        None,
        &w.state.plugins.overlay(w.state.dashboard.now()),
        None,
    );
    assert_eq!(view.items.len(), 1);
    assert!(view.awaiting.is_empty());
}
