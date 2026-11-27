//! The review loop through its routes, over the recorded GitHub answers of `fixtures/review-loop/github` served by
//! wiremock; no real network, no analysis unless a test adds one.

use axum::Router;
use axum::body::Body;
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::{Request, StatusCode};
use contracts::api::access_tokens::TokenScope;
use contracts::api::review_loop::{
    CommentAssessment, DecisionKind, Gate, Necessity, NecessityVerdict, Recommendation, Validity,
};
use contracts::ids::JobId;
use contracts::time::Timestamp;
use db::repo::DEFAULT_TENANT;
use db::repo::review_loop::ReadResult;
use db::repo::users::GithubProfile;
use http_body_util::BodyExt;
use jiff::SignedDuration;
use secrecy::SecretString;
use serde_json::{Value, json};
use tower::ServiceExt;
use url::Url;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::api;
use crate::auth::{lock_user, store_sign_in_tokens};
use crate::github::oauth::TokenGrant;
use crate::github::review_loop::tests::recorded;
use crate::state::AppState;
use crate::test_support::session_cookie;

mod assessment;
mod freshness;
mod marks;
mod pr_history;
mod propagation;
mod watching;

const PR: &str = "/api/v1/prs/acme/node-client/482";
const L2: &str = "ef3c2e21454eefc7";
const L6: &str = "fedcba9876543210";

/// Edits of recorded answers: (fixture file, edit).
type Changes<'a> = &'a [(&'a str, &'a dyn Fn(&mut Value))];

struct World {
    state: AppState,
    app: Router,
    server: MockServer,
    cookie: String,
}

/// The fixture answers with `changes` applied (file, edit of the recorded answer).
async fn serve(server: &MockServer, changes: Changes<'_>) {
    server.reset().await;
    for (marker, file) in [
        ("reviewDecision", "pr.json"),
        ("reviewThreads(first:100", "review-threads.json"),
        ("reviews(first:100", "reviews.json"),
        ("comments(first:100", "comments.json"),
    ] {
        let mut answer = recorded(file);
        for (changed, change) in changes {
            if *changed == file {
                change(&mut answer);
            }
        }
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .and(body_string_contains(marker))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .mount(server)
            .await;
    }
}

async fn world() -> World {
    world_with(|_| {}).await
}

/// [`world`] with `adjust` applied to the state before the router is built.
async fn world_with(adjust: impl FnOnce(&mut AppState)) -> World {
    let server = MockServer::start().await;
    serve(&server, &[]).await;
    let mut config = crate::config::tests::test_config();
    config.github.api_url = Url::parse(&format!("{}/api", server.uri())).expect("url");
    let dir = std::env::temp_dir().join(format!("diffpilot-review-loop-{}", uuid::Uuid::new_v4()));
    config.github.clone_url = Url::from_directory_path(&dir).expect("fixture URL");
    config.data_dir = dir;
    let mut state = AppState::with_queue(config, db::repo::Db::in_memory().await.expect("db"));
    adjust(&mut state);
    state
        .db
        .users()
        .upsert_member(
            DEFAULT_TENANT,
            "u1",
            &GithubProfile {
                github_id: 1,
                login: "lklimek".into(),
                name: None,
                avatar_url: None,
            },
            Timestamp::now(),
        )
        .await
        .expect("user");
    let lock = lock_user("u1").await;
    let grant = TokenGrant {
        access_token: SecretString::from("ghu_review_loop_u1"),
        expires_in: 8 * 3600,
        refresh_token: SecretString::from("ghr_review_loop_u1"),
        refresh_token_expires_in: 180 * 86_400,
    };
    store_sign_in_tokens(&state, &lock, DEFAULT_TENANT, &grant, Timestamp::now())
        .await
        .expect("tokens");
    drop(lock);
    let cookie = session_cookie(&state, DEFAULT_TENANT).await;
    World {
        app: api::router(state.clone()),
        state,
        server,
        cookie,
    }
}

impl World {
    async fn link_llm(&self) {
        use contracts::gateway::{CredentialMode, CredentialState, CredentialStatus};
        self.state
            .db
            .llm_credentials()
            .put_status(
                DEFAULT_TENANT,
                "u1",
                &CredentialStatus {
                    telemetry: Default::default(),
                    challenge: None,
                    mode: CredentialMode::ChatgptPlan,
                    state: CredentialState::Linked,
                    relink_reason: None,
                    label: None,
                    plan: None,
                    paused_until: None,
                    last_call: None,
                },
            )
            .await
            .unwrap();
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(COOKIE, &self.cookie)
            .header("x-csrf-token", "csrf")
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .expect("request");
        let response = self.app.clone().oneshot(request).await.expect("infallible");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn get(&self, uri: &str) -> Value {
        let (status, body) = self.call("GET", uri, None).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        body
    }

    async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.call("POST", uri, Some(body)).await
    }

    async fn items(&self) -> Vec<Value> {
        self.get(&format!("{PR}/comments/items"))
            .await
            .as_array()
            .cloned()
            .expect("items")
    }

    async fn item(&self, key: &str) -> Value {
        self.items()
            .await
            .into_iter()
            .find(|item| item["stable_key"] == key)
            .unwrap_or_else(|| panic!("no item {key}"))
    }

    async fn pr_id(&self) -> i64 {
        self.state
            .db
            .pull_requests()
            .find(
                DEFAULT_TENANT,
                &crate::github::review_loop::tests::pr482().repo,
                482,
            )
            .await
            .expect("find")
            .expect("stored")
    }

    /// Makes the last read old, so the next page read reads GitHub again.
    async fn age_read(&self) {
        let id = self.pr_id().await;
        let rl = self.state.db.review_loop();
        let row = rl.pr_state(id).await.expect("state").expect("row");
        rl.record_read(
            id,
            &ReadResult::Read {
                snapshot: row.github_snapshot.expect("snapshot"),
                round_count: row.round_count,
            },
            Timestamp::now() - SignedDuration::from_mins(5),
        )
        .await
        .expect("aged");
    }

