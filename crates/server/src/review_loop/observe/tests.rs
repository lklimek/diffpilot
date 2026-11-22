use contracts::api::dashboard::default_known_bots;
use contracts::api::review_loop::BotThreadStatus;
use serde_json::Value;

use super::*;
use crate::github::review_loop::tests::recorded;
use crate::github::review_loop::{PrComment, PrFacts, Review, Thread};

fn nodes<T: serde::de::DeserializeOwned>(file: &str, connection: &str) -> Vec<T> {
    let answer = recorded(file);
    serde_json::from_value(answer["data"]["repository"]["pullRequest"][connection]["nodes"].clone())
        .expect("nodes")
}

/// The fixture read of PR 482 (`fixtures/review-loop/github`).
pub(crate) fn fixture_read() -> PrRead {
    let facts: Value = recorded("pr.json")["data"]["repository"]["pullRequest"].clone();
    PrRead {
        repo_github_id: 100_000_482,
        private: false,
        pr: serde_json::from_value::<PrFacts>(facts).expect("facts"),
        threads: nodes::<Thread>("review-threads.json", "reviewThreads"),
        reviews: nodes::<Review>("reviews.json", "reviews"),
        comments: nodes::<PrComment>("comments.json", "comments"),
        complete: true,
    }
}

fn by_key<'a>(observed: &'a [Observed], key: &str) -> &'a Observed {
    observed
        .iter()
        .find(|o| o.item.stable_key == key)
        .unwrap_or_else(|| panic!("no item {key}"))
}

#[test]
fn the_fixture_reads_into_items_rounds_and_a_snapshot() {
    let (observed, snapshot) = observe(&fixture_read(), &BotRule::new(&default_known_bots()));
    let keys: Vec<&str> = observed
        .iter()
        .map(|o| o.item.stable_key.as_str())
        .collect();
    assert_eq!(
        keys,
        [
            "821d7cdc93fec6e1",
            "ef3c2e21454eefc7",
            "3ba0e7fbff87a8f0",
            "0a1b2c3d4e5f6a7b",
            "PRRT_fixture482_l5",
            "fedcba9876543210",
            "PRRT_fixture482_l7",
            "PRRT_fixture482_l8",
            "review:PRR_fixture482_r1:1",
            "review:PRR_fixture482_h1:1",
            "comment:5000000103:1",
            "comment:5000000105:1",
        ],
        "dedupe= when present, else the thread id (legacy l5); one body finding of r1; the human CHANGES_REQUESTED \
         body; human PR-level comments only (gate, CI state and bot replies are not items)"
    );
    let first = by_key(&observed, "821d7cdc93fec6e1");
    assert_eq!(
        first.item.author_kind,
        AuthorKind::Bot,
        "thepastaclaw via known_bots"
    );
    assert_eq!(first.item.bot_severity.as_deref(), Some("Blocking"));
    assert_eq!(
        first.item.title,
        "Do not retry a call that may have been applied"
    );
    assert_eq!(
        first.item.github.bot_status,
        Some(BotThreadStatus::Resolved)
    );
    assert_eq!(first.item.github.replies, 2);
    assert_eq!(first.item.round, Some(1));
    assert!(
        !first.item.body.contains("thepastaclaw"),
        "markers stripped"
    );
    assert_eq!(
        first.thread,
        Some((
            "PRRT_fixture482_l1".into(),
            true,
            Some("thepastaclaw".into())
        ))
    );
    assert_eq!(
        first.item.location.as_ref().and_then(|l| l.lines),
        Some(LineRange {
            start_line: 96,
            end_line: 99
        })
    );
    let outdated = by_key(&observed, "PRRT_fixture482_l5");
    assert_eq!(
        outdated.item.location.as_ref().and_then(|l| l.lines),
        None,
        "an outdated thread has no head lines"
    );
    assert_eq!(
        by_key(&observed, "PRRT_fixture482_l7").item.author_kind,
        AuthorKind::Human
    );
    assert_eq!(
        by_key(&observed, "PRRT_fixture482_l8").item.author_kind,
        AuthorKind::Bot
    );
    let body = by_key(&observed, "review:PRR_fixture482_r1:1");
    assert_eq!(body.item.title, "Bound the total retry time");
    assert_eq!(
        body.item.location.as_ref().map(|l| l.path.as_str()),
        Some("src/rpc/retry.rs")
    );

    assert_eq!(
        snapshot
            .rounds
            .iter()
            .map(|round| (round.number, round.review_id.as_str()))
            .collect::<Vec<_>>(),
        [
            (1, "PRR_fixture482_r1"),
            (2, "PRR_fixture482_r2"),
            (3, "PRR_fixture482_r5")
        ],
        "rounds are final-phase reviews only"
    );
    assert_eq!(
        snapshot.gate,
        Some((GateFact::Complete, Some("52d2da14".into()))),
        "the gate comment is a status"
    );
    assert_eq!(
        snapshot
            .bot_change_requests
            .iter()
            .map(|r| r.review_id.as_str())
            .collect::<Vec<_>>(),
        ["PRR_fixture482_r5"]
    );
    assert!(snapshot.changes_requested_by.contains(&"bob".to_owned()));
    assert_eq!(snapshot.unresolved_threads, 6);
    assert_eq!(snapshot.counts, (8, 9, 5));
    assert_eq!(
        snapshot
            .thread_ids
            .get("821d7cdc93fec6e1")
            .map(String::as_str),
        Some("PRRT_fixture482_l1")
    );
}

#[test]
fn without_known_bots_thepastaclaw_is_a_person_and_its_markers_are_not_read() {
    let (observed, snapshot) = observe(&fixture_read(), &BotRule::new(&[]));
    assert!(snapshot.rounds.is_empty(), "rounds come from bots only");
    assert!(
        observed
            .iter()
            .all(|o| o.item.stable_key != "821d7cdc93fec6e1"),
        "a person's text cannot claim a dedupe key"
    );
    let rule = BotRule::new(&[]);
    for (login, typename, kind) in [
        ("coderabbitai", "Bot", AuthorKind::Bot),
        ("renovate[bot]", "User", AuthorKind::Bot),
        ("claude", "User", AuthorKind::Human),
    ] {
        let author = Author {
            login: login.into(),
            typename: typename.into(),
        };
        assert_eq!(rule.kind(Some(&author)), kind, "{login}");
    }
}

#[test]
fn a_duplicate_dedupe_key_falls_back_to_the_thread_id() {
    let mut read = fixture_read();
    let mut copy = read.threads[1].clone();
    copy.id = "PRRT_copy".into();
    read.threads.push(copy);
    let (observed, _) = observe(&read, &BotRule::new(&default_known_bots()));
    assert!(observed.iter().any(|o| o.item.stable_key == "PRRT_copy"));
}
