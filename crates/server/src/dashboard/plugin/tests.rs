//! The plug-in host (presence gating, cadence, freshness, change signal, logging) and the PR Hygiene plug-in
//! against the in-process fake service (`test_support::pr_hygiene`); no real network.

use std::collections::VecDeque;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;

use super::pr_hygiene::PrHygiene;
use super::*;
use crate::state::tests::test_state;
use crate::test_support::logs::LogCapture;
use crate::test_support::pr_hygiene::{FakePrHygiene, record};

fn now() -> Timestamp {
    "2026-11-18T09:00:00Z".parse().expect("now")
}

fn repo(owner: &str, name: &str) -> RepoRef {
    RepoRef {
        owner: owner.to_owned(),
        name: name.to_owned(),
    }
}

fn statement(mark: &str, readiness: Option<Readiness>) -> PrStatement {
    PrStatement {
        url: "https://source.example.test/pr/1".to_owned(),
        summary: Some("Author must post /self-reviewed".to_owned()),
        details: vec!["Author must post /self-reviewed".to_owned()],
        facts: vec![Fact {
            label: "Stage".to_owned(),
            value: "self-review".to_owned(),
        }],
        ready_to_merge: readiness,
        mark: mark.to_owned(),
    }
}

fn data(as_of: Timestamp, prs: &[(u32, &str)]) -> PluginData {
    PluginData {
        as_of,
        prs: prs
            .iter()
            .map(|(number, mark)| {
                (
                    PrKey::new(&repo("acme", "node-client"), *number),
                    statement(mark, None),
                )
            })
            .collect(),
    }
}

/// Answers its queued results in order; `Unchanged` once they run out.
#[derive(Debug)]
struct Scripted {
    home: Url,
    answers: Mutex<VecDeque<Result<Polled, PluginError>>>,
}

impl Scripted {
    fn new(answers: Vec<Result<Polled, PluginError>>) -> Arc<Self> {
        Arc::new(Self {
            home: Url::parse("https://source.example.test/").expect("url"),
            answers: Mutex::new(answers.into()),
        })
    }
}

impl DashboardPlugin for Scripted {
    fn id(&self) -> &'static str {
        "scripted"
    }

    fn name(&self) -> &'static str {
        "Scripted"
    }

    fn home(&self) -> &Url {
        &self.home
    }

    fn poll(&self) -> BoxFuture<'_, Result<Polled, PluginError>> {
        let answer = lock(&self.answers)
            .pop_front()
            .unwrap_or(Ok(Polled::Unchanged));
        Box::pin(async move { answer })
    }
}

fn host(plugin: Arc<dyn DashboardPlugin>, timing: Timing) -> PluginHost {
    PluginHost::new(vec![plugin], timing)
}

fn status_error() -> PluginError {
    PluginError::Status(503)
}

// ---------------------------------------------------------------------------------------------------------------
// Host

#[tokio::test]
async fn the_change_signal_names_added_changed_and_removed_pull_requests_but_not_on_first_data() {
    let plugin = Scripted::new(vec![
        Ok(Polled::Data(data(now(), &[(1, "a"), (2, "b"), (3, "c")]))),
        Ok(Polled::Unchanged),
        Ok(Polled::Data(data(now(), &[(1, "a"), (2, "B"), (4, "d")]))),
    ]);
    let host = host(plugin, Timing::default());
    assert!(host.poll_once(now()).await.is_empty(), "first data");
    assert!(host.poll_once(now()).await.is_empty(), "304");
    let changed = host.poll_once(now()).await;
    let node = |n| PrKey::new(&repo("acme", "node-client"), n);
    assert_eq!(changed, HashSet::from([node(2), node(3), node(4)]));
}

