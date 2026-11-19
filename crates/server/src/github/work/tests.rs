//! The scan client against the real fake GitHub (`tools/fake-github` with `fixtures/dashboard/github`),
//! in-process: its request log runs on tokio's clock, so paused time asserts pacing and retry waits.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use contracts::common::RepoRef;
use jiff::civil::Date;
use reqwest::header::{ACCEPT, LOCATION};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;
use wiremock::matchers::{method, path, query_param_contains};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::enumerate::mentions;
use super::*;
use crate::auth::UserTokenError;
use crate::test_support::logs::LogCapture;

const VERIFIER: &str = "work-client-verifier-0123456789-0123456789-0123456789";
const STALE: &str = "ghu_stale_token_of_the_test";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Starts the fake with the dashboard world; returns its base URL.
async fn spawn_fake() -> String {
    spawn_fake_in(repo_root().join("fixtures/dashboard/github")).await
}

async fn spawn_fake_in(fixture: PathBuf) -> String {
    let config = fake_github::Config {
        fixture_dir: repo_root().join("fixtures/pr-482/github"),
        app_url: Url::parse("http://app.test:3000").unwrap(),
        user_login: None,
        user_id: None,
        app_id: None,
        app_public_key_file: None,
        dashboard_fixture_dir: Some(fixture),
        dashboard_world_at: None,
    };
    let router = fake_github::router(&config).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// The fake's OAuth web flow; returns the access token.
async fn sign_in(base: &str) -> String {
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()));
    let mut authorize = Url::parse(&format!("{base}/login/oauth/authorize")).unwrap();
    authorize.query_pairs_mut().extend_pairs([
        ("client_id", "fixture-app-client-id"),
        ("redirect_uri", "http://app.test:3000/auth/github/callback"),
        ("state", "s"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
    ]);
    let response = http().get(authorize).send().await.unwrap();
    let location = Url::parse(response.headers()[LOCATION].to_str().unwrap()).unwrap();
    let code = location
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();
    let body: Value = http()
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

/// A token source: `current` until rejected, then `refreshed` (if any).
#[derive(Clone, Default)]
struct TestTokens(Arc<TokenState>);

#[derive(Default)]
struct TokenState {
    current: Mutex<String>,
    refreshed: Mutex<Option<String>>,
    rejected: AtomicUsize,
}

impl TestTokens {
    fn new(current: &str, refreshed: Option<&str>) -> Self {
        let tokens = Self::default();
        *tokens.0.current.lock().unwrap() = current.to_owned();
        *tokens.0.refreshed.lock().unwrap() = refreshed.map(str::to_owned);
        tokens
    }

    fn rejections(&self) -> usize {
        self.0.rejected.load(Ordering::SeqCst)
    }
}

impl TokenSource for TestTokens {
    async fn token(&self) -> Result<SecretString, UserTokenError> {
        Ok(SecretString::from(self.0.current.lock().unwrap().clone()))
    }

    async fn rejected(&self, token: &SecretString) {
        self.0.rejected.fetch_add(1, Ordering::SeqCst);
        let mut current = self.0.current.lock().unwrap();
        if *current == token.expose_secret()
            && let Some(next) = self.0.refreshed.lock().unwrap().take()
        {
            *current = next;
        }
    }
}

struct Fake {
    base: String,
    token: String,
}

impl Fake {
    async fn start() -> Self {
        let base = spawn_fake().await;
        let token = sign_in(&base).await;
        Self { base, token }
    }

    fn client(&self) -> WorkClient<TestTokens> {
        self.client_with(TestTokens::new(&self.token, None))
    }

    fn client_with(&self, tokens: TestTokens) -> WorkClient<TestTokens> {
        WorkClient::new(Url::parse(&self.base).unwrap(), tokens).unwrap()
    }

    async fn faults(&self, faults: Value) {
        let status = http()
            .post(format!("{}/_fake/dashboard/faults", self.base))
            .json(&json!({ "faults": faults }))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, 204);
    }

    async fn requests(&self) -> Vec<Value> {
        http()
            .get(format!("{}/_fake/dashboard/requests", self.base))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn requests_on(&self, on: &str) -> Vec<Value> {
        self.requests()
            .await
            .into_iter()
            .filter(|r| r["on"] == on)
            .collect()
    }
}

fn item(text: &str) -> ItemRef {
    let (repo, number) = text.split_once('#').unwrap();
    ItemRef::parse(repo, number.parse().unwrap()).unwrap()
}

fn repo(text: &str) -> RepoRef {
    parse_repo(text).unwrap()
}

fn logins(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn at_ms(record: &Value) -> u64 {
    record["at_ms"].as_u64().unwrap()
}

#[tokio::test(start_paused = true)]
async fn search_is_paced_and_retried_on_secondary_limit() {
    let fake = Fake::start().await;
    fake.faults(json!([
        {"on": "search", "matching": "author:alice", "times": 1,
         "answer": {"kind": "secondary_rate_limit", "retry_after": 7}},
        {"on": "search", "matching": "assignee:alice", "times": 1,
         "answer": {"kind": "secondary_rate_limit"}},
        {"on": "search", "matching": "review-requested:alice", "times": 2,
         "answer": {"kind": "secondary_rate_limit", "status": 429}},
    ]))
    .await;

    let found = fake
        .client()
        .search_pull_requests(&logins(&["alice"]))
        .await
        .unwrap();

    let searches = fake.requests_on("search").await;
    let faulted: Vec<bool> = searches.iter().map(|r| r["fault"].is_string()).collect();
    assert_eq!(
        faulted,
        [true, false, true, false, true, true, false, false]
    );
    let gaps: Vec<u64> = searches
        .windows(2)
        .map(|pair| at_ms(&pair[1]) - at_ms(&pair[0]))
        .collect();
    assert!(gaps.iter().all(|gap| *gap >= 3_000), "{gaps:?}");
    // retry-after 7 s; then the ladder: 60 s, and 60 s then 120 s for the twice-limited search.
    assert!((7_000..60_000).contains(&gaps[0]), "{gaps:?}");
    assert!(
        gaps[2] >= 60_000 && gaps[4] >= 60_000 && gaps[5] >= 120_000,
        "{gaps:?}"
    );

    assert!(found.unsearchable.is_empty() && found.notes.is_empty());
    assert_eq!(found.answered, 4, "a retried search counts once");
    let prs = &found.candidates.pull_requests;
    let relations = |key: &str| prs[&item(key)].relations["alice"].clone();
    assert_eq!(
        relations("acme/wallet-ui#311"),
        BTreeSet::from([PrRelation::Authored])
    );
    assert_eq!(
        relations("acme/ops-runbooks#14"),
        BTreeSet::from([PrRelation::Assigned])
    );
    assert_eq!(
        relations("acme/platform#1203"),
        BTreeSet::from([PrRelation::ReviewRequested])
    );
    assert!(!prs.contains_key(&item("acme/legacy-wallet#9")), "archived");
    assert!(!prs.contains_key(&item("acme/platform#1190")), "merged");
}

#[tokio::test(start_paused = true)]
async fn third_secondary_hit_fails_the_search() {
    let fake = Fake::start().await;
    fake.faults(json!([{"on": "search", "times": 3,
        "answer": {"kind": "secondary_rate_limit"}}]))
        .await;

    let error = fake
        .client()
        .search_pull_requests(&logins(&["alice"]))
        .await
        .unwrap_err();

    assert!(
        matches!(error, WorkError::SecondaryRateLimit { .. }),
        "{error:?}"
    );
    assert!(error.ends_refresh());
    assert_eq!(fake.requests_on("search").await.len(), 3);
}

#[tokio::test(start_paused = true)]
async fn primary_limit_fails_with_retry_at() {
    let fake = Fake::start().await;
    let reset = 1_795_000_000;
    fake.faults(json!([
        {"on": "search", "answer": {"kind": "primary_rate_limit", "reset": reset}},
        {"on": "graphql", "answer": {"kind": "primary_rate_limit", "reset": reset}},
    ]))
    .await;
    let client = fake.client();

    let search = client.search_pull_requests(&logins(&["alice"])).await;
    let details = client.pull_requests(&[item("acme/platform#1203")]).await;

    let expected = Timestamp::from_second(reset);
    for error in [search.unwrap_err(), details.unwrap_err()] {
        assert!(
            matches!(error, WorkError::PrimaryRateLimit { reset } if reset == expected),
            "{error:?}"
        );
        assert!(error.ends_refresh());
    }
    assert_eq!(
        fake.requests().await.len(),
        2,
        "no retry after a primary limit"
    );
}

#[tokio::test(start_paused = true)]
async fn unsearchable_login_falls_back_to_enumeration() {
    let fake = Fake::start().await;
    let client = fake.client();

    let searched = client
        .search_pull_requests(&logins(&["alice", "alice-helper"]))
        .await
        .unwrap();
    assert_eq!(searched.unsearchable, ["alice-helper"]);
    let helper_searches = fake
        .requests_on("search")
        .await
        .into_iter()
        .filter(|r| r["q"].as_str().unwrap().contains("alice-helper"))
        .count();
    assert_eq!(
        helper_searches, 1,
        "the first refusal ends the login's searches"
    );

    let enumerated = client
        .enumerate_owners(&logins(&["acme"]), &searched.unsearchable)
        .await
        .unwrap();

    assert!(enumerated.notes.is_empty(), "{:?}", enumerated.notes);
    let found = &enumerated.candidates;
    assert_eq!(
        found
            .pull_requests
            .iter()
            .filter(|(_, pr)| pr.ranked())
            .map(|(item, _)| item.clone())
            .collect::<Vec<_>>(),
        [item("acme/consensus#1541")]
    );
    assert_eq!(
        found.pull_requests[&item("acme/consensus#1541")].relations["alice-helper"],
        BTreeSet::from([PrRelation::Authored, PrRelation::Mentioned])
    );
    assert!(
        found.pull_requests.values().all(|pr| pr.mentioned()),
        "enumerated PRs are potential mentions until their bounded read"
    );
    assert_eq!(
        found.issues.keys().cloned().collect::<Vec<_>>(),
        [item("acme/consensus#1525")]
    );
    assert_eq!(
        found.issues[&item("acme/consensus#1525")].relations,
        BTreeSet::from([IssueRelation::Assigned])
    );
    assert!(
        found.issues[&item("acme/consensus#1525")]
            .discussion
            .is_some(),
        "enumeration reuses the complete discussion"
    );
    let graphql = fake.requests_on("graphql").await;
    assert!(graphql.iter().all(|r| r["rate_limit_selected"] == true));
}

#[tokio::test(start_paused = true)]
async fn issue_search_merges_relations_of_watched_repositories_and_logins() {
    let fake = Fake::start().await;

    let found = fake
        .client()
        .search_issues(&[repo("acme/platform")], &logins(&["alice"]))
        .await
        .unwrap();

    assert!(found.notes.is_empty());
    assert_eq!(
        found.answered, 3,
        "repository, assignee and mention searches"
    );
    let issues = &found.candidates.issues;
    let relations = |key: &str| issues[&item(key)].relations.clone();
    assert_eq!(
        relations("acme/platform#1187"),
        BTreeSet::from([
            IssueRelation::RepoOpen,
            IssueRelation::Assigned,
            IssueRelation::Mentioned
        ])
    );
    assert_eq!(
        relations("acme/platform#1215"),
        BTreeSet::from([IssueRelation::RepoOpen])
    );
    assert_eq!(
        relations("acme/wallet-ui#305"),
        BTreeSet::from([IssueRelation::Mentioned])
    );
    assert!(!issues.contains_key(&item("acme/platform#1150")), "closed");
    let platform = &issues[&item("acme/platform#1187")];
    assert_eq!(platform.milestone.as_ref().unwrap().title, "v2.4");
    assert_eq!(platform.assignees, ["alice"]);
}

#[tokio::test(start_paused = true)]
async fn pr_details_are_batched_by_ten() {
    let fake = Fake::start().await;
    let world: Value = serde_json::from_str(
        &std::fs::read_to_string(repo_root().join("fixtures/dashboard/github/pull-requests.json"))
            .unwrap(),
    )
    .unwrap();
    let mut pulls: Vec<ItemRef> = world
        .as_array()
        .unwrap()
        .iter()
        .map(|pr| {
            ItemRef::parse(
                pr["repository"]["nameWithOwner"].as_str().unwrap(),
                u32::try_from(pr["number"].as_u64().unwrap()).unwrap(),
            )
            .unwrap()
        })
        .collect();
    assert_eq!(pulls.len(), 26);
    pulls.extend([item("acme/platform#9999"), item("acme/secret#1")]);

    let fetched = fake.client().pull_requests(&pulls).await.unwrap();

    let graphql = fake.requests_on("graphql").await;
    let mut sizes: Vec<u64> = graphql
        .iter()
        .map(|r| r["pull_requests"].as_u64().unwrap())
        .collect();
    sizes.sort_unstable();
    assert_eq!(sizes, [8, 10, 10]);
    assert!(graphql.iter().all(|r| r["rate_limit_selected"] == true));
    assert!(sizes.iter().all(|size| *size <= MAX_PR_BATCH as u64));

    assert_eq!(fetched.found.len(), 26);
    let noted: Vec<_> = fetched.notes.iter().map(|n| n.item.clone()).collect();
    assert_eq!(
        noted,
        [
            item("acme/platform#9999").pr_key(),
            item("acme/secret#1").pr_key()
        ]
    );
    let fix = &fetched.found[&item("acme/platform#1203")];
    assert_eq!(fix.closing_issues_references.nodes[0].number, 1187);
    let rollup = fix.commits.nodes[0]
        .commit
        .status_check_rollup
        .as_ref()
        .unwrap();
    assert_eq!(rollup.state, "SUCCESS");
    assert_eq!(rollup.contexts.nodes[0].workflow.as_deref(), Some("ci"));
    assert!(fix.review_requests.nodes.iter().any(|r| {
        r.requested_reviewer
            .as_ref()
            .and_then(|r| r.login.as_deref())
            == Some("alice")
    }));
    assert_eq!(
        fetched.found[&item("acme/wallet-ui#298")].base_ref_name,
        "feat/pending-withdrawals"
    );
    let runbooks = &fetched.found[&item("acme/ops-runbooks#14")];
    assert!(runbooks.base_repository.as_ref().unwrap().is_private);
    assert!(fetched.found[&item("acme/node-client#490")].is_draft);
}

#[tokio::test(start_paused = true)]
async fn failed_batch_retries_as_two_halves() {
    let fake = Fake::start().await;
    let pulls: Vec<ItemRef> = [
        "acme/platform#1203",
        "acme/platform#1199",
        "acme/platform#1176",
        "acme/wallet-ui#320",
        "acme/wallet-ui#311",
        "acme/wallet-ui#298",
        "acme/node-client#482",
        "acme/node-client#490",
        "acme/node-client#476",
        "acme/docs-site#77",
    ]
    .map(item)
    .to_vec();
    fake.faults(json!([{"on": "graphql", "answer": {"kind": "server_error"}}]))
        .await;

    let fetched = fake.client().pull_requests(&pulls).await.unwrap();

    let sizes: Vec<u64> = fake
        .requests_on("graphql")
        .await
        .iter()
        .map(|r| r["pull_requests"].as_u64().unwrap())
        .collect();
    assert_eq!(sizes, [10, 5, 5]);
    assert_eq!(fetched.found.len(), 10);
    assert!(fetched.notes.is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_half_failing_again_is_left_out_with_notes() {
    let fake = Fake::start().await;
    let pulls: Vec<ItemRef> = [
        "acme/platform#1203",
        "acme/platform#1199",
        "acme/wallet-ui#320",
        "acme/wallet-ui#311",
    ]
    .map(item)
    .to_vec();
    fake.faults(json!([{"on": "graphql", "times": 2,
        "answer": {"kind": "server_error", "status": 504, "html": false}}]))
        .await;

    let fetched = fake.client().pull_requests(&pulls).await.unwrap();

    assert_eq!(
        fake.requests_on("graphql").await.len(),
        3,
        "batch, then each half once"
    );
    assert_eq!(
        fetched.found.keys().cloned().collect::<Vec<_>>(),
        [item("acme/wallet-ui#311"), item("acme/wallet-ui#320")]
    );
    let noted: Vec<_> = fetched.notes.iter().map(|n| n.item.clone()).collect();
    assert_eq!(
        noted,
        [
            item("acme/platform#1203").pr_key(),
            item("acme/platform#1199").pr_key()
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn blocker_relations_from_one_ref_listing_per_repository() {
    let fake = Fake::start().await;
    let repos = [
        "acme/wallet-ui",
        "acme/consensus",
        "acme/node-client",
        "acme/docs-site",
        "acme/secret",
    ]
    .map(repo);

    let listing = fake.client().open_pr_refs(&repos).await.unwrap();

    let graphql = fake.requests_on("graphql").await;
    assert_eq!(graphql.len(), 1);
    assert_eq!(graphql[0]["lookups"], 5);
    let blockers = |key: &str| listing.blockers(&item(key)).unwrap();
    assert_eq!(blockers("acme/wallet-ui#311").dependents, [298]);
    assert_eq!(blockers("acme/wallet-ui#298").stacked_on, [311]);
    assert_eq!(blockers("acme/consensus#1541").stacked_on, [1534]);
    assert_eq!(blockers("acme/consensus#1534").dependents, [1541]);
    assert_eq!(blockers("acme/node-client#482"), Blockers::default());
    // A fork's `main` is no branch of the base repository: nothing there is based on it or stacked under it.
    assert_eq!(blockers("acme/docs-site#80"), Blockers::default());
    assert_eq!(blockers("acme/docs-site#77"), Blockers::default());
    assert_eq!(
        listing.blockers(&item("acme/secret#1")),
        None,
        "listing failed"
    );
    assert_eq!(listing.notes.len(), 1);
    assert!(listing.notes[0].text.contains("acme/secret"));
}

#[tokio::test(start_paused = true)]
async fn invisible_repository_is_reported() {
    let fake = Fake::start().await;

    let reach = fake
        .client()
        .reach(
            &[
                repo("acme/platform"),
                repo("acme/secret"),
                repo("acme/ops-runbooks"),
            ],
            &logins(&["acme", "ghost-org"]),
        )
        .await
        .unwrap();

    assert_eq!(reach.invisible_repositories, [repo("acme/secret")]);
    assert_eq!(reach.invisible_owners, ["ghost-org"]);
    assert_eq!(
        reach.private_repositories,
        [repo("acme/ops-runbooks")],
        "privacy in the same query"
    );
    let texts: Vec<&str> = reach.notes.iter().map(|n| n.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Not visible to your GitHub token: acme/secret — install the GitHub App there or grant it access",
            "Not visible to your GitHub token: ghost-org — install the GitHub App there or grant it access",
        ]
    );
    assert_eq!(fake.requests_on("graphql").await.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn rejected_token_is_refreshed_once() {
    let fake = Fake::start().await;
    let tokens = TestTokens::new(STALE, Some(&fake.token));
    let client = fake.client_with(tokens.clone());
    let pulls: Vec<ItemRef> = (0..20)
        .map(|n| item(&format!("acme/platform#{}", 1190 + n)))
        .collect();

    let fetched = client.pull_requests(&pulls).await.unwrap();
    let search = client
        .search_pull_requests(&logins(&["alice"]))
        .await
        .unwrap();

    assert!(fetched.found.contains_key(&item("acme/platform#1203")));
    assert!(!search.candidates.pull_requests.is_empty());
    assert_eq!(tokens.rejections(), 1, "concurrent 401s refresh once");
}

#[tokio::test(start_paused = true)]
async fn token_rejected_after_refresh_fails() {
    let fake = Fake::start().await;
    let tokens = TestTokens::new(STALE, None);

    let error = fake
        .client_with(tokens.clone())
        .search_pull_requests(&logins(&["alice"]))
        .await
        .unwrap_err();

    assert!(matches!(error, WorkError::Unauthorized), "{error:?}");
    assert!(error.ends_refresh());
    assert_eq!(tokens.rejections(), 1);
}

#[tokio::test(start_paused = true)]
async fn rest_reads_answer_q3_q4_q7_q8_q14() {
    let fake = Fake::start().await;
    let mut client = fake.client();
    client.page_size = 2;
    let platform = repo("acme/platform");

    let policy = client
        .file_contents(&repo("acme/review-policies"), "policies/platform.json")
        .await
        .unwrap();
    let files = client
        .pull_files(&item("acme/platform#1203"))
        .await
        .unwrap();
    let latest = client.latest_release(&platform).await.unwrap();
    let none = client
        .latest_release(&repo("acme/docs-site"))
        .await
        .unwrap();
    let releases = client.releases(&platform).await.unwrap();
    let milestones = client.milestones(&platform).await.unwrap();
    let events = client.user_events("alice-helper").await.unwrap();
    let mut refused = Vec::new();
    for path in [
        "../secret/policy.json",
        "policies/%2e%2e/%2e%2e/%2e%2e/%2e%2e/user/installations",
        "policies/.%2e/x.json",
        "policies\\..\\x.json",
        "./policies/platform.json",
        "policies//platform.json",
        "policies/plat form.json",
    ] {
        refused.push((path, client.file_contents(&platform, path).await));
    }

    let policy: Value = serde_json::from_str(&policy).unwrap();
    assert_eq!(policy["areas"][0]["paths"][0], "packages/sync/");
    assert_eq!(files.len(), 3);
    assert_eq!(files[0].filename, "packages/sync/src/header_sync.rs");
    assert_eq!(latest.unwrap().tag_name, "v2.3.0");
    assert_eq!(none, None);
    assert_eq!(releases.len(), 3);
    assert_eq!(
        milestones
            .iter()
            .map(|m| (m.title.as_str(), m.state.as_str()))
            .collect::<Vec<_>>(),
        [("v2.3", "closed"), ("v2.4", "open"), ("v3.0", "open")]
    );
    assert_eq!(events.reaches_back_to, None, "a short feed is read whole");
    let events = events.events;
    assert!(!events.is_empty());
    assert!(events.iter().all(|e| e.kind.starts_with("PullRequest")));
    assert!(events.iter().any(|e| e.item == item("acme/consensus#1541")));
    assert!(
        events.iter().any(|e| e.item == item("acme/consensus#1501")),
        "a `merged` event with a slim payload"
    );
    for (path, answer) in &refused {
        assert!(
            matches!(answer, Err(WorkError::NotFound)),
            "{path}: {answer:?}"
        );
    }
    let paths: Vec<String> = fake
        .requests_on("rest")
        .await
        .iter()
        .map(|r| r["path"].as_str().unwrap().to_owned())
        .collect();
    let read = |part: &str| paths.iter().filter(|p| p.contains(part)).count();
    assert_eq!(
        read("/contents/"),
        1,
        "only the valid policy path: {paths:?}"
    );
    assert_eq!(read("installations"), 0, "{paths:?}");
    let file_pages = fake
        .requests_on("rest")
        .await
        .into_iter()
        .filter(|r| r["path"].as_str().unwrap().ends_with("/pulls/1203/files"))
        .count();
    assert_eq!(file_pages, 2, "three files at two per page");
}

#[tokio::test(start_paused = true)]
async fn history_searches_and_details_answer_q9_q12_q13() {
    let fake = Fake::start().await;
    let client = fake.client();
    let since = Date::new(2026, 8, 26).unwrap();

    let history = client.search_history("alice", since).await.unwrap();
    let details = client
        .history_details(
            &[
                "acme/platform#1190",
                "acme/wallet-ui#300",
                "acme/platform#1195",
            ]
            .map(item),
            &logins(&["alice"]),
        )
        .await
        .unwrap();
    let milestones = client
        .issue_milestones(
            &[
                "acme/platform#1187",
                "acme/platform#1215",
                "acme/consensus#1520",
            ]
            .map(item),
        )
        .await
        .unwrap();

    assert!(history.opened.contains(&item("acme/platform#1190")));
    assert!(
        !history.opened.contains(&item("acme/platform#1100")),
        "outside the window"
    );
    assert_eq!(
        history.closed,
        BTreeSet::from([item("acme/platform#1190"), item("acme/wallet-ui#300")])
    );
    assert_eq!(
        history.reviewed,
        BTreeSet::from([item("acme/platform#1195"), item("acme/consensus#1510")])
    );
    let merged = &details.found[&item("acme/platform#1190")];
    assert!(merged.merged_at.is_some() && merged.author.as_deref() == Some("alice"));
    let closed = &details.found[&item("acme/wallet-ui#300")];
    assert!(closed.closed_at.is_some() && closed.merged_at.is_none());
    assert_eq!(
        details.found[&item("acme/platform#1195")].reviews["alice"].len(),
        2
    );
    let title = |key: &str| {
        milestones.found[&item(key)]
            .as_ref()
            .map(|m| m.title.clone())
    };
    assert_eq!(title("acme/platform#1187").as_deref(), Some("v2.4"));
    assert_eq!(title("acme/platform#1215"), None);
    assert_eq!(title("acme/consensus#1520").as_deref(), Some("1.9"));
}

#[tokio::test(start_paused = true)]
async fn work_client_sends_no_mutation_and_never_logs_the_token() {
    let capture = LogCapture::start();
    let fake = Fake::start().await;
    let client = fake.client_with(TestTokens::new(STALE, Some(&fake.token)));
    fake.faults(
        json!([{"on": "graphql", "answer": {"kind": "server_error"}},
        {"on": "rest", "answer": {"kind": "server_error", "status": 500, "html": false}}]),
    )
    .await;

    let refused = client.graphql("mutation { deleteRepository { id } }").await;
    let searched = client
        .search_pull_requests(&logins(&["alice", "alice-helper"]))
        .await
        .unwrap();
    client
        .search_issues(&[repo("acme/platform")], &logins(&["alice"]))
        .await
        .unwrap();
    client
        .enumerate_owners(&logins(&["acme"]), &searched.unsearchable)
        .await
        .unwrap();
    let pulls: Vec<ItemRef> = searched.candidates.pull_requests.keys().cloned().collect();
    client.pull_requests(&pulls).await.unwrap();
    client
        .open_pr_refs(&[repo("acme/wallet-ui")])
        .await
        .unwrap();
    client.reach(&[repo("acme/secret")], &[]).await.unwrap();
    let mut errors = Vec::new();
    errors.extend(client.pull_files(&item("acme/platform#1203")).await.err());
    errors.extend(client.user_events("alice-helper").await.err());
    errors.extend(
        fake.client_with(TestTokens::new(STALE, None))
            .latest_release(&repo("acme/platform"))
            .await
            .err(),
    );

    assert!(matches!(refused, Err(WorkError::NotAQuery)));
    let requests = fake.requests().await;
    assert!(
        requests
            .iter()
            .all(|r| r["method"] == "GET" || (r["method"] == "POST" && r["path"] == "/graphql")),
        "{requests:?}"
    );
    assert!(
        matches!(
            errors[..],
            [WorkError::ServerError(_), WorkError::Unauthorized]
        ),
        "{errors:?}"
    );
    let logged = capture.text();
    for secret in [fake.token.as_str(), STALE] {
        assert!(!logged.contains(secret), "token in the log");
        for error in &errors {
            assert!(
                !format!("{error:?} {error}").contains(secret),
                "token in an error"
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn retry_after_beyond_the_ladder_ends_the_search_at_once() {
    let fake = Fake::start().await;
    fake.faults(json!([{"on": "search",
        "answer": {"kind": "secondary_rate_limit", "retry_after": 86_400}}]))
        .await;
    let started = tokio::time::Instant::now();

    let error = fake
        .client()
        .search_pull_requests(&logins(&["alice"]))
        .await
        .unwrap_err();

    assert!(
        matches!(error, WorkError::SecondaryRateLimit { retry_after: Some(wait) }
            if wait == Duration::from_secs(86_400)),
        "{error:?}"
    );
    assert!(error.ends_refresh());
    assert!(started.elapsed() < Duration::from_secs(1), "no wait");
    assert_eq!(fake.requests_on("search").await.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn invalid_logins_and_repositories_never_reach_a_search() {
    let fake = Fake::start().await;
    let client = fake.client();
    let odd_repo = RepoRef {
        owner: "acme".to_owned(),
        name: "platform is:closed".to_owned(),
    };
    let since = Date::new(2026, 8, 26).unwrap();

    let prs = client
        .search_pull_requests(&logins(&["alice repo:acme/secret", "alice"]))
        .await
        .unwrap();
    let issues = client
        .search_issues(&[odd_repo], &logins(&["bob OR x"]))
        .await
        .unwrap();
    let history = client.search_history("x y", since).await.unwrap();

    let searches = fake.requests_on("search").await;
    assert_eq!(searches.len(), 4, "only alice's searches: {searches:?}");
    assert!(
        searches
            .iter()
            .all(|r| !r["q"].as_str().unwrap().contains("repo:"))
    );
    assert!(!prs.candidates.pull_requests.is_empty());
    let texts = |notes: &[contracts::api::dashboard::ScanNote]| {
        notes.iter().map(|n| n.text.clone()).collect::<Vec<_>>()
    };
    assert_eq!(
        texts(&prs.notes),
        ["`alice repo:acme/secret` is not a valid GitHub login; its searches are skipped"]
    );
    assert_eq!(
        texts(&issues.notes),
        [
            "`acme/platform is:closed` is not a valid repository name; its searches are skipped",
            "`bob OR x` is not a valid GitHub login; its searches are skipped",
        ]
    );
    assert_eq!(
        texts(&history.notes),
        ["`x y` is not a valid GitHub login; its searches are skipped"]
    );
}

/// A client of a wiremock server, for answers the fake cannot produce.
fn mock_client(server: &MockServer) -> WorkClient<TestTokens> {
    WorkClient::new(
        Url::parse(&server.uri()).unwrap(),
        TestTokens::new("ghu_mock_token", None),
    )
    .unwrap()
}

#[tokio::test]
async fn incomplete_or_capped_search_results_leave_a_note() {
    let server = MockServer::start().await;
    for (repo, total, incomplete) in [("acme/big", 1500, false), ("acme/slow", 3, true)] {
        Mock::given(method("GET"))
            .and(path("/search/issues"))
            .and(query_param_contains("q", format!("repo:{repo}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"total_count": total, "incomplete_results": incomplete, "items": []}),
            ))
            .mount(&server)
            .await;
    }

    // One client per search: no pacing wait.
    let big = mock_client(&server)
        .search_issues(&[repo("acme/big")], &[])
        .await
        .unwrap();
    let slow = mock_client(&server)
        .search_issues(&[repo("acme/slow")], &[])
        .await
        .unwrap();

    let text = |found: &IssueSearch| {
        found
            .notes
            .iter()
            .map(|n| n.text.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        text(&big),
        ["GitHub found more than 1000 matches for issues of acme/big; the rest is not shown"]
    );
    assert_eq!(
        text(&slow),
        ["GitHub returned an incomplete result for issues of acme/slow; some items may be missing"]
    );
}

#[tokio::test]
async fn graphql_execution_timeout_retries_as_two_halves() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": null, "errors": [
            {"message": "Something went wrong while executing your query. This may be the result of a timeout."}
        ]})))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {}})))
        .mount(&server)
        .await;
    let pulls: Vec<ItemRef> = (1..=10)
        .map(|n| item(&format!("acme/platform#{n}")))
        .collect();

    mock_client(&server).pull_requests(&pulls).await.unwrap();

    let sizes: Vec<usize> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| {
            String::from_utf8_lossy(&r.body)
                .matches("pullRequest(")
                .count()
        })
        .collect();
    assert_eq!(sizes, [10, 5, 5]);
}

#[tokio::test]
async fn refused_graphql_document_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"errors": [{"message": "Parse error on \"}\""}]})),
        )
        .mount(&server)
        .await;
    let pulls: Vec<ItemRef> = (1..=4)
        .map(|n| item(&format!("acme/platform#{n}")))
        .collect();

    let fetched = mock_client(&server).pull_requests(&pulls).await.unwrap();

    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert_eq!(fetched.notes.len(), 4);
}