    /// Reads GitHub again with `changes` to the fixture.
    async fn reread(&self, changes: Changes<'_>) {
        serve(&self.server, changes).await;
        self.age_read().await;
        self.get(&format!("{PR}/comments")).await;
    }

    async fn decide(&self, no: u64, body: Value) -> (StatusCode, Value) {
        self.post(&format!("{PR}/comments/items/C{no}/decisions"), body)
            .await
    }

    async fn decide_ok(&self, key: &str, body: Value) -> Value {
        let item = self.item(key).await;
        let mut body = body;
        body["item_revision"] = item["revision"].clone();
        let (status, decision) = self.decide(item["no"].as_u64().expect("no"), body).await;
        assert_eq!(status, StatusCode::CREATED, "{decision}");
        decision
    }

    async fn assess(&self, key: &str, recommendation: Recommendation, gates: Vec<Gate>) {
        let item = self.item(key).await;
        let assessment = CommentAssessment {
            proposed_decision: None,
            evidence_text: None,
            recorded_decision: None,
            job: JobId::new_random(),
            head: contracts::ids::CommitSha(crate::test_support::github::HEAD.into()),
            assessed_at: Timestamp::now(),
            reused_since: None,
            validity: Validity::Valid,
            evidence: vec![],
            pre_existing: Some(false),
            category: "correctness".into(),
            necessity: Necessity {
                verdict: NecessityVerdict::In,
                reason: "in this change".into(),
                better_pr: None,
            },
            gates,
            elsewhere: vec![],
            fix_sketch: None,
            fix_size: None,
            reply_drafts: vec![],
            recommendation,
            code: None,
            prompt_version: 1,
            skills_commit: None,
        };
        self.state
            .db
            .review_loop()
            .add_assessment(
                DEFAULT_TENANT,
                serde_json::from_value(item["id"].clone()).expect("id"),
                &assessment,
                "fp",
            )
            .await
            .expect("assessment");
    }
}

fn decision(kind: &str) -> Value {
    json!({"kind": kind, "reason": null, "target": null, "override": null, "resolve_ticked": true,
        "reply_text": null, "comment": null, "item_revision": 0})
}

fn reject(reason: &str) -> Value {
    let mut body = decision("reject");
    body["reason"] = json!(reason);
    body
}

fn recommend(kind: DecisionKind, class: &str) -> Recommendation {
    Recommendation {
        kind: kind.review_kind().expect("recommendable decision"),
        reason: "The lines are older than this change.".into(),
        target: None,
        reason_class: class.into(),
    }
}

fn thread_node<'a>(answer: &'a mut Value, id: &str) -> &'a mut Value {
    answer["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"]
        .as_array_mut()
        .expect("threads")
        .iter_mut()
        .find(|thread| thread["id"] == id)
        .expect("thread")
}

#[tokio::test]
async fn comments_work_without_analysis() {
    let w = world().await;
    let overview = w.get(&format!("{PR}/comments")).await;
    assert_eq!(overview["analysis"], Value::Null);
    assert_eq!(
        overview["title"],
        "feat(rpc): retry with backoff on timeout"
    );
    assert_eq!(overview["read"]["threads"], 8);
    assert_eq!(overview["read"]["complete"], true);
    assert_eq!(overview["rounds"].as_array().map(Vec::len), Some(3));
    assert!(!overview["queue"].as_array().expect("queue").is_empty());
    let items = w.items().await;
    assert_eq!(
        items.len(),
        15,
        "8 inline threads, legacy body findings, newly eligible prose sources and the merge question"
    );
    for item in items
        .iter()
        .filter(|item| item["kind"] != "pr_question" && item["status_comment"] != true)
    {
        assert_eq!(
            item["assessment_state"],
            json!({"state": "not_assessed", "reason": "not_run"}),
            "{item}"
        );
        assert_eq!(item["assessment"], Value::Null);
    }
    let numbers: Vec<u64> = items.iter().filter_map(|i| i["no"].as_u64()).collect();
    assert_eq!(
        numbers,
        (1..=items.len() as u64).collect::<Vec<_>>(),
        "C<no> numbered at first sight"
    );
    let page = w.get(PR).await;
    assert_eq!(page["analysis"], Value::Null);
    assert_eq!(page["comments"]["rounds"], 3);
    assert_eq!(page["head"], crate::test_support::github::HEAD);
    assert_eq!(page["draft"], false);
}

#[tokio::test]
async fn outside_resolution_not_recorded() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let l1 = w.item("821d7cdc93fec6e1").await;
    assert_eq!(l1["state"], "outside");
    assert_eq!(l1["outside"]["by"], "thepastaclaw");
    assert_eq!(l1["decisions"], json!([]));
    assert_eq!(
        w.item("0a1b2c3d4e5f6a7b").await["outside"]["by"],
        "carol-agent"
    );
    let overview = w.get(&format!("{PR}/comments")).await;
    let queued: Vec<u64> = overview["queue"]
        .as_array()
        .expect("queue")
        .iter()
        .flat_map(|entry| entry["item_nos"].as_array().cloned().unwrap_or_default())
        .filter_map(|no| no.as_u64())
        .collect();
    assert!(!queued.contains(&l1["no"].as_u64().expect("no")));
    assert_eq!(overview["counts"]["outside"], 2);
    let log = w.get(&format!("{PR}/comments/log")).await;
    assert_eq!(log["decisions"], json!([]), "nothing is recorded");
}

