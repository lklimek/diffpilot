//! The dashboard world over real TCP: REST search qualifiers, REST reads, GraphQL alias batches, injected faults
//! and the request log (plan v1.1 §A.2, §A.3).
#![expect(clippy::unwrap_used, reason = "test helpers outside #[test] functions")]

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use fake_github::{Config, StartError};
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_TYPE, LINK, LOCATION, RETRY_AFTER, USER_AGENT,
};
use reqwest::{Client, StatusCode, redirect};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

const VERIFIER: &str = "dashboard-verifier-0123456789-0123456789-0123456789";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn config(dashboard: Option<PathBuf>) -> Config {
    Config {
        fixture_dir: repo_root().join("fixtures/pr-482/github"),
        app_url: Url::parse("http://app.test:3000").unwrap(),
        user_login: None,
        user_id: None,
        app_id: None,
        app_public_key_file: None,
        dashboard_fixture_dir: dashboard,
        dashboard_world_at: None,
    }
}

fn world_dir() -> PathBuf {
    repo_root().join("fixtures/dashboard/github")
}

fn client() -> Client {
    Client::builder()
        .redirect(redirect::Policy::none())
        .build()
        .unwrap()
}

/// A fake with the dashboard world and a signed-in user's access token.
struct Fake {
    base: String,
    token: String,
}

async fn spawn() -> Fake {
    let base = spawn_with(config(Some(world_dir()))).await;
    let token = sign_in(&base).await;
    Fake { base, token }
}

async fn spawn_with(config: Config) -> String {
    let router = fake_github::router(&config).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}

/// The OAuth web flow of the fake; returns the access token.
async fn sign_in(base: &str) -> String {
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()));
    let response = client()
        .get(format!("{base}/login/oauth/authorize"))
        .query(&[
            ("client_id", "fixture-app-client-id"),
            ("redirect_uri", "http://app.test:3000/auth/github/callback"),
            ("state", "s"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ])
        .send()
        .await
        .unwrap();
    let location = Url::parse(response.headers()[LOCATION].to_str().unwrap()).unwrap();
    let code = location
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();
    let body: Value = client()
        .post(format!("{base}/login/oauth/access_token"))
        .header(ACCEPT, "application/json")
        .form(&[
            ("client_id", "fixture-app-client-id"),
            ("client_secret", "fixture-client-secret"),
            ("code", &code),
            ("redirect_uri", "http://app.test:3000/auth/github/callback"),
            ("code_verifier", VERIFIER),
        ])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    body["access_token"].as_str().unwrap().to_owned()
}

impl Fake {
    async fn get_with(
        &self,
        path: &str,
        query: &[(&str, &str)],
        token: Option<&str>,
    ) -> reqwest::Response {
        let mut request = client()
            .get(format!("{}{path}", self.base))
            .query(query)
            .header(USER_AGENT, "smart-review")
            .header(ACCEPT, "application/vnd.github+json");
        if let Some(token) = token {
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        request.send().await.unwrap()
    }

    async fn get(&self, path: &str, query: &[(&str, &str)]) -> reqwest::Response {
        self.get_with(path, query, Some(&self.token)).await
    }

    async fn search(&self, q: &str) -> (StatusCode, Value) {
        let response = self
            .get("/search/issues", &[("q", q), ("per_page", "100")])
            .await;
        (response.status(), response.json().await.unwrap())
    }

    /// `owner/name#number` of every item a search finds.
    async fn found(&self, q: &str) -> BTreeSet<String> {
        let (status, body) = self.search(q).await;
        assert_eq!(status, StatusCode::OK, "{q}: {body}");
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                let repository = item["repository_url"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("https://api.github.com/repos/");
                format!("{repository}#{}", item["number"])
            })
            .collect()
    }

    async fn graphql_raw(&self, query: &str, variables: Value) -> reqwest::Response {
        client()
            .post(format!("{}/graphql", self.base))
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(USER_AGENT, "smart-review")
            .json(&json!({"query": query, "variables": variables}))
            .send()
            .await
            .unwrap()
    }

    async fn graphql(&self, query: &str) -> Value {
        let response = self.graphql_raw(query, Value::Null).await;
        assert_eq!(response.status(), StatusCode::OK);
        response.json().await.unwrap()
    }

    async fn faults(&self, faults: Value) -> StatusCode {
        client()
            .post(format!("{}/_fake/dashboard/faults", self.base))
            .json(&json!({ "faults": faults }))
            .send()
            .await
            .unwrap()
            .status()
    }