#[tokio::test]
async fn statements_are_matched_ignoring_case_and_shown_only_while_fresh() {
    let timing = Timing {
        stale_after: Duration::from_millis(150),
        ..Timing::default()
    };
    let plugin = Scripted::new(vec![Ok(Polled::Data(PluginData {
        as_of: now(),
        prs: [
            (
                PrKey::new(&repo("acme", "node-client"), 1),
                statement(
                    "a",
                    Some(Readiness::Holds {
                        reason: "Author must post /self-reviewed".to_owned(),
                    }),
                ),
            ),
            (
                PrKey::new(&repo("acme", "node-client"), 2),
                statement("b", Some(Readiness::Agrees)),
            ),
        ]
        .into(),
    }))]);
    let host = host(plugin, timing);
    assert_eq!(host.statuses(now())[0].state, PluginState::Waiting);
    assert!(
        host.overlay(now())
            .notes(&repo("acme", "node-client"), 1)
            .is_empty()
    );
    host.poll_once(now()).await;
    let overlay = host.overlay(now());
    let notes = overlay.notes(&repo("Acme", "Node-Client"), 1);
    assert_eq!(notes.len(), 1);
    assert_eq!(
        (
            notes[0].plugin.as_str(),
            notes[0].source.as_str(),
            notes[0].as_of
        ),
        ("scripted", "Scripted", now())
    );
    assert_eq!(
        overlay
            .hold(&repo("acme", "node-client"), 1)
            .map(|h| h.reason),
        Some("Author must post /self-reviewed".to_owned())
    );
    assert_eq!(
        overlay.hold(&repo("acme", "node-client"), 2),
        None,
        "agrees"
    );
    assert_eq!(
        overlay.hold(&repo("acme", "node-client"), 3),
        None,
        "uncovered"
    );
    assert_eq!(host.statuses(now())[0].state, PluginState::Current);

    let late = now() + SignedDuration::from_mins(61);
    assert!(
        host.overlay(late)
            .notes(&repo("acme", "node-client"), 1)
            .is_empty(),
        "data older than an hour is not shown"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        host.overlay(now())
            .notes(&repo("acme", "node-client"), 1)
            .is_empty(),
        "no successful poll within the staleness bound"
    );
    let status = &host.statuses(now())[0];
    assert_eq!(
        (status.state, status.as_of),
        (PluginState::Stale, Some(now()))
    );
}

#[tokio::test]
async fn a_failing_source_is_logged_once_per_change_and_its_data_ages_out() {
    let logs = LogCapture::start();
    let timing = Timing {
        stale_after: Duration::from_millis(150),
        ..Timing::default()
    };
    let plugin = Scripted::new(vec![
        Ok(Polled::Data(data(now(), &[(1, "a")]))),
        Err(status_error()),
        Err(status_error()),
        Err(status_error()),
        Ok(Polled::Unchanged),
    ]);
    let host = host(plugin, timing);
    host.poll_once(now()).await;
    for _ in 0..3 {
        assert!(host.poll_once(now()).await.is_empty());
    }
    assert_eq!(
        host.overlay(now())
            .notes(&repo("acme", "node-client"), 1)
            .len(),
        1,
        "the last good data stays while fresh"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        host.overlay(now())
            .notes(&repo("acme", "node-client"), 1)
            .is_empty()
    );
    host.poll_once(now()).await;
    assert_eq!(
        host.overlay(now())
            .notes(&repo("acme", "node-client"), 1)
            .len(),
        1,
        "a 304 after the failures makes the data current again"
    );
    let text = logs.text();
    assert_eq!(
        text.matches("dashboard plug-in poll failed").count(),
        1,
        "{text}"
    );
    assert_eq!(
        text.matches("dashboard plug-in answers again").count(),
        1,
        "{text}"
    );
    assert_eq!(
        text.matches("dashboard plug-in statements are shown")
            .count(),
        1,
        "{text}"
    );
}