#[test]
fn mentions_respect_login_boundaries() {
    assert!(mentions("ping @alice, please", "alice"));
    assert!(mentions("@Alice.", "alice"));
    assert!(!mentions("ping @alice-helper", "alice"));
    assert!(!mentions("mail bob@alice.dev", "alice"));
    assert!(!mentions("no mention", "alice"));
}

#[tokio::test(start_paused = true)]
async fn authored_pull_requests_are_read_per_repository_whoever_merged_them() {
    let fake = Fake::start().await;
    let since: contracts::time::Timestamp = "2026-08-25T22:00:00Z".parse().unwrap();

    let found = fake
        .client()
        .authored_pull_requests(
            &[repo("acme/consensus"), repo("ACME/Platform")],
            &logins(&["acme"]),
            &logins(&["Alice-Helper"]),
            since,
        )
        .await
        .unwrap();

    let items: Vec<String> = found.found.keys().map(ToString::to_string).collect();
    assert_eq!(
        items,
        [
            "acme/consensus#1501",
            "acme/consensus#1536",
            "acme/consensus#1541",
            "acme/platform#1231",
            "acme/platform#1233",
            "acme/review-policies#7",
        ]
    );
    let merged = &found.found[&item("acme/consensus#1536")];
    assert!(merged.merged_at.is_some() && merged.author.as_deref() == Some("alice-helper"));
    let repos: Vec<String> = found.repositories.iter().map(repo_name).collect();
    assert!(
        !repos.iter().any(|r| r == "acme/legacy-wallet"),
        "archived and not pushed in the window: {repos:?}"
    );
    assert_eq!(
        repos
            .iter()
            .filter(|r| r.eq_ignore_ascii_case("acme/platform"))
            .count(),
        1,
        "a configured repository is read once: {repos:?}"
    );
    assert!(found.cut_short.is_empty() && found.notes.is_empty());
    let requests = fake.requests_on("graphql").await.len();
    assert_eq!(
        requests,
        1 + repos.len(),
        "one owner listing, one page per repository"
    );
}