    async fn requests(&self) -> Vec<Value> {
        client()
            .get(format!("{}/_fake/dashboard/requests", self.base))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
}

/// GitHub's answer to a document it refuses before execution: `errors` only, no `data`; returns the messages.
fn refused(body: &Value) -> Vec<String> {
    assert!(
        body.get("data").is_none(),
        "no data for a refused document: {body}"
    );
    body["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("errors expected: {body}"))
        .iter()
        .map(|e| e["message"].as_str().unwrap().to_owned())
        .collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

/// An alias batch of `numbers` pull requests of `acme/platform` (`#n` beyond the world are missing).
fn pull_batch(count: usize) -> String {
    let lookups: String = (0..count)
        .map(|i| format!("p{i}: repository(owner: \"acme\", name: \"platform\") {{ pullRequest(number: {}) {{ number }} }} ", 1100 + i))
        .collect();
    format!("query {{ rateLimit {{ cost remaining resetAt }} {lookups} }}")
}

#[tokio::test]
async fn pull_request_searches_honour_q1_qualifiers() {
    let fake = spawn().await;
    assert_eq!(
        fake.found("is:pr is:open archived:false author:alice")
            .await,
        set(&[
            "acme/node-client#476",
            "acme/node-client#490",
            "acme/platform#1176",
            "acme/wallet-ui#298",
            "acme/wallet-ui#311"
        ]),
        "the archived repository's open pull request and closed ones are excluded"
    );
    assert_eq!(
        fake.found("is:pr is:open archived:false review-requested:alice")
            .await,
        set(&[
            "acme/consensus#1534",
            "acme/docs-site#77",
            "acme/node-client#482",
            "acme/node-client#495",
            "acme/platform#1199",
            "acme/platform#1203",
            "acme/wallet-ui#320",
        ])
    );
    assert_eq!(
        fake.found("is:pr is:open archived:false assignee:alice")
            .await,
        set(&["acme/ops-runbooks#14"])
    );
    let anonymous = fake
        .get_with(
            "/search/issues",
            &[("q", "is:pr is:open assignee:alice")],
            None,
        )
        .await;
    let body: Value = anonymous.json().await.unwrap();
    assert_eq!(body["total_count"], 0, "private repositories need a token");
    assert_eq!(
        fake.found("is:pr is:open archived:true author:alice").await,
        set(&["acme/legacy-wallet#9"])
    );
}

#[tokio::test]
async fn search_items_have_github_rest_shape() {
    let fake = spawn().await;
    let (_, body) = fake
        .search("is:pr is:open repo:acme/platform author:alice")
        .await;
    assert_eq!(body["total_count"], 1);
    assert_eq!(body["incomplete_results"], false);
    let item = &body["items"][0];
    assert_eq!(
        item["html_url"],
        "https://github.com/acme/platform/pull/1176"
    );
    assert_eq!(
        item["repository_url"],
        "https://api.github.com/repos/acme/platform"
    );
    assert_eq!(item["user"]["login"], "alice");
    assert_eq!(item["state"], "open");
    assert_eq!(item["draft"], false);
    assert_eq!(item["labels"], json!([{"name": "ready-for-human"}]));
    assert_eq!(item["milestone"]["title"], "v2.4");
    assert!(item["pull_request"]["merged_at"].is_null());
    let (_, issues) = fake.search("is:issue repo:acme/platform is:open").await;
    assert!(
        issues["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i.get("pull_request").is_none())
    );
}

#[tokio::test]
async fn issue_searches_honour_q10_qualifiers() {
    let fake = spawn().await;
    assert_eq!(
        fake.found("is:issue is:open archived:false repo:acme/platform")
            .await,
        set(&[
            "acme/platform#1187",
            "acme/platform#1210",
            "acme/platform#1215"
        ])
    );
    assert_eq!(
        fake.found("is:issue is:open archived:false assignee:alice")
            .await,
        set(&[
            "acme/consensus#1520",
            "acme/platform#1187",
            "acme/platform#1210"
        ])
    );
    assert_eq!(
        fake.found("is:issue is:open archived:false mentions:alice")
            .await,
        set(&["acme/platform#1187", "acme/wallet-ui#305"]),
        "mentions in the body or a comment; `@alice-helper` is not `@alice`"
    );
    assert_eq!(
        fake.found("is:issue is:open repo:acme/platform repo:acme/wallet-ui mentions:alice")
            .await,
        set(&["acme/platform#1187", "acme/wallet-ui#305"]),
        "several repo: qualifiers are OR-ed"
    );
}

#[tokio::test]
async fn history_searches_honour_q12_date_ranges() {
    let fake = spawn().await;
    let window = ">=2026-08-26";
    let created = fake
        .found(&format!("is:pr author:alice created:{window}"))
        .await;
    assert!(created.contains("acme/platform#1190") && created.contains("acme/wallet-ui#300"));
    assert!(
        !created.contains("acme/platform#1100"),
        "created before the window"
    );
    assert_eq!(
        fake.found(&format!("is:pr author:alice closed:{window}"))
            .await,
        set(&["acme/platform#1190", "acme/wallet-ui#300"])
    );
    assert_eq!(
        fake.found(&format!("is:pr reviewed-by:alice updated:{window}"))
            .await,
        set(&["acme/consensus#1510", "acme/platform#1195"])
    );
    assert_eq!(
        fake.found("is:pr reviewed-by:alice updated:2026-11-01..2026-11-13")
            .await,
        set(&["acme/platform#1195"]),
        "a range includes its last day"
    );
    assert_eq!(
        fake.found("is:pr author:alice merged:<2026-07-02").await,
        set(&["acme/platform#1100"])
    );
}

#[tokio::test]
async fn unsearchable_logins_and_invisible_repositories_answer_422() {
    let fake = spawn().await;
    for q in [
        "is:pr is:open author:alice-helper",
        "is:pr is:open author:nobody-here",
        "is:issue is:open repo:acme/secret",
    ] {
        let (status, body) = fake.search(q).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{q}");
        assert_eq!(body["message"], "Validation Failed");
        assert!(
            body["errors"][0]["message"]
                .as_str()
                .unwrap()
                .contains("cannot be searched"),
            "{body}"
        );
    }
    let anonymous = fake
        .get_with(
            "/search/issues",
            &[("q", "is:pr repo:acme/ops-runbooks")],
            None,
        )
        .await;
    assert_eq!(
        anonymous.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "a private repository needs a token"
    );
    let (status, body) = fake.search("is:open author:alice").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["errors"][0]["message"],
        "Query must include 'is:issue' or 'is:pull-request'"
    );
    let (status, body) = fake.search("is:pr author:alice label:bug").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        body["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("label:bug")
    );
}

#[tokio::test]
async fn search_pages_with_link_headers_up_to_the_1000_cap() {
    let fake = spawn().await;
    let q = "is:pr is:open review-requested:alice";
    let response = fake
        .get("/search/issues", &[("q", q), ("per_page", "3")])
        .await;
    let link = response.headers()[LINK].to_str().unwrap().to_owned();
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["total_count"], 7);
    assert_eq!(body["items"].as_array().unwrap().len(), 3);
    assert!(
        link.contains("page=2>; rel=\"next\"") && link.contains("page=3>; rel=\"last\""),
        "{link}"
    );
    let next = link.split(['<', '>']).nth(1).unwrap();
    let page2: Value = client()
        .get(next)
        .header(USER_AGENT, "t")
        .header(AUTHORIZATION, format!("Bearer {}", fake.token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page2["items"].as_array().unwrap().len(), 3);
    let beyond = fake
        .get(
            "/search/issues",
            &[("q", q), ("per_page", "100"), ("page", "11")],
        )
        .await;
    assert_eq!(beyond.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = beyond.json().await.unwrap();
    assert_eq!(
        body["message"],
        "Only the first 1000 search results are available"
    );
}

#[tokio::test]
async fn search_answers_carry_search_rate_limit_headers() {
    let fake = spawn().await;
    let first = fake
        .get("/search/issues", &[("q", "is:pr author:alice")])
        .await;
    let second = fake
        .get("/search/issues", &[("q", "is:pr author:alice")])
        .await;
    let header = |response: &reqwest::Response, name: &str| {
        response.headers()[name].to_str().unwrap().to_owned()
    };
    assert_eq!(header(&first, "x-ratelimit-resource"), "search");
    assert_eq!(header(&first, "x-ratelimit-limit"), "30");
    assert_eq!(header(&first, "x-ratelimit-remaining"), "29");
    assert_eq!(header(&second, "x-ratelimit-remaining"), "28");
    assert_eq!(header(&second, "x-ratelimit-used"), "2");
    let releases = fake.get("/repos/acme/platform/releases", &[]).await;
    assert_eq!(header(&releases, "x-ratelimit-resource"), "core");
    assert_eq!(header(&releases, "x-ratelimit-limit"), "5000");
}

#[tokio::test]
async fn injected_secondary_limit_answers_403_with_retry_after_then_recovers() {
    let fake = spawn().await;
    let status = fake
        .faults(json!([
            {"on": "search", "matching": "review-requested:alice", "times": 2,
             "answer": {"kind": "secondary_rate_limit", "retry_after": 60}},
            {"on": "search", "answer": {"kind": "secondary_rate_limit", "status": 429}},
        ]))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let other = fake
        .get("/search/issues", &[("q", "is:pr author:alice")])
        .await;
    assert_eq!(
        other.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the unfiltered fault takes the first other search"
    );
    assert!(
        other.headers().get(RETRY_AFTER).is_none(),
        "GitHub may omit retry-after"
    );
    for _ in 0..2 {
        let limited = fake
            .get("/search/issues", &[("q", "is:pr review-requested:alice")])
            .await;
        assert_eq!(limited.status(), StatusCode::FORBIDDEN);
        assert_eq!(limited.headers()[RETRY_AFTER], "60");
        assert_ne!(limited.headers()["x-ratelimit-remaining"], "0");
        let body: Value = limited.json().await.unwrap();
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .contains("secondary rate limit")
        );
    }
    let recovered = fake
        .get("/search/issues", &[("q", "is:pr review-requested:alice")])
        .await;
    assert_eq!(recovered.status(), StatusCode::OK);
    let faults: Vec<Value> = fake
        .requests()
        .await
        .into_iter()
        .map(|r| r.get("fault").cloned().unwrap_or(Value::Null))
        .collect();
    assert_eq!(
        faults,
        vec![
            json!("secondary_rate_limit"),
            json!("secondary_rate_limit"),
            json!("secondary_rate_limit"),
            Value::Null
        ]
    );
}

#[tokio::test]
async fn injected_primary_limit_answers_remaining_zero_and_reset() {
    let fake = spawn().await;
    fake.faults(json!([
        {"on": "search", "answer": {"kind": "primary_rate_limit", "reset": 1_795_000_000}},
        {"on": "rest", "answer": {"kind": "primary_rate_limit", "reset": 1_795_000_001}},
        {"on": "graphql", "answer": {"kind": "primary_rate_limit", "reset": 1_795_000_002}},
    ]))
    .await;
    let search = fake
        .get("/search/issues", &[("q", "is:pr author:alice")])
        .await;
    assert_eq!(search.status(), StatusCode::FORBIDDEN);
    assert_eq!(search.headers()["x-ratelimit-remaining"], "0");
    assert_eq!(search.headers()["x-ratelimit-reset"], "1795000000");
    assert_eq!(search.headers()["x-ratelimit-resource"], "search");
    let rest = fake.get("/repos/acme/platform/milestones", &[]).await;
    assert_eq!(rest.status(), StatusCode::FORBIDDEN);
    assert_eq!(rest.headers()["x-ratelimit-reset"], "1795000001");
    let graphql = fake.graphql_raw(&pull_batch(1), Value::Null).await;
    assert_eq!(
        graphql.status(),
        StatusCode::OK,
        "GraphQL reports the primary limit in a 200 answer"
    );
    assert_eq!(graphql.headers()["x-ratelimit-remaining"], "0");
    let body: Value = graphql.json().await.unwrap();
    assert_eq!(body["errors"][0]["type"], "RATE_LIMITED");
}

#[tokio::test]
async fn fault_requests_are_validated() {
    let fake = spawn().await;
    let bad_status = fake
        .faults(json!([{"on": "graphql", "answer": {"kind": "server_error", "status": 200}}]))
        .await;
    assert_eq!(bad_status, StatusCode::BAD_REQUEST);
    let unknown_field = fake
        .faults(json!([{"on": "graphql", "when": "x", "answer": {"kind": "server_error"}}]))
        .await;
    assert!(unknown_field.is_client_error());
}

const DETAIL: &str = r#"
fragment Detail on PullRequest {
  number title state isDraft headRefOid baseRefName headRefName additions deletions reviewDecision
  milestone { title number state }
  baseRepository { defaultBranchRef { name } }
  author { __typename login }
  labels(first: 20) { nodes { name } }
  closingIssuesReferences(first: 5) { nodes { number title } }
  reviewRequests(first: 20) { nodes { requestedReviewer { __typename ... on User { login } ... on Team { name } } } }
  reviews(last: 20) { totalCount nodes { author { login } state submittedAt } }
  reviewThreads(first: 100) { nodes { isResolved comments(first: 5) { nodes { author { login } body } } } }
  timelineItems(last: 50, itemTypes: [REVIEW_REQUESTED_EVENT]) {
    nodes { ... on ReviewRequestedEvent { createdAt requestedReviewer { ... on User { login } } } }
  }
  commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(first: 100) {
    nodes { __typename ... on CheckRun { name conclusion } ... on StatusContext { context state } } } } } } }
}"#;

