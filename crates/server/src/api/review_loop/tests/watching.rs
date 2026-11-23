//! Watch (RW1–RW2, plan §2.8): the probe, its batches and reads, the wait for the bot's round and the
//! "questions ready" event, over the recorded GitHub answers served by wiremock.

use contracts::api::review_loop::{UserEventKind, WatchState};
use contracts::common::{PrRef, RepoRef};
use contracts::ids::CommitSha;
use db::repo::review_loop::{NewPass, PassCause, PassStart, WatchRecord};
use db::repo::tokens::PairCondition;
use wiremock::Request as Received;

use super::*;
use crate::review_loop::watch::{self, BOT_ROUND_WAIT, PROBE_INTERVAL};

/// What only the watch probe's query holds.
const PROBE_MARKER: &str = "reviews(last: 1)";
/// What only the full read's pull-request query holds.
const READ_MARKER: &str = "reviewDecision";

fn pr_482() -> PrRef {
    PrRef {
        repo: RepoRef {
            owner: "acme".into(),
            name: "node-client".into(),
        },
        number: 482,
    }
}

/// The probe of PR 482 as the recorded answer holds it (spike S13-2), open.
fn probe_answer(updated_at: &str) -> Value {
    json!({"data": {"a0": {"pullRequest": {
        "state": "OPEN",
        "headRefOid": crate::test_support::github::HEAD,
        "updatedAt": updated_at,
        "reviewThreads": {"totalCount": 8},
        "comments": {"totalCount": 5},
        "reviews": {"nodes": [{"id": "PRR_fixture482_a1"}]}
    }}}})
}

async fn mount_probe(server: &MockServer, answer: Value) {
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_string_contains(PROBE_MARKER))
        .respond_with(ResponseTemplate::new(200).set_body_json(answer))
        .mount(server)
        .await;
}

async fn received(server: &MockServer, marker: &str) -> Vec<Received> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| String::from_utf8_lossy(&request.body).contains(marker))
        .collect()
}

/// PR 482 read from GitHub (no assessment pass asked) and watched by `u1`.
async fn watched_482(w: &World) -> i64 {
    let read = crate::review_loop::read_if_due(&w.state, DEFAULT_TENANT, "u1", &pr_482(), false)
        .await
        .expect("read");
    let id = read.handle.id;
    w.state
        .db
        .review_loop()
        .watch(
            DEFAULT_TENANT,
            "u1",
            id,
            watch::WATCH_LIMIT,
            Timestamp::now(),
        )
        .await
        .expect("watch");
    id
}

async fn the_watch(w: &World, pr: i64) -> WatchRecord {
    w.state
        .db
        .review_loop()
        .watches("u1")
        .await
        .expect("watches")
        .into_iter()
        .find(|watch| watch.pull_request_id == pr)
        .expect("watched")
}

