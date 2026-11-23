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
