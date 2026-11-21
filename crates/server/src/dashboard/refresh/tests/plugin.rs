//! Dashboard plug-ins over the dashboard world (ADR-022): PR Hygiene statements on work items, the Ready to merge
//! cross-check (held, agreeing, uncovered, stale, off), presence from dashboard reads, and the targeted re-read of
//! changed pull requests (details query only, no search), alone and driven by the poller.

use std::collections::HashSet;

use contracts::api::dashboard::PluginState;

use crate::dashboard::plugin::pr_hygiene::PrHygiene;
use crate::dashboard::plugin::{PluginHost, PrKey, Timing};
use crate::dashboard::refresh::reread::{Reread, reread};
use crate::dashboard::snapshot::{self, Snapshot};
use crate::test_support::pr_hygiene::{FakePrHygiene, record};

use super::*;

const NC_476: &str = "acme/node-client";

/// A fresh generation time on the world's clock.
const GENERATED: &str = "2026-11-18T08:59:00Z";

fn with_plugin(w: &mut World, fake: &FakePrHygiene, timing: Timing) {
    w.state.plugins = PluginHost::new(
        vec![Arc::new(PrHygiene::new(fake.url.clone()).expect("plugin"))],
        timing,
    );
}

async fn raw_summary(w: &World) -> Value {
    let answer = w.send("GET", "/api/v1/dashboard", ALICE, &[]).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    answer.body
}

fn ready_ids(summary: &DashboardSummary) -> Vec<&str> {
    summary
        .snapshot
        .as_ref()
        .expect("snapshot")
        .ready_to_merge
        .iter()
        .map(|r| r.id.as_str())
        .collect()
}

/// The stored snapshot of alice, as written (TODOs and hidden mentions included).
async fn stored(w: &World) -> (RefreshId, Snapshot) {
    let (record, document) = w
        .state
        .db
        .dashboard()
        .latest_snapshot::<Value>(DEFAULT_TENANT, "u1")
        .await
        .expect("read")
        .expect("snapshot");
    (record.id, snapshot::readable(document).expect("readable"))
}

async fn write(w: &World, id: RefreshId, snapshot: &Snapshot) {
    w.state
        .db
        .dashboard()
        .replace_current_snapshot(DEFAULT_TENANT, "u1", id, snapshot)
        .await
        .expect("write");
}

fn pr_mut<'a>(snapshot: &'a mut Snapshot, repo: &str, number: u32) -> &'a mut snapshot::StoredPr {
    snapshot
        .pull_requests
        .iter_mut()
        .find(|pr| {
            format!("{}/{}", pr.facts.repo.owner, pr.facts.repo.name) == repo
                && pr.facts.number == number
        })
        .expect("pull request")
}

async fn github_log(w: &World) -> Vec<Value> {
    http()
        .get(format!("{}/_fake/dashboard/requests", w.github))
        .send()
        .await
        .expect("log")
        .json()
        .await
        .expect("json")
}

