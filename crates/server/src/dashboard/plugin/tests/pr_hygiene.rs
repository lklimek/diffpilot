use super::*;

fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/pr-hygiene-1117.json")).expect("fixture")
}

async fn read_fixture(body: serde_json::Value) -> PluginData {
    let fake = FakePrHygiene::start("2026-11-23T15:05:25Z", vec![body["pr"].clone()]).await;
    fake.set_detail("dashpay/dash-evo-tool", 1117, body);
    let plugin = PrHygiene::new(fake.url.clone()).expect("plugin");
    let Polled::Data(data) = plugin.poll().await.expect("poll") else {
        panic!("data")
    };
    bounded(data, &fake.url)
}

#[tokio::test]
async fn pr_hygiene_1117_exposes_readable_full_context_and_latest_reviews() {
    let data = read_fixture(fixture()).await;
    let note = &data.prs[&PrKey::new(&repo("dashpay", "dash-evo-tool"), 1117)];
    assert_eq!(
        note.summary.as_deref(),
        Some("coderabbitai has not reported for the current head")
    );
    let text = note
        .facts
        .iter()
        .map(|f| format!("{}: {}", f.label, f.value))
        .chain(note.details.clone())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "Stage: Bot reviews",
        "Engine state: Waiting for bot reviews",
        "Since: 2026-11-23T13:25:54Z (1h 39m at data time)",
        "Since basis: Engine",
        "Changes requested: Yes",
        "CI failing: Yes",
        "Merge conflict: No",
        "Unresolved comments: 0",
        "Idle: No",
        "Draft: No",
        "Tracked: Yes",
        "Lateness: on time",
        "Areas: fallback",
        "Base: v1.0-dev",
        "Milestone: None",
        "Author: Claudius-Maginificent (bot)",
        "Updated: 2026-11-23T14:09:06Z",
        "Blocker: thepastaclaw requested changes on this head; dismiss the review or push a fix",
        "Latest review by lklimek: Approved at 2026-11-23T13:50:47Z",
        "Latest review by thepastaclaw: Changes requested at 2026-11-23T14:08:16Z",
        "Stage change: Self review; Waiting for author; since 2026-11-23T06:40:24Z; observed 2026-11-23T06:39:51.795763289Z",
        "Stale: No",
        "Data generated: 2026-11-23T15:05:25.864696284Z",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in {text}");
    }
    assert_eq!(text.matches("Latest review by lklimek:").count(), 1);
    assert!(!text.contains("changes-requested"));
    assert!(!text.contains("waiting-bots"));
}

#[tokio::test]
async fn pr_hygiene_asks_and_objectors_are_plain_points_and_optional_fields_are_tolerant() {
    let mut body = fixture();
    body["pr"]["asks"] = json!([{ "area": "wallet", "approvers": ["alice", "bob"] }, {"area": "<b>core</b>", "approvers": ["carol"]}]);
    body["pr"]["objectors"] = json!(["dave", "<img src=x>"]);
    // The latest review must win even when the service's array is not ordered.
    body["pr"]["reviews"]
        .as_array_mut()
        .expect("reviews")
        .reverse();
    let data = read_fixture(body).await;
    let details = &data.prs.values().next().expect("PR").details;
    assert!(details.contains(&"Review requested for wallet: alice, bob".to_owned()));
    assert!(details.contains(&"Review requested for <b>core</b>: carol".to_owned()));
    assert!(details.contains(&"Objectors: dave, <img src=x>".to_owned()));
    assert!(
        details.contains(&"Latest review by lklimek: Approved at 2026-11-23T13:50:47Z".to_owned())
    );
    let mut body = fixture();
    body["pr"]["reviews"] = json!("bad shape");
    body["pr"]["asks"] = json!([{"approvers": false}]);
    body["pr"]["since"] = json!("bad time");
    assert_eq!(read_fixture(body).await.prs.len(), 1);
}