#[tokio::test]
async fn pr_detail_batches_answer_each_alias_with_the_selected_fields() {
    let fake = spawn().await;
    let query = format!(
        "query {{ rateLimit {{ cost remaining resetAt }}
           p0: repository(owner: \"acme\", name: \"platform\") {{ pullRequest(number: 1203) {{ ...Detail }} }}
           p1: repository(owner: \"acme\", name: \"wallet-ui\") {{ pullRequest(number: 311) {{ ...Detail }} }}
           p2: repository(owner: \"acme\", name: \"platform\") {{ pullRequest(number: 9999) {{ ...Detail }} }}
         }} {DETAIL}"
    );
    let response = fake.graphql_raw(&query, Value::Null).await;
    assert_eq!(response.headers()["x-ratelimit-resource"], "graphql");
    let body: Value = response.json().await.unwrap();
    let data = &body["data"];
    assert_eq!(data["rateLimit"]["cost"], 3);
    assert_eq!(data["rateLimit"]["remaining"], 4997);
    assert!(
        data["rateLimit"]["resetAt"]
            .as_str()
            .unwrap()
            .ends_with('Z')
    );
    let fix = &data["p0"]["pullRequest"];
    assert_eq!(
        fix["title"],
        "fix(sync): retry header fetch after peer timeout"
    );
    assert_eq!(fix["baseRepository"]["defaultBranchRef"]["name"], "main");
    assert_eq!(fix["closingIssuesReferences"]["nodes"][0]["number"], 1187);
    assert_eq!(
        fix["reviewRequests"]["nodes"][0]["requestedReviewer"],
        json!({"__typename": "User", "login": "alice"}),
        "inline fragments of other types add nothing"
    );
    assert_eq!(
        fix["timelineItems"]["nodes"][0]["requestedReviewer"]["login"],
        "alice"
    );
    assert_eq!(
        fix["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"],
        "SUCCESS"
    );
    assert!(
        fix.get("body").is_none(),
        "only selected fields are answered"
    );
    let mine = &data["p1"]["pullRequest"];
    assert_eq!(mine["reviewThreads"]["nodes"].as_array().unwrap().len(), 3);
    let contexts = &mine["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"];
    assert_eq!(
        contexts[1],
        json!({"__typename": "CheckRun", "name": "test (e2e)", "conclusion": "FAILURE"})
    );
    assert_eq!(
        contexts[2],
        json!({"__typename": "StatusContext", "context": "preview/deploy", "state": "SUCCESS"})
    );
    assert_eq!(data["p2"], json!({"pullRequest": null}));
    assert_eq!(body["errors"][0]["type"], "NOT_FOUND");
    assert_eq!(body["errors"][0]["path"], json!(["p2", "pullRequest"]));
    let log = fake.requests().await;
    assert_eq!(log[0]["on"], "graphql");
    assert_eq!(log[0]["lookups"], 3);
    assert_eq!(log[0]["pull_requests"], 3);
    assert_eq!(log[0]["cost"], 3);
}

#[tokio::test]
async fn batch_cost_follows_the_spike_and_oversized_batches_fail() {
    let fake = spawn().await;
    let ten = fake.graphql(&pull_batch(10)).await;
    assert_eq!(ten["data"]["rateLimit"]["cost"], 12, "S11-2: N=10 cost 12");
    let twenty = fake.graphql_raw(&pull_batch(20), Value::Null).await;
    assert_eq!(twenty.status(), StatusCode::OK);
    let thirty = fake.graphql_raw(&pull_batch(30), Value::Null).await;
    assert_eq!(thirty.status(), StatusCode::BAD_GATEWAY);
    assert!(
        thirty.headers()[CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let fifty = fake.graphql_raw(&pull_batch(50), Value::Null).await;
    assert_eq!(fifty.status(), StatusCode::GATEWAY_TIMEOUT);
    let log = fake.requests().await;
    let summary: Vec<(Value, Value)> = log
        .iter()
        .map(|r| {
            (
                r["pull_requests"].clone(),
                r.get("fault").cloned().unwrap_or(Value::Null),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (json!(10), Value::Null),
            (json!(20), Value::Null),
            (json!(30), json!("oversized_batch")),
            (json!(50), json!("oversized_batch")),
        ]
    );
}

#[tokio::test]
async fn injected_graphql_server_error_fails_once_then_halves_succeed() {
    let fake = spawn().await;
    fake.faults(
        json!([{"on": "graphql", "matching": "number: 1100", "answer": {"kind": "server_error"}}]),
    )
    .await;
    let failed = fake.graphql_raw(&pull_batch(10), Value::Null).await;
    assert_eq!(failed.status(), StatusCode::BAD_GATEWAY);
    assert!(failed.text().await.unwrap().contains("Unicorn"));
    let half = fake.graphql(&pull_batch(5)).await;
    assert!(half["errors"].as_array().unwrap().len() <= 5);
    fake.faults(json!([{"on": "graphql", "answer": {"kind": "server_error", "status": 504, "html": false}}]))
        .await;
    let timeout = fake.graphql_raw(&pull_batch(1), Value::Null).await;
    assert_eq!(timeout.status(), StatusCode::GATEWAY_TIMEOUT);
    assert!(timeout.json::<Value>().await.unwrap()["message"].is_string());
}

#[tokio::test]
async fn open_pr_ref_listings_page_by_cursor() {
    let fake = spawn().await;
    let listing = |first: u32, after: &str| {
        format!(
            "query {{ r0: repository(owner: \"acme\", name: \"wallet-ui\") {{
                pullRequests(states: OPEN, first: {first}{after}) {{
                  totalCount pageInfo {{ hasNextPage endCursor }} nodes {{ number baseRefName headRefName isDraft }} }} }}
              r1: repository(owner: \"acme\", name: \"node-client\") {{
                pullRequests(states: [OPEN], first: 100) {{ nodes {{ number }} }} }} }}"
        )
    };
    let body = fake.graphql(&listing(2, "")).await;
    let page = &body["data"]["r0"]["pullRequests"];
    assert_eq!(page["totalCount"], 3);
    assert_eq!(
        page["nodes"],
        json!([
            {"number": 298, "baseRefName": "feat/pending-withdrawals", "headRefName": "feat/export-csv", "isDraft": false},
            {"number": 311, "baseRefName": "main", "headRefName": "feat/pending-withdrawals", "isDraft": false},
        ]),
        "a stacked pull request's base is another's head"
    );
    assert_eq!(page["pageInfo"]["hasNextPage"], true);
    let numbers: Vec<u64> = body["data"]["r1"]["pullRequests"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["number"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers, vec![476, 482, 490, 495]);
    let cursor = page["pageInfo"]["endCursor"].as_str().unwrap();
    let next = fake
        .graphql(&listing(2, &format!(", after: \"{cursor}\"")))
        .await;
    let rest = &next["data"]["r0"]["pullRequests"];
    assert_eq!(
        rest["nodes"],
        json!([{"number": 320, "baseRefName": "main", "headRefName": "fix/focus-ring", "isDraft": false}])
    );
    assert_eq!(rest["pageInfo"]["hasNextPage"], false);
}

