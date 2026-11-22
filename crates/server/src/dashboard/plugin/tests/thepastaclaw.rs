//! The thepastaclaw status plug-in against the in-process fake status file (`test_support::thepastaclaw`).

use axum::http::StatusCode;
use serde_json::json;

use super::*;
use crate::dashboard::plugin::thepastaclaw::Thepastaclaw;
use crate::test_support::logs::LogCapture;
use crate::test_support::thepastaclaw::{FakeThepastaclaw, active, queued, status};

/// The data time of the fake's files: about 20 minutes before [`at`], as the live file trails.
const DATA_AS_OF: &str = "2026-11-22T20:16:14Z";

fn at() -> Timestamp {
    "2026-11-22T20:36:00Z".parse().expect("now")
}

fn platform(number: u32) -> PrKey {
    PrKey::new(&repo("dashpay", "platform"), number)
}

async fn polled(plugin: &Thepastaclaw) -> PluginData {
    match plugin.poll().await {
        Ok(Polled::Data(data)) => data,
        other => panic!("data expected, got {other:?}"),
    }
}

fn facts(statement: &PrStatement) -> Vec<(&str, &str)> {
    statement
        .facts
        .iter()
        .map(|f| (f.label.as_str(), f.value.as_str()))
        .collect()
}

#[tokio::test]
async fn thepastaclaw_states_reviewing_and_queued_pull_requests_without_a_readiness_opinion() {
    let fake = FakeThepastaclaw::start(status(
        DATA_AS_OF,
        vec![
            active("dashpay/platform", 6084, 13043, "context", 4),
            active("not a repo", 1, 1, "context", 4),
        ],
        vec![
            queued("dashpay/platform", 6051, "waiting for a review slot"),
            queued("dashpay/platform", 0, "waiting for a review slot"),
        ],
    ))
    .await;
    let plugin = Thepastaclaw::new(fake.url.clone()).expect("plugin");
    assert_eq!(
        (plugin.id(), plugin.name()),
        ("thepastaclaw", "thepastaclaw")
    );
    let home = fake.url.join("../").expect("home");
    assert_eq!(plugin.home(), &home, "the human page above data/");

    let data = polled(&plugin).await;
    assert_eq!(data.as_of, DATA_AS_OF.parse().expect("as of"), "data_as_of");
    let mut numbers: Vec<_> = data.prs.keys().map(|k| k.number).collect();
    numbers.sort_unstable();
    assert_eq!(numbers, [6051, 6084], "invalid records get no statement");

    let reviewing = &data.prs[&platform(6084)];
    assert_eq!(
        reviewing.summary.as_deref(),
        Some("reviewing (context, step 4 of 10)")
    );
    assert_eq!(
        facts(reviewing),
        [("Since", "2026-11-22 19:53 UTC"), ("Head", "ebe26cc")]
    );
    assert_eq!(reviewing.url, format!("{home}#run=13043"));
    assert_eq!(reviewing.ready_to_merge, None, "never an opinion");
    assert!(reviewing.details.is_empty());

    let waiting = &data.prs[&platform(6051)];
    assert_eq!(
        waiting.summary.as_deref(),
        Some("queued (waiting for a review slot)")
    );
    assert_eq!(
        facts(waiting),
        [
            ("Eligible", "2026-11-22 19:15 UTC"),
            ("Position", "1"),
            ("Head", "7e29566")
        ]
    );
    assert_eq!(waiting.url, format!("{home}#queue"));
    assert_eq!(waiting.ready_to_merge, None);
}

#[tokio::test]
async fn thepastaclaw_reviewing_and_queued_again_is_one_statement() {
    let fake = FakeThepastaclaw::start(status(
        DATA_AS_OF,
        vec![active("dashpay/platform", 6084, 13043, "phase2", 8)],
        vec![queued(
            "DashPay/Platform",
            6084,
            "waiting for a review slot",
        )],
    ))
    .await;
    let plugin = Thepastaclaw::new(fake.url.clone()).expect("plugin");
    let data = polled(&plugin).await;
    assert_eq!(data.prs.len(), 1);
    let statement = &data.prs[&platform(6084)];
    assert_eq!(
        statement.summary.as_deref(),
        Some("reviewing (phase2, step 8 of 10)")
    );
    assert_eq!(
        statement.details,
        ["queued again for head 7e29566 (waiting for a review slot)"]
    );
}