/// A busy repository is read to the window start: dashpay/platform has ~1200 pull requests updated in 12 weeks.
#[tokio::test(start_paused = true)]
async fn authored_pull_requests_page_to_the_window_start() {
    let fake = Fake::start().await;
    let since: contracts::time::Timestamp = "2026-08-25T22:00:00Z".parse().unwrap();
    let mut client = fake.client();
    client.page_size = 1;

    let found = client
        .authored_pull_requests(
            &[repo("acme/platform")],
            &[],
            &logins(&["alice-helper"]),
            since,
        )
        .await
        .unwrap();

    // Seven pull requests updated in the window, then the older one that ends the paging.
    assert_eq!(fake.requests_on("graphql").await.len(), 8);
    assert!(found.cut_short.is_empty(), "{:?}", found.cut_short);
    assert_eq!(found.found.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn authored_pull_requests_stop_at_the_page_cap_and_say_where() {
    let fake = Fake::start().await;
    let since: contracts::time::Timestamp = "2026-08-25T22:00:00Z".parse().unwrap();
    let mut client = fake.client();
    client.page_size = 1;
    client.authored_pages = 3;

    let found = client
        .authored_pull_requests(
            &[repo("acme/platform"), repo("acme/secret")],
            &[],
            &logins(&["alice-helper"]),
            since,
        )
        .await
        .unwrap();

    assert_eq!(fake.requests_on("graphql").await.len(), 3 + 1);
    assert_eq!(found.cut_short.len(), 1, "{:?}", found.cut_short);
    assert_eq!(found.cut_short[0].0, repo("acme/platform"));
    assert_eq!(found.repositories, [repo("acme/platform")]);
    assert_eq!(
        found.unreadable,
        [repo("acme/secret")],
        "not visible to the token"
    );
}

#[tokio::test(start_paused = true)]
async fn pr_mentions_are_discovered_by_search() {
    let fake = Fake::start().await;
    fake.client()
        .search_pull_requests(&logins(&["alice"]))
        .await
        .unwrap();
    assert!(fake.requests_on("search").await.iter().any(|r| {
        r["q"]
            .as_str()
            .is_some_and(|q| q == "is:pr is:open archived:false mentions:alice")
    }));
}

#[tokio::test(start_paused = true)]
async fn pr_mention_search_and_enumeration_find_comment_only_candidates() {
    let dir = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(repo_root().join("fixtures/dashboard/github")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
    }
    let file = dir.path().join("pull-requests.json");
    let mut prs: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let pr = prs
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|p| p["number"] == 1203)
        .unwrap();
    pr["comments"] = json!({"nodes": [{"author": {"__typename": "User", "login": "bob"}, "body": "@alice @alice-helper please respond", "createdAt": "2026-11-18T08:59:00Z"}]} );
    std::fs::write(&file, serde_json::to_vec(&prs).unwrap()).unwrap();
    let base = spawn_fake_in(dir.path().to_owned()).await;
    let token = sign_in(&base).await;
    let fake = Fake { base, token };
    let client = fake.client();
    let searched = client
        .search_pull_requests(&logins(&["alice"]))
        .await
        .unwrap();
    assert!(
        searched.candidates.pull_requests[&item("acme/platform#1203")].relations["alice"]
            .contains(&PrRelation::Mentioned)
    );
    let listed = client
        .enumerate_owners(&logins(&["acme"]), &logins(&["alice-helper"]))
        .await
        .unwrap();
    assert!(
        listed.candidates.pull_requests[&item("acme/platform#1203")].relations["alice-helper"]
            .contains(&PrRelation::Mentioned),
        "{:?}",
        listed.notes
    );
}

#[tokio::test]
async fn truncated_pr_discussion_keeps_read_posts_without_rest_refills() {
    let fake = Fake::start().await;
    let key = item("acme/platform#1203");
    let mut discussion = fake
        .client()
        .pull_discussions(std::slice::from_ref(&key))
        .await
        .unwrap()
        .found
        .remove(&key)
        .unwrap();
    discussion.issue.comments.nodes.push(
        serde_json::from_value(json!({
            "author":{"__typename":"User","login":"bob"}, "body":"@alice read me",
            "url":"#issuecomment-123", "createdAt":"2026-11-18T08:00:00Z"
        }))
        .unwrap(),
    );
    let before = discussion.posts();
    discussion.issue.comments.total_count = Some(100);
    assert_eq!(discussion.posts(), before, "keep the read window");
    assert!(discussion.partly_read());
    assert!(fake.requests_on("issue_comments").await.is_empty());
}

#[tokio::test]
async fn mention_query_is_light_and_q2_excludes_review_bodies() {
    let fake = Fake::start().await;
    let key = item("acme/platform#1203");
    let details = fake
        .client()
        .pull_requests(std::slice::from_ref(&key))
        .await
        .unwrap();
    assert!(
        details.found[&key]
            .reviews
            .nodes
            .iter()
            .all(|r| r.body.is_empty())
    );
    let discussion = fake.client().pull_discussions(&[key]).await.unwrap();
    assert_eq!(discussion.found.len(), 1);
}

#[tokio::test]
async fn mention_conversation_budget_never_reads_more_than_forty_items() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"a0":{"pullRequest":null}}})),
        )
        .expect(40)
        .mount(&mock)
        .await;
    let mut candidates = Candidates::default();
    for n in 1..=45 {
        candidates.add_pr(
            ItemRef::new("acme", "app", n).unwrap(),
            "Mention",
            "",
            "alice",
            PrRelation::Mentioned,
        );
    }
    let settings = contracts::api::dashboard::DashboardSettings::default();
    let (mentions, notes) = crate::dashboard::refresh::mentions::collect(
        &mock_client(&mock),
        &crate::dashboard::score::ScoreRules::new("alice", &settings),
        &["alice".into()],
        &candidates,
        &Default::default(),
        &Default::default(),
    )
    .await
    .unwrap();
    assert!(mentions.is_empty());
    assert!(
        notes
            .iter()
            .any(|n| n.text.contains("45 items") && n.text.contains("not checked"))
    );
    mock.verify().await;
}

