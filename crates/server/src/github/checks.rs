//! CI status of the analysed head commit from its check runs (REST, user token): the PR-context CI fact.

use std::time::Duration;

use contracts::api::tour::{CiCheck, CiState, CiStatus};
use contracts::common::RepoRef;
use contracts::ids::CommitSha;
use reqwest::header::{ACCEPT, USER_AGENT};
use reqwest::{Client, StatusCode, redirect};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use crate::config::GithubConfig;
use crate::github::oauth::endpoint;
use crate::github::rest::GithubError;
use crate::github::threads::github_web_url;

const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const CLIENT_NAME: &str = "smart-review";
const GITHUB_API_VERSION: &str = "2022-11-28";
/// Check runs are read in pages of 100; at most this many pages.
const MAX_PAGES: u32 = 10;
/// At most this many failing test names per check, each at most this many characters.
const MAX_FAILING_TESTS: usize = 20;
const MAX_TEST_NAME_CHARS: usize = 200;

/// The CI fact when the check runs could not be read: nothing is claimed.
pub const NOT_CHECKED: CiStatus = CiStatus {
    state: CiState::NotChecked,
    total: 0,
    failing: Vec::new(),
};

#[derive(Deserialize)]
struct CheckRunsPage {
    total_count: u32,
    check_runs: Vec<CheckRun>,
}

#[derive(Deserialize)]
struct CheckRun {
    name: String,
    status: String,
    conclusion: Option<String>,
    html_url: Option<String>,
    output: Option<CheckOutput>,
}

#[derive(Deserialize)]
struct CheckOutput {
    summary: Option<String>,
}

/// How one check run ended, as far as the CI fact is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Passed,
    Failed,
    Running,
}

fn outcome(run: &CheckRun) -> Outcome {
    if run.status != "completed" {
        return Outcome::Running;
    }
    match run.conclusion.as_deref() {
        Some("success" | "neutral" | "skipped") => Outcome::Passed,
        // failure, timed_out, cancelled, action_required, startup_failure, stale and anything unknown: never
        // reported as passing.
        _ => Outcome::Failed,
    }
}

/// Test names a check reports in its summary as `<name>: <reason>` lines, where the name is a path of `::`
/// segments without spaces (`module::test`, `tests/x.py::test_y`); other summaries yield none.
fn failing_tests(summary: &str) -> Vec<String> {
    summary
        .lines()
        .filter_map(|line| {
            let line = line.trim().trim_start_matches(['-', '*']).trim_start();
            let (name, _) = line.split_once(": ")?;
            let plausible = name.contains("::")
                && !name.starts_with(':')
                && !name.ends_with(':')
                && name.chars().count() <= MAX_TEST_NAME_CHARS
                && !name.chars().any(|c| c.is_whitespace() || c.is_control());
            plausible.then(|| name.to_owned())
        })
        .take(MAX_FAILING_TESTS)
        .collect()
}

fn status_of(runs: &[CheckRun]) -> CiStatus {
    let outcomes: Vec<Outcome> = runs.iter().map(outcome).collect();
    let state = if runs.is_empty() {
        CiState::None
    } else if outcomes.contains(&Outcome::Failed) {
        CiState::Failing
    } else if outcomes.contains(&Outcome::Running) {
        CiState::Pending
    } else {
        CiState::Passing
    };
    let failing = runs
        .iter()
        .zip(&outcomes)
        .filter(|(_, outcome)| **outcome == Outcome::Failed)
        .map(|(run, _)| CiCheck {
            name: run.name.clone(),
            url: run.html_url.as_deref().and_then(github_web_url),
            failing_tests: run
                .output
                .as_ref()
                .and_then(|output| output.summary.as_deref())
                .map(failing_tests)
                .unwrap_or_default(),
        })
        .collect();
    CiStatus {
        state,
        total: u32::try_from(runs.len()).unwrap_or(u32::MAX),
        failing,
    }
}