/// RW1 PUT watches (after reading the pull request) and RW2 lists it with the daily pass counter; DELETE stops it.
#[tokio::test]
async fn watch_routes_watch_list_and_unwatch() {
    let w = world().await;
    let (status, body) = w.call("PUT", &format!("{PR}/watch"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let list = w.get("/api/v1/watches").await;
    assert_eq!(list["watches"][0]["pr"]["number"], 482, "{list}");
    assert_eq!(list["watches"][0]["state"], "active");
    assert_eq!(list["limit"], 20);
    assert_eq!(list["daily_limit"], 30);
    assert!(list["passes_today"].is_u64(), "{list}");
    let (status, _) = w.call("DELETE", &format!("{PR}/watch"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(w.get("/api/v1/watches").await["watches"], json!([]));
}

/// Twelve watched pull requests are probed with two aliased queries (ten and two), not twelve requests.
#[tokio::test]
async fn watch_probe_batches_ten_prs() {
    let w = world().await;
    let first = watched_482(&w).await;
    let repo = w
        .state
        .db
        .repos()
        .upsert(DEFAULT_TENANT, &pr_482().repo, None)
        .await
        .expect("repo");
    for number in 483..494 {
        let summary = contracts::api::pr::PrSummary {
            repo: repo.repo.clone(),
            number,
            title: format!("PR {number}"),
            author: "alice".into(),
            html_url: format!("https://github.com/acme/node-client/pull/{number}"),
            base_sha: CommitSha("1".repeat(40)),
            head_sha: CommitSha("2".repeat(40)),
        };
        let id = w
            .state
            .db
            .pull_requests()
            .upsert(DEFAULT_TENANT, repo.id, &summary, Timestamp::now())
            .await
            .expect("pr");
        w.state
            .db
            .review_loop()
            .watch(
                DEFAULT_TENANT,
                "u1",
                id,
                watch::WATCH_LIMIT,
                Timestamp::now(),
            )
            .await
            .expect("watch");
    }
    // Unresolved answers (`null`): nothing is read.
    mount_probe(&w.server, json!({"data": {}})).await;
    watch::tick_at(&w.state, Timestamp::now())
        .await
        .expect("tick");
    let probes = received(&w.server, PROBE_MARKER).await;
    assert_eq!(probes.len(), 2, "one aliased query per ten pull requests");
    let mut aliases: Vec<usize> = probes
        .iter()
        .map(|request| {
            String::from_utf8_lossy(&request.body)
                .matches("pullRequest(number:")
                .count()
        })
        .collect();
    aliases.sort_unstable();
    assert_eq!(aliases, [2, 10]);
    assert!(
        the_watch(&w, first).await.last_probe_at.is_some(),
        "an unresolved probe still counts as probed"
    );
}

/// A probe whose values did not change since the last one reads nothing more from GitHub and makes no pass due.
#[tokio::test]
async fn unchanged_probe_reads_nothing_more() {
    let w = world().await;
    let pr = watched_482(&w).await;
    w.age_read().await;
    mount_probe(&w.server, probe_answer("2026-11-12T10:40:12Z")).await;
    let now = Timestamp::now();
    watch::tick_at(&w.state, now).await.expect("first tick");
    let watched = the_watch(&w, pr).await;
    assert!(watched.probe.is_some(), "the first probe stores its values");
    let reads = received(&w.server, READ_MARKER).await.len();
    w.state
        .db
        .review_loop()
        .set_pass_due("u1", pr, None)
        .await
        .expect("clear due");

    let later = now + PROBE_INTERVAL + SignedDuration::from_secs(1);
    watch::tick_at(&w.state, later).await.expect("second tick");
    assert_eq!(received(&w.server, PROBE_MARKER).await.len(), 2);
    assert_eq!(
        received(&w.server, READ_MARKER).await.len(),
        reads,
        "an unchanged probe reads nothing more"
    );
    assert_eq!(the_watch(&w, pr).await.pass_due_at, None);
}

/// A probe that finds a change reads the pull request in full and makes an automatic pass due.
#[tokio::test]
async fn changed_probe_reads_the_pull_request_and_makes_a_pass_due() {
    let w = world().await;
    let pr = watched_482(&w).await;
    mount_probe(&w.server, probe_answer("2026-11-12T10:40:12Z")).await;
    let now = Timestamp::now();
    watch::tick_at(&w.state, now).await.expect("first tick");
    w.state
        .db
        .review_loop()
        .set_pass_due("u1", pr, None)
        .await
        .expect("clear due");
    w.age_read().await;
    let reads = received(&w.server, READ_MARKER).await.len();
    w.server.reset().await;
    serve(&w.server, &[]).await;
    mount_probe(&w.server, probe_answer("2026-11-12T11:02:00Z")).await;
    let later = now + PROBE_INTERVAL + SignedDuration::from_secs(1);
    watch::tick_at(&w.state, later).await.expect("second tick");
    assert_eq!(
        received(&w.server, READ_MARKER).await.len(),
        1,
        "the changed pull request is read once (before: {reads})"
    );
    let watched = the_watch(&w, pr).await;
    assert!(watched.pass_due_at.is_some() || watched.state == WatchState::JobRunning);
}

/// T27: the probe and the reads it starts only query GitHub; nothing is written there.
#[tokio::test]
async fn watch_probe_sends_no_mutation() {
    let w = world().await;
    watched_482(&w).await;
    w.age_read().await;
    mount_probe(&w.server, probe_answer("2026-11-12T10:40:12Z")).await;
    watch::tick_at(&w.state, Timestamp::now())
        .await
        .expect("tick");
    let requests = w.server.received_requests().await.unwrap_or_default();
    assert!(!received(&w.server, PROBE_MARKER).await.is_empty());
    for request in requests {
        let body = String::from_utf8_lossy(&request.body);
        assert!(
            request.method.as_str() == "GET" || !body.contains("mutation"),
            "{body}"
        );
        assert!(
            request.method.as_str() == "GET" || request.url.path() == "/api/graphql",
            "only GraphQL reads: {} {}",
            request.method,
            request.url
        );
    }
}

/// T27: without a stored token pair the user's watches pause; they resume once a probe reads GitHub again.
#[tokio::test]
async fn watch_pauses_without_token_pair() {
    let w = world().await;
    let pr = watched_482(&w).await;
    mount_probe(&w.server, probe_answer("2026-11-12T10:40:12Z")).await;
    w.state
        .db
        .user_tokens()
        .end_sign_in("u1", PairCondition::Any)
        .await
        .expect("pair deleted");
    watch::tick_at(&w.state, Timestamp::now())
        .await
        .expect("tick");
    assert_eq!(the_watch(&w, pr).await.state, WatchState::Paused);
    assert!(
        received(&w.server, PROBE_MARKER).await.is_empty(),
        "no token, no request"
    );
}

/// A due automatic pass waits while thepastaclaw reviews the head (its gate comment in progress), and is requested
/// once the wait of 90 minutes is over.
#[tokio::test]
async fn watched_pass_waits_for_bot_round() {
    let w = world().await;
    serve(
        &w.server,
        &[("comments.json", &|answer: &mut Value| {
            answer["data"]["repository"]["pullRequest"]["comments"]["nodes"][0]["body"] = json!(
                "<!-- thepastaclaw-gate v1 -->\n🔍 Review in progress (commit 52d2da14) · [live](x)"
            );
        })],
    )
    .await;
    let pr = watched_482(&w).await;
    mount_probe(&w.server, json!({"data": {}})).await;
    let rl = w.state.db.review_loop();
    let now = Timestamp::now();
    rl.set_pass_due("u1", pr, Some(now)).await.expect("due");
    watch::tick_at(&w.state, now).await.expect("tick");
    assert_eq!(
        the_watch(&w, pr).await.state,
        WatchState::WaitingForBotRound
    );
    assert!(
        rl.live_passes().await.expect("passes").is_empty(),
        "no pass while the bot reviews"
    );

    rl.set_pass_due("u1", pr, None).await.expect("clear");
    rl.set_pass_due(
        "u1",
        pr,
        Some(now - BOT_ROUND_WAIT - SignedDuration::from_mins(1)),
    )
    .await
    .expect("due long ago");
    watch::tick_at(&w.state, now).await.expect("tick");
    let watched = the_watch(&w, pr).await;
    assert_eq!(watched.state, WatchState::JobRunning);
    assert_eq!(watched.pass_due_at, None, "the pass covers the change");
    let passes = rl.live_passes().await.expect("passes");
    assert_eq!(passes.len(), 1);
    assert_eq!(passes[0].cause, PassCause::Watch);
}

/// An automatic pass that leaves undecided questions writes one `questions_ready` event per round and watcher.
#[tokio::test]
async fn questions_ready_event_once_per_round() {
    let w = world().await;
    let pr = watched_482(&w).await;
    let rl = w.state.db.review_loop();
    let pass = NewPass {
        pull_request_id: pr,
        user_id: Some("u1".into()),
        cause: PassCause::Watch,
        head: CommitSha(crate::test_support::github::HEAD.to_owned()),
        sent: Vec::new(),
        question: None,
    };
    let PassStart::Started(id) = rl
        .start_pass(DEFAULT_TENANT, &pass, Timestamp::now())
        .await
        .expect("start")
    else {
        panic!("a pass is live");
    };
    let record = rl.pass(id).await.expect("pass");
    watch::pass_ended(&w.state, &record).await.expect("ended");
    watch::pass_ended(&w.state, &record)
        .await
        .expect("ended again");
    let events = rl.user_events("u1", None, 10).await.expect("events");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].kind, UserEventKind::QuestionsReady);
    assert!(events[0].count > 0);
    assert_eq!(
        events[0].text,
        format!("acme/node-client #482: {} questions ready", events[0].count)
    );
    assert_eq!(
        events[0].url.as_deref(),
        Some("/pr/acme/node-client/482/comments")
    );
    let watched = the_watch(&w, pr).await;
    assert_eq!(watched.notified_for.as_deref(), Some("round:3"));
    assert_eq!(
        watched.last_auto_pass.map(|auto| auto.questions),
        Some(events[0].count)
    );

    // The next round notifies again.
    rl.set_notified("u1", pr, "round:2")
        .await
        .expect("older round");
    watch::pass_ended(&w.state, &record)
        .await
        .expect("next round");
    assert_eq!(
        rl.user_events("u1", None, 10).await.expect("events").len(),
        2
    );
}

/// Plan §2.8 item 2: a plug-in change signal for a watched pull request probes it at once, before its 5 minutes are
/// up (owner and name compared without case); a signal for a pull request nobody watches probes nothing.
#[tokio::test]
async fn a_plugin_change_signal_probes_the_watched_pull_request_at_once() {
    use std::collections::HashSet;

    use crate::dashboard::plugin::PrKey;

    let w = world().await;
    let pr = watched_482(&w).await;
    mount_probe(&w.server, probe_answer("2026-11-12T10:40:12Z")).await;
    watch::tick_at(&w.state, Timestamp::now())
        .await
        .expect("first tick");
    watch::tick_at(&w.state, Timestamp::now())
        .await
        .expect("a tick within the interval");
    assert_eq!(
        received(&w.server, PROBE_MARKER).await.len(),
        1,
        "not due again yet"
    );
    let probed_at = the_watch(&w, pr).await.last_probe_at;

    let unwatched = RepoRef {
        owner: "acme".into(),
        name: "other".into(),
    };
    watch::plugin_changed(&w.state, &HashSet::from([PrKey::new(&unwatched, 7)])).await;
    assert_eq!(received(&w.server, PROBE_MARKER).await.len(), 1);

    let shouted = RepoRef {
        owner: "ACME".into(),
        name: "Node-Client".into(),
    };
    watch::plugin_changed(&w.state, &HashSet::from([PrKey::new(&shouted, 482)])).await;
    assert_eq!(
        received(&w.server, PROBE_MARKER).await.len(),
        2,
        "the signal probes the watched pull request at once"
    );
    assert!(the_watch(&w, pr).await.last_probe_at >= probed_at);
}

/// A change whose full read is skipped as too soon after another read keeps the old probe values: the next probe
/// sees the change again and reads it (the probe time is recorded, so the interval holds).
#[tokio::test]
async fn a_probe_skipped_as_too_soon_keeps_the_change_for_the_next_probe() {
    let w = world().await;
    let pr = watched_482(&w).await;
    mount_probe(&w.server, probe_answer("2026-11-12T10:40:12Z")).await;
    let reads = received(&w.server, READ_MARKER).await.len();
    let now = Timestamp::now();
    watch::tick_at(&w.state, now).await.expect("tick");
    let watched = the_watch(&w, pr).await;
    assert!(
        watched
            .last_probe_at
            .is_some_and(|at| at.duration_since(now).abs() < SignedDuration::from_millis(1)),
        "the probe time is recorded: {:?}",
        watched.last_probe_at
    );
    assert!(
        watched.probe.as_ref().is_none_or(Value::is_null),
        "values of an unread change are not stored: {:?}",
        watched.probe
    );
    assert_eq!(watched.pass_due_at, None);
    assert_eq!(received(&w.server, READ_MARKER).await.len(), reads);

    w.age_read().await;
    let later = now + PROBE_INTERVAL + SignedDuration::from_secs(1);
    watch::tick_at(&w.state, later).await.expect("next tick");
    assert!(
        received(&w.server, READ_MARKER).await.len() > reads,
        "the change is read now"
    );
    let watched = the_watch(&w, pr).await;
    assert!(watched.probe.as_ref().is_some_and(|probe| !probe.is_null()));
    assert!(watched.pass_due_at.is_some() || watched.state == WatchState::JobRunning);
}

/// While GitHub's rate limit is used up a user's watches are not probed (neither by the scheduler nor by a plug-in
/// signal) until the reset GitHub named; the limit is logged at `warn` once, and its end once.
#[tokio::test]
async fn a_rate_limited_user_is_not_probed_until_the_reset() {
    use std::collections::HashSet;

    use crate::dashboard::plugin::PrKey;
    use crate::test_support::logs::LogCapture;

    let w = world().await;
    watched_482(&w).await;
    let capture = LogCapture::start();
    let now = Timestamp::now();
    let reset = now + SignedDuration::from_mins(45);
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_string_contains(PROBE_MARKER))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("x-ratelimit-remaining", "0")
                .insert_header("x-ratelimit-reset", reset.as_second().to_string().as_str()),
        )
        .mount(&w.server)
        .await;
    watch::tick_at(&w.state, now).await.expect("limited tick");
    assert_eq!(received(&w.server, PROBE_MARKER).await.len(), 1);
    for minutes in [1, 6, 44] {
        watch::tick_at(&w.state, now + SignedDuration::from_mins(minutes))
            .await
            .expect("tick");
    }
    watch::plugin_changed(&w.state, &HashSet::from([PrKey::new(&pr_482().repo, 482)])).await;
    assert_eq!(
        received(&w.server, PROBE_MARKER).await.len(),
        1,
        "nothing is sent before the reset"
    );
    watch::tick_at(&w.state, now + SignedDuration::from_mins(46))
        .await
        .expect("after the reset");
    assert_eq!(received(&w.server, PROBE_MARKER).await.len(), 2);

    w.server.reset().await;
    serve(&w.server, &[]).await;
    mount_probe(&w.server, probe_answer("2026-11-12T10:40:12Z")).await;
    watch::tick_at(&w.state, now + SignedDuration::from_mins(47))
        .await
        .expect("probed again");
    assert_eq!(received(&w.server, PROBE_MARKER).await.len(), 1);
    let text = capture.text();
    assert_eq!(
        text.matches("GitHub's rate limit stops this user's watch probes;")
            .count(),
        1,
        "{text}"
    );
    assert_eq!(
        text.matches("watch probes read GitHub again after its rate limit")
            .count(),
        1,
        "{text}"
    );
}

