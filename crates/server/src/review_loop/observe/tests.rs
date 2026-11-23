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
            "review:PRR_fixture482_h1:1",
            "comment:5000000103:1",
            "comment:5000000105:1",
        ],
        "dedupe= when present, else the thread id (legacy l5); no body finding (the latest round r5 lists none); the \
         human CHANGES_REQUESTED body; human PR-level comments only (gate, CI state and bot replies are not items)"
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
        (snapshot.base.as_str(), snapshot.base_sha.as_deref()),
        ("main", Some("92f2274efbb93570cc8f820a49c058cf66b0dbe9")),
        "the base branch and commit of the read"
    );
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

fn body_keys(observed: &[Observed]) -> Vec<&str> {
    observed
        .iter()
        .filter(|o| o.item.kind == CommentItemKind::ReviewBody)
        .map(|o| o.item.stable_key.as_str())
        .collect()
}

fn review<'a>(read: &'a mut PrRead, id: &str) -> &'a mut Review {
    read.reviews
        .iter_mut()
        .find(|review| review.id == id)
        .unwrap_or_else(|| panic!("no review {id}"))
}

/// A copy of review `id` submitted at `at` under the id `copy`.
fn resubmitted(read: &mut PrRead, id: &str, copy: &str, at: &str) {
    let mut review = review(read, id).clone();
    review.id = copy.into();
    review.submitted_at = Some(at.into());
    read.reviews.push(review);
}

const R1_BODY: &str = "review:PRR_fixture482_r1:1";
const H1_BODY: &str = "review:PRR_fixture482_h1:1";

/// Plan §2.1 I2 (owner, 2026-11-23): a bot's body findings are items of its latest submitted, non-dismissed review
/// with a body only; an earlier one stays an item while that review carries it (S13-1 R5). People's bodies are not
/// affected.
#[test]
fn only_the_latest_round_of_a_bot_keeps_its_body_findings() {
    let bots = BotRule::new(&default_known_bots());
    let (observed, snapshot) = observe(&fixture_read(), &bots);
    assert_eq!(
        body_keys(&observed),
        [H1_BODY],
        "r5, the latest round, lists no body finding"
    );
    assert_eq!(
        snapshot.review_ids.get(R1_BODY).map(String::as_str),
        Some("PRR_fixture482_r1"),
        "a superseded finding keeps its review for the effects of its decisions"
    );

    let mut read = fixture_read();
    read.reviews
        .retain(|review| review.id != "PRR_fixture482_r5");
    let (observed, _) = observe(&read, &bots);
    assert_eq!(
        body_keys(&observed),
        [R1_BODY, H1_BODY],
        "the latest round r2 carries r1's finding: the same item"
    );
    let body = by_key(&observed, R1_BODY);
    assert_eq!(body.item.title, "Bound the total retry time");
    assert_eq!(
        body.item.location.as_ref().map(|l| l.path.as_str()),
        Some("src/rpc/retry.rs")
    );
    assert_eq!(body.item.round, Some(1));

    review(&mut read, "PRR_fixture482_r4").submitted_at = Some("2026-11-13T09:00:00Z".into());
    let (observed, _) = observe(&read, &bots);
    assert_eq!(
        body_keys(&observed),
        [R1_BODY, H1_BODY],
        "a later review without a body (thread answers) is not a round"
    );

    let mut read = fixture_read();
    review(&mut read, "PRR_fixture482_r5").state = "DISMISSED".into();
    let (observed, _) = observe(&read, &bots);
    assert_eq!(
        body_keys(&observed),
        [R1_BODY, H1_BODY],
        "a dismissed round does not count: r2 is the latest"
    );

    let mut read = fixture_read();
    resubmitted(
        &mut read,
        "PRR_fixture482_r1",
        "PRR_fixture482_r6",
        "2026-11-12T12:00:00Z",
    );
    let (observed, _) = observe(&read, &bots);
    assert_eq!(
        body_keys(&observed),
        [H1_BODY, "review:PRR_fixture482_r6:1"],
        "a finding restated in full is the latest round's item; the earlier copy is superseded"
    );
}

/// Plan §2.1 I2: the latest-round rule holds per bot, for bots without thepastaclaw's grammar too.
#[test]
fn another_bots_earlier_change_request_body_is_superseded() {
    let bots = BotRule::new(&default_known_bots());
    let mut read = fixture_read();
    review(&mut read, "PRR_fixture482_c1").state = "CHANGES_REQUESTED".into();
    let (observed, _) = observe(&read, &bots);
    assert!(body_keys(&observed).contains(&"review:PRR_fixture482_c1:1"));
    resubmitted(
        &mut read,
        "PRR_fixture482_c1",
        "PRR_fixture482_c2",
        "2026-11-12T11:00:00Z",
    );
    let (observed, _) = observe(&read, &bots);
    assert_eq!(
        body_keys(&observed),
        [H1_BODY, "review:PRR_fixture482_c2:1"]
    );
}