#[tokio::test]
async fn round_three_asks_merge_question_once() {
    let w = world().await;
    let overview = w.get(&format!("{PR}/comments")).await;
    assert_eq!(
        overview["queue"][0],
        json!({"item_nos": [w.item("pr:3:merge").await["no"]], "reason": "pr_question"})
    );
    let merge = w.item("pr:3:merge").await;
    assert_eq!(merge["author_kind"], "diffpilot");
    assert_eq!(merge["title"], "Merge now, or another round?");
    w.reread(&[]).await;
    let questions = w
        .items()
        .await
        .into_iter()
        .filter(|item| item["kind"] == "pr_question")
        .count();
    assert_eq!(questions, 1, "asked once per round");

    let fourth: &dyn Fn(&mut Value) = &|answer| {
        let nodes = answer["data"]["repository"]["pullRequest"]["reviews"]["nodes"]
            .as_array_mut()
            .expect("reviews");
        let mut round = nodes
            .iter()
            .find(|review| review["id"] == "PRR_fixture482_r5")
            .cloned()
            .expect("r5");
        round["id"] = json!("PRR_fixture482_r6");
        round["submittedAt"] = json!("2026-11-12T12:00:00Z");
        nodes.push(round);
    };
    w.reread(&[("reviews.json", fourth)]).await;
    let overview = w.get(&format!("{PR}/comments")).await;
    let latest = w.item("pr:4:merge").await;
    assert_eq!(overview["queue"][0]["item_nos"], json!([latest["no"]]));
    assert!(
        overview["queue"]
            .as_array()
            .expect("queue")
            .iter()
            .all(|entry| entry["item_nos"] != json!([merge["no"]])),
        "only the latest merge question waits"
    );
}

#[tokio::test]
async fn stale_review_dismissal_question_after_round() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let rows: Vec<Value> = w
        .items()
        .await
        .into_iter()
        .filter(|item| item["author"] == "thepastaclaw" && item["state"] != "outside")
        .map(|item| {
            let mut row = reject("Per the maintainers, out of scope.");
            row["item_no"] = item["no"].clone();
            row["item_revision"] = item["revision"].clone();
            row
        })
        .collect();
    assert_eq!(rows.len(), 6, "gate status is not an actionable source");
    let (status, body) = w
        .post(
            &format!("{PR}/comments/decision-batches"),
            json!({"via": "table", "rows": rows}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(
        w.items()
            .await
            .iter()
            .all(|item| item["stable_key"] != "pr:3:dismiss"),
        "asked at the end of a round only"
    );
    w.reread(&[]).await;
    let dismiss = w.item("pr:3:dismiss").await;
    assert_eq!(dismiss["title"], "Dismiss stale bot reviews?");
    assert!(
        dismiss["body"]
            .as_str()
            .expect("body")
            .contains("thepastaclaw: https://github.com/acme/node-client/pull/482#pullrequestreview-4000000107"),
        "{dismiss}"
    );
    w.reread(&[]).await;
    let count = w
        .items()
        .await
        .iter()
        .filter(|item| item["stable_key"] == "pr:3:dismiss")
        .count();
    assert_eq!(count, 1, "asked once");
    let decision = w
        .decide_ok("pr:3:dismiss", decision("dismiss_stale_reviews"))
        .await;
    assert_eq!(decision["effects"][0]["kind"], "dismiss_review");
    assert_eq!(
        decision["effects"][0]["params"]["review_id"],
        "PRR_fixture482_r5"
    );
}

#[tokio::test]
async fn repeat_by_dedupe_marker_is_carried_and_logged() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let first = w
        .decide_ok(L2, {
            let mut body = decision("fix_here");
            body["comment"] = json!("keep the test small");
            body
        })
        .await;
    let new_thread: &dyn Fn(&mut Value) = &|answer| {
        let thread = thread_node(answer, "PRRT_fixture482_l2");
        thread["id"] = json!("PRRT_fixture482_l2b");
        thread["comments"]["nodes"][0]["url"] =
            json!("https://github.com/acme/node-client/pull/482#discussion_r3000000999");
    };
    w.reread(&[("review-threads.json", new_thread)]).await;
    let item = w.item(L2).await;
    assert_eq!(item["state"], "repeat");
    let decisions = item["decisions"].as_array().expect("decisions");
    assert_eq!(decisions.len(), 2);
    assert_eq!(decisions[1]["via"], "carried");
    assert_eq!(decisions[1]["carried_from"], first["id"]);
    assert_eq!(decisions[1]["kind"], "fix_here");
    assert_eq!(decisions[1]["comment"], "keep the test small");
    assert_eq!(decisions[1]["decided_by"], "lklimek");
    assert_eq!(
        decisions[1]["effects"][1]["params"]["thread_id"], "PRRT_fixture482_l2b",
        "the carried decision acts on the new thread"
    );
    let log = w.get(&format!("{PR}/comments/log")).await;
    let events = log["carry_events"].as_array().expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"], "carried");
    assert_eq!(events[0]["source_decision"], first["id"]);
    assert_eq!(events[0]["item_no"], item["no"]);
    let all = w.get("/api/v1/comments/carry-overs").await;
    assert_eq!(all["totals"]["carried"], 1);
    assert_eq!(all["per_pr"][0]["pr"]["number"], 482);
    let one = w
        .get("/api/v1/comments/carry-overs?pr=acme/node-client/482")
        .await;
    assert_eq!(one["events"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn bot_edit_or_code_change_reasks() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    w.decide_ok(L2, decision("fix_here")).await;
    w.decide_ok(L6, reject("Covered by the integration test."))
        .await;
    let edited: &dyn Fn(&mut Value) = &|answer| {
        let thread = thread_node(answer, "PRRT_fixture482_l2");
        thread["id"] = json!("PRRT_fixture482_l2b");
        let comment = &mut thread["comments"]["nodes"][0];
        comment["url"] =
            json!("https://github.com/acme/node-client/pull/482#discussion_r3000000998");
        let body = comment["body"]
            .as_str()
            .expect("body")
            .replace("doubles without a ceiling", "doubles without any ceiling");
        comment["body"] = json!(body);
    };
    w.reread(&[("review-threads.json", edited)]).await;
    let item = w.item(L2).await;
    assert_eq!(item["state"], "reopened");
    assert_eq!(item["github"]["edited_after_decision"], true);
    let log = w.get(&format!("{PR}/comments/log")).await;
    let events = log["carry_events"].as_array().expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(
        (events[0]["kind"].clone(), events[0]["reason"].clone()),
        (json!("asked_again"), json!("bot_edited"))
    );
    assert_ne!(
        events[0]["evidence"]["body_hash_before"],
        events[0]["evidence"]["body_hash_after"]
    );
    let overview = w.get(&format!("{PR}/comments")).await;
    assert!(
        overview["queue"]
            .as_array()
            .expect("queue")
            .iter()
            .any(|entry| entry["reason"] == "asked_again"
                && entry["item_nos"] == json!([item["no"]]))
    );

    // The head moves and the bot raises C(l6) again unchanged: no diff can be made here, so the code at the
    // decided lines counts as changed (S13-4, conservative).
    let moved = "9999999999999999999999999999999999999999";
    let head: &dyn Fn(&mut Value) = &|answer| {
        answer["data"]["repository"]["pullRequest"]["headRefOid"] = json!(moved);
    };
    let raised: &dyn Fn(&mut Value) = &|answer| {
        let thread = thread_node(answer, "PRRT_fixture482_l6");
        thread["isResolved"] = json!(false);
        thread["comments"]["nodes"][0]["url"] =
            json!("https://github.com/acme/node-client/pull/482#discussion_r3000000997");
        thread["id"] = json!("PRRT_fixture482_l6b");
    };
    w.reread(&[("pr.json", head), ("review-threads.json", raised)])
        .await;
    let l6 = w.item(L6).await;
    assert_eq!(l6["state"], "reopened");
    let log = w.get(&format!("{PR}/comments/log")).await;
    let last = log["carry_events"]
        .as_array()
        .and_then(|events| events.last().cloned())
        .expect("event");
    assert_eq!(last["reason"], "code_changed");
    assert_eq!(last["evidence"]["commit"], moved);
}

#[tokio::test]
async fn resolve_ticked_by_default_and_submitted_state_wins() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    w.assess(L2, recommend(DecisionKind::FixHere, "real bug"), vec![])
        .await;
    let rest = w.get(&format!("{PR}/comments/decide-rest")).await;
    let no = w.item(L2).await["no"].clone();
    let row = rest["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["row"]["item_no"] == no)
        .cloned()
        .expect("row");
    assert_eq!(row["row"]["resolve_ticked"], true, "ticked by default");
    assert_eq!(row["effects"][2]["kind"], "resolve");

    let mut unticked = decision("fix_here");
    unticked["resolve_ticked"] = json!(false);
    let stored = w.decide_ok(L2, unticked).await;
    assert_eq!(stored["resolve_ticked"], false);
    let kinds: Vec<&Value> = stored["effects"]
        .as_array()
        .expect("effects")
        .iter()
        .map(|effect| &effect["kind"])
        .collect();
    assert_eq!(
        kinds,
        [&json!("fix"), &json!("reply")],
        "nothing resolved without the tick"
    );
    assert_eq!(
        stored["recommendation"]["kind"], "fix_here",
        "the recommendation shown is stored"
    );
    let again = w.decide_ok(L2, decision("fix_here")).await;
    assert_eq!(again["resolve_ticked"], true);
    assert_eq!(again["supersedes"], stored["id"]);
    assert_eq!(again["effects"][2]["kind"], "resolve");
}

#[tokio::test]
async fn owner_comment_in_feed_never_posted() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let mut body = decision("fix_here");
    body["comment"] = json!("keep the test small");
    let stored = w.decide_ok(L2, body).await;
    let feed = w
        .state
        .db
        .review_loop()
        .feed_decision(serde_json::from_value(stored["id"].clone()).expect("id"))
        .await
        .expect("feed")
        .expect("decision");
    assert_eq!(
        feed.decision.comment.as_deref(),
        Some("keep the test small")
    );
    let effects = serde_json::to_string(&feed.decision.effects).expect("json");
    assert!(!effects.contains("keep the test small"), "{effects}");
    assert!(
        !feed
            .decision
            .reply_text
            .unwrap_or_default()
            .contains("keep the test small")
    );
}

#[tokio::test]
async fn follow_up_pr_needs_no_link() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let stored = w.decide_ok(L2, decision("follow_up_pr")).await;
    assert_eq!(stored["effects"][0]["kind"], "follow_up_pr");
    assert_eq!(stored["effects"][0]["params"]["url"], Value::Null);
}