#[tokio::test]
async fn thepastaclaw_shows_what_the_record_carries() {
    let mut bare = json!({"repo": "dashpay/platform", "number": 7, "id": 5});
    let fake = FakeThepastaclaw::start(status(DATA_AS_OF, vec![bare.clone()], vec![])).await;
    let plugin = Thepastaclaw::new(fake.url.clone()).expect("plugin");
    let data = polled(&plugin).await;
    let statement = &data.prs[&platform(7)];
    assert_eq!(statement.summary.as_deref(), Some("reviewing"));
    assert!(statement.facts.is_empty());

    bare["phase"] = json!("gate");
    bare["progress"] = json!(0.5);
    fake.set_body(status(DATA_AS_OF, vec![bare], vec![]));
    let data = polled(&plugin).await;
    assert_eq!(
        data.prs[&platform(7)].summary.as_deref(),
        Some("reviewing (gate)"),
        "a bare fraction is not shown (no percentages, brief §1)"
    );
}

#[tokio::test]
async fn thepastaclaw_revalidates_with_the_etag() {
    let fake = FakeThepastaclaw::start(status(DATA_AS_OF, vec![], vec![])).await;
    let plugin = Thepastaclaw::new(fake.url.clone()).expect("plugin");
    polled(&plugin).await;
    assert_eq!(plugin.poll().await.expect("poll"), Polled::Unchanged);
    let served = fake.served();
    assert_eq!(served.len(), 2);
    assert_eq!(
        (served[0].if_none_match.as_deref(), served[0].status),
        (None, 200)
    );
    assert!(
        served[1].if_none_match.is_some() && served[1].status == 304,
        "{served:?}"
    );
    fake.set_body(status(
        DATA_AS_OF,
        vec![active("dashpay/platform", 6084, 13043, "context", 4)],
        vec![],
    ));
    assert_eq!(polled(&plugin).await.prs.len(), 1, "a new file is read");
}

#[tokio::test]
async fn thepastaclaw_round_end_is_a_change_but_progress_within_a_run_is_not() {
    let run = |phase, steps| active("dashpay/platform", 6084, 13043, phase, steps);
    let fake = FakeThepastaclaw::start(status(
        DATA_AS_OF,
        vec![run("context", 4)],
        vec![queued(
            "dashpay/platform",
            6051,
            "waiting for a review slot",
        )],
    ))
    .await;
    let host = PluginHost::new(
        vec![Arc::new(
            Thepastaclaw::new(fake.url.clone()).expect("plugin"),
        )],
        Timing::default(),
    );
    assert!(host.poll_once(at()).await.is_empty(), "first data");
    assert_eq!(
        host.overlay(at()).notes(&repo("dashpay", "platform"), 6084)[0]
            .summary
            .as_deref(),
        Some("reviewing (context, step 4 of 10)")
    );

    let mut moved = status(
        DATA_AS_OF,
        vec![run("verify2", 9)],
        vec![queued(
            "dashpay/platform",
            6051,
            "waiting for a review slot",
        )],
    );
    moved["live"]["active"][0]["progress"] = json!(0.9);
    moved["live"]["active"][0]["heartbeat_at"] = json!("2026-11-22T20:30:00Z");
    moved["live"]["queued"][0]["age_seconds"] = json!(4000);
    moved["live"]["queued"][0]["eligible_in_seconds"] = json!(5);
    fake.set_body(moved);
    assert!(
        host.poll_once(at()).await.is_empty(),
        "phase, steps and clocks advance within the run"
    );
    assert_eq!(
        host.overlay(at()).notes(&repo("dashpay", "platform"), 6084)[0]
            .summary
            .as_deref(),
        Some("reviewing (verify2, step 9 of 10)"),
        "the shown statement follows the run"
    );

    fake.set_body(status(
        DATA_AS_OF,
        vec![active("dashpay/platform", 6051, 13050, "worktree", 1)],
        vec![],
    ));
    assert_eq!(
        host.poll_once(at()).await,
        HashSet::from([platform(6084), platform(6051)]),
        "6084's round ended; 6051 left the queue for a run"
    );
    assert!(
        host.overlay(at())
            .notes(&repo("dashpay", "platform"), 6084)
            .is_empty()
    );
}