#[tokio::test]
async fn mention_search_cap_has_a_note_and_never_fetches_the_next_page() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"total_count":0,"items":[]})))
        .with_priority(10)
        .expect(3)
        .mount(&mock)
        .await;
    let items: Vec<_> = (1..=100)
        .map(|number| {
            json!({
                "number":number,"title":"Mention","html_url":"https://github.com/acme/app/pull/1",
                "repository_url":"https://api.github.com/repos/acme/app",
                "created_at":"2026-11-18T08:00:00Z","updated_at":"2026-11-18T08:00:00Z"
            })
        })
        .collect();
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .and(query_param_contains("q", "mentions:alice"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "link",
                    "<https://api.github.com/search/issues?page=2>; rel=\"next\"",
                )
                .set_body_json(json!({"total_count":101,"items":items})),
        )
        .with_priority(1)
        .expect(1)
        .mount(&mock)
        .await;
    let mut client = mock_client(&mock);
    client.search_interval = Duration::ZERO;
    let result = client
        .search_pull_requests(&["alice".into()])
        .await
        .unwrap();
    assert_eq!(result.candidates.pull_requests.len(), 100);
    assert!(
        result
            .notes
            .iter()
            .any(|n| n.text.contains("100") && n.text.contains("mention"))
    );
    mock.verify().await;
}

