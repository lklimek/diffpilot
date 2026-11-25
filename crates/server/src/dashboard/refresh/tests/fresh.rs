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
    let (id, snapshot) = stored(&w).await;
    let listed = snapshot.pull_requests.len() as u64;
    assert!(snapshot.pull_requests.iter().all(|pr| pr.mark.is_some()));
    let before = github_log(&w).await.len();
    let summary = w.summary(ALICE).await;
    assert_eq!(w.state.dashboard.readers().checked_at(&alice(), id), None);

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
    let checked = w.state.dashboard.readers().checked_at(&alice(), id);
    assert!(
        checked.is_some_and(|at| at > snapshot.scanned_at),
        "{checked:?}"
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

#[tokio::test]
async fn concurrent_fact_check_and_plugin_reread_keep_both_updates() {
    use crate::dashboard::plugin::PrKey;
    use crate::dashboard::refresh::reread::reread;
    use std::collections::HashSet;
    let w = world().await;
    w.refreshed().await;
    let (id, mut snapshot) = stored(&w).await;
    let a = pr_mut(&mut snapshot, "acme/wallet-ui", 320);
    a.facts.title = "stale A".into();
    a.mark = None;
    // Only the plug-in signal sees B: its mark still matches GitHub.
    pr_mut(&mut snapshot, "acme/node-client", 476).facts.title = "stale B".into();
    write(&w, id, &snapshot).await;
    w.gate.close();
    let state = w.state.clone();
    let first = tokio::spawn(async move { check(&state, &alice()).await });
    w.gate.held().await;
    let state = w.state.clone();
    let second = tokio::spawn(async move {
        reread(
            &state,
            &alice(),
            &HashSet::from([PrKey::new(&repo("acme", "node-client"), 476)]),
        )
        .await
    });
    // A serialised reader waits for the first; an unguarded reader reaches GitHub with a stale snapshot.
    let _ = tokio::time::timeout(Duration::from_millis(100), w.gate.held()).await;
    w.gate.open();
    first.await.expect("check task").expect("check");
    second.await.expect("plugin task").expect("reread");
    let (_, mut after) = stored(&w).await;
    assert_ne!(
        pr_mut(&mut after, "acme/wallet-ui", 320).facts.title,
        "stale A"
    );
    assert_ne!(
        pr_mut(&mut after, "acme/node-client", 476).facts.title,
        "stale B"
    );
}

#[tokio::test]
async fn a_fact_check_rate_limit_stops_ticks_and_plugin_reads_until_retry() {
    for secondary in [false, true] {
        let w = world().await;
        w.refreshed().await;
        let readers = w.state.dashboard.readers();
        let later = w.state.dashboard.now() + SignedDuration::from_mins(6);
        let reset = later + SignedDuration::from_mins(10);
        let answer = if secondary {
            json!({"kind": "secondary_rate_limit", "retry_after": 1200})
        } else {
            json!({"kind": "primary_rate_limit", "reset": reset.as_second()})
        };
        w.faults(json!([{"on": "graphql", "times": 1, "answer": answer}]))
            .await;
        readers.mark_open(&alice(), later);
        fresh::tick(&w.state, later).await;
        let before = github_log(&w).await.len();
        assert!(before > 0);
        fresh::tick(&w.state, later + SignedDuration::from_mins(1)).await;
        assert_eq!(
            github_log(&w).await.len(),
            before,
            "no request before retry; secondary={secondary}"
        );
        assert!(matches!(
            check(&w.state, &alice()).await.expect("skipped"),
            Reread::Skipped(_)
        ));
        assert_eq!(
            github_log(&w).await.len(),
            before,
            "the shared reader respects the limit"
        );
        assert!(
            matches!(
                start(&w.state, &alice(), RefreshTrigger::Automatic).await,
                Err(StartError::RateLimited { .. })
            ),
            "a scan respects the fact-check limit"
        );
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(27 * 60)).await;
        tokio::time::resume();
        let retry = w.state.dashboard.now();
        readers.mark_open(&alice(), retry);
        fresh::tick(&w.state, retry).await;
        let restarted = w
            .summary(ALICE)
            .await
            .latest_refresh
            .expect("automatic retry");
        assert_eq!(restarted.trigger, RefreshTrigger::Automatic);
        assert_eq!(w.settle(restarted.id).await.state, RefreshState::Finished);
        assert!(github_log(&w).await.len() > before, "retry after the limit");
    }
}

#[tokio::test]
async fn failed_or_missing_probe_and_detail_responses_keep_the_last_good_check_time() {
    for details in [false, true] {
        for missing in [1, usize::MAX] {
            let w = world().await;
            w.refreshed().await;
            check(&w.state, &alice()).await.expect("good check");
            let (id, mut snapshot) = stored(&w).await;
            let good = w.state.dashboard.readers().checked_at(&alice(), id);
            assert!(good.is_some());
            if details {
                for pr in &mut snapshot.pull_requests {
                    pr.mark = None;
                }
                write(&w, id, &snapshot).await;
            }
            *w.gate.missing.lock().expect("missing") = Some((details, missing));
            assert!(
                check(&w.state, &alice()).await.is_err(),
                "missing={missing}, details={details}"
            );
            assert_eq!(w.state.dashboard.readers().checked_at(&alice(), id), good);
        }
    }
    for fragment in ["fragment Probe", "fragment Pull"] {
        let w = world().await;
        w.refreshed().await;
        check(&w.state, &alice()).await.expect("good check");
        let (id, mut snapshot) = stored(&w).await;
        let good = w.state.dashboard.readers().checked_at(&alice(), id);
        assert!(good.is_some());
        for pr in &mut snapshot.pull_requests {
            pr.mark = None;
        }
        write(&w, id, &snapshot).await;
        w.faults(json!([{"on": "graphql", "matching": fragment, "times": 100,
            "answer": {"kind": "server_error"}}]))
            .await;
        assert!(
            check(&w.state, &alice()).await.is_err(),
            "failed {fragment}"
        );
        assert_eq!(w.state.dashboard.readers().checked_at(&alice(), id), good);
    }
}