#[tokio::test]
async fn thepastaclaw_trailing_data_is_shown_within_the_hour_bound() {
    let fake = FakeThepastaclaw::start(status(
        DATA_AS_OF,
        vec![active("dashpay/platform", 6084, 13043, "context", 4)],
        vec![],
    ))
    .await;
    let host = PluginHost::new(
        vec![Arc::new(
            Thepastaclaw::new(fake.url.clone()).expect("plugin"),
        )],
        Timing::default(),
    );
    host.poll_once(at()).await;
    assert_eq!(
        host.overlay(at())
            .notes(&repo("dashpay", "platform"), 6084)
            .len(),
        1,
        "data 20 minutes behind is shown"
    );
    assert_eq!(host.statuses(at())[0].state, PluginState::Current);

    let late = DATA_AS_OF.parse::<Timestamp>().expect("as of") + SignedDuration::from_mins(61);
    host.poll_once(late).await;
    assert!(
        host.overlay(late)
            .notes(&repo("dashpay", "platform"), 6084)
            .is_empty(),
        "a file not updated for over an hour is not shown"
    );
    assert_eq!(host.statuses(late)[0].state, PluginState::Stale);
}

#[tokio::test]
async fn thepastaclaw_failures_are_errors() {
    let fake = FakeThepastaclaw::start(status(DATA_AS_OF, vec![], vec![])).await;
    let plugin = Thepastaclaw::new(fake.url.clone()).expect("plugin");
    fake.set_status(Some(StatusCode::NOT_FOUND));
    assert!(matches!(plugin.poll().await, Err(PluginError::Status(404))));
    fake.set_status(None);
    fake.set_raw(Some("<html>"));
    assert!(matches!(
        plugin.poll().await,
        Err(PluginError::Malformed(_))
    ));
    fake.set_raw(Some("{\"live\": {\"active\": [], \"queued\": []}}"));
    assert!(
        matches!(plugin.poll().await, Err(PluginError::Malformed(_))),
        "no data time"
    );
    fake.set_raw(Some(&" ".repeat(8 * 1024 * 1024 + 1)));
    assert!(matches!(
        plugin.poll().await,
        Err(PluginError::TooLarge { .. })
    ));
    fake.set_raw(None);
    assert!(matches!(plugin.poll().await, Ok(Polled::Data(_))));

    let closed = Thepastaclaw::new(Url::parse("http://127.0.0.1:9/data/status.json").expect("url"))
        .expect("plugin");
    assert!(matches!(closed.poll().await, Err(PluginError::Http(_))));
}

#[tokio::test]
async fn thepastaclaw_logs_unreadable_records_once_per_change_of_their_count() {
    let logs = LogCapture::start();
    let fake = FakeThepastaclaw::start(status(
        DATA_AS_OF,
        vec![
            active("dashpay/platform", 6084, 13043, "context", 4),
            json!({"repo": "dashpay/platform", "number": "two"}),
        ],
        vec![queued("not a repo", 3, "waiting for a review slot")],
    ))
    .await;
    let plugin = Thepastaclaw::new(fake.url.clone()).expect("plugin");
    assert_eq!(polled(&plugin).await.prs.len(), 1);
    let unreadable =
        "thepastaclaw status records of an unexpected shape or repository name are left out";
    assert_eq!(
        logs.text().matches(unreadable).count(),
        1,
        "{}",
        logs.text()
    );
    assert!(logs.text().contains("unreadable=2"), "{}", logs.text());

    let mut body = fake.body();
    body["live"]["active"][0]["phase"] = json!("phase1");
    fake.set_body(body);
    polled(&plugin).await;
    assert_eq!(
        logs.text().matches(unreadable).count(),
        1,
        "same count: no new line"
    );

    fake.set_body(status(DATA_AS_OF, vec![], vec![]));
    polled(&plugin).await;
    assert_eq!(
        logs.text()
            .matches("thepastaclaw status records are all readable again")
            .count(),
        1
    );
}
