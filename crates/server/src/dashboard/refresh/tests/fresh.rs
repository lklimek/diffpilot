//! My work kept fresh while it is open (plan §M) over the dashboard world: the fact check reads only the change
//! probe when nothing changed and the details of the pull requests whose mark moved; an open page starts an
//! automatic scan, then checks facts; nobody open reads nothing.

use contracts::api::dashboard::RefreshTrigger;

use super::plugin::{github_log, pr_mut, stored, write};
use super::*;
use crate::dashboard::fresh;
use crate::dashboard::refresh::reread::{Reread, check};

/// GitHub requests since `before`, and their `pullRequest` lookups.
async fn since(w: &World, before: usize) -> (Vec<Value>, u64) {
    let new = github_log(w).await[before..].to_vec();
    let lookups = new.iter().filter_map(|r| r["pull_requests"].as_u64()).sum();
    (new, lookups)
}

/// The marks of the scan (with the merge states Q17 read again) match the probe: nothing is read again.
#[tokio::test]
async fn a_fact_check_without_changes_reads_only_the_probe() {
    let w = world().await;
    w.refreshed().await;
    let (_, snapshot) = stored(&w).await;
    let listed = snapshot.pull_requests.len() as u64;
    assert!(snapshot.pull_requests.iter().all(|pr| pr.mark.is_some()));
    let before = github_log(&w).await.len();
    let summary = w.summary(ALICE).await;
    assert_eq!(
        summary
            .snapshot
            .as_ref()
            .expect("snapshot")
            .facts_checked_at,
        None
    );

    assert_eq!(
        check(&w.state, &alice()).await.expect("check"),
        Reread::Unchanged
    );
    let (new, lookups) = since(&w, before).await;
    assert!(
        new.iter().all(|r| r["on"] == "graphql"),
        "no search: {new:?}"
    );
    assert_eq!(
        lookups, listed,
        "one probe lookup per listed pull request, no details"
    );
    let after = w.summary(ALICE).await;
    assert_eq!(after.revision, summary.revision, "nothing written");
    let meta = after.snapshot.expect("snapshot");
    assert!(
        meta.facts_checked_at.is_some_and(|at| at > meta.scanned_at),
        "{meta:?}"
    );
}

#[tokio::test]
async fn a_fact_check_rereads_the_pull_requests_whose_mark_moved() {
    let w = world().await;
    w.refreshed().await;
    let (id, mut tampered) = stored(&w).await;
    let listed = tampered.pull_requests.len() as u64;
    let moved = pr_mut(&mut tampered, "acme/wallet-ui", 320);
    moved.facts.title = "stale title".to_owned();
    moved.mark.as_mut().expect("mark").ci = Some("PENDING".to_owned());
    pr_mut(&mut tampered, "acme/node-client", 482).mark = None;
    pr_mut(&mut tampered, "acme/node-client", 476).facts.title = "kept".to_owned();
    write(&w, id, &tampered).await;
    let revision = w.summary(ALICE).await.revision;
    let before = github_log(&w).await.len();

    assert_eq!(
        check(&w.state, &alice()).await.expect("check"),
        Reread::Written {
            updated: 2,
            closed: 0
        }
    );
    let (_, lookups) = since(&w, before).await;
    assert_eq!(
        lookups,
        listed + 2,
        "the probe, then the details of the two"
    );
    let (_, mut after) = stored(&w).await;
    assert_eq!(
        pr_mut(&mut after, "acme/wallet-ui", 320).facts.title,
        "fix(ui): restore the focus ring on tabs"
    );
    assert_eq!(
        pr_mut(&mut after, "acme/node-client", 476).facts.title,
        "kept",
        "an unchanged mark is not read again"
    );
    assert_ne!(w.summary(ALICE).await.revision, revision);
    assert_eq!(
        check(&w.state, &alice()).await.expect("check again"),
        Reread::Unchanged,
        "the marks read back match the probe"
    );
}

#[tokio::test]
async fn an_open_page_starts_an_automatic_scan_then_checks_facts() {
    let w = world().await;
    let start = w.state.dashboard.now();
    let readers = w.state.dashboard.readers();
    fresh::tick(&w.state, start).await;
    assert_eq!(w.gate.requests(), 0, "nobody open: nothing read");

    w.summary(ALICE).await;
    fresh::tick(&w.state, w.state.dashboard.now()).await;
    let latest = w.summary(ALICE).await.latest_refresh.expect("started");
    assert_eq!(latest.trigger, RefreshTrigger::Automatic);
    let settled = w.settle(latest.id).await;
    assert_eq!(settled.state, RefreshState::Finished, "{settled:?}");

    let later = w.state.dashboard.now() + SignedDuration::from_mins(6);
    readers.mark_open(&alice(), later);
    let before = github_log(&w).await.len();
    fresh::tick(&w.state, later).await;
    let (new, lookups) = since(&w, before).await;
    assert!(lookups > 0, "a fact check ran");
    assert!(new.iter().all(|r| r["on"] == "graphql"), "no scan: {new:?}");
    assert_eq!(
        w.summary(ALICE).await.latest_refresh.expect("latest").id,
        latest.id,
        "no second scan within 30 min"
    );
}