fn client() -> Result<Client, GithubError> {
    Client::builder()
        .redirect(redirect::Policy::none())
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(GithubError::Client)
}

/// The check runs of `head` (`GET /repos/{owner}/{name}/commits/{sha}/check-runs`, latest run per check, at
/// most 1000) summarised as the CI fact, read with the user's token.
pub async fn fetch(
    config: &GithubConfig,
    token: &SecretString,
    repo: &RepoRef,
    head: &CommitSha,
) -> Result<CiStatus, GithubError> {
    let client = client()?;
    let url = endpoint(
        &config.api_url,
        &format!(
            "repos/{}/{}/commits/{}/check-runs",
            repo.owner, repo.name, head.0
        ),
    );
    let mut runs = Vec::new();
    for page in 1..=MAX_PAGES {
        let mut page_url = url.clone();
        page_url
            .query_pairs_mut()
            .append_pair("per_page", "100")
            .append_pair("page", &page.to_string());
        let response = client
            .get(page_url)
            .bearer_auth(token.expose_secret())
            .header(ACCEPT, "application/vnd.github+json")
            .header(USER_AGENT, CLIENT_NAME)
            .header("x-github-api-version", GITHUB_API_VERSION)
            .send()
            .await
            .map_err(GithubError::Http)?;
        match response.status() {
            StatusCode::OK => {}
            StatusCode::NOT_FOUND => return Err(GithubError::NotFound),
            StatusCode::UNAUTHORIZED => return Err(GithubError::Unauthorized),
            status => return Err(GithubError::Status(status)),
        }
        let body: CheckRunsPage = response.json().await.map_err(GithubError::Http)?;
        let last = body.check_runs.is_empty();
        runs.extend(body.check_runs);
        if last || runs.len() >= usize::try_from(body.total_count).unwrap_or(usize::MAX) {
            return Ok(status_of(&runs));
        }
    }
    tracing::warn!(
        count = runs.len(),
        "the head commit has more check runs than are read; the CI fact counts the first ones"
    );
    Ok(status_of(&runs))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use contracts::api::tour::Tour;
    use serde_json::{Value, json};
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::github::graphql::tests::github_config;

    const HEAD: &str = "52d2da140191a9d47becf6a446310be803c007fc";

    fn fixture(relative: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/pr-482")
            .join(relative);
        serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("JSON")
    }

    fn repo() -> RepoRef {
        RepoRef {
            owner: "acme".to_owned(),
            name: "node-client".to_owned(),
        }
    }

    fn checks_path() -> String {
        format!("/api/repos/acme/node-client/commits/{HEAD}/check-runs")
    }

    async fn fetch_from(server: &MockServer) -> Result<CiStatus, GithubError> {
        fetch(
            &github_config(server),
            &SecretString::from("ghu_checks"),
            &repo(),
            &CommitSha(HEAD.to_owned()),
        )
        .await
    }

    fn run(name: &str, status: &str, conclusion: Option<&str>) -> Value {
        json!({"name": name, "status": status, "conclusion": conclusion,
               "html_url": format!("https://github.com/acme/node-client/runs/{name}"), "output": null})
    }

    async fn serve_runs(runs: Vec<Value>) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(checks_path()))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"total_count": runs.len(), "check_runs": runs})),
            )
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn recorded_check_runs_match_the_tour_seed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(checks_path()))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "1"))
            .and(header("authorization", "Bearer ghu_checks"))
            .and(header("x-github-api-version", GITHUB_API_VERSION))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(&fixture("github/rest/check-runs.json")["response"]),
            )
            .expect(1)
            .mount(&server)
            .await;
        let tour: Tour =
            serde_json::from_value(fixture("api-seed/default/tour.json")).expect("tour");
        assert_eq!(
            fetch_from(&server).await.expect("status"),
            tour.pr_context.facts.ci
        );
    }

    #[tokio::test]
    async fn state_is_derived_from_every_run() {
        let cases = [
            (vec![], CiState::None),
            (
                vec![
                    run("a", "completed", Some("success")),
                    run("b", "completed", Some("skipped")),
                    run("c", "completed", Some("neutral")),
                ],
                CiState::Passing,
            ),
            (
                vec![
                    run("a", "completed", Some("success")),
                    run("b", "in_progress", None),
                ],
                CiState::Pending,
            ),
            (
                vec![
                    run("a", "queued", None),
                    run("b", "completed", Some("timed_out")),
                ],
                CiState::Failing,
            ),
            (
                vec![run("a", "completed", Some("something_new"))],
                CiState::Failing,
            ),
        ];
        for (runs, expected) in cases {
            let count = runs.len();
            let server = serve_runs(runs).await;
            let status = fetch_from(&server).await.expect("status");
            assert_eq!(status.state, expected, "{status:?}");
            assert_eq!(status.total as usize, count);
        }
    }

    #[tokio::test]
    async fn failing_check_links_only_to_github() {
        let mut foreign = run("lint", "completed", Some("failure"));
        foreign["html_url"] = json!("https://ci.example.test/build/1");
        let mut script = run("x", "completed", Some("failure"));
        script["html_url"] = json!("javascript:alert(1)");
        let server = serve_runs(vec![
            foreign,
            script,
            run("ok", "completed", Some("failure")),
        ])
        .await;
        let status = fetch_from(&server).await.expect("status");
        let urls: Vec<Option<&str>> = status.failing.iter().map(|c| c.url.as_deref()).collect();
        assert_eq!(
            urls,
            [
                None,
                None,
                Some("https://github.com/acme/node-client/runs/ok")
            ]
        );
    }

    #[tokio::test]
    async fn pages_are_read_until_the_total() {
        let server = MockServer::start().await;
        let page = |names: &[&str]| {
            json!({"total_count": 3, "check_runs": names.iter()
                .map(|n| run(n, "completed", Some("success"))).collect::<Vec<_>>()})
        };
        Mock::given(method("GET"))
            .and(path(checks_path()))
            .and(query_param("page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(&["a", "b"])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(checks_path()))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(&["c"])))
            .expect(1)
            .mount(&server)
            .await;
        let status = fetch_from(&server).await.expect("status");
        assert_eq!((status.state, status.total), (CiState::Passing, 3));
    }

    #[tokio::test]
    async fn http_failures_are_errors() {
        for (code, check) in [
            (
                401,
                (|e| matches!(e, GithubError::Unauthorized)) as fn(&GithubError) -> bool,
            ),
            (404, |e| matches!(e, GithubError::NotFound)),
            (403, |e| {
                matches!(e, GithubError::Status(StatusCode::FORBIDDEN))
            }),
            (502, |e| {
                matches!(e, GithubError::Status(StatusCode::BAD_GATEWAY))
            }),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(checks_path()))
                .respond_with(ResponseTemplate::new(code))
                .mount(&server)
                .await;
            let error = fetch_from(&server).await.expect_err("error");
            assert!(check(&error), "{code}: {error:?}");
        }
    }

    #[test]
    fn failing_tests_come_only_from_name_reason_lines() {
        let summary = "sync_headers::slow_peer_2000_headers: timed out after 5 s\n\
                       - tests/test_x.py::test_y: AssertionError\n\
                       Error: process exited with 1\n\
                       some text :: with spaces: no\n\
                       ::: odd\n";
        assert_eq!(
            failing_tests(summary),
            [
                "sync_headers::slow_peer_2000_headers",
                "tests/test_x.py::test_y"
            ]
        );
        let many: String = (0..30).map(|n| format!("m::t{n}: failed\n")).collect();
        assert_eq!(failing_tests(&many).len(), MAX_FAILING_TESTS);
    }
}
