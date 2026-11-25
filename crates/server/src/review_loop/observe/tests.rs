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
            database_id: None,
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
        [H1_BODY],
        "a dismissed round still supersedes (RQ11): r1's finding does not come back"
    );
    read.reviews
        .retain(|review| review.id != "PRR_fixture482_r5");
    review(&mut read, "PRR_fixture482_r2").state = "DISMISSED".into();
    let (observed, _) = observe(&read, &bots);
    assert_eq!(
        body_keys(&observed),
        [R1_BODY, H1_BODY],
        "a dismissed latest round yields no items of its own but still carries r1's finding"
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

/// r1 with a second body finding of the same title and path at other lines, r2 (the latest round once r5 is
/// removed) carrying the entries `carried` (`path:range` each).
fn repeated_title(carried: &[&str]) -> PrRead {
    let mut read = fixture_read();
    read.reviews
        .retain(|review| review.id != "PRR_fixture482_r5");
    let second = "**🔴 Blocking: Bound the total retry time**\n`src/rpc/retry.rs:40-44`\n\n\
        The reconnect loop waits up to the full timeout too.\n\n";
    let r1 = review(&mut read, "PRR_fixture482_r1");
    r1.body = r1
        .body
        .replacen("<details>", &format!("{second}<details>"), 1);
    let entries: Vec<String> = carried
        .iter()
        .map(|range| format!("- 🔴 **Blocking**: Bound the total retry time — `{range}` (carried)"))
        .collect();
    let r2 = review(&mut read, "PRR_fixture482_r2");
    r2.body = r2.body.replace(
        "- 🔴 **Blocking**: Bound the total retry time — `src/rpc/retry.rs:20-26` (carried)",
        &entries.join("\n"),
    );
    read
}

/// S13-1 R5 with a repeated title: the latest round carries only the second of two earlier findings that share
/// title and path; that finding, not its namesake, stays the item (plan §2.1 I2).
#[test]
fn a_carried_entry_keeps_the_finding_it_names_when_titles_repeat() {
    let bots = BotRule::new(&default_known_bots());
    let mut read = repeated_title(&["src/rpc/retry.rs:40-44"]);
    assert_eq!(
        markers::body_findings(&review(&mut read, "PRR_fixture482_r1").body).len(),
        2,
        "r1 lists two body findings"
    );
    let (observed, _) = observe(&read, &bots);
    assert_eq!(
        body_keys(&observed),
        ["review:PRR_fixture482_r1:2", H1_BODY],
        "the carried entry names lines 40-44: the second finding stays open, the first is superseded"
    );
}

/// S13-1 R5 with a repeated title: the latest round carries both earlier findings that share title and path; both
/// stay items (plan §2.1 I2).
#[test]
fn two_carried_entries_with_one_title_keep_both_findings() {
    let bots = BotRule::new(&default_known_bots());
    let read = repeated_title(&["src/rpc/retry.rs:20-26", "src/rpc/retry.rs:40-44"]);
    let (observed, _) = observe(&read, &bots);
    assert_eq!(
        body_keys(&observed),
        [R1_BODY, "review:PRR_fixture482_r1:2", H1_BODY],
        "both entries are carried: neither finding is superseded"
    );
}

/// QA: an entry of the latest round that matches no earlier body finding is reported (logged once per read), so no
/// finding the bot still lists leaves the queue unnoticed.
#[test]
fn carried_entries_without_an_earlier_finding_are_reported() {
    let bots = BotRule::new(&default_known_bots());
    let mut read = fixture_read();
    read.reviews
        .retain(|review| review.id != "PRR_fixture482_r5");
    let r2 = review(&mut read, "PRR_fixture482_r2");
    r2.body = r2.body.replace(
        "(carried)\n",
        "(carried)\n- 🟡 **Suggestion**: Never raised before — `src/rpc/retry.rs:1-2` (carried)\n",
    );
    let latest = latest_rounds(&read, &bots);
    let Carried { kept, unmatched } = carried(&read, &latest);
    assert_eq!(
        kept.into_iter().collect::<Vec<_>>(),
        [("PRR_fixture482_r1", 1)]
    );
    assert_eq!(
        unmatched,
        [("PRR_fixture482_r2", "Never raised before".to_owned())]
    );
}

/// Plan §2.1 I2 (owner, 2026-11-23, RQ11): an entry whose most recent earlier copy is in a dismissed round takes that
/// copy, which yields no item; an older copy the dismissed round had superseded stays superseded.
#[test]
fn a_carried_entry_whose_latest_copy_was_dismissed_does_not_revive_an_older_copy() {
    let bots = BotRule::new(&default_known_bots());
    let mut read = fixture_read();
    read.reviews
        .retain(|review| review.id != "PRR_fixture482_r5");
    resubmitted(
        &mut read,
        "PRR_fixture482_r1",
        "PRR_fixture482_rx",
        "2026-11-11T18:00:00Z",
    );
    review(&mut read, "PRR_fixture482_rx").state = "DISMISSED".into();
    resubmitted(
        &mut read,
        "PRR_fixture482_r2",
        "PRR_fixture482_ry",
        "2026-11-12T12:00:00Z",
    );
    let (observed, _) = observe(&read, &bots);
    assert_eq!(
        body_keys(&observed),
        [H1_BODY],
        "ry carries the copy of dismissed rx; r1's older copy, superseded by rx, does not come back"
    );
}

/// Plan §2.1 I2: each finding is taken once, and an entry without a path does not take the only finding an entry
/// with a path could take.
#[test]
fn an_entry_without_a_path_leaves_the_finding_at_a_named_path_to_its_entry() {
    let bots = BotRule::new(&default_known_bots());
    let mut read = fixture_read();
    read.reviews
        .retain(|review| review.id != "PRR_fixture482_r5");
    let second = "**🔴 Blocking: Bound the total retry time**\n`src/rpc/other.rs:5-6`\n\n\
        The other loop waits too.\n\n";
    let r1 = review(&mut read, "PRR_fixture482_r1");
    r1.body = r1
        .body
        .replacen("<details>", &format!("{second}<details>"), 1);
    let r2 = review(&mut read, "PRR_fixture482_r2");
    r2.body = r2.body.replace(
        "- 🔴 **Blocking**: Bound the total retry time — `src/rpc/retry.rs:20-26` (carried)",
        "- 🔴 **Blocking**: Bound the total retry time (carried)\n\
         - 🔴 **Blocking**: Bound the total retry time — `src/rpc/retry.rs:30-36` (carried)",
    );
    let latest = latest_rounds(&read, &bots);
    let Carried { kept, unmatched } = carried(&read, &latest);
    let mut kept: Vec<_> = kept.into_iter().collect();
    kept.sort_unstable();
    assert_eq!(
        (kept, unmatched),
        (
            vec![("PRR_fixture482_r1", 1), ("PRR_fixture482_r1", 2)],
            Vec::new()
        ),
        "the path-less entry takes other.rs, the retry.rs entry takes retry.rs"
    );
}
