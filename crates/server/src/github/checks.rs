//! CI status of the analysed head commit from check runs and commit statuses (REST, user token): the PR-context CI fact.

use std::time::Duration;

use contracts::api::tour::{CiCheck, CiCounts, CiState, CiStatus};
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
const CLIENT_NAME: &str = "diffpilot";
const GITHUB_API_VERSION: &str = "2022-11-28";
/// Check runs are read in pages of 100; at most this many pages (1000 runs). More runs: CI is not checked.
const MAX_PAGES: u32 = 10;
/// At most this many failing test names per check, each at most this many characters.
const MAX_FAILING_TESTS: usize = 20;
const MAX_TEST_NAME_CHARS: usize = 200;

/// The CI fact when the check runs could not be read: nothing is claimed.
pub const NOT_CHECKED: CiStatus = CiStatus {
    state: CiState::NotChecked,
    total: 0,
    failing: Vec::new(),
    check_runs: None,
    statuses: None,
};

#[derive(Deserialize)]
struct CiPage<T> {
    total_count: u32,
    #[serde(rename = "check_runs", alias = "statuses")]
    items: Vec<T>,
}

#[derive(Deserialize)]
struct CommitStatus {
    context: String,
    state: String,
    target_url: Option<String>,
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
    Skipped,
    Neutral,
}

fn outcome(run: &CheckRun) -> Outcome {
    if run.status != "completed" {
        return Outcome::Running;
    }
    match run.conclusion.as_deref() {
        Some("success") => Outcome::Passed,
        Some("skipped") => Outcome::Skipped,
        Some("neutral") => Outcome::Neutral,
        // failure, timed_out, cancelled, action_required, startup_failure, stale and anything unknown: never
        // reported as passing.
        _ => Outcome::Failed,
    }
}

/// A test name as a failure format prints it: no whitespace or control characters, bounded length.
fn test_name(name: &str) -> Option<&str> {
    let valid = !name.is_empty()
        && name.chars().count() <= MAX_TEST_NAME_CHARS
        && !name.chars().any(|c| c.is_whitespace() || c.is_control());
    valid.then_some(name)
}

/// Test names from the formats that mark a failure unambiguously; any other summary text yields none.
/// - Rust libtest: `test <name> ... FAILED`, and the closing `failures:` block whose every line is
///   `    <name>` (a block with any other line, such as captured output, is ignored).
/// - pytest short summary: `FAILED <file>::<test>[ - <reason>]`.
///
/// At most [`MAX_FAILING_TESTS`] names of at most [`MAX_TEST_NAME_CHARS`] characters, without repeats.
fn failing_tests(summary: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut add = |name: &str| {
        if names.len() < MAX_FAILING_TESTS && !names.iter().any(|known| known == name) {
            names.push(name.to_owned());
        }
    };
    let lines: Vec<&str> = summary.lines().map(|line| line.trim_end()).collect();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        if line.trim_start() == "failures:" {
            let block: Vec<&str> = lines[index..]
                .iter()
                .take_while(|line| !line.is_empty())
                .copied()
                .collect();
            index += block.len();
            let listed: Option<Vec<&str>> = block
                .iter()
                .map(|line| line.strip_prefix("    ").and_then(test_name))
                .collect();
            for name in listed.unwrap_or_default() {
                add(name);
            }
        } else if let Some(name) = line
            .trim_start()
            .strip_prefix("test ")
            .and_then(|rest| rest.strip_suffix(" ... FAILED"))
            .and_then(test_name)
        {
            add(name);
        } else if let Some(rest) = line.strip_prefix("FAILED ") {
            let name = rest.split_once(" - ").map_or(rest, |(name, _)| name);
            if let Some(name) = test_name(name).filter(|name| name.contains("::")) {
                add(name);
            }
        }
    }
    names
}

fn count(outcomes: impl Iterator<Item = Outcome>) -> CiCounts {
    let mut counts = CiCounts::default();
    for outcome in outcomes {
        match outcome {
            Outcome::Passed => counts.passed += 1,
            Outcome::Failed => counts.failed += 1,
            Outcome::Running => counts.pending += 1,
            Outcome::Skipped => counts.skipped += 1,
            Outcome::Neutral => counts.neutral += 1,
        }
    }
    counts
}

