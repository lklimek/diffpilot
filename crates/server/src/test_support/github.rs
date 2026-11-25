//! Shared GitHub fixtures of the server's unit tests: the wiremock GitHub of fixture PR `acme/node-client#482` with a
//! finished analysis and its mirror, and the in-process fake GitHub (`tools/fake-github`) with its OAuth sign-in.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use contracts::api::analyses::{AnalysisState, AnalysisSummary};
use contracts::api::pr::PrSummary;
use contracts::ids::{AnalysisId, CommitSha};
use contracts::time::Timestamp;
use db::repo::DEFAULT_TENANT;
use db::repo::analyses::NewAnalysis;
use db::repo::documents::DocumentKind;
use reqwest::header::{ACCEPT, LOCATION};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use url::Url;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::github::review::PULL_REQUEST_HEAD;
use crate::review::tests::github_state_with;
use crate::state::AppState;

/// Head commit of fixture PR 482.
pub(crate) const HEAD: &str = "52d2da140191a9d47becf6a446310be803c007fc";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482")
}

/// The fixture: state with the wiremock GitHub, the finished analysis and the mock server.
pub(crate) struct Pr482 {
    pub(crate) state: AppState,
    pub(crate) summary: AnalysisSummary,
    pub(crate) server: MockServer,
    _dir: TempDir,
}

/// A finished analysis of the fixture PR (flow documents stored) with its mirror, for user `u1`.
pub(crate) async fn pr482() -> Pr482 {
    let dir = tempfile::tempdir().expect("temp dir");
    let built = Command::new(fixtures().join("repo/build-repo.sh"))
        .arg(dir.path().join("remotes/acme/node-client.git"))
        .output()
        .expect("build-repo.sh");
    assert!(built.status.success(), "{built:?}");
    let server = MockServer::start().await;
    let (state, _) = github_state_with(&server, true, |config| {
        config.data_dir = dir.path().join("data");
        config.github.clone_url =
            Url::parse(&format!("file://{}", dir.path().join("remotes").display())).expect("url");
    })
    .await;
    let text =
        std::fs::read_to_string(fixtures().join("api-seed/default/pr-resolve.json")).expect("seed");
    let pr: PrSummary =
        serde_json::from_value(serde_json::from_str::<Value>(&text).expect("JSON")["pr"].clone())
            .expect("pr");
    let repo = state
        .db
        .repos()
        .upsert(DEFAULT_TENANT, &pr.repo, None)
        .await
        .expect("repo");
    let pr_id = state
        .db
        .pull_requests()
        .upsert(DEFAULT_TENANT, repo.id, &pr, Timestamp::now())
        .await
        .expect("pr");
    let id = state
        .db
        .analyses()
        .create(
            NewAnalysis {
                id: AnalysisId::new_random(),
                tenant_id: DEFAULT_TENANT.to_owned(),
                pull_request_id: pr_id,
                base_sha: pr.base_sha.clone(),
                head_sha: pr.head_sha.clone(),
                language: Default::default(),
                created_by: "u1".to_owned(),
            },
            Timestamp::now(),
        )
        .await
        .expect("analysis")
        .id;
    for n in 1..=5 {
        let text =
            std::fs::read_to_string(fixtures().join(format!("agent/flow-{n}.json"))).expect("flow");
        let flow: Value = serde_json::from_str(&text).expect("JSON");
        state
            .db
            .documents()
            .put(
                id,
                DocumentKind::Flow,
                &format!("f{n}"),
                &flow,
                Timestamp::now(),
            )
            .await
            .expect("flow document");
    }
    state
        .db
        .analyses()
        .set_state(id, &AnalysisState::Finished { total: 5 }, Timestamp::now())
        .await
        .expect("finished");
    let summary = state.db.analyses().summary(id).await.expect("summary");
    assert_eq!(summary.pr.head_sha, CommitSha(HEAD.to_owned()));
    Pr482 {
        state,
        summary,
        server,
        _dir: dir,
    }
}

impl Pr482 {
    pub(crate) async fn mock(&self, request: Value, response: ResponseTemplate, calls: u64) {
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .and(body_partial_json(request))
            .respond_with(response)
            .expect(calls)
            .mount(&self.server)
            .await;
    }

    pub(crate) async fn head_is(&self, head: &str, calls: u64) {
        let response = json!({"data": {"repository": {"pullRequest": {"id": "PR_fixture482", "headRefOid": head}}}});
        self.mock(
            json!({"query": PULL_REQUEST_HEAD}),
            ResponseTemplate::new(200).set_body_json(response),
            calls,
        )
        .await;
    }

    pub(crate) async fn graphql_bodies(&self) -> Vec<Value> {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|request| request.url.path() == "/api/graphql")
            .map(|request| serde_json::from_slice(&request.body).expect("JSON body"))
            .collect()
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// PKCE verifier of [`fake_github_sign_in`].
const VERIFIER: &str = "work-client-verifier-0123456789-0123456789-0123456789";

/// Starts the fake GitHub of fixture PR 482 with its review-loop reads (and the dashboard world `dashboard`, if
/// any); returns its base URL.
pub(crate) async fn spawn_fake_github(dashboard: Option<PathBuf>) -> String {
    let config = fake_github::Config {
        fixture_dir: repo_root().join("fixtures/pr-482/github"),
        app_url: Url::parse("http://app.test:3000").expect("url"),
        user_login: None,
        user_id: None,
        app_id: None,
        app_public_key_file: None,
        dashboard_fixture_dir: dashboard,
        dashboard_world_at: None,
        review_loop_fixture_dir: Some(repo_root().join("fixtures/review-loop/github")),
    };
    let router = fake_github::router(&config).expect("fake GitHub");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let addr: SocketAddr = listener.local_addr().expect("address");
    tokio::spawn(async move { axum::serve(listener, router).await.expect("serve") });
    format!("http://{addr}")
}

/// An HTTP client that does not follow redirects.
pub(crate) fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client")
}

/// The fake's OAuth web flow; returns the user access token.
pub(crate) async fn fake_github_sign_in(base: &str) -> String {
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()));
    let mut authorize = Url::parse(&format!("{base}/login/oauth/authorize")).expect("url");
    authorize.query_pairs_mut().extend_pairs([
        ("client_id", "fixture-app-client-id"),
        ("redirect_uri", "http://app.test:3000/auth/github/callback"),
        ("state", "s"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
    ]);
    let response = http().get(authorize).send().await.expect("authorize");
    let location =
        Url::parse(response.headers()[LOCATION].to_str().expect("location")).expect("url");
    let code = location
        .query_pairs()
        .find(|(k, _)| k == "code")
        .expect("code")
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
        .expect("token")
        .json()
        .await
        .expect("JSON");
    body["access_token"]
        .as_str()
        .expect("access token")
        .to_owned()
}