#[tokio::test]
async fn connections_follow_github_pagination_limits() {
    let fake = spawn().await;
    let body = fake
        .graphql("{ r: repository(owner: \"acme\", name: \"platform\") { pullRequests(states: OPEN) { nodes { number } } } }")
        .await;
    assert_eq!(body["errors"][0]["type"], "MISSING_PAGINATION_BOUNDARIES");
    refused(&body);
    let body = fake
        .graphql("{ r: repository(owner: \"acme\", name: \"platform\") { pullRequests(first: 101) { nodes { number } } } }")
        .await;
    assert_eq!(body["errors"][0]["type"], "EXCESSIVE_PAGE_SIZE");
    refused(&body);
    let body = fake
        .graphql("{ r: repository(owner: \"acme\", name: \"platform\") { pullRequests(first: 5, after: \"bogus\") { nodes { number } } } }")
        .await;
    assert_eq!(body["errors"][0]["type"], "INVALID_CURSOR_ARGUMENTS");
    refused(&body);
    let end = STANDARD.encode(format!("cursor:{}", u64::MAX));
    let body = fake
        .graphql(&format!(
            "{{ r: repository(owner: \"acme\", name: \"platform\") {{ pullRequests(first: 5, after: \"{end}\") {{ nodes {{ number }} }} }} }}"
        ))
        .await;
    assert_eq!(
        body["errors"][0]["type"], "INVALID_CURSOR_ARGUMENTS",
        "a cursor at the end of the range"
    );
    let body = fake
        .graphql("{ r: repository(owner: \"acme\", name: \"platform\") { pullRequests(states: OPEN) { totalCount } } }")
        .await;
    assert!(
        body.get("errors").is_none(),
        "totalCount alone needs no page size"
    );
    assert_eq!(body["data"]["r"]["pullRequests"]["totalCount"], 3);
}