#[tokio::test(start_paused = true)]
async fn mention_search_pacing_does_not_starve_later_logins() {
    let fake = Fake::start().await;
    let logins = logins(&[
        "alice", "renata-s", "mira-k", "jun-park", "tobias-w", "olek-d", "dana",
    ]);
    let found = fake.client().search_pull_requests(&logins).await.unwrap();
    let requests = fake.requests_on("search").await;
    for login in logins {
        assert!(
            requests.iter().any(|r| r["q"]
                .as_str()
                .is_some_and(|q| q.ends_with(&format!("mentions:{login}")))),
            "missing {login}: {:?}",
            found.notes
        );
    }
}

fn mention_response() -> Value {
    json!({"data":{"a0":{"pullRequest":{
        "isDraft":false,"state":"OPEN","body":"@alice please","createdAt":"2026-11-18T08:00:00Z",
        "author":{"__typename":"User","login":"bob"},
        "comments":{"nodes":[],"totalCount":0},"reviews":{"nodes":[],"totalCount":0},
        "reviewThreads":{"nodes":[],"totalCount":0}
    }}}})
}

#[tokio::test(start_paused = true)]
async fn mention_deadline_keeps_finished_results_and_counts_unchecked_items() {
    let router = axum::Router::new().route(
        "/graphql",
        axum::routing::post(|axum::Json(request): axum::Json<Value>| async move {
            if request["query"].as_str().unwrap().contains("number: 2)") {
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
            axum::Json(mention_response())
        }),
    );
    let client = local_client(router).await;
    let mut candidates = Candidates::default();
    for n in 1..=2 {
        candidates.add_pr(
            ItemRef::new("acme", "app", n).unwrap(),
            "Mention",
            "",
            "alice",
            PrRelation::Mentioned,
        );
    }
    let settings = contracts::api::dashboard::DashboardSettings::default();
    let (mentions, notes) = crate::dashboard::refresh::mentions::collect(
        &client,
        &crate::dashboard::score::ScoreRules::new("alice", &settings),
        &["alice".into()],
        &candidates,
        &Default::default(),
        &Default::default(),
    )
    .await
    .unwrap();
    assert_eq!(mentions.len(), 1, "finished results survive the deadline");
    assert!(
        notes
            .iter()
            .any(|n| n.text.contains("1 item") && n.text.contains("not checked")),
        "{notes:?}"
    );
}

#[tokio::test]
async fn mention_enumeration_does_not_select_conversations_on_ranked_pages() {
    let mock = MockServer::start().await;
    Mock::given(method("POST")).and(path("/graphql"))
        .respond_with(|r: &wiremock::Request| {
            let body = String::from_utf8_lossy(&r.body);
            let data = if body.contains("repositoryOwner") {
                json!({"repositories":{"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[
                    {"nameWithOwner":"acme/app","pullRequests":{"totalCount":1},"issues":{"totalCount":0}}
                ]}})
            } else {
                json!({"pullRequests":{"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[
                    {"number":1,"title":"Ranked","url":"","updatedAt":"2026-11-18T08:00:00Z","body":"","comments":{"nodes":[]},
                     "author":{"login":"alice"},"assignees":{"nodes":[]},"reviewRequests":{"nodes":[]}}
                ]}})
            };
            ResponseTemplate::new(200).set_body_json(json!({"data":{"a0":data}}))
        }).mount(&mock).await;
    let result = mock_client(&mock)
        .enumerate_owners(&logins(&["acme"]), &logins(&["alice"]))
        .await
        .unwrap();
    assert!(result.candidates.pull_requests[&item("acme/app#1")].ranked());
    for request in mock.received_requests().await.unwrap() {
        let query = String::from_utf8_lossy(&request.body);
        if query.contains("pullRequests(states: OPEN, first: 50") {
            assert!(
                !query.contains("body"),
                "ranked enumeration must not carry conversation bodies: {query}"
            );
        }
    }
}