#[test]
fn external_text_is_bounded_plain_and_links_are_web_links() {
    let home = Url::parse("https://source.example.test/").expect("url");
    let mut long = statement(
        "a",
        Some(Readiness::Holds {
            reason: "r\u{7}".repeat(400),
        }),
    );
    long.summary = Some(format!("line\none\u{1b}[31m{}", "x".repeat(400)));
    long.details = (0..20).map(|n| format!("detail {n}")).collect();
    long.facts = (0..10)
        .map(|n| Fact {
            label: format!("label {n}"),
            value: "v".repeat(200),
        })
        .collect();
    long.url = "javascript:alert(1)".to_owned();
    let key = PrKey::new(&repo("acme", "node-client"), 1);
    let bounded = bounded(
        PluginData {
            as_of: now(),
            prs: [(key.clone(), long)].into(),
        },
        &home,
    );
    let s = &bounded.prs[&key];
    let summary = s.summary.as_deref().expect("summary");
    assert_eq!(summary.chars().count(), TEXT_CHARS);
    assert!(!summary.chars().any(char::is_control), "{summary:?}");
    assert_eq!(s.details.len(), DETAILS_KEPT);
    assert_eq!(s.facts.len(), FACTS_KEPT);
    assert_eq!(s.facts[0].value.chars().count(), FACT_CHARS);
    assert!(
        matches!(&s.ready_to_merge, Some(Readiness::Holds { reason })
        if (1..=TEXT_CHARS).contains(&reason.chars().count()) && !reason.chars().any(char::is_control))
    );
    assert_eq!(s.url, "https://source.example.test/");
}

#[tokio::test]
async fn nothing_is_requested_while_no_user_is_active_and_polls_follow_the_interval() {
    let fake = FakePrHygiene::start(
        "2026-11-18T08:59:00Z",
        vec![record(
            "dashpay/platform",
            1,
            Some("waiting-author"),
            "Fix it",
        )],
    )
    .await;
    let mut state = test_state().await;
    let timing = Timing {
        interval: Duration::from_millis(100),
        active_for: Duration::from_millis(400),
        ..Timing::default()
    };
    state.plugins = host(
        Arc::new(PrHygiene::new(fake.url.clone()).expect("plugin")),
        timing,
    );
    assert!(spawn(state.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        fake.served(),
        Vec::new(),
        "no user active: no request at all"
    );

    state.plugins.mark_active("t", "u1");
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(5), async {
        while fake.served().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("an active user starts the polls at once");
    assert!(
        started.elapsed() < Duration::from_millis(90),
        "not after a full interval"
    );
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let polls = fake
        .served()
        .iter()
        .filter(|s| s.path == "/api/v1/prs")
        .count();
    // Active for 400 ms after the mark: polls at 0, 100, 200, 300 and (at most) 400 ms.
    assert!((3..=6).contains(&polls), "{polls} polls");
    let after = fake.served().len();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        fake.served().len(),
        after,
        "no request once nobody is active"
    );
    let _ = state.stopping.send(true);
}

// ---------------------------------------------------------------------------------------------------------------
// PR Hygiene

#[tokio::test]
async fn pr_hygiene_states_governed_pull_requests_of_fresh_repositories() {
    let mut ungoverned = record("dashpay/platform", 3, None, "x");
    ungoverned["stage"] = json!("not-governed");
    let fake = FakePrHygiene::start(
        "2026-11-18T08:59:00Z",
        vec![
            record(
                "dashpay/platform",
                1,
                Some("waiting-self-review"),
                "Author must post /self-reviewed 1a2b3c",
            ),
            record("dashpay/platform", 2, Some("ready-to-merge"), "Merge it"),
            ungoverned,
            record("dashpay/grovedb", 4, Some("waiting-author"), "Fix it"),
            record("not a repo", 5, Some("waiting-author"), "Fix it"),
        ],
    )
    .await;
    let mut prs = fake.prs();
    prs["stale_repos"] = json!(["DashPay/GroveDB"]);
    fake.set_prs(prs);
    let plugin = PrHygiene::new(fake.url.clone()).expect("plugin");
    let Ok(Polled::Data(data)) = plugin.poll().await else {
        panic!("data expected");
    };
    assert_eq!(data.as_of, "2026-11-18T08:59:00Z".parse().expect("as of"));
    let mut keys: Vec<_> = data.prs.keys().map(|k| k.number).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [1, 2],
        "ungoverned, stale-repository and invalid records get no statement"
    );
    let first = &data.prs[&PrKey::new(&repo("dashpay", "platform"), 1)];
    assert_eq!(
        first.summary.as_deref(),
        Some("Author must post /self-reviewed 1a2b3c")
    );
    assert_eq!(
        first.details,
        [
            "Author must post /self-reviewed 1a2b3c",
            "Human approval or objection resolution is required"
        ]
    );
    let facts: Vec<_> = first
        .facts
        .iter()
        .map(|f| (f.label.as_str(), f.value.as_str()))
        .collect();
    assert_eq!(
        facts,
        [
            ("Stage", "self-review"),
            ("Whose move", "author"),
            ("Lateness", "late")
        ]
    );
    assert_eq!(
        first.ready_to_merge,
        Some(Readiness::Holds {
            reason: "Author must post /self-reviewed 1a2b3c".to_owned()
        })
    );
    assert_eq!(
        first.url,
        format!("{}api/v1/prs/dashpay/platform/1", fake.url)
    );
    assert_eq!(
        data.prs[&PrKey::new(&repo("dashpay", "platform"), 2)].ready_to_merge,
        Some(Readiness::Agrees)
    );
}