/// A due change that an on-demand pass already assessed (the automatic pass has nothing to send) still tells the
/// watcher, once per round, that questions wait.
/// Two settles that both loaded the due watch before either wrote its event (the scheduler and a plug-in signal)
/// tell the watcher once.
#[tokio::test]
async fn concurrent_settles_write_questions_ready_once() {
    let w = world().await;
    let pr = watched_482(&w).await;
    mount_probe(&w.server, json!({"data": {}})).await;
    let rl = w.state.db.review_loop();
    for item in rl.items(pr).await.expect("items") {
        if item.kind != contracts::api::review_loop::CommentItemKind::PrQuestion
            && item.state != contracts::api::review_loop::ItemState::Outside
        {
            rl.set_item_state(
                item.id,
                item.revision,
                contracts::api::review_loop::ItemState::Decided,
                Timestamp::now(),
            )
            .await
            .expect("decided");
        }
    }
    let now = Timestamp::now();
    rl.set_pass_due("u1", pr, Some(now)).await.expect("due");
    let (first, second) =
        tokio::join!(watch::tick_at(&w.state, now), watch::tick_at(&w.state, now));
    first.expect("tick");
    second.expect("tick");
    let events = rl.user_events("u1", None, 10).await.expect("events");
    assert_eq!(events.len(), 1, "{events:?}");
}