async fn local_client(router: axum::Router) -> WorkClient<TestTokens> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    WorkClient::new(
        Url::parse(&format!("http://{address}")).unwrap(),
        TestTokens::new("ghu_fake", None),
    )
    .unwrap()
}

fn search_item(number: u32, updated: &str) -> Value {
    json!({"number":number,"title":"Mention","html_url":"https://github.com/acme/app/pull/1",
        "repository_url":"https://api.github.com/repos/acme/app", "created_at":"2026-11-18T08:00:00Z",
        "updated_at":updated})
}

#[tokio::test(start_paused = true)]
async fn mention_search_deadline_keeps_finished_logins_and_continues_after_timeout() {
    let router = axum::Router::new().route(
        "/search/issues",
        axum::routing::get(
            |axum::extract::Query(query): axum::extract::Query<
                std::collections::HashMap<String, String>,
            >| async move {
                let q = &query["q"];
                if q.ends_with("mentions:slow") {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
                let items = if q.contains("mentions:") {
                    vec![search_item(
                        if q.ends_with("later") { 3 } else { 1 },
                        "2026-11-18T08:00:00Z",
                    )]
                } else {
                    vec![]
                };
                axum::Json(json!({"total_count":items.len(),"items":items}))
            },
        ),
    );
    let client = local_client(router).await;
    let found = client
        .search_pull_requests(&logins(&["alice", "slow", "later"]))
        .await
        .unwrap();
    assert!(
        found
            .candidates
            .pull_requests
            .contains_key(&item("acme/app#1"))
    );
    assert!(
        found
            .candidates
            .pull_requests
            .contains_key(&item("acme/app#3"))
    );
    assert_eq!(found.notes.len(), 1);
    assert!(found.notes[0].text.contains("slow"));
}

#[tokio::test]
async fn mention_budget_selects_newest_across_prs_and_issues() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .respond_with(|r: &wiremock::Request| {
            let q = r.url.query_pairs().find(|(k, _)| k == "q").unwrap().1;
            let items = if q.contains("mentions:") {
                if q.contains("is:pr") {
                    (1..=41)
                        .map(|n| search_item(n, &format!("2026-11-18T08:{n:02}:00Z")))
                        .collect()
                } else {
                    vec![search_item(99, "2026-11-18T09:00:00Z")]
                }
            } else {
                vec![]
            };
            ResponseTemplate::new(200)
                .set_body_json(json!({"total_count":items.len(),"items":items}))
        })
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(mention_response()))
        .expect(40)
        .mount(&mock)
        .await;
    let mut client = mock_client(&mock);
    client.search_interval = Duration::ZERO;
    let mut candidates = client
        .search_pull_requests(&logins(&["alice"]))
        .await
        .unwrap()
        .candidates;
    candidates.merge(
        client
            .search_issues(&[], &logins(&["alice"]))
            .await
            .unwrap()
            .candidates,
    );
    let settings = contracts::api::dashboard::DashboardSettings::default();
    let (found, notes) = crate::dashboard::refresh::mentions::collect(
        &client,
        &crate::dashboard::score::ScoreRules::new("alice", &settings),
        &logins(&["alice"]),
        &candidates,
        &Default::default(),
        &Default::default(),
    )
    .await
    .unwrap();
    assert!(found.iter().any(|m| m.number == 41));
    assert!(!found.iter().any(|m| m.number == 1));
    assert!(
        mock.received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| String::from_utf8_lossy(&r.body).contains("issue(number: 99)"))
    );
    assert!(
        notes
            .iter()
            .any(|n| n.text.contains("3 items") && n.text.contains("not checked"))
    );
}

