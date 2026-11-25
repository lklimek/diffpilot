//! The review loop end to end through the public and runner routes: the in-process fake GitHub serves the recorded
//! review-loop reads, the real job queue holds the `verify_comments` job, and a runner stand-in answers it the way
//! the runner's fake provider does (`runner::provider::fake`, which the server cannot depend on).

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::header::COOKIE;
use axum::http::{Request, StatusCode};
use contracts::agent::JobKind;
use contracts::agent::verify_comments::VerifyCommentsInput;
use contracts::job::protocol::{Claim, ClaimResponse};
use contracts::job::result::{JobOutcome, JobResult};
use contracts::routes::{RUNNER_CLAIM, RUNNER_RESULT};
use contracts::time::Timestamp;
use db::repo::DEFAULT_TENANT;
use db::repo::review_loop::PassState;
use db::repo::users::GithubProfile;
use http_body_util::BodyExt;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;
use url::Url;

use crate::api;
use crate::auth::{lock_user, store_sign_in_tokens};
use crate::github::oauth::TokenGrant;
use crate::state::AppState;
use crate::test_support::github::{fake_github_sign_in, spawn_fake_github};
use crate::test_support::session_cookie;

const PR: &str = "/api/v1/prs/acme/node-client/482";

struct World {
    state: AppState,
    app: Router,
    runner: Router,
    cookie: String,
    _dir: TempDir,
}

/// Fake GitHub with the review-loop recordings, the fixture repository as clone base, the real job queue and a
/// signed-in member `u1` holding the fake's user token.
async fn world() -> World {
    let dir = tempfile::tempdir().expect("temp dir");
    let built = Command::new(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482/repo/build-repo.sh"),
    )
    .arg(dir.path().join("remotes/acme/node-client.git"))
    .output()
    .expect("build-repo.sh");
    assert!(built.status.success(), "{built:?}");
    let base = spawn_fake_github(None).await;
    let mut config = crate::config::tests::test_config();
    config.github.api_url = Url::parse(&base).expect("url");
    config.github.clone_url =
        Url::parse(&format!("file://{}", dir.path().join("remotes").display())).expect("url");
    config.data_dir = dir.path().join("data");
    let state = AppState::with_queue(config, db::repo::Db::in_memory().await.expect("db"));
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
    state
        .db
        .assessments()
        .set_config(
            DEFAULT_TENANT,
            &contracts::ids::UserId("u1".into()),
            &contracts::assessment::AssessmentConfig {
                primary_method: contracts::assessment::AssessmentMethod::PromptV4,
                ..Default::default()
            },
        )
        .await
        .expect("native assessment fixture");
    let lock = lock_user("u1").await;
    let grant = TokenGrant {
        access_token: SecretString::from(fake_github_sign_in(&base).await),
        expires_in: 8 * 3600,
        refresh_token: SecretString::from("ghr_end_to_end_u1"),
        refresh_token_expires_in: 180 * 86_400,
    };
    store_sign_in_tokens(&state, &lock, DEFAULT_TENANT, &grant, Timestamp::now())
        .await
        .expect("tokens");
    drop(lock);
    let cookie = session_cookie(&state, DEFAULT_TENANT).await;
    World {
        app: api::router(state.clone()),
        runner: api::runner_router(state.clone()),
        state,
        cookie,
        _dir: dir,
    }
}