#[tokio::test]
async fn accept_despite_gate_requires_reason_and_resolves() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    w.assess(
        L2,
        recommend(DecisionKind::FixHere, "real bug"),
        vec![Gate::Funds],
    )
    .await;
    let mut d8 = decision("reject");
    d8["override"] = json!({"type": "blocker_gate", "gate": "G-FUNDS"});
    let item = w.item(L2).await;
    d8["item_revision"] = item["revision"].clone();
    let no = item["no"].as_u64().expect("no");
    let (status, body) = w.decide(no, d8.clone()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_request");
    let mut unticked = d8.clone();
    unticked["reason"] = json!("Bounded by the caller.");
    unticked["resolve_ticked"] = json!(false);
    let (status, _) = w.decide(no, unticked).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "always resolves");
    d8["reason"] = json!("Bounded by the caller.");
    let (status, stored) = w.decide(no, d8).await;
    assert_eq!(status, StatusCode::CREATED, "{stored}");
    assert_eq!(
        stored["override"],
        json!({"type": "blocker_gate", "gate": "G-FUNDS"})
    );
    assert_eq!(stored["effects"][1]["kind"], "resolve");
    assert_eq!(
        stored["reply_text"],
        "Per maintainer decision, accepted despite G-FUNDS: Bounded by the caller."
    );
}