#[tokio::test]
async fn pr_hygiene_history_is_cached_until_the_record_changes_and_failure_keeps_the_note() {
    let body = fixture();
    let fake = FakePrHygiene::start("2026-11-23T15:05:25Z", vec![body["pr"].clone()]).await;
    fake.set_detail("dashpay/dash-evo-tool", 1117, body);
    let plugin = PrHygiene::new(fake.url.clone()).expect("plugin");
    plugin.poll().await.expect("first poll");
    let mut prs = fake.prs();
    prs["generated_at"] = json!("2026-11-23T15:06:25Z");
    fake.set_prs(prs.clone());
    plugin.poll().await.expect("second poll");
    assert_eq!(
        fake.served()
            .iter()
            .filter(|s| s.path.ends_with("/1117"))
            .count(),
        1
    );
    prs["prs"][0]["ci_failing"] = json!(false);
    fake.set_prs(prs);
    plugin.poll().await.expect("changed poll");
    assert_eq!(
        fake.served()
            .iter()
            .filter(|s| s.path.ends_with("/1117"))
            .count(),
        2
    );
    let fake = FakePrHygiene::start("2026-11-23T15:05:25Z", vec![fixture()["pr"].clone()]).await;
    let plugin = PrHygiene::new(fake.url.clone()).expect("plugin");
    let Polled::Data(data) = plugin.poll().await.expect("history failure is optional") else {
        panic!("data")
    };
    assert!(
        data.prs
            .values()
            .next()
            .expect("PR")
            .details
            .contains(&"Stage history: unavailable".to_owned())
    );
}

#[tokio::test]
async fn pr_hygiene_slow_history_keeps_notes_and_thepastaclaw_within_poll_budget() {
    use crate::dashboard::plugin::thepastaclaw::Thepastaclaw;
    use crate::test_support::thepastaclaw::{FakeThepastaclaw, active, status};

    let records: Vec<_> = (1..=132)
        .map(|number| {
            record(
                "acme/node-client",
                number,
                Some("waiting-author"),
                "Review needed",
            )
        })
        .collect();
    let fake = FakePrHygiene::start(&now().to_string(), records).await;
    for number in 1..=132 {
        fake.set_detail(
            "acme/node-client",
            number,
            json!({
                "generated_at": now(), "pr": {"repo": "acme/node-client", "number": number},
                "stage_changes": [], "stale": false
            }),
        );
    }
    fake.set_detail_delay(Duration::from_millis(600));
    let claw = FakeThepastaclaw::start(status(
        &now().to_string(),
        vec![active("acme/node-client", 1, 42, "phase1", 5)],
        vec![],
    ))
    .await;
    let host = PluginHost::new(
        vec![
            Arc::new(PrHygiene::new(fake.url.clone()).expect("hygiene")),
            Arc::new(Thepastaclaw::new(claw.url.clone()).expect("thepastaclaw")),
        ],
        Timing::default(),
    );

    for pass in 0..2 {
        let started = tokio::time::Instant::now();
        tokio::time::timeout(Duration::from_secs(3), host.poll_once(now()))
            .await
            .expect("optional history must not block aggregate notes or thepastaclaw");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(
            host.statuses(now())
                .iter()
                .all(|s| s.state == PluginState::Current)
        );
        let overlay = host.overlay(now());
        let mut with_history = 0;
        let mut without_history = 0;
        for number in 1..=132 {
            let notes = overlay.notes(&repo("acme", "node-client"), number);
            let note = notes
                .iter()
                .find(|note| note.plugin == "pr-hygiene")
                .expect("core note");
            assert_eq!(
                note.summary.as_deref(),
                Some(if pass == 0 {
                    "Review needed"
                } else {
                    "Updated action"
                })
            );
            with_history += usize::from(note.details.contains(&"Stale: No".to_owned()));
            without_history += usize::from(
                note.details
                    .contains(&"Stage history: unavailable".to_owned()),
            );
        }
        assert!(
            with_history > 0,
            "completed history survives the shared deadline"
        );
        assert!(without_history > 0, "pending history is optional");
        assert_eq!(with_history + without_history, 132);
        assert!(
            overlay
                .notes(&repo("acme", "node-client"), 1)
                .iter()
                .any(|note| note.plugin == "thepastaclaw")
        );
        assert_eq!(claw.served().len(), pass + 1);
        let started_details = fake
            .served()
            .iter()
            .filter(|s| s.path.starts_with("/api/v1/prs/"))
            .count();
        assert!(
            started_details <= 16 * (pass + 1),
            "history concurrency is bounded"
        );

        let mut prs = fake.prs();
        for record in prs["prs"].as_array_mut().expect("records") {
            record["next_action"] = json!("Updated action");
        }
        fake.set_prs(prs);
    }
}