async fn json_of(response: axum::response::Response) -> (StatusCode, Value) {
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

impl World {
    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(COOKIE, &self.cookie)
            .header("x-csrf-token", "csrf")
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .expect("request");
        json_of(self.app.clone().oneshot(request).await.expect("infallible")).await
    }

    async fn runner_call(&self, uri: &str, bearer: &str, body: Value) -> (StatusCode, Value) {
        let request = Request::post(uri)
            .header("authorization", format!("Bearer {bearer}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("request");
        json_of(
            self.runner
                .clone()
                .oneshot(request)
                .await
                .expect("infallible"),
        )
        .await
    }

    async fn pr_id(&self) -> i64 {
        self.state
            .db
            .pull_requests()
            .find(
                DEFAULT_TENANT,
                &contracts::common::RepoRef {
                    owner: "acme".into(),
                    name: "node-client".into(),
                },
                482,
            )
            .await
            .expect("find PR")
            .expect("stored PR")
    }

    /// One claim of the pipeline credential, as the runner sends it.
    async fn claim(&self) -> Option<Claim> {
        let request = json!({
            "runner_id": "runner-1",
            "credentials": [{
                "credential": { "id": db::repo::analyses::testing::CREDENTIAL, "kind": "codex_chatgpt" },
                "usable": true,
            }],
            "retired": [],
        });
        let secret = self.state.config.runner_secret.expose_secret().to_owned();
        let (status, body) = self.runner_call(RUNNER_CLAIM.path, &secret, request).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        serde_json::from_value::<ClaimResponse>(body)
            .expect("claim response")
            .claim
    }

    /// Posts the attempt's result as the runner does.
    async fn complete(&self, claim: &Claim, output: Value) {
        let result = JobResult {
            assessment: claim.spec.assessment.clone().map(Box::new),
            report: None,
            job_id: claim.spec.job_id,
            attempt: claim.spec.attempt,
            outcome: JobOutcome::Succeeded { output },
            session: None,
            usage: None,
            skills_commit: None,
            llm_call: None,
        };
        let uri = RUNNER_RESULT
            .path
            .replace("{id}", &claim.spec.job_id.to_string());
        let (status, body) = self
            .runner_call(&uri, claim.job_token.expose(), json!(result))
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }

    /// Answers every claimable job until the PR has no live pass left; returns the jobs answered.
    async fn run_passes(&self, pr: i64) -> usize {
        let mut answered = 0;
        for _ in 0..500 {
            if let Some(claim) = self.claim().await {
                assert_eq!(claim.spec.kind, JobKind::VerifyComments);
                assert_eq!(claim.spec.access.memcan, None, "no MemCan for verify jobs");
                self.complete(&claim, fake_provider_answer(&claim.spec.prompt))
                    .await;
                answered += 1;
                continue;
            }
            let rl = self.state.db.review_loop();
            let live = rl.live_passes().await.expect("passes");
            if answered > 0 && live.iter().all(|pass| pass.pull_request_id != pr) {
                return answered;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the assessment passes did not end in time");
    }
}

/// The runner's fake provider answer to a `verify_comments` prompt: a neutral assessment of every item of the data
/// block, recommended for a fix here.
fn fake_provider_answer(prompt: &str) -> Value {
    let input = VerifyCommentsInput::from_prompt(prompt).expect("verify input in the prompt");
    let items: Vec<Value> = input
        .items
        .iter()
        .map(|item| {
            let code = item.location.as_ref().and_then(|location| {
                location.lines.map(|lines| {
                    json!({"path": location.path, "enclosing": lines,
                        "description": "Fake description of the enclosing code for tests."})
                })
            });
            json!({
                "item": item.id, "validity": "partly_valid", "evidence": [], "pre_existing": false,
                "category": "other", "necessity": "in", "necessity_reason": "Fake assessment for tests.",
                "better_pr": null, "gates": [], "elsewhere": [], "fix_sketch": "Fake fix sketch.", "fix_size": "XS",
                "reply_drafts": ["Fixed per maintainer decision."],
                "recommendation": {"kind": "fix_here", "reason": "Fake recommendation for tests.",
                    "reason_class": "fake", "target_pr": null, "target_url": null, "title": null},
                "code": code
            })
        })
        .collect();
    json!({"purpose": "Fake purpose for tests.", "items": items, "answer": null})
}

#[tokio::test]
async fn pr_page_opening_queues_no_llm_work() {
    let w = world().await;
    let (status, page) = w.call("GET", PR, None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let pr = w.pr_id().await;
    let rl = w.state.db.review_loop();
    assert!(
        !rl.items(pr).await.unwrap().is_empty(),
        "the read found comments"
    );
    assert!(rl.latest_pass(pr).await.unwrap().is_none());
    assert!(w.claim().await.is_none());
}

/// Reading Comments, explicitly requesting an assessment, running its jobs and storing a decision with its effects.
#[tokio::test]
async fn comments_open_assess_and_decide_end_to_end() {
    let w = world().await;
    let (status, overview) = w.call("GET", &format!("{PR}/comments"), None).await;
    assert_eq!(status, StatusCode::OK, "{overview}");
    assert_eq!(overview["read"]["complete"], true);
    let pr = w.pr_id().await;
    let (_, items) = w.call("GET", &format!("{PR}/comments/items"), None).await;
    let threads = items
        .as_array()
        .expect("items")
        .iter()
        .filter(|item| item["kind"] == "thread")
        .count();
    assert!(threads > 0, "{items}");

    let rl = w.state.db.review_loop();
    assert!(rl.latest_pass(pr).await.expect("pass lookup").is_none());
    crate::review_loop::assess::request_pass_for(
        &w.state,
        pr,
        crate::review_loop::assess::PassReason::Opened,
        "u1",
    )
    .await
    .expect("request assessment");
    assert!(w.run_passes(pr).await >= 1);
    let pass = rl.latest_pass(pr).await.expect("pass").expect("a pass");
    assert_eq!(pass.state, PassState::Finished);
    assert_eq!(
        rl.pr_state(pr)
            .await
            .expect("state")
            .and_then(|row| row.purpose)
            .as_deref(),
        Some("Fake purpose for tests.")
    );

    let (_, items) = w.call("GET", &format!("{PR}/comments/items"), None).await;
    let item = items
        .as_array()
        .expect("items")
        .iter()
        .find(|item| {
            item["kind"] == "thread" && item["state"] == "undecided" && item["gates"] == json!([])
        })
        .expect("an undecided thread item")
        .clone();
    assert_eq!(item["assessment_state"], json!({"state": "assessed"}));
    assert_eq!(item["assessment"]["recommendation"]["kind"], "fix_here");

    let (status, decision) = w
        .call(
            "POST",
            &format!("{PR}/comments/items/C{}/decisions", item["no"]),
            Some(
                json!({"kind": "fix_here", "reason": null, "target": null, "override": null,
                "resolve_ticked": true, "reply_text": null, "comment": "Keep the test small.",
                "item_revision": item["revision"]}),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{decision}");
    assert_eq!(
        decision["recommendation"]["kind"], "fix_here",
        "the recommendation shown at that moment"
    );
    let stored = rl.pr_decisions(pr).await.expect("decisions");
    assert_eq!(stored.len(), 1);
    let kinds: Vec<Value> = stored[0]
        .decision
        .effects
        .iter()
        .map(|effect| json!(effect.kind))
        .collect();
    assert_eq!(kinds, [json!("fix"), json!("reply"), json!("resolve")]);
    assert_eq!(
        stored[0].decision.comment.as_deref(),
        Some("Keep the test small.")
    );
}