#[tokio::test(start_paused = true)]
async fn mention_policy_is_shared_and_file_reads_are_concurrent_with_finished_results_kept() {
    use contracts::api::dashboard::{DashboardSettings, PolicyRef, RepositoryRule};
    let policy_reads = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let policy_count = policy_reads.clone();
    let file_active = active.clone();
    let file_peak = peak.clone();
    let router = axum::Router::new()
        .route("/graphql", axum::routing::post(|| async { axum::Json(mention_response()) }))
        .route("/repos/acme/policy/contents/policy.json", axum::routing::get(move || {
            policy_count.fetch_add(1, Ordering::SeqCst);
            async { r#"{"areas":[{"paths":["src/"],"owners":["alice"]}]}"# }
        }))
        .route("/repos/acme/app/pulls/{number}/files", axum::routing::get(move |axum::extract::Path(number): axum::extract::Path<u32>| {
            let active = file_active.clone(); let peak = file_peak.clone();
            async move {
                let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(n, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_secs(if number == 5 {60} else {2})).await;
                active.fetch_sub(1, Ordering::SeqCst);
                axum::Json(json!([{"filename":"src/lib.rs","status":"modified","additions":20,"deletions":0}]))
            }
        }));
    let client = local_client(router).await;
    let settings = DashboardSettings {
        repositories: vec![RepositoryRule {
            repo: repo("acme/app"),
            alias: None,
            watch_issues: false,
            branch_patterns: vec![],
            ownership_policy: Some(PolicyRef {
                repo: repo("acme/policy"),
                path: "policy.json".into(),
            }),
        }],
        ..Default::default()
    };
    let ownership = crate::dashboard::refresh::owned::Ownership::new(&settings);
    let rules = crate::dashboard::score::ScoreRules::new("alice", &settings);
    // Ranking has already read this policy. Mention-only PRs reuse it.
    assert!(
        ownership
            .paths(&client, &rules, &repo("acme/app"))
            .await
            .unwrap()
            .is_some()
    );
    let mut candidates = Candidates::default();
    for n in 1..=5 {
        candidates.add_pr(
            item(&format!("acme/app#{n}")),
            "Mention",
            "",
            "alice",
            PrRelation::Mentioned,
        );
    }
    let (found, notes) = crate::dashboard::refresh::mentions::collect(
        &client,
        &rules,
        &logins(&["alice"]),
        &candidates,
        &Default::default(),
        &ownership,
    )
    .await
    .unwrap();
    assert_eq!(found.len(), 4);
    assert_eq!(policy_reads.load(Ordering::SeqCst), 1);
    assert!((2..=4).contains(&peak.load(Ordering::SeqCst)));
    assert!(
        notes
            .iter()
            .any(|n| n.text.contains("1 item") && n.text.contains("not checked"))
    );
}

#[tokio::test]
async fn mention_truncated_issue_keeps_the_read_window() {
    let mock = MockServer::start().await;
    let mut issue = mention_response()["data"]["a0"]["pullRequest"].clone();
    issue["comments"] = json!({"totalCount":60,"nodes":[{
        "author":{"__typename":"User","login":"bob"}, "body":"@alice read me",
        "url":"#issuecomment-123", "createdAt":"2026-11-18T08:00:00Z"
    }]});
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"a0":{"issue":issue}}})),
        )
        .expect(1)
        .mount(&mock)
        .await;
    let found = mock_client(&mock)
        .issue_discussions(&[item("acme/app#1")])
        .await
        .unwrap();
    assert_eq!(found.found[&item("acme/app#1")].comments.nodes.len(), 1);
}