#[tokio::test]
async fn table_submit_blocked_until_gate_rows_chosen() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let gated = w.item(L6).await;
    let (status, marked) = w
        .post(
            &format!("{PR}/comments/items/C{}/gates", gated["no"]),
            json!({"gate": "G-DATA", "marked": true, "item_revision": gated["revision"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{marked}");
    assert_eq!(
        marked["gates"],
        json!([{"gate": "G-DATA", "source": "owner"}])
    );
    let other = w.item(L2).await;
    let mut row = decision("fix_here");
    row["item_no"] = other["no"].clone();
    row["item_revision"] = other["revision"].clone();
    let (status, body) = w
        .post(
            &format!("{PR}/comments/decision-batches"),
            json!({"via": "table", "rows": [row.clone()]}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .expect("message")
            .contains(&format!("C{}", gated["no"])),
        "{body}"
    );
    let mut gate_row = reject("Not in this change.");
    gate_row["item_no"] = marked["no"].clone();
    gate_row["item_revision"] = marked["revision"].clone();
    for via in ["group", "bulk"] {
        let (status, body) = w
            .post(
                &format!("{PR}/comments/decision-batches"),
                json!({"via": via, "rows": [row.clone(), gate_row.clone()]}),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{via}: {body}");
    }
    let (status, body) = w
        .post(
            &format!("{PR}/comments/decision-batches"),
            json!({"via": "table", "rows": [row, gate_row]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body.as_array().map(Vec::len), Some(2));
    assert!(
        body.as_array()
            .expect("decisions")
            .iter()
            .all(|d| d["via"] == "table")
    );
}

/// T22: a gated thread is resolved only by the owner's explicit choice; bulk never decides it.
#[tokio::test]
async fn gate_resolve_needs_explicit_choice() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    w.assess(
        L2,
        recommend(DecisionKind::Resolve, "fixed"),
        vec![Gate::Secret],
    )
    .await;
    let rest = w.get(&format!("{PR}/comments/decide-rest")).await;
    let no = w.item(L2).await["no"].clone();
    assert!(
        rest["left_out"]
            .as_array()
            .expect("left out")
            .contains(&json!({"item_no": no, "reason": "gate"}))
    );
    assert!(
        rest["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .all(|row| row["row"]["item_no"] != no)
    );
    let mut unticked = reject("Not reachable.");
    unticked["resolve_ticked"] = json!(false);
    let stored = w.decide_ok(L2, unticked).await;
    assert!(
        stored["effects"]
            .as_array()
            .expect("effects")
            .iter()
            .all(|effect| effect["kind"] != "resolve"),
        "{stored}"
    );
}

#[tokio::test]
async fn a_stale_revision_answers_item_changed() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let item = w.item(L2).await;
    let mut body = decision("fix_here");
    body["item_revision"] = json!(item["revision"].as_u64().expect("revision") + 7);
    let (status, error) = w.decide(item["no"].as_u64().expect("no"), body).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["code"], "item_changed");
    assert!(
        error["message"]
            .as_str()
            .expect("message")
            .contains(&format!("C{}", item["no"]))
    );
}

#[tokio::test]
async fn undo_reopen_and_skip() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    w.decide_ok(L2, decision("fix_here")).await;
    let item = w.item(L2).await;
    let no = item["no"].clone();
    let (status, undone) = w
        .post(
            &format!("{PR}/comments/items/C{no}/undo"),
            json!({"item_revision": item["revision"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{undone}");
    assert_eq!(undone["state"], "undecided");
    assert!(undone["decisions"][0]["undone_at"].is_string());
    let (status, body) = w
        .post(
            &format!("{PR}/comments/items/C{no}/undo"),
            json!({"item_revision": undone["revision"]}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "nothing left to undo: {body}");

    w.decide_ok(L2, decision("fix_here")).await;
    let item = w.item(L2).await;
    let (status, reopened) = w
        .post(
            &format!("{PR}/comments/items/C{no}/reopen"),
            json!({"item_revision": item["revision"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reopened}");
    assert_eq!(reopened["state"], "reopened");

    w.assess(L6, recommend(DecisionKind::FixHere, "correctness"), vec![])
        .await;
    let overview_url = format!("{PR}/comments");
    let before = w.get(&overview_url).await;
    let skipped = w.item(L6).await;
    let rest_url = format!("{PR}/comments/decide-rest");
    let in_rest = |value: &Value| {
        value["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["row"]["item_no"] == skipped["no"])
    };
    assert!(in_rest(&w.get(&rest_url).await));
    let (status, _) = w
        .post(
            &format!("{PR}/comments/items/C{}/skip", skipped["no"]),
            json!({"item_revision": skipped["revision"]}),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let after = w.get(&overview_url).await;
    assert_eq!(
        after["counts"]["needs_you"].as_u64().unwrap() + 1,
        before["counts"]["needs_you"].as_u64().unwrap()
    );
    assert_eq!(
        w.get(PR).await["comments"]["counts"]["needs_you"],
        after["counts"]["needs_you"]
    );
    assert!(!in_rest(&w.get(&rest_url).await));
    assert_eq!(skipped["decisions"], w.item(L6).await["decisions"]);
    let queue = &after["queue"];
    let last = queue
        .as_array()
        .and_then(|q| q.last())
        .cloned()
        .expect("entry");
    assert_eq!(
        last,
        json!({"item_nos": [skipped["no"]], "reason": "single", "skipped": true})
    );
}

#[tokio::test]
async fn effects_preview_and_github_failure_keeps_the_last_read() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let item = w.item(L2).await;
    let mut row = decision("todo");
    row["item_no"] = item["no"].clone();
    row["target"] = json!({"type": "todo", "title": "Cap the delay"});
    let (status, preview) = w
        .post(
            &format!("{PR}/comments/effects-preview"),
            json!({"rows": [row]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview[0]["effects"][0]["kind"], "todo");
    assert_eq!(
        w.get(&format!("{PR}/comments/log")).await["decisions"],
        json!([]),
        "not stored"
    );

    let (status, body) = w.post(&format!("{PR}/comments/read"), json!({})).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["code"], "rate_limited");
    w.server.reset().await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&w.server)
        .await;
    w.age_read().await;
    let (status, _) = w.post(&format!("{PR}/comments/read"), json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let overview = w.get(&format!("{PR}/comments")).await;
    assert_eq!(overview["read"]["failure"], "unavailable");
    assert!(
        overview["read"]["read_at"].is_string(),
        "the last good read stays"
    );
    assert_eq!(w.items().await.len(), 15);
}

#[tokio::test]
async fn read_again_queues_no_llm_work() {
    let w = world().await;
    w.link_llm().await;
    let (status, _) = w.post(&format!("{PR}/comments/read"), json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(w.items().await.len(), 15, "the refresh stored new items");
    let review_loop = w.state.db.review_loop();
    let passes = review_loop.live_passes().await.expect("passes");
    assert!(passes.is_empty(), "refresh queued {passes:?}");
}

#[tokio::test]
async fn github_thread_reads_use_exact_pr_and_item_identity_without_refreshing_comments() {
    use wiremock::matchers::{body_partial_json, header};

    let w = world().await;
    let mapped = w.item("821d7cdc93fec6e1").await;
    let pr = w.pr_id().await;
    w.age_read().await;
    let rl = w.state.db.review_loop();
    let previous_read = rl.pr_state(pr).await.unwrap().unwrap().read_at;
    let previous_items = rl.items(pr).await.unwrap().len();
    for (id, number, after, next, comments) in [
        (
            "PRRT_fixture482_l1",
            482,
            None,
            None,
            vec![("root", "SUBMITTED")],
        ),
        (
            "PRRT_unmapped",
            482,
            None,
            Some("reply-page"),
            vec![("same title", "SUBMITTED")],
        ),
        (
            "PRRT_unmapped",
            482,
            Some("reply-page"),
            None,
            vec![
                ("original reply", "SUBMITTED"),
                ("pending draft", "PENDING"),
            ],
        ),
        (
            "PRRT_foreign",
            483,
            None,
            None,
            vec![("other PR secret", "SUBMITTED")],
        ),
    ] {
        let nodes: Vec<_> = comments.iter().map(|(body, state)| {
            json!({"body": body, "state": state, "author": {"login": "bob"}, "url": "https://github.com/acme/node-client/pull/482", "createdAt": "2026-11-27T10:00:00Z"})
        }).collect();
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .and(header("authorization", "Bearer ghu_review_loop_u1"))
            .and(body_partial_json(json!({"variables": {"id": id, "after": after}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"node": {
                "__typename":"PullRequestReviewThread", "path":"src/rpc/client.rs", "pullRequest": {"number": number, "title": "Retry RPC calls",
                    "repository": {"nameWithOwner": "Acme/Node-Client", "isPrivate": false}},
                "comments": {"nodes": nodes, "pageInfo": {"hasNextPage": next.is_some(), "endCursor": next}}
            }}})))
            .with_priority(1)
            .expect(1)
            .mount(&w.server)
            .await;
    }
    let detail_path = format!("{PR}/comments/threads");
    let anonymous = w
        .app
        .clone()
        .oneshot(
            Request::get(format!("{detail_path}/PRRT_unmapped"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let stored = w.get(&format!("{detail_path}/PRRT_fixture482_l1")).await;
    assert_eq!(stored["item_no"], mapped["no"]);
    let open = w.get(&format!("{detail_path}/PRRT_unmapped")).await;
    assert!(open["item_no"].is_null(), "{open}");
    assert_eq!(open["complete"], true);
    assert_eq!(open["comments"].as_array().unwrap().len(), 2);
    assert_eq!(open["comments"][1]["body"], "original reply");
    let (status, foreign) = w
        .call("GET", &format!("{detail_path}/PRRT_foreign"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{foreign}");
    assert!(!foreign.to_string().contains("other PR secret"));
    assert_eq!(rl.items(pr).await.unwrap().len(), previous_items);
    assert_eq!(
        rl.pr_state(pr).await.unwrap().unwrap().read_at,
        previous_read
    );
    assert!(rl.live_passes().await.unwrap().is_empty());
}

/// T22: decisions, gate marks, re-reads and item actions refuse access tokens.
#[tokio::test]
async fn no_token_reaches_decision_routes() {
    let w = world().await;
    w.get(&format!("{PR}/comments")).await;
    let (token, _) = crate::auth::access_tokens::stored_token(
        &w.state.db,
        "u1",
        vec![TokenScope::Read],
        Timestamp::now() + SignedDuration::from_hours(24),
    )
    .await;
    for rest in [
        "/comments/read",
        "/comments/items/C2/assess",
        "/comments/items/C2/decisions",
        "/comments/decision-batches",
        "/comments/items/C2/undo",
        "/comments/items/C2/reopen",
        "/comments/items/C2/skip",
        "/comments/items/C2/gates",
        "/comments/effects-preview",
    ] {
        let request = Request::post(format!("{PR}{rest}"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(decision("fix_here").to_string()))
            .expect("request");
        let response = w.app.clone().oneshot(request).await.expect("infallible");
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{rest}");
    }
    let request = Request::get(format!("{PR}/comments"))
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request");
    let response = w.app.clone().oneshot(request).await.expect("infallible");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a token reads what is stored"
    );
    assert_eq!(
        w.state
            .db
            .review_loop()
            .pr_decisions(w.pr_id().await)
            .await
            .expect("d")
            .len(),
        0
    );
}

/// ADR-022 §3: PR page, Comments and item reads in a browser session mark the reader present for the plug-in host;
/// a token read does not.
#[tokio::test]
async fn pr_page_reads_mark_presence_for_browser_sessions_only() {
    use crate::dashboard::plugin::pr_hygiene::PrHygiene;
    use crate::dashboard::plugin::{PluginHost, Timing};
    let w = world_with(|state| {
        let plugin =
            PrHygiene::new(Url::parse("http://127.0.0.1:9/").expect("url")).expect("plugin");
        state.plugins = PluginHost::new(vec![std::sync::Arc::new(plugin)], Timing::default());
    })
    .await;
    let (token, _) = crate::auth::access_tokens::stored_token(
        &w.state.db,
        "u1",
        vec![TokenScope::Read],
        Timestamp::now() + SignedDuration::from_hours(24),
    )
    .await;
    for rest in ["", "/comments", "/comments/items"] {
        let request = Request::get(format!("{PR}{rest}"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .expect("request");
        let response = w.app.clone().oneshot(request).await.expect("infallible");
        assert!(
            response.status().is_success() || response.status().is_client_error(),
            "{rest}"
        );
    }
    assert!(
        w.state.plugins.active_users().is_empty(),
        "a token read is not a user in the session"
    );
    for rest in ["", "/comments", "/comments/items"] {
        w.get(&format!("{PR}{rest}")).await;
        assert_eq!(
            w.state.plugins.active_users(),
            [(DEFAULT_TENANT.to_owned(), "u1".to_owned())],
            "{rest}"
        );
    }
}

/// RC1 for a pull request never read from GitHub (known from its analysis) still serves the page: the header from
/// the stored pull request, the failed read in the comments summary only, nothing invented about its GitHub state.
#[tokio::test]
async fn pr_page_without_any_github_read_degrades() {
    let w = world().await;
    let repo = w
        .state
        .db
        .repos()
        .upsert(
            DEFAULT_TENANT,
            &contracts::common::RepoRef {
                owner: "acme".into(),
                name: "node-client".into(),
            },
            None,
        )
        .await
        .expect("repo");
    let summary = contracts::api::pr::PrSummary {
        repo: repo.repo.clone(),
        number: 482,
        title: "Stored title".into(),
        author: "alice".into(),
        html_url: "https://github.com/acme/node-client/pull/482".into(),
        base_sha: contracts::ids::CommitSha("1".repeat(40)),
        head_sha: contracts::ids::CommitSha("2".repeat(40)),
    };
    w.state
        .db
        .pull_requests()
        .upsert(DEFAULT_TENANT, repo.id, &summary, Timestamp::now())
        .await
        .expect("pr");
    w.server.reset().await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&w.server)
        .await;
    let (status, page) = w.call("GET", PR, None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["title"], "Stored title");
    assert_eq!(page["author"], "alice");
    assert_eq!(page["head"], "2".repeat(40));
    assert_eq!(page["state"], "", "GitHub state unknown");
    assert_eq!(page["base"], "");
    assert_eq!(page["comments"]["read"]["failure"], "unavailable");
    assert_eq!(page["comments"]["read"]["read_at"], Value::Null);
    assert_eq!(page["analysis"], Value::Null);
}

/// RC6 while a pass of the pull request is queued or running answers 409 `conflict` instead of dropping the request
/// silently; once no pass runs it is accepted.
#[tokio::test]
async fn assess_again_while_a_pass_runs_is_a_conflict() {
    let w = world().await;
    w.link_llm().await;
    assert_eq!(
        w.call("POST", &format!("{PR}/comments/assess"), None)
            .await
            .0,
        StatusCode::ACCEPTED
    );
    let item = w.item(L2).await;
    let uri = format!("{PR}/comments/items/C{}/assess", item["no"]);
    let (status, body) = w.post(&uri, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "conflict");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("queued or running")),
        "{body}"
    );
}

/// RC1 names the pull request's author.
#[tokio::test]
async fn pr_page_names_the_author() {
    let w = world().await;
    assert_eq!(w.get(PR).await["author"], "alice");
}

/// RC4 says where GitHub records each comment as written: a bot round's own review names that round with its head
/// and end; any other review comment its review's commit and its own time, never a round's; a PR comment its time
/// only.
#[tokio::test]
async fn items_name_where_github_says_they_were_written() {
    let w = world().await;
    let rounds = w.get(&format!("{PR}/comments")).await["rounds"].clone();
    let items = w.items().await;
    let by_url = |part: &str| {
        items
            .iter()
            .find(|item| item["url"].as_str().is_some_and(|url| url.ends_with(part)))
            .unwrap_or_else(|| panic!("an item at {part}"))
    };
    // bob's thread in his review on 52d2da1, after rounds 1 and 2 ended: not "round 2 on 9f3c2e1".
    assert_eq!(
        by_url("#discussion_r3000000701")["written"],
        json!({"commit": "52d2da140191a9d47becf6a446310be803c007fc", "at": "2026-11-12T09:00:00Z"})
    );
    assert_eq!(
        by_url("#issuecomment-5000000103")["written"],
        json!({"at": "2026-11-12T09:05:00Z"})
    );
    let mut in_rounds = 0;
    for item in &items {
        let Some(number) = item["written"]["round"].as_u64() else {
            continue;
        };
        let round = rounds
            .as_array()
            .expect("rounds")
            .iter()
            .find(|round| round["number"] == number)
            .expect("its round");
        assert_eq!(item["author"], "thepastaclaw", "{item}");
        assert_eq!(
            item["written"],
            json!({"round": number, "commit": round["head"], "at": round["finished_at"]}),
            "{item}"
        );
        in_rounds += 1;
    }
    assert!(
        in_rounds > 0,
        "the fixture has items of a round's own review"
    );
}

/// RC4 counts the reader's answered Ask questions about each item; open questions and other readers' answers are
/// not counted.
#[tokio::test]
async fn items_count_the_readers_ask_answers() {
    use contracts::agent::ask::AskOutput;
    use contracts::ids::QuestionId;
    use db::repo::questions::{Asked, NewPrQuestion, PrQuestionLimits, QuestionUpdate};
    let w = world().await;
    w.state
        .db
        .users()
        .upsert_member(
            DEFAULT_TENANT,
            "u2",
            &GithubProfile {
                github_id: 2,
                login: "bob".into(),
                name: None,
                avatar_url: None,
            },
            Timestamp::now(),
        )
        .await
        .expect("user");
    let item = w.item(L2).await;
    assert_eq!(item["ask_answers"], 0);
    let id = serde_json::from_value(item["id"].clone()).expect("id");
    let pr = w.pr_id().await;
    let questions = w.state.db.questions();
    let limits = PrQuestionLimits {
        per_pull_request: 10,
        per_user: 10,
        user_window_start: Timestamp::now() - SignedDuration::from_hours(1),
    };
    for (user, answered) in [("u1", true), ("u1", false), ("u2", true)] {
        let Asked::Stored(stored) = questions
            .create_for_pr(
                NewPrQuestion {
                    id: QuestionId::new_random(),
                    pull_request_id: pr,
                    item: id,
                    parent_id: None,
                    user_id: user.into(),
                    text: "Why is this pre-existing?".into(),
                },
                limits,
                Timestamp::now(),
            )
            .await
            .expect("stored")
        else {
            panic!("expected a stored question");
        };
        if answered {
            questions
                .update(
                    stored.question.id,
                    &QuestionUpdate::Answered(AskOutput {
                        answer: "Because the base has it.".into(),
                        evidence: Vec::new(),
                    }),
                    Timestamp::now(),
                )
                .await
                .expect("answered");
        }
    }
    assert_eq!(w.item(L2).await["ask_answers"], 1);
}

mod feed;
mod loop_rules;
mod notify;
mod qa_core;

#[tokio::test]
async fn bot_status_comment_not_a_question() {
    let w = world().await;
    serve(&w.server, &[("comments.json", &|answer| {
        let nodes = &mut answer["data"]["repository"]["pullRequest"]["comments"]["nodes"];
        *nodes = json!([
            {
                "id": "IC_status", "databaseId": 801,
                "author": {"login": "ci[bot]", "__typename": "User"},
                "body": "All checks completed.", "createdAt": "2026-11-09T13:43:19Z",
                "url": "https://github.com/acme/node-client/pull/482#issuecomment-801"
            },
            {
                "id": "IC_finding", "databaseId": 802,
                "author": {"login": "ci[bot]", "__typename": "User"},
                "body": "<!-- thepastaclaw-review v1 finding=0123456789ab -->\n\nPlease handle the failed check.",
                "createdAt": "2026-11-09T13:43:19Z",
                "url": "https://github.com/acme/node-client/pull/482#issuecomment-802"
            }
        ]);
    })]).await;
    let overview = w.get(&format!("{PR}/comments")).await;
    let status = w.item("comment:801:1").await;
    let finding = w.item("comment:802:1").await;
    assert_eq!(status["status_comment"], true);
    assert!(finding.get("status_comment").is_none());
    assert_eq!(status["assessment_state"]["state"], "not_assessed");
    let entries = overview["queue"].as_array().unwrap();
    let queued = |no: &Value| {
        entries
            .iter()
            .any(|entry| entry["item_nos"].as_array().unwrap().contains(no))
    };
    assert!(!queued(&status["no"]));
    assert!(queued(&finding["no"]));
    w.link_llm().await;
    assert_eq!(
        w.call("POST", &format!("{PR}/comments/assess"), None)
            .await
            .0,
        StatusCode::ACCEPTED
    );
    let pass = w
        .state
        .db
        .review_loop()
        .latest_pass(w.pr_id().await)
        .await
        .unwrap()
        .unwrap();
    let status_id = serde_json::from_value(status["id"].clone()).unwrap();
    assert!(pass.sent.iter().all(|item| item.item != status_id));

    w.reread(&[("comments.json", &|answer| {
        answer["data"]["repository"]["pullRequest"]["comments"]["nodes"] = json!([{
            "id": "IC_finding", "databaseId": 802,
            "author": {"login": "ci[bot]", "__typename": "User"},
            "body": "Please handle the failed check.",
            "createdAt": "2026-11-09T13:43:19Z",
            "url": "https://github.com/acme/node-client/pull/482#issuecomment-802"
        }]);
    })])
    .await;
    let updated = w.item("comment:802:1").await;
    assert_eq!(updated["no"], finding["no"]);
    assert_eq!(updated["body"], finding["body"]);
    assert_eq!(updated["status_comment"], true);
}

#[tokio::test]
async fn bot_typename_counts_as_bot_in_fallback() {
    let w = world().await;
    serve(&w.server, &[("comments.json", &|answer| {
        answer["data"]["repository"]["pullRequest"]["comments"]["nodes"] = json!([{
            "id": "IC_finding", "databaseId": 803,
            "author": {"login": "type-only-reviewer", "__typename": "Bot"},
            "body": "### 1 finding(s) not shown inline\n\n**🔴 Blocking: Missing validation**\n\nPlease validate the input.",
            "createdAt": "2026-11-09T13:43:19Z",
            "url": "https://github.com/acme/node-client/pull/482#issuecomment-803"
        }]);
    })]).await;
    w.get(PR).await;
    let item = w.item("comment:803:1").await;
    assert_eq!(item["author_kind"], "bot");
    assert!(item.get("status_comment").is_none());
}

#[tokio::test]
async fn coderabbit_review_skipped_is_status_comment() {
    let w = world().await;
    let comment: Value = serde_json::from_str(include_str!(
        "../../../../../fixtures/review-loop/regressions/tenderdash-1600-comment.json"
    ))
    .unwrap();
    serve(
        &w.server,
        &[
            ("reviews.json", &|answer| {
                answer["data"]["repository"]["pullRequest"]["reviews"]["nodes"] = json!([]);
            }),
            ("review-threads.json", &|answer| {
                answer["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"] = json!([]);
            }),
            ("comments.json", &|answer| {
                answer["data"]["repository"]["pullRequest"]["comments"]["nodes"] = json!([comment]);
            }),
        ],
    )
    .await;
    let overview = w.get(&format!("{PR}/comments")).await;
    let item = w.item("comment:160001:1").await;
    assert_eq!(item["author_kind"], "bot");
    assert_eq!(item["status_comment"], true);
    let (status, error) = w
        .post(
            &format!("{PR}/comments/items/C{}/assess", item["no"]),
            json!(null),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["code"], "conflict");
    assert_eq!(error["message"], "Status comments cannot be assessed.");

    assert_eq!(overview["queue"], json!([]));
    assert_eq!(overview["counts"]["needs_you"], 0);
    assert_ne!(overview["badge"]["badge"], "questions_waiting");
    assert!(
        w.state
            .db
            .review_loop()
            .latest_pass(w.pr_id().await)
            .await
            .unwrap()
            .is_none()
    );
}
