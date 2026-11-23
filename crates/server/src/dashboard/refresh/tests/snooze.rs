//! Snooze (owner SN1) over the dashboard world: R12/R13 (session and CSRF only, the user's own snapshot items, 1 to
//! 365 days), the snoozed item listed under Excluded with its end, and the wake rules (date, a newer review request
//! with a known time, a newer mention that is not hidden), which last once a new snapshot is stored.

use contracts::api::dashboard::Mention;
use db::repo::dashboard::SnoozeEnd;

use crate::dashboard::snapshot::{self, Snapshot};

use super::*;

const PLAT_1203: &str = "acme~platform~pr~1203";

fn key(text: &str) -> ItemKey {
    ItemKey::parse(text).expect("key")
}

fn uri(key: &str) -> String {
    format!("/api/v1/dashboard/items/{key}/snooze")
}

impl World {
    async fn snooze(&self, key: &str, body: &str, cookie: &str, csrf: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method("PUT")
            .uri(uri(key))
            .header("cookie", cookie)
            .header("x-csrf-token", csrf)
            .header("content-type", "application/json")
            .body(Body::from(body.to_owned()))
            .expect("request");
        let response = api::router(self.state.clone())
            .oneshot(request)
            .await
            .expect("response");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, body["code"].clone())
    }

    async fn stored_snapshot(&self) -> Snapshot {
        let (_, document) = self
            .state
            .db
            .dashboard()
            .latest_snapshot::<Value>(DEFAULT_TENANT, "u1")
            .await
            .expect("read")
            .expect("snapshot");
        snapshot::readable(document).expect("readable")
    }

    async fn snoozed_keys(&self) -> Vec<String> {
        self.state
            .db
            .dashboard()
            .snoozes(DEFAULT_TENANT, "u1")
            .await
            .expect("snoozes")
            .into_iter()
            .map(|s| s.key.as_str().to_owned())
            .collect()
    }

    /// Writes a snooze as if made at `at`.
    async fn snoozed_at(&self, key: &str, at: Timestamp, until: Timestamp) {
        self.state
            .db
            .dashboard()
            .snooze(DEFAULT_TENANT, "u1", &self::key(key), at, until)
            .await
            .expect("snooze");
    }
}

fn excluded(summary: &DashboardSummary) -> Vec<&ExcludedItem> {
    summary
        .snapshot
        .as_ref()
        .expect("snapshot")
        .exclusions
        .as_ref()
        .expect("exclusions")
        .items
        .as_ref()
        .expect("listed")
        .iter()
        .collect()
}

fn ranked_keys(page: &ItemPage) -> Vec<&str> {
    page.items.iter().map(|i| i.key.as_str()).collect()
}

