//! Owner captures of 2026-11-23, replayed through fact assembly and the dashboard read path.

use std::collections::BTreeMap;

use super::plugin::{awaiting_ids, stored, with_plugin, write};
use super::*;
use crate::dashboard::plugin::{PluginHost, Timing};
use crate::dashboard::refresh::facts::{PrContext, pr};
use crate::dashboard::score::{CiEffective, MilestoneContext, OwnedChange, ScoreRules};
use crate::dashboard::snapshot::Snapshot;
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