#[tokio::test]
async fn issue_milestone_batches_answer_q9() {
    let fake = spawn().await;
    let body = fake
        .graphql(
            "query($o: String!) {
               i0: repository(owner: $o, name: \"platform\") { issue(number: 1187) { milestone { title number state } } }
               i1: repository(owner: $o, name: \"wallet-ui\") { issue(number: 305) { milestone { title } } }
               i2: repository(owner: $o, name: \"platform\") { issue(number: 4040) { milestone { title } } } }",
        )
        .await;
    assert_eq!(
        refused(&body),
        vec!["Variable $o of type String! was provided invalid value"],
        "a required variable without a value refuses the document"
    );
    let response = fake
        .graphql_raw(
            "query($o: String!) {
               i0: repository(owner: $o, name: \"platform\") { issue(number: 1187) { milestone { title number state } } }
               i1: repository(owner: $o, name: \"wallet-ui\") { issue(number: 305) { milestone { title } } }
               i2: repository(owner: $o, name: \"platform\") { issue(number: 4040) { milestone { title } } } }",
            json!({"o": "acme"}),
        )
        .await;
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["data"]["i0"]["issue"]["milestone"],
        json!({"title": "v2.4", "number": 12, "state": "OPEN"})
    );
    assert_eq!(body["data"]["i1"]["issue"]["milestone"], Value::Null);
    assert_eq!(body["data"]["i2"]["issue"], Value::Null);
    assert_eq!(
        body["errors"][0]["message"],
        "Could not resolve to an Issue with the number of 4040."
    );
}

#[tokio::test]
async fn history_batches_filter_reviews_by_author_for_q13() {
    let fake = spawn().await;
    let body = fake
        .graphql(
            "{ rateLimit { cost }
               n0: repository(owner: \"acme\", name: \"platform\") { pullRequest(number: 1195) {
                 createdAt closedAt mergedAt author { login }
                 reviews(author: \"alice\", last: 20) { nodes { state submittedAt } } } }
               n1: repository(owner: \"acme\", name: \"wallet-ui\") { pullRequest(number: 311) {
                 reviews(author: \"alice\", last: 20) { totalCount } } } }",
        )
        .await;
    let merged = &body["data"]["n0"]["pullRequest"];
    assert_eq!(merged["mergedAt"], "2026-11-13T12:00:00Z");
    assert_eq!(merged["author"]["login"], "mira-k");
    assert_eq!(
        merged["reviews"]["nodes"],
        json!([
            {"state": "CHANGES_REQUESTED", "submittedAt": "2026-11-11T15:00:00Z"},
            {"state": "APPROVED", "submittedAt": "2026-11-12T16:30:00Z"},
        ])
    );
    assert_eq!(
        body["data"]["n1"]["pullRequest"]["reviews"]["totalCount"],
        0
    );
}

#[tokio::test]
async fn reach_lookups_report_invisible_repositories_and_owners_for_q15() {
    let fake = spawn().await;
    let body = fake
        .graphql(
            "{ a0: repository(owner: \"acme\", name: \"platform\") { id }
               a1: repository(owner: \"acme\", name: \"secret\") { id }
               a2: repository(owner: \"acme\", name: \"ops-runbooks\") { id isPrivate }
               o0: repositoryOwner(login: \"acme\") { id login }
               o1: repositoryOwner(login: \"nobody-here\") { id } }",
        )
        .await;
    assert_eq!(body["data"]["a0"]["id"], "R_acme_platform");
    assert_eq!(body["data"]["a1"], Value::Null);
    assert_eq!(body["data"]["a2"]["isPrivate"], true);
    assert_eq!(body["data"]["o0"]["login"], "acme");
    assert_eq!(body["data"]["o1"], Value::Null);
    let errors = body["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "an unknown owner is null without an error");
    assert_eq!(
        errors[0]["message"],
        "Could not resolve to a Repository with the name 'acme/secret'."
    );
    assert_eq!(fake.requests().await[0]["lookups"], 5);
}

#[tokio::test]
async fn owner_enumeration_lists_repositories_and_open_items_for_q11() {
    let fake = spawn().await;
    let body = fake
        .graphql(
            "query($login: String!, $after: String) { repositoryOwner(login: $login) {
               repositories(first: 100, after: $after, isArchived: false) { pageInfo { hasNextPage }
                 nodes { nameWithOwner pullRequests(states: OPEN) { totalCount } issues(states: OPEN) { totalCount } } } } }",
        )
        .await;
    assert_eq!(
        refused(&body),
        vec!["Variable $login of type String! was provided invalid value"]
    );
    let response = fake
        .graphql_raw(
            "query($login: String!, $after: String) { repositoryOwner(login: $login) {
               repositories(first: 100, after: $after, isArchived: false) { pageInfo { hasNextPage }
                 nodes { nameWithOwner pullRequests(states: OPEN) { totalCount } issues(states: OPEN) { totalCount } } } } }",
            json!({"login": "acme"}),
        )
        .await;
    let body: Value = response.json().await.unwrap();
    let repositories = &body["data"]["repositoryOwner"]["repositories"]["nodes"];
    let names: Vec<&str> = repositories
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["nameWithOwner"].as_str().unwrap())
        .collect();
    assert!(
        !names.contains(&"acme/legacy-wallet"),
        "archived repositories are skipped"
    );
    let consensus = repositories
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["nameWithOwner"] == "acme/consensus")
        .unwrap();
    assert_eq!(consensus["pullRequests"]["totalCount"], 2);
    assert_eq!(consensus["issues"]["totalCount"], 2);
    let listing = fake
        .graphql(
            "{ r: repository(owner: \"acme\", name: \"consensus\") {
                 pullRequests(states: OPEN, first: 50) { nodes { number author { login } } }
                 issues(states: OPEN, first: 50) { nodes { number assignees(first: 20) { nodes { login } } } } } }",
        )
        .await;
    assert_eq!(
        listing["data"]["r"]["pullRequests"]["nodes"][1],
        json!({"number": 1541, "author": {"login": "alice-helper"}})
    );
    assert_eq!(
        listing["data"]["r"]["issues"]["nodes"][1]["assignees"]["nodes"][0]["login"],
        "alice-helper"
    );
}

#[tokio::test]
async fn v1_review_documents_and_recordings_still_reach_the_recordings() {
    let fake = spawn().await;
    let threads = fake
        .graphql_raw(
            "query($o:String!,$n:String!,$pr:Int!){repository(owner:$o,name:$n){pullRequest(number:$pr){reviewThreads(first:100){nodes{id}}}}}",
            json!({"o": "acme", "n": "node-client", "pr": 482}),
        )
        .await;
    let body: Value = threads.json().await.unwrap();
    assert!(
        !body["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        fake.requests().await.is_empty(),
        "v1 documents are not dashboard requests"
    );
    let files = fake
        .get("/repos/acme/node-client/pulls/482/files", &[])
        .await;
    assert_eq!(files.status(), StatusCode::OK);
    let recorded: Value = files.json().await.unwrap();
    assert_eq!(
        recorded.as_array().unwrap().len(),
        13,
        "the recording, not the world's (empty) file list"
    );
}

#[tokio::test]
async fn the_recorded_pull_request_head_moves_in_dashboard_answers() {
    let fake = spawn().await;
    let query = "{ rateLimit { cost } p: repository(owner: \"acme\", name: \"node-client\") { pullRequest(number: 482) { headRefOid } } }";
    let before = fake.graphql(query).await;
    assert_eq!(
        before["data"]["p"]["pullRequest"]["headRefOid"],
        "52d2da140191a9d47becf6a446310be803c007fc"
    );
    let moved = "1111111111111111111111111111111111111111";
    let status = client()
        .post(format!("{}/_fake/head", fake.base))
        .json(&json!({"sha": moved}))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::NO_CONTENT);
    let after = fake.graphql(query).await;
    assert_eq!(after["data"]["p"]["pullRequest"]["headRefOid"], moved);
}

#[tokio::test]
async fn releases_and_milestones_answer_q7_and_q8() {
    let fake = spawn().await;
    let latest: Value = fake
        .get("/repos/acme/platform/releases/latest", &[])
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        latest["tag_name"], "v2.3.0",
        "prereleases are not the latest release"
    );
    let latest: Value = fake
        .get("/repos/acme/node-client/releases/latest", &[])
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        latest["tag_name"], "v1.8.2",
        "drafts are not the latest release"
    );
    assert_eq!(
        fake.get("/repos/acme/docs-site/releases/latest", &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let releases: Value = fake
        .get("/repos/acme/platform/releases", &[])
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(releases.as_array().unwrap().len(), 3);
    let open: Value = fake
        .get("/repos/acme/platform/milestones", &[])
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(open.as_array().unwrap().len(), 2, "default state is open");
    let all = fake
        .get(
            "/repos/acme/platform/milestones",
            &[("state", "all"), ("per_page", "2")],
        )
        .await;
    assert!(
        all.headers()[LINK]
            .to_str()
            .unwrap()
            .contains("rel=\"next\"")
    );
    let all: Value = all.json().await.unwrap();
    assert_eq!(all.as_array().unwrap().len(), 2);
    assert_eq!(
        fake.get("/repos/acme/secret/releases", &[]).await.status(),
        StatusCode::NOT_FOUND
    );
    let private = fake
        .get_with("/repos/acme/ops-runbooks/milestones", &[], None)
        .await;
    assert_eq!(
        private.status(),
        StatusCode::NOT_FOUND,
        "private repositories are invisible without a token"
    );
}

#[tokio::test]
async fn contents_answer_raw_or_base64_json_for_q3() {
    let fake = spawn().await;
    let path = "/repos/acme/review-policies/contents/policies/platform.json";
    let raw = client()
        .get(format!("{}{path}", fake.base))
        .header(USER_AGENT, "t")
        .header(AUTHORIZATION, format!("Bearer {}", fake.token))
        .header(ACCEPT, "application/vnd.github.raw+json")
        .send()
        .await
        .unwrap();
    assert_eq!(raw.status(), StatusCode::OK);
    let policy: Value = serde_json::from_str(&raw.text().await.unwrap()).unwrap();
    assert_eq!(policy["areas"][0]["owners"], json!(["alice"]));
    let wrapped: Value = fake.get(path, &[]).await.json().await.unwrap();
    assert_eq!(wrapped["encoding"], "base64");
    let decoded = STANDARD
        .decode(wrapped["content"].as_str().unwrap().replace('\n', ""))
        .unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&decoded).unwrap(), policy);
    assert_eq!(
        fake.get(
            "/repos/acme/review-policies/contents/policies/missing.json",
            &[]
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn pull_request_files_page_for_q4() {
    let fake = spawn().await;
    let first = fake
        .get(
            "/repos/acme/platform/pulls/1203/files",
            &[("per_page", "2")],
        )
        .await;
    assert!(
        first.headers()[LINK]
            .to_str()
            .unwrap()
            .contains("page=2>; rel=\"next\"")
    );
    let files: Value = first.json().await.unwrap();
    assert_eq!(files[0]["filename"], "packages/sync/src/header_sync.rs");
    let second: Value = fake
        .get(
            "/repos/acme/platform/pulls/1203/files",
            &[("per_page", "2"), ("page", "2")],
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(second.as_array().unwrap().len(), 1);
    let none: Value = fake
        .get("/repos/acme/docs-site/pulls/77/files", &[])
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(none, json!([]));
    assert_eq!(
        fake.get("/repos/acme/platform/pulls/4040/files", &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn user_events_page_up_to_300_for_q14() {
    let fake = spawn().await;
    let events: Value = fake
        .get("/users/alice-helper/events", &[("per_page", "100")])
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(events.as_array().unwrap().len(), 8);
    assert_eq!(events[0]["type"], "PullRequestEvent");
    let beyond = fake
        .get(
            "/users/alice-helper/events",
            &[("per_page", "100"), ("page", "4")],
        )
        .await;
    assert_eq!(beyond.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        fake.get("/users/nobody-here/events", &[]).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn credentials_and_user_agent_are_checked() {
    let fake = spawn().await;
    let rotated = fake
        .get_with(
            "/search/issues",
            &[("q", "is:pr author:alice")],
            Some("not-a-token"),
        )
        .await;
    assert_eq!(rotated.status(), StatusCode::UNAUTHORIZED);
    let no_agent = client()
        .get(format!("{}/repos/acme/platform/releases", fake.base))
        .send()
        .await
        .unwrap();
    assert_eq!(no_agent.status(), StatusCode::FORBIDDEN);
    let anonymous = client()
        .post(format!("{}/graphql", fake.base))
        .header(USER_AGENT, "smart-review")
        .json(&json!({"query": pull_batch(1)}))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let statuses: Vec<Value> = fake
        .requests()
        .await
        .iter()
        .map(|r| r["status"].clone())
        .collect();
    assert_eq!(
        statuses,
        vec![json!(401), json!(403), json!(401)],
        "refused requests are logged too"
    );
}

#[tokio::test(start_paused = true)]
async fn request_log_records_order_time_queries_and_batch_sizes() {
    let fake = spawn().await;
    fake.search("is:pr is:open archived:false author:alice")
        .await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    fake.search("is:pr is:open archived:false assignee:alice")
        .await;
    fake.graphql(&pull_batch(4)).await;
    let log = fake.requests().await;
    assert_eq!(log.len(), 3);
    assert_eq!(log[0]["seq"], 1);
    assert_eq!(log[0]["on"], "search");
    assert_eq!(log[0]["method"], "GET");
    assert_eq!(log[0]["path"], "/search/issues");
    assert_eq!(log[0]["q"], "is:pr is:open archived:false author:alice");
    let gap = log[1]["at_ms"].as_u64().unwrap() - log[0]["at_ms"].as_u64().unwrap();
    assert!(gap >= 3000, "pacing is measured on tokio's clock: {gap} ms");
    assert_eq!(log[2]["on"], "graphql");
    assert_eq!(log[2]["lookups"], 4);
    assert_eq!(log[2]["status"], 200);
}

#[tokio::test]
async fn without_the_dashboard_world_its_endpoints_are_absent() {
    let base = spawn_with(config(None)).await;
    let token = sign_in(&base).await;
    let search = client()
        .get(format!("{base}/search/issues?q=is:pr"))
        .header(USER_AGENT, "t")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(search.status(), StatusCode::NOT_FOUND);
    let log = client()
        .get(format!("{base}/_fake/dashboard/requests"))
        .header(USER_AGENT, "t")
        .send()
        .await
        .unwrap();
    assert_eq!(log.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_missing_or_inconsistent_world_refuses_to_start() {
    let missing = fake_github::router(&config(Some(repo_root().join("fixtures/dashboard/absent"))));
    assert!(matches!(missing, Err(StartError::FixtureDirMissing { .. })));
    let dir = tempfile::tempdir().unwrap();
    for file in [
        "accounts.json",
        "repositories.json",
        "issues.json",
        "rest.json",
    ] {
        std::fs::copy(world_dir().join(file), dir.path().join(file)).unwrap();
    }
    std::fs::write(
        dir.path().join("pull-requests.json"),
        r#"[{"repository": {"nameWithOwner": "acme/unknown"}, "number": 1}]"#,
    )
    .unwrap();
    let inconsistent = fake_github::router(&config(Some(dir.path().to_path_buf())));
    assert!(matches!(
        inconsistent,
        Err(StartError::BadRecording {
            field: "repository.nameWithOwner",
            ..
        })
    ));
}

#[tokio::test]
async fn invalid_documents_are_refused_as_a_whole() {
    let fake = spawn().await;
    let pull = |selection: &str| {
        format!(
            "{{ rateLimit {{ cost }} p: repository(owner: \"acme\", name: \"platform\") {{ {selection} }} }}"
        )
    };
    let typo = fake
        .graphql(&pull("pullRequest(number: 1203) { titel }"))
        .await;
    assert_eq!(
        refused(&typo),
        vec!["Field 'titel' doesn't exist on type 'PullRequest'"]
    );
    let misnamed = fake
        .graphql_raw(
            "query($o: String!) { a0: repository(owner: $o, name: \"platform\") { id } }",
            json!({"owner": "acme"}),
        )
        .await;
    assert_eq!(
        refused(&misnamed.json().await.unwrap()),
        vec!["Variable $o of type String! was provided invalid value"]
    );
    let text_number = fake
        .graphql(&pull("pullRequest(number: \"1203\") { title }"))
        .await;
    assert_eq!(
        refused(&text_number),
        vec![
            "Argument 'number' on Field 'pullRequest' has an invalid value (\"1203\"). Expected type 'Int!'."
        ]
    );
    let missing = fake.graphql(&pull("pullRequest { title }")).await;
    assert_eq!(
        refused(&missing),
        vec!["Field 'pullRequest' is missing required arguments: number"]
    );
    let labelled = fake
        .graphql(&pull(
            "pullRequests(first: 5, labels: [\"bug\"]) { nodes { number } }",
        ))
        .await;
    assert!(
        refused(&labelled)[0].starts_with("Field 'pullRequests' doesn't accept argument 'labels'")
    );
    let bad_order = fake
        .graphql(&pull(
            "pullRequests(first: 5, orderBy: {field: UPDATED_AT}) { nodes { number } }",
        ))
        .await;
    assert!(
        !refused(&bad_order).is_empty(),
        "an order needs a direction"
    );
    let page_info = fake
        .graphql(&pull("pullRequests(first: 5) { pageInfo { endcursor } }"))
        .await;
    assert_eq!(
        refused(&page_info),
        vec!["Field 'endcursor' doesn't exist on type 'PageInfo'"]
    );
    let scalar_args = fake
        .graphql(&pull("pullRequest(number: 1203) { title(first: 1) }"))
        .await;
    assert!(refused(&scalar_args)[0].starts_with("Field 'title' doesn't accept argument 'first'"));
    let timeline = fake
        .graphql(&pull(
            "pullRequest(number: 1203) { a: timelineItems(last: 50, itemTypes: [REVIEW_REQUESTED_EVENT]) { totalCount }
               b: timelineItems(last: 50, itemTypes: [PULL_REQUEST_COMMIT]) { totalCount } }",
        ))
        .await;
    assert_eq!(timeline["data"]["p"]["pullRequest"]["a"]["totalCount"], 2);
    assert_eq!(
        timeline["data"]["p"]["pullRequest"]["b"]["totalCount"], 0,
        "itemTypes filters"
    );
}

#[tokio::test]
async fn only_v1_documents_reach_the_recordings() {
    let fake = spawn().await;
    let single = fake
        .graphql(
            "query { repository(owner: \"acme\", name: \"platform\") { pullRequest(number: 1203) {
               title reviewThreads(first: 100) { nodes { isResolved } } } } }",
        )
        .await;
    let pull = &single["data"]["repository"]["pullRequest"];
    assert_eq!(
        pull["title"],
        "fix(sync): retry header fetch after peer timeout"
    );
    assert_eq!(
        pull["reviewThreads"]["nodes"],
        json!([]),
        "not the recorded pull request's threads"
    );
    let unparsable = fake
        .graphql("{ rateLimit { cost } a: repository(owner: \"acme\" ")
        .await;
    assert!(refused(&unparsable)[0].starts_with("Parse error"));
    let log = fake.requests().await;
    assert_eq!(log.len(), 2);
    assert_eq!(log[0]["rate_limit_selected"], false);
    assert_eq!(log[1]["rate_limit_selected"], false);
    fake.graphql(&pull_batch(1)).await;
    assert_eq!(fake.requests().await[2]["rate_limit_selected"], true);
}

#[tokio::test]
async fn deep_nesting_fragment_loops_and_fan_out_are_refused() {
    let fake = spawn().await;
    let deep = format!(
        "{{ r: repository(owner: \"acme\", name: \"platform\") {}{} }}",
        "{ a ".repeat(50_000),
        "}".repeat(50_000)
    );
    let body = fake.graphql(&deep).await;
    assert!(refused(&body)[0].starts_with("Parse error"));
    let looped = fake
        .graphql(
            "{ ...A } fragment A on Query { rateLimit { cost } ...B } fragment B on Query { ...A }",
        )
        .await;
    assert_eq!(
        refused(&looped),
        vec!["Fragment A contains an infinite loop"]
    );
    let mut fan_out = String::from("{ ...F0 } fragment F15 on Query { rateLimit { cost } }");
    for level in 0..15 {
        fan_out.push_str(&format!(
            " fragment F{level} on Query {{ {} }}",
            format!("...F{} ", level + 1).repeat(8)
        ));
    }
    let body = fake.graphql(&fan_out).await;
    assert!(refused(&body)[0].contains("more than"));
}

#[tokio::test]
async fn primary_limit_status_is_configurable_and_graphql_needs_a_user_agent() {
    let fake = spawn().await;
    fake.faults(json!([
        {"on": "rest", "answer": {"kind": "primary_rate_limit", "reset": 1_795_000_000, "status": 429}},
        {"on": "graphql", "answer": {"kind": "primary_rate_limit", "reset": 1_795_000_000, "status": 403}},
    ]))
    .await;
    let rest = fake.get("/repos/acme/platform/milestones", &[]).await;
    assert_eq!(rest.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(rest.headers()["x-ratelimit-remaining"], "0");
    let graphql = fake.graphql_raw(&pull_batch(1), Value::Null).await;
    assert_eq!(graphql.status(), StatusCode::FORBIDDEN);
    assert_eq!(graphql.headers()["x-ratelimit-remaining"], "0");
    let bad = fake
        .faults(json!([{"on": "rest", "answer": {"kind": "primary_rate_limit", "reset": 1, "status": 500}}]))
        .await;
    assert_eq!(bad, StatusCode::BAD_REQUEST);
    let anonymous_agent = client()
        .post(format!("{}/graphql", fake.base))
        .header(AUTHORIZATION, format!("Bearer {}", fake.token))
        .json(&json!({"query": pull_batch(1)}))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous_agent.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn nested_typos_are_refused_whatever_the_data() {
    let fake = spawn().await;
    let pull = |number: u32, selection: &str| {
        format!(
            "{{ rateLimit {{ cost }} p0: repository(owner: \"acme\", name: \"platform\") {{ pullRequest(number: {number}) {{ {selection} }} }} }}"
        )
    };
    for (number, selection) in [
        (1203, "milestone { titel }"),
        (1203, "labels(first: 5) { nodes { nmae } }"),
        (
            1203,
            "commits(last: 1) { nodes { commit { statusCheckRollup { stat } } } }",
        ),
        (1203, "baseRepository { defaultBranchRef { nam } }"),
        (1203, "reviews(last: 5) { nodes { stat } }"),
        (1203, "reviewThreads(first: 5) { nodes { isResolvd } }"),
        (1203, "labels(first: 5) { edges { nod { name } } }"),
        (1203, "title { length }"),
        (1203, "milestone"),
        (4040, "titel"),
    ] {
        let body = fake.graphql(&pull(number, selection)).await;
        assert_eq!(refused(&body).len(), 1, "{selection}: {body}");
    }
    let hidden = fake
        .graphql("{ rateLimit { cost } a: repository(owner: \"acme\", name: \"secret\") { pullRequest(number: 1) { titel } } }")
        .await;
    refused(&hidden);
    let valid = fake
        .graphql(&pull(1203, "milestone { title } labels(first: 5) { totalCount edges { cursor node { name } } pageInfo { hasNextPage } }"))
        .await;
    assert_eq!(
        valid["data"]["p0"]["pullRequest"]["milestone"]["title"],
        "v2.4"
    );
    assert_eq!(
        valid["data"]["p0"]["pullRequest"]["labels"]["edges"][0]["node"]["name"],
        "bug"
    );
}

#[tokio::test]
async fn empty_fragment_fan_out_is_refused_and_tokens_are_checked_first() {
    let fake = spawn().await;
    let mut fan_out = String::from("{ ...F0 } fragment F12 on Query { }");
    for level in 0..12 {
        fan_out.push_str(&format!(
            " fragment F{level} on Query {{ {} }}",
            format!("...F{} ", level + 1).repeat(8)
        ));
    }
    let timed = || {
        Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap()
    };
    let anonymous = timed()
        .post(format!("{}/graphql", fake.base))
        .header(USER_AGENT, "smart-review")
        .json(&json!({"query": fan_out}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        anonymous.status(),
        StatusCode::UNAUTHORIZED,
        "no parsing before the token check"
    );
    let body: Value = timed()
        .post(format!("{}/graphql", fake.base))
        .header(USER_AGENT, "smart-review")
        .header(AUTHORIZATION, format!("Bearer {}", fake.token))
        .json(&json!({"query": fan_out}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        refused(&body)[0].starts_with("Parse error"),
        "an empty selection set is not GraphQL"
    );
    let mut spreads_only = String::from("{ ...F0 } fragment F12 on Query { rateLimit { cost } }");
    for level in 0..12 {
        spreads_only.push_str(&format!(
            " fragment F{level} on Query {{ {} }}",
            format!("...F{} ", level + 1).repeat(8)
        ));
    }
    let body: Value = timed()
        .post(format!("{}/graphql", fake.base))
        .header(USER_AGENT, "smart-review")
        .header(AUTHORIZATION, format!("Bearer {}", fake.token))
        .json(&json!({"query": spreads_only}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(refused(&body)[0].contains("more than"));
}

#[tokio::test]
async fn pull_requests_are_ordered_by_update_time_newest_first() {
    let fake = spawn().await;
    let body = fake
        .graphql(
            "{ a0: repository(owner: \"acme\", name: \"consensus\") { nameWithOwner pullRequests(first: 2, \
             orderBy: {field: UPDATED_AT, direction: DESC}) { pageInfo { hasNextPage endCursor } nodes { number \
             updatedAt author { login } } } } }",
        )
        .await;
    let connection = &body["data"]["a0"]["pullRequests"];
    let numbers: Vec<u64> = connection["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["number"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers, [1536, 1541], "{body}");
    assert_eq!(connection["pageInfo"]["hasNextPage"], true);
    assert_eq!(body["data"]["a0"]["nameWithOwner"], "acme/consensus");
}

/// The fixture node `repository#number` of `file` (unshifted).
fn fixture_node(file: &str, repository: &str, number: u64) -> Value {
    let nodes: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(world_dir().join(file)).unwrap()).unwrap();
    nodes
        .into_iter()
        .find(|n| n["repository"]["nameWithOwner"] == repository && n["number"] == number)
        .unwrap()
}

fn at(text: &str) -> jiff::Timestamp {
    text.parse().unwrap()
}

#[tokio::test]
async fn world_dates_are_shifted_so_that_world_at_is_now() {
    let world_at = at("2026-11-18T19:30:00Z");
    let base = spawn_with(Config {
        dashboard_world_at: Some(world_at),
        ..config(Some(world_dir()))
    })
    .await;
    let fake = Fake {
        token: sign_in(&base).await,
        base,
    };
    let clock: Value = client()
        .get(format!("{}/_fake/dashboard/clock", fake.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(clock["world_at"], "2026-11-18T19:30:00Z");
    let shift = clock["shift_seconds"].as_i64().unwrap();
    let expected = jiff::Timestamp::now().as_second() - world_at.as_second();
    assert!((expected - shift).abs() < 60, "{shift} vs {expected}");

    // Every date moves by the same whole number of seconds: a pull request's, and a history search window.
    let raw = fixture_node("pull-requests.json", "acme/platform", 1176);
    let (_, body) = fake
        .search("is:pr is:open repo:acme/platform author:alice")
        .await;
    let served = at(body["items"][0]["updated_at"].as_str().unwrap());
    let recorded = at(raw["updatedAt"].as_str().unwrap());
    assert_eq!(served.as_second() - recorded.as_second(), shift);
    let window = (at("2026-08-26T00:00:00Z") + jiff::SignedDuration::from_secs(shift))
        .strftime("%Y-%m-%d")
        .to_string();
    let created = fake
        .found(&format!("is:pr author:alice created:>={window}"))
        .await;
    assert!(created.contains("acme/platform#1190") && !created.contains("acme/platform#1100"));
}

#[tokio::test]
async fn world_dates_stay_as_written_without_world_at() {
    let fake = spawn().await;
    let clock: Value = client()
        .get(format!("{}/_fake/dashboard/clock", fake.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(clock, json!({"world_at": null, "shift_seconds": 0}));
    let raw = fixture_node("pull-requests.json", "acme/platform", 1176);
    let (_, body) = fake
        .search("is:pr is:open repo:acme/platform author:alice")
        .await;
    assert_eq!(
        at(body["items"][0]["updated_at"].as_str().unwrap()),
        at(raw["updatedAt"].as_str().unwrap())
    );
}