#[tokio::test]
async fn pr_hygiene_revalidates_with_etags_and_marks_ignore_time_only_values() {
    let fake = FakePrHygiene::start(
        "2026-11-18T08:59:00Z",
        vec![record(
            "dashpay/platform",
            1,
            Some("waiting-author"),
            "Fix it",
        )],
    )
    .await;
    let plugin = PrHygiene::new(fake.url.clone()).expect("plugin");
    let Ok(Polled::Data(first)) = plugin.poll().await else {
        panic!("data expected");
    };
    assert_eq!(plugin.poll().await.expect("poll"), Polled::Unchanged);
    let served = fake.served();
    assert_eq!(served.len(), 4);
    assert!(
        served[..2]
            .iter()
            .all(|s| s.if_none_match.is_none() && s.status == 200)
    );
    assert!(
        served[2..]
            .iter()
            .all(|s| s.if_none_match.is_some() && s.status == 304),
        "{served:?}"
    );

    let mut prs = fake.prs();
    prs["prs"][0]["lateness"] = json!("very-late");
    prs["prs"][0]["idle"] = json!(true);
    fake.set_prs(prs.clone());
    let Ok(Polled::Data(later)) = plugin.poll().await else {
        panic!("data expected");
    };
    let key = PrKey::new(&repo("dashpay", "platform"), 1);
    assert_eq!(
        later.prs[&key].mark, first.prs[&key].mark,
        "time alone changes nothing"
    );
    assert_eq!(later.prs[&key].facts[2].value, "very late");

    prs["prs"][0]["next_action"] = json!("Resolve the threads");
    fake.set_prs(prs);
    let Ok(Polled::Data(changed)) = plugin.poll().await else {
        panic!("data expected");
    };
    assert_ne!(changed.prs[&key].mark, first.prs[&key].mark);
}

#[tokio::test]
async fn pr_hygiene_failures_are_errors() {
    let fake = FakePrHygiene::start("2026-11-18T08:59:00Z", Vec::new()).await;
    let plugin = PrHygiene::new(fake.url.clone()).expect("plugin");
    fake.set_status(Some(StatusCode::SERVICE_UNAVAILABLE));
    assert!(matches!(plugin.poll().await, Err(PluginError::Status(503))));
    fake.set_status(Some(StatusCode::INTERNAL_SERVER_ERROR));
    assert!(matches!(plugin.poll().await, Err(PluginError::Status(500))));
    fake.set_status(None);
    fake.set_raw(Some("{\"generated_at\": 12"));
    assert!(matches!(
        plugin.poll().await,
        Err(PluginError::Malformed(_))
    ));
    fake.set_raw(Some("{\"prs\": []}"));
    assert!(
        matches!(plugin.poll().await, Err(PluginError::Malformed(_))),
        "no generation time"
    );
    let huge = " ".repeat(8 * 1024 * 1024 + 1);
    fake.set_raw(Some(&huge));
    assert!(matches!(
        plugin.poll().await,
        Err(PluginError::TooLarge { .. })
    ));
    fake.set_raw(None);
    assert!(matches!(plugin.poll().await, Ok(Polled::Data(_))));

    let closed = PrHygiene::new(Url::parse("http://127.0.0.1:9/").expect("url")).expect("plugin");
    assert!(matches!(closed.poll().await, Err(PluginError::Http(_))));
}