/// R12/R13: a snoozed item leaves the ranking and is listed under Excluded with its end until Wake now; the
/// revision changes each time; the scan invariant (Needs you + Waiting on others + Excluded) holds.
#[tokio::test]
async fn a_snoozed_item_leaves_the_ranking_until_woken() {
    let w = world().await;
    w.refreshed().await;
    let before = w.summary(ALICE).await;
    assert!(ranked_keys(&w.items("?limit=500").await).contains(&PLAT_1203));

    let (status, _) = w.snooze(PLAT_1203, r#"{"days": 3}"#, ALICE, "csrf").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let after = w.summary_with(ALICE, "?excluded=true").await;
    assert_ne!(after.revision, before.revision);
    assert!(!ranked_keys(&w.items("?limit=500").await).contains(&PLAT_1203));
    let snoozed: Vec<_> = excluded(&after)
        .into_iter()
        .filter(|e| e.reason == ExclusionReason::Snoozed)
        .collect();
    assert_eq!(snoozed.len(), 1);
    assert_eq!(
        (snoozed[0].id.as_str(), snoozed[0].merged_into.as_ref()),
        ("PLAT-1203", None)
    );
    let until = snoozed[0].snoozed_until.expect("its end");
    let three_days = now() + SignedDuration::from_hours(72);
    assert!(
        until >= three_days && until < three_days + SignedDuration::from_mins(5),
        "{until}"
    );
    let meta = after.snapshot.as_ref().expect("snapshot");
    assert!(
        meta.exclusions
            .as_ref()
            .expect("exclusions")
            .counts
            .contains(&ExclusionCount {
                reason: ExclusionReason::Snoozed,
                count: 1
            })
    );
    assert_eq!(
        meta.checked.pr,
        meta.item_counts.pr + u32::try_from(meta.awaiting.len()).expect("count") + meta.excluded,
        "scanned = Needs you + Waiting on others + Excluded"
    );

    let wake = w.send("DELETE", &uri(PLAT_1203), ALICE, &[]).await;
    assert_eq!(wake.status, StatusCode::NO_CONTENT, "{}", wake.body);
    let woken = w.summary(ALICE).await;
    assert_ne!(woken.revision, after.revision);
    assert!(ranked_keys(&w.items("?limit=500").await).contains(&PLAT_1203));
    let again = w.send("DELETE", &uri(PLAT_1203), ALICE, &[]).await;
    assert_eq!(again.status, StatusCode::NO_CONTENT, "idempotent");
}

/// R12 refuses what it cannot apply: days outside 1–365, a malformed body or key, an item outside the user's
/// snapshot, another user's item, and a missing CSRF token.
#[tokio::test]
async fn snooze_requests_are_validated() {
    let w = world().await;
    w.refreshed().await;
    let invalid = json!("invalid_request");
    for body in [
        r#"{"days": 0}"#,
        r#"{"days": 366}"#,
        r#"{"days": -1}"#,
        "{}",
        "x",
    ] {
        assert_eq!(
            w.snooze(PLAT_1203, body, ALICE, "csrf").await,
            (StatusCode::BAD_REQUEST, invalid.clone()),
            "{body}"
        );
    }
    assert_eq!(
        w.snooze("not-a-key", r#"{"days": 1}"#, ALICE, "csrf").await,
        (StatusCode::BAD_REQUEST, invalid.clone())
    );
    assert_eq!(
        w.snooze("acme~platform~pr~9999", r#"{"days": 1}"#, ALICE, "csrf")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        w.snooze(PLAT_1203, r#"{"days": 1}"#, COLLEAGUE, "csrf")
            .await
            .0,
        StatusCode::NOT_FOUND,
        "the colleague has no snapshot holding it"
    );
    assert_eq!(
        w.snooze(PLAT_1203, r#"{"days": 1}"#, ALICE, "wrong")
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        w.send(
            "DELETE",
            &uri(PLAT_1203),
            ALICE,
            &[("x-csrf-token", "wrong")]
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        w.snooze(PLAT_1203, r#"{"days": 365}"#, ALICE, "csrf")
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert!(w.snoozed_keys().await.contains(&PLAT_1203.to_owned()));
}

/// A snooze ends at its date, or earlier when the snapshot shows a review request to the user with a known time or
/// a mention of the user newer than the snooze (a hidden mention or an approximate request time does not count);
/// the next stored snapshot marks the ended ones ended (kept, with the reason), so the wake lasts.
#[tokio::test]
async fn snoozes_wake_at_their_date_or_on_a_newer_request_or_mention_and_stay_woken() {
    let w = world().await;
    w.refreshed().await;
    let stored = w.stored_snapshot().await;
    let first = w.items("?limit=500").await;
    let listed = ranked_keys(&first);
    let requested = stored
        .pull_requests
        .iter()
        .find(|pr| {
            pr.facts.review_pending_for_us
                && !pr.facts.review_request_age_is_approx
                && pr.facts.review_requested_at.is_some()
        })
        .expect("a review requested from alice at a known time");
    let request_key = ItemKey::new(&ItemTarget::PullRequest {
        repo: requested.facts.repo.clone(),
        number: requested.facts.number,
    })
    .expect("key");
    assert!(listed.contains(&request_key.as_str()));
    let request_at = requested.facts.review_requested_at.expect("time");
    let mentioned: &Mention = stored
        .mentions
        .iter()
        .find(|m| listed.contains(&m.item_key.as_str()) && m.item_key != request_key)
        .expect("a mention of alice on a ranked item");
    let far = now() + SignedDuration::from_hours(24 * 30);
    let hour = SignedDuration::from_hours(1);
    let quiet = stored
        .issues
        .iter()
        .map(|issue| {
            ItemKey::new(&ItemTarget::Issue {
                repo: issue.facts.repo.clone(),
                number: issue.facts.number,
            })
            .expect("key")
        })
        .find(|k| listed.contains(&k.as_str()) && !stored.mentions.iter().any(|m| &m.item_key == k))
        .expect("a ranked issue nobody mentions alice on");
    let todo = ItemKey::new(&ItemTarget::Todo(stored.todos[0].facts.id)).expect("key");
    assert!(listed.contains(&todo.as_str()));
    let snoozed_now = |summary: &DashboardSummary| -> Vec<String> {
        let mut keys: Vec<String> = excluded(summary)
            .into_iter()
            .filter(|e| e.reason == ExclusionReason::Snoozed)
            .map(|e| e.key.as_str().to_owned())
            .collect();
        keys.sort_unstable();
        keys
    };

    // Woken by the request and by the mention, ended by its date; kept: the quiet issue.
    w.snoozed_at(request_key.as_str(), request_at - hour, far)
        .await;
    w.snoozed_at(
        mentioned.item_key.as_str(),
        mentioned.created_at - hour,
        far,
    )
    .await;
    w.snoozed_at(todo.as_str(), now() - hour * 48, now() - hour)
        .await;
    w.snoozed_at(quiet.as_str(), now(), far).await;
    let summary = w.summary_with(ALICE, "?excluded=true").await;
    assert_eq!(snoozed_now(&summary), [quiet.as_str()]);
    let mut reasons: Vec<(String, Option<SnoozeEnd>)> = w
        .state
        .db
        .dashboard()
        .snoozes(DEFAULT_TENANT, "u1")
        .await
        .expect("snoozes")
        .iter()
        .map(|s| {
            (
                s.key.as_str().to_owned(),
                crate::dashboard::snooze::ended(s, &stored, &[], w.state.dashboard.now()),
            )
        })
        .collect();
    reasons.sort();
    // The quiet issue's snooze write already marked the past TODO snooze ended at its date.
    let mut expected = vec![
        (
            request_key.as_str().to_owned(),
            Some(SnoozeEnd::ReviewRequest),
        ),
        (
            mentioned.item_key.as_str().to_owned(),
            Some(SnoozeEnd::Mention),
        ),
        (quiet.as_str().to_owned(), None),
    ];
    expected.sort();
    assert_eq!(reasons, expected, "each end has its reason");
    let ranked = w.items("?limit=500").await;
    for woken in [&request_key, &mentioned.item_key, &todo] {
        assert!(ranked_keys(&ranked).contains(&woken.as_str()), "{woken:?}");
    }

    // A hidden mention does not wake it.
    let hide = w
        .send(
            "POST",
            &format!("/api/v1/dashboard/mentions/{}/hide", mentioned.id),
            ALICE,
            &[],
        )
        .await;
    assert_eq!(hide.status, StatusCode::NO_CONTENT, "{}", hide.body);
    let mut kept = vec![
        quiet.as_str().to_owned(),
        mentioned.item_key.as_str().to_owned(),
    ];
    kept.sort_unstable();
    let summary = w.summary_with(ALICE, "?excluded=true").await;
    assert_eq!(
        snoozed_now(&summary),
        kept,
        "the hidden mention does not count"
    );

    // An approximate request time (the PR's last update) does not wake it.
    let (record, document) = w
        .state
        .db
        .dashboard()
        .latest_snapshot::<Value>(DEFAULT_TENANT, "u1")
        .await
        .expect("read")
        .expect("snapshot");
    let mut tampered = snapshot::readable(document).expect("readable");
    let pr = tampered
        .pull_requests
        .iter_mut()
        .find(|pr| {
            pr.facts.number == requested.facts.number && pr.facts.repo == requested.facts.repo
        })
        .expect("pr");
    pr.facts.review_request_age_is_approx = true;
    w.state
        .db
        .dashboard()
        .replace_current_snapshot(DEFAULT_TENANT, "u1", record.id, &tampered)
        .await
        .expect("write");
    assert!(
        !ranked_keys(&w.items("?limit=500").await).contains(&request_key.as_str()),
        "an approximate request time does not wake it"
    );

    // A stored snapshot marks the ended snoozes ended (the request's again known time wakes it for good).
    w.refreshed().await;
    let mut left = w.snoozed_keys().await;
    left.sort_unstable();
    assert_eq!(
        left, kept,
        "the stored snapshot ended the woken and past snoozes"
    );
}