#[tokio::test]
async fn without_a_plugin_the_wire_and_ready_to_merge_are_unchanged_and_nobody_is_tracked() {
    let w = world().await;
    w.refreshed().await;
    let body = raw_summary(&w).await;
    assert!(body.get("plugins").is_none(), "{body}");
    assert!(body["snapshot"].get("ready_to_merge_held").is_none());
    let items = w.items("").await;
    assert!(items.items.iter().all(|i| i.plugin_notes.is_empty()));
    let page = w.send("GET", "/api/v1/dashboard/items", ALICE, &[]).await;
    assert!(
        page.body["items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|i| i.get("plugin_notes").is_none())
    );
    assert!(w.state.plugins.active_users().is_empty());
}

#[tokio::test]
async fn a_current_statement_holds_a_pull_request_out_of_ready_to_merge_and_notes_its_work_item() {
    let mut w = world().await;
    w.refreshed().await;
    assert_eq!(ready_ids(&w.summary(ALICE).await), ["NC-476"]);
    let fake = FakePrHygiene::start(
        GENERATED,
        vec![record(
            NC_476,
            476,
            Some("waiting-self-review"),
            "Author must post /self-reviewed 1a2b3c",
        )],
    )
    .await;
    with_plugin(&mut w, &fake, Timing::default());
    let before = w.summary(ALICE).await;
    assert_eq!(before.plugins[0].state, PluginState::Waiting);
    assert_eq!(
        w.state.plugins.active_users(),
        [(DEFAULT_TENANT.to_owned(), "u1".to_owned())],
        "a summary read marks its reader active"
    );
    w.state.plugins.poll_once(w.state.dashboard.now()).await;

    let summary = w.summary(ALICE).await;
    assert_ne!(
        summary.revision, before.revision,
        "new statements change the revision"
    );
    assert_eq!(summary.plugins[0].state, PluginState::Current);
    let meta = summary.snapshot.as_ref().expect("snapshot");
    assert!(meta.ready_to_merge.is_empty());
    let held = &meta.ready_to_merge_held;
    assert_eq!(held.len(), 1);
    assert_eq!(
        (
            held[0].item.id.as_str(),
            held[0].source.as_str(),
            held[0].reason.as_str(),
            held[0].url.clone()
        ),
        (
            "NC-476",
            "PR Hygiene",
            "Author must post /self-reviewed 1a2b3c",
            format!("{}api/v1/prs/acme/node-client/476", fake.url)
        )
    );
    let items = w.items("").await;
    let noted: Vec<_> = items
        .items
        .iter()
        .filter(|i| !i.plugin_notes.is_empty())
        .map(|i| i.id.as_str())
        .collect();
    assert_eq!(noted, ["NC-476"], "only the covered pull request");
    let note = &items
        .items
        .iter()
        .find(|i| i.id == "NC-476")
        .expect("item")
        .plugin_notes[0];
    assert_eq!(
        (note.source.as_str(), note.summary.as_deref()),
        ("PR Hygiene", Some("Author must post /self-reviewed 1a2b3c"))
    );

    let mut prs = fake.prs();
    prs["prs"][0]["engine_state"] = json!("ready-to-merge");
    fake.set_prs(prs);
    w.state.plugins.poll_once(w.state.dashboard.now()).await;
    let agreed = w.summary(ALICE).await;
    assert_eq!(ready_ids(&agreed), ["NC-476"], "the engine agrees");
    assert!(
        agreed
            .snapshot
            .as_ref()
            .expect("snapshot")
            .ready_to_merge_held
            .is_empty()
    );

    fake.set_prs(json!({"generated_at": GENERATED, "stale_repos": [],
        "prs": [record("acme/node-client", 482, Some("waiting-author"), "Fix it")]}));
    w.state.plugins.poll_once(w.state.dashboard.now()).await;
    assert_eq!(ready_ids(&w.summary(ALICE).await), ["NC-476"], "uncovered");

    fake.set_prs(
        json!({"generated_at": GENERATED, "stale_repos": ["acme/node-client"],
        "prs": [record(NC_476, 476, Some("waiting-author"), "Fix it")]}),
    );
    w.state.plugins.poll_once(w.state.dashboard.now()).await;
    assert_eq!(
        ready_ids(&w.summary(ALICE).await),
        ["NC-476"],
        "stale repository"
    );

    fake.set_prs(
        json!({"generated_at": "2026-11-18T07:00:00Z", "stale_repos": [],
        "prs": [record(NC_476, 476, Some("waiting-author"), "Fix it")]}),
    );
    w.state.plugins.poll_once(w.state.dashboard.now()).await;
    let old = w.summary(ALICE).await;
    assert_eq!(ready_ids(&old), ["NC-476"], "data older than an hour");
    assert_eq!(old.plugins[0].state, PluginState::Stale);

    fake.set_prs(json!({"generated_at": GENERATED, "stale_repos": [],
        "prs": [record(NC_476, 476, Some("waiting-author"), "Fix it")]}));
    fake.set_status(Some(StatusCode::SERVICE_UNAVAILABLE));
    let timing = Timing {
        stale_after: Duration::from_millis(100),
        ..Timing::default()
    };
    with_plugin(&mut w, &fake, timing);
    w.state.plugins.poll_once(w.state.dashboard.now()).await;
    assert_eq!(
        ready_ids(&w.summary(ALICE).await),
        ["NC-476"],
        "a failing source"
    );
}

#[tokio::test]
async fn a_change_signal_rereads_only_the_changed_pull_requests_with_the_details_query() {
    let w = world().await;
    w.refreshed().await;
    let (id, mut tampered) = stored(&w).await;
    pr_mut(&mut tampered, "acme/wallet-ui", 320).facts.title = "stale title".to_owned();
    pr_mut(&mut tampered, "acme/node-client", 482).facts.title = "stale too".to_owned();
    let mut merged = pr_mut(&mut tampered, "acme/wallet-ui", 320).clone();
    merged.facts.repo = RepoRef {
        owner: "acme".to_owned(),
        name: "platform".to_owned(),
    };
    merged.facts.number = 1190;
    tampered.pull_requests.push(merged);
    write(&w, id, &tampered).await;
    let relations = pr_mut(&mut tampered, "acme/wallet-ui", 320)
        .facts
        .relations
        .clone();
    let before = github_log(&w).await.len();
    let revision = w.summary(ALICE).await.revision;

    let changed = HashSet::from([
        PrKey::new(
            &RepoRef {
                owner: "ACME".to_owned(),
                name: "wallet-ui".to_owned(),
            },
            320,
        ),
        PrKey::new(
            &RepoRef {
                owner: "acme".to_owned(),
                name: "platform".to_owned(),
            },
            1190,
        ),
    ]);
    let done = reread(&w.state, &alice(), &changed).await.expect("reread");
    assert_eq!(
        done,
        Reread::Written {
            updated: 1,
            closed: 1
        }
    );

    let (same, mut after) = stored(&w).await;
    assert_eq!(same, id, "the same snapshot, written back");
    assert!(after.prs_reread_at.is_some());
    let reread_pr = pr_mut(&mut after, "acme/wallet-ui", 320);
    assert_eq!(
        reread_pr.facts.title,
        "fix(ui): restore the focus ring on tabs"
    );
    assert_eq!(
        reread_pr.facts.relations, relations,
        "relations kept from the scan"
    );
    assert_eq!(
        pr_mut(&mut after, "acme/node-client", 482).facts.title,
        "stale too"
    );
    assert!(
        !after.pull_requests.iter().any(|pr| pr.facts.number == 1190),
        "a merged pull request is dropped"
    );
    let log = github_log(&w).await;
    let new = &log[before..];
    assert!(!new.is_empty());
    assert!(
        new.iter().all(|r| r["on"] == "graphql"),
        "details only, no search: {new:?}"
    );
    let lookups: u64 = new.iter().filter_map(|r| r["pull_requests"].as_u64()).sum();
    assert_eq!(lookups, 2);
    assert_ne!(
        w.summary(ALICE).await.revision,
        revision,
        "a re-read changes the revision"
    );

    let none = HashSet::from([PrKey::new(
        &RepoRef {
            owner: "other".to_owned(),
            name: "repo".to_owned(),
        },
        1,
    )]);
    assert_eq!(
        reread(&w.state, &alice(), &none).await.expect("reread"),
        Reread::Skipped("none of the user's pull requests changed")
    );
    assert_eq!(github_log(&w).await.len(), log.len());
}

#[tokio::test]
async fn no_reread_while_a_scan_runs() {
    let w = world().await;
    w.refreshed().await;
    let started = w.start_held().await;
    let changed = HashSet::from([PrKey::new(
        &RepoRef {
            owner: "acme".to_owned(),
            name: "wallet-ui".to_owned(),
        },
        320,
    )]);
    let requests = w.gate.requests();
    assert_eq!(
        reread(&w.state, &alice(), &changed).await.expect("reread"),
        Reread::Skipped("a scan runs")
    );
    assert_eq!(w.gate.requests(), requests);
    w.gate.open();
    w.settle(started.id).await;
}

#[tokio::test]
async fn the_poller_rereads_a_pull_request_whose_statement_changed_for_an_active_reader() {
    let mut w = world().await;
    w.refreshed().await;
    let fake = FakePrHygiene::start(
        GENERATED,
        vec![record(NC_476, 476, Some("waiting-author"), "Fix it")],
    )
    .await;
    let timing = Timing {
        interval: Duration::from_millis(50),
        ..Timing::default()
    };
    with_plugin(&mut w, &fake, timing);
    assert!(crate::dashboard::plugin::spawn(w.state.clone()));
    raw_summary(&w).await;
    tokio::time::timeout(WAIT, async {
        while fake.served().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("first poll");
    let (id, mut tampered) = stored(&w).await;
    pr_mut(&mut tampered, NC_476, 476).facts.title = "stale title".to_owned();
    write(&w, id, &tampered).await;

    let mut prs = fake.prs();
    prs["prs"][0]["next_action"] = json!("Resolve the threads");
    fake.set_prs(prs);
    let reread = tokio::time::timeout(WAIT, async {
        loop {
            let (_, mut now) = stored(&w).await;
            if now.prs_reread_at.is_some() {
                return pr_mut(&mut now, NC_476, 476).facts.title.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("re-read");
    assert_eq!(reread, "fix(rpc): close idle connections");
    let _ = w.state.stopping.send(true);
}