fn status_outcome(status: &CommitStatus) -> Outcome {
    match status.state.as_str() {
        "success" => Outcome::Passed,
        "pending" => Outcome::Running,
        _ => Outcome::Failed,
    }
}

fn status_of(runs: Option<Vec<CheckRun>>, statuses: Option<Vec<CommitStatus>>) -> CiStatus {
    let check_counts = runs.as_ref().map(|runs| count(runs.iter().map(outcome)));
    let status_counts = statuses
        .as_ref()
        .map(|statuses| count(statuses.iter().map(status_outcome)));
    let counts: Vec<CiCounts> = [check_counts, status_counts]
        .into_iter()
        .flatten()
        .collect();
    let total = counts
        .iter()
        .map(|c| c.passed + c.failed + c.pending + c.skipped + c.neutral)
        .sum();
    let complete = check_counts.is_some() && status_counts.is_some();
    let state = if counts.iter().any(|c| c.failed > 0) {
        CiState::Failing
    } else if counts.iter().any(|c| c.pending > 0) {
        CiState::Pending
    } else if total == 0 {
        if complete {
            CiState::None
        } else {
            CiState::NotChecked
        }
    } else if complete && counts.iter().all(|c| c.skipped == 0 && c.neutral == 0) {
        CiState::Passing
    } else {
        CiState::NoFailures
    };
    let mut failing: Vec<CiCheck> = runs
        .unwrap_or_default()
        .iter()
        .filter(|run| outcome(run) == Outcome::Failed)
        .map(|run| CiCheck {
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
    failing.extend(
        statuses
            .unwrap_or_default()
            .iter()
            .filter(|status| status_outcome(status) == Outcome::Failed)
            .map(|status| CiCheck {
                name: status.context.clone(),
                url: status.target_url.as_deref().and_then(github_web_url),
                failing_tests: Vec::new(),
            }),
    );
    CiStatus {
        state,
        total,
        failing,
        check_runs: check_counts,
        statuses: status_counts,
    }
}

fn client() -> Result<Client, GithubError> {
    Client::builder()
        .redirect(redirect::Policy::none())
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(GithubError::Client)
}

/// Reads both CI sources independently; a refused, incomplete or failed source stays not checked.
pub async fn fetch(
    config: &GithubConfig,
    token: &SecretString,
    repo: &RepoRef,
    head: &CommitSha,
) -> Result<CiStatus, GithubError> {
    Ok(fetch_with_pending(config, token, repo, head).await?.0)
}

/// [`fetch`] with the names of the check runs and statuses still pending (so a reader can leave out ignored ones).
pub async fn fetch_with_pending(
    config: &GithubConfig,
    token: &SecretString,
    repo: &RepoRef,
    head: &CommitSha,
) -> Result<(CiStatus, Vec<String>), GithubError> {
    let client = client()?;
    let base = format!("repos/{}/{}/commits/{}", repo.owner, repo.name, head.0);
    let runs = source(&client, config, token, &format!("{base}/check-runs")).await?;
    let statuses = source(&client, config, token, &format!("{base}/status")).await?;
    let pending = pending_names(runs.as_deref(), statuses.as_deref());
    Ok((status_of(runs, statuses), pending))
}

/// Names of the pending check runs, then of the pending statuses.
fn pending_names(runs: Option<&[CheckRun]>, statuses: Option<&[CommitStatus]>) -> Vec<String> {
    let runs = runs
        .unwrap_or_default()
        .iter()
        .filter(|run| outcome(run) == Outcome::Running)
        .map(|run| run.name.clone());
    let statuses = statuses
        .unwrap_or_default()
        .iter()
        .filter(|status| status_outcome(status) == Outcome::Running)
        .map(|status| status.context.clone());
    runs.chain(statuses).collect()
}

async fn source<T: serde::de::DeserializeOwned>(
    client: &Client,
    config: &GithubConfig,
    token: &SecretString,
    path: &str,
) -> Result<Option<Vec<T>>, GithubError> {
    match read_source(client, config, token, path).await {
        Ok(items) => Ok(items),
        Err(GithubError::Unauthorized) => Err(GithubError::Unauthorized),
        Err(_) => {
            tracing::warn!(
                source = path,
                "CI source could not be read; source is not checked"
            );
            Ok(None)
        }
    }
}

async fn read_source<T: serde::de::DeserializeOwned>(
    client: &Client,
    config: &GithubConfig,
    token: &SecretString,
    path: &str,
) -> Result<Option<Vec<T>>, GithubError> {
    let url = endpoint(&config.api_url, path);
    let mut items = Vec::new();
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
            StatusCode::UNAUTHORIZED => return Err(GithubError::Unauthorized),
            status => return Err(GithubError::Status(status)),
        }
        let body: CiPage<T> = response.json().await.map_err(GithubError::Http)?;
        let empty = body.items.is_empty();
        items.extend(body.items);
        if items.len() >= usize::try_from(body.total_count).unwrap_or(usize::MAX) {
            return Ok(Some(items));
        }
        if empty {
            break;
        }
    }
    tracing::warn!(
        source = path,
        "CI source exceeded the read cap or ended early; source is not checked"
    );
    Ok(None)
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
        serve_statuses(
            &server,
            fixture("github/rest/commit-status.json")["response"]["statuses"]
                .as_array()
                .expect("statuses")
                .clone(),
        )
        .await;
        let tour: Tour =
            serde_json::from_value(fixture("api-seed/default/tour.json")).expect("tour");
        assert_eq!(
            fetch_from(&server).await.expect("status"),
            tour.pr_context.facts.ci
        );
    }

    fn commit_status(context: &str, state: &str) -> Value {
        json!({"context": context, "state": state, "description": null,
               "target_url": format!("https://github.com/acme/node-client/actions/{context}")})
    }

    fn statuses_path() -> String {
        format!("/api/repos/acme/node-client/commits/{HEAD}/status")
    }

    async fn serve_statuses(server: &MockServer, statuses: Vec<Value>) {
        Mock::given(method("GET"))
            .and(path(statuses_path()))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"state": "pending", "total_count": statuses.len(), "statuses": statuses}),
            ))
            .mount(server)
            .await;
    }

    fn counts(passed: u32, failed: u32, pending: u32, skipped: u32, neutral: u32) -> CiCounts {
        CiCounts {
            passed,
            failed,
            pending,
            skipped,
            neutral,
        }
    }

    /// Skipped and neutral runs are counted as such and never make CI "passing".
    #[tokio::test]
    async fn state_is_derived_from_every_run_and_status() {
        let success = || run("a", "completed", Some("success"));
        let cases = [
            (vec![], vec![], CiState::None),
            (vec![success()], vec![], CiState::Passing),
            (
                vec![success(), run("b", "completed", Some("skipped"))],
                vec![],
                CiState::NoFailures,
            ),
            (
                vec![success(), run("c", "completed", Some("neutral"))],
                vec![],
                CiState::NoFailures,
            ),
            (
                vec![success(), run("b", "in_progress", None)],
                vec![],
                CiState::Pending,
            ),
            (
                vec![success()],
                vec![commit_status("hygiene", "pending")],
                CiState::Pending,
            ),
            (
                vec![
                    run("a", "queued", None),
                    run("b", "completed", Some("timed_out")),
                ],
                vec![],
                CiState::Failing,
            ),
            (
                vec![run("a", "completed", Some("something_new"))],
                vec![],
                CiState::Failing,
            ),
            (
                vec![success()],
                vec![commit_status("deploy", "error")],
                CiState::Failing,
            ),
            (
                vec![],
                vec![commit_status("lint", "success")],
                CiState::Passing,
            ),
        ];
        for (runs, statuses, expected) in cases {
            let count = runs.len() + statuses.len();
            let server = serve_runs(runs).await;
            serve_statuses(&server, statuses).await;
            let status = fetch_from(&server).await.expect("status");
            assert_eq!(status.state, expected, "{status:?}");
            assert_eq!(status.total as usize, count);
        }
    }

    /// The real #1169 shape: one success and four skipped runs plus a pending status.
    #[tokio::test]
    async fn counts_per_source_say_what_was_skipped_and_pending() {
        let mut runs = vec![run("check", "completed", Some("success"))];
        for name in ["clippy", "test", "fmt", "deny"] {
            runs.push(run(name, "completed", Some("skipped")));
        }
        let server = serve_runs(runs).await;
        serve_statuses(&server, vec![commit_status("PR Hygiene", "pending")]).await;
        let status = fetch_from(&server).await.expect("status");
        assert_eq!(status.state, CiState::Pending);
        assert_eq!(status.total, 6);
        assert_eq!(status.check_runs, Some(counts(1, 0, 0, 4, 0)));
        assert_eq!(status.statuses, Some(counts(0, 0, 1, 0, 0)));
        assert!(status.failing.is_empty());
    }

    /// The review loop keeps the names of pending checks, so an ignored pending one need not count as CI running.
    #[tokio::test]
    async fn pending_checks_are_named() {
        let server = serve_runs(vec![
            run("build", "in_progress", None),
            run("test", "completed", Some("success")),
        ])
        .await;
        serve_statuses(
            &server,
            vec![
                commit_status("PR Hygiene", "pending"),
                commit_status("lint", "success"),
            ],
        )
        .await;
        let (status, pending) = fetch_with_pending(
            &github_config(&server),
            &SecretString::from("ghu_checks"),
            &repo(),
            &CommitSha(HEAD.to_owned()),
        )
        .await
        .expect("status");
        assert_eq!(status.state, CiState::Pending);
        assert_eq!(pending, ["build", "PR Hygiene"]);
    }

    #[tokio::test]
    async fn failed_status_is_listed_as_failing() {
        let server = serve_runs(vec![]).await;
        let mut foreign = commit_status("ci/external", "failure");
        foreign["target_url"] = json!("https://ci.example.test/1");
        serve_statuses(&server, vec![commit_status("lint", "error"), foreign]).await;
        let status = fetch_from(&server).await.expect("status");
        assert_eq!(status.state, CiState::Failing);
        let failing: Vec<(&str, Option<&str>)> = status
            .failing
            .iter()
            .map(|check| (check.name.as_str(), check.url.as_deref()))
            .collect();
        assert_eq!(
            failing,
            [
                (
                    "lint",
                    Some("https://github.com/acme/node-client/actions/lint")
                ),
                ("ci/external", None)
            ]
        );
    }

    /// A source GitHub refuses (403/404/5xx, e.g. the App lacks `checks:read`) is not checked; the other still
    /// counts, and CI is then never "passing".
    #[tokio::test]
    async fn an_unreadable_source_is_not_checked_and_never_passing() {
        for code in [403, 404, 502] {
            let server = serve_runs(vec![run("a", "completed", Some("success"))]).await;
            Mock::given(method("GET"))
                .and(path(statuses_path()))
                .respond_with(ResponseTemplate::new(code))
                .mount(&server)
                .await;
            let status = fetch_from(&server).await.expect("status");
            assert_eq!(status.state, CiState::NoFailures, "{code}");
            assert_eq!(status.check_runs, Some(counts(1, 0, 0, 0, 0)));
            assert_eq!(status.statuses, None, "{code}");

            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(checks_path()))
                .respond_with(ResponseTemplate::new(code))
                .mount(&server)
                .await;
            serve_statuses(&server, vec![commit_status("lint", "success")]).await;
            let status = fetch_from(&server).await.expect("status");
            assert_eq!(status.state, CiState::NoFailures, "{code}");
            assert_eq!((status.check_runs, status.total), (None, 1));
        }
        let server = MockServer::start().await;
        let status = fetch_from(&server).await.expect("status");
        assert_eq!(status, NOT_CHECKED, "neither source read");
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
        serve_statuses(&server, vec![]).await;
        let status = fetch_from(&server).await.expect("status");
        assert_eq!((status.state, status.total), (CiState::Passing, 3));
    }

    #[tokio::test]
    async fn status_pages_include_late_failure_and_early_end_is_not_checked() {
        for early_end in [false, true] {
            let server = serve_runs(vec![run("build", "completed", Some("success"))]).await;
            for (page, statuses) in [
                ("1", vec![commit_status("lint", "success")]),
                (
                    "2",
                    if early_end {
                        vec![]
                    } else {
                        vec![commit_status("deploy", "failure")]
                    },
                ),
            ] {
                Mock::given(method("GET"))
                    .and(path(statuses_path()))
                    .and(query_param("page", page))
                    .respond_with(
                        ResponseTemplate::new(200)
                            .set_body_json(json!({"total_count":2,"statuses":statuses})),
                    )
                    .expect(1)
                    .mount(&server)
                    .await;
            }
            let result = fetch_from(&server).await.expect("CI");
            if early_end {
                assert_eq!(result.statuses, None);
                assert_eq!(result.state, CiState::NoFailures);
            } else {
                assert_eq!(result.statuses, Some(counts(1, 1, 0, 0, 0)));
                assert_eq!(result.state, CiState::Failing);
                assert_eq!(result.failing[0].name, "deploy");
            }
        }
    }

    #[tokio::test]
    async fn rejected_token_is_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(checks_path()))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        serve_statuses(&server, vec![]).await;
        let error = fetch_from(&server).await.expect_err("error");
        assert!(matches!(error, GithubError::Unauthorized), "{error:?}");
    }

    #[tokio::test]
    async fn more_runs_than_the_cap_are_not_checked() {
        let server = MockServer::start().await;
        let runs: Vec<Value> = (0..100)
            .map(|n| run(&format!("r{n}"), "completed", Some("success")))
            .collect();
        Mock::given(method("GET"))
            .and(path(checks_path()))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"total_count": 1001, "check_runs": runs})),
            )
            .expect(u64::from(MAX_PAGES))
            .mount(&server)
            .await;
        serve_statuses(&server, vec![]).await;
        let status = fetch_from(&server).await.expect("status");
        assert_eq!(
            (status.state, status.check_runs),
            (CiState::NotChecked, None),
            "the 1001st run may fail; 1000 passing runs are no proof"
        );
    }

    #[test]
    fn failing_tests_come_only_from_unambiguous_failure_formats() {
        let libtest = "running 3 tests\n\
                       test a::passes ... ok\n\
                       test a::ignored ... ignored\n\
                       test a::breaks ... FAILED\n\
                       \n\
                       failures:\n\
                       \n\
                       ---- a::breaks stdout ----\n\
                       thread 'a::breaks' panicked\n\
                       \n\
                       failures:\n    a::breaks\n    b::also_breaks\n\
                       \n\
                       test result: FAILED. 1 passed; 2 failed; 1 ignored\n";
        assert_eq!(failing_tests(libtest), ["a::breaks", "b::also_breaks"]);
        let pytest = "FAILED tests/test_x.py::test_y - AssertionError: 1 != 2\n\
                      PASSED tests/test_x.py::test_z\n\
                      SKIPPED [1] tests/test_x.py:9: no network\n";
        assert_eq!(failing_tests(pytest), ["tests/test_x.py::test_y"]);
        for unknown in [
            "suite::ok: passed\nsuite::bad: failed",
            "sync_headers::slow_peer_2000_headers: timed out after 5 s",
            "Error: process exited with 1",
            "failures:\n    a::x\n  not a name line\n",
        ] {
            assert!(failing_tests(unknown).is_empty(), "{unknown}");
        }
        let many: String = (0..30)
            .map(|n| format!("test m::t{n} ... FAILED\n"))
            .collect();
        assert_eq!(failing_tests(&many).len(), MAX_FAILING_TESTS);
        let long = format!("test {} ... FAILED", "x".repeat(MAX_TEST_NAME_CHARS + 1));
        assert!(
            failing_tests(&long).is_empty(),
            "over-long names are dropped"
        );
        assert_eq!(
            failing_tests("test a::x ... FAILED\nfailures:\n    a::x\n"),
            ["a::x"],
            "no repeats"
        );
    }
}