#[tokio::test]
async fn a_due_change_assessed_on_demand_still_notifies_the_watcher() {
    let w = world().await;
    let pr = watched_482(&w).await;
    mount_probe(&w.server, json!({"data": {}})).await;
    let rl = w.state.db.review_loop();
    for item in rl.items(pr).await.expect("items") {
        if item.kind != contracts::api::review_loop::CommentItemKind::PrQuestion
            && item.state != contracts::api::review_loop::ItemState::Outside
        {
            rl.set_item_state(
                item.id,
                item.revision,
                contracts::api::review_loop::ItemState::Decided,
                Timestamp::now(),
            )
            .await
            .expect("decided");
        }
    }
    let now = Timestamp::now();
    for _ in 0..2 {
        rl.set_pass_due("u1", pr, Some(now)).await.expect("due");
        watch::tick_at(&w.state, now).await.expect("tick");
    }
    assert!(
        rl.live_passes().await.expect("passes").is_empty(),
        "nothing to assess"
    );
    let watched = the_watch(&w, pr).await;
    assert_eq!(watched.pass_due_at, None);
    let events = rl.user_events("u1", None, 10).await.expect("events");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].kind, UserEventKind::QuestionsReady);
    assert_eq!(watched.notified_for.as_deref(), Some("round:3"));
}
