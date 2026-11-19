//! GitHub REST calls made with the signed-in user's token: pull-request metadata and the App installations the
//! user can access. Also the strict parsing of pull-request URLs and repository names (they become mirror paths
//! and API paths).

use std::time::Duration;

use contracts::api::pr::PrSummary;
use contracts::api::settings::AppInstallation;
use contracts::common::{PrRef, RepoRef};
use contracts::ids::CommitSha;
use reqwest::header::{ACCEPT, USER_AGENT};
use reqwest::{Client, StatusCode, redirect};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use url::Url;

use crate::config::GithubConfig;
use crate::github::oauth::endpoint;

const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const CLIENT_NAME: &str = "diffpilot";
const GITHUB_API_VERSION: &str = "2022-11-28";
const GITHUB_HOSTS: [&str; 2] = ["github.com", "www.github.com"];

/// A GitHub REST call failed.
#[derive(Debug, thiserror::Error)]
pub enum GithubError {
    /// The HTTP client could not be built.
    #[error("the HTTP client for GitHub could not be created")]
    Client(#[source] reqwest::Error),
    /// The request did not complete or the body was not the expected JSON.
    #[error("the request to GitHub failed")]
    Http(#[source] reqwest::Error),
    /// GitHub does not know the item or hides it from this user.
    #[error("GitHub answered 404 for the requested item")]
    NotFound,
    /// GitHub rejected the user token.
    #[error("GitHub rejected the user token")]
    Unauthorized,
    /// GitHub answered with another unexpected status.
    #[error("GitHub answered with HTTP status {0}")]
    Status(StatusCode),
    /// The answer carried a value the server cannot use (e.g. a malformed commit id).
    #[error("GitHub answered with an unusable {field}")]
    Unexpected {
        /// The field.
        field: &'static str,
    },
}

/// A pull request as fetched from GitHub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubPull {
    /// Additions plus deletions and changed files, from the same REST response when available.
    pub size: Option<(u64, u32)>,
    /// The pull request at its current head.
    pub summary: PrSummary,
    /// GitHub id of the base repository.
    pub repo_github_id: i64,
    /// The base repository is private (not supported in v1).
    pub private: bool,
    /// The pull request description (author-written, untrusted); none when empty.
    pub description: Option<String>,
}

#[derive(Deserialize)]
struct PullResponse {
    additions: Option<u64>,
    deletions: Option<u64>,
    changed_files: Option<u32>,
    number: u32,
    title: String,
    html_url: String,
    user: Account,
    head: PullSide,
    base: PullBase,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Deserialize)]
struct Account {
    login: String,
}

#[derive(Deserialize)]
struct PullSide {
    sha: String,
}

#[derive(Deserialize)]
struct PullBase {
    sha: String,
    repo: RepoResponse,
}

#[derive(Deserialize)]
struct RepoResponse {
    id: i64,
    name: String,
    owner: Account,
    private: bool,
}

fn client() -> Result<Client, GithubError> {
    Client::builder()
        .redirect(redirect::Policy::none())
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(GithubError::Client)
}

/// `GET /repos/{owner}/{name}/pulls/{number}` with the user's token.
pub async fn get_pull(
    config: &GithubConfig,
    token: &SecretString,
    pr: &PrRef,
) -> Result<GithubPull, GithubError> {
    let url = endpoint(
        &config.api_url,
        &format!(
            "repos/{}/{}/pulls/{}",
            pr.repo.owner, pr.repo.name, pr.number
        ),
    );
    let response = client()?
        .get(url)
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
    let pull: PullResponse = response.json().await.map_err(GithubError::Http)?;
    let repo = RepoRef {
        owner: pull.base.repo.owner.login,
        name: pull.base.repo.name,
    };
    if !is_valid_repo(&repo) {
        return Err(GithubError::Unexpected {
            field: "repository",
        });
    }
    Ok(GithubPull {
        size: pull
            .additions
            .zip(pull.deletions)
            .and_then(|(a, d)| a.checked_add(d))
            .zip(pull.changed_files),
        summary: PrSummary {
            repo,
            number: pull.number,
            title: pull.title,
            author: pull.user.login,
            html_url: pull.html_url,
            base_sha: commit_sha(pull.base.sha)?,
            head_sha: commit_sha(pull.head.sha)?,
        },
        repo_github_id: pull.base.repo.id,
        private: pull.base.repo.private,
        description: pull.body.filter(|body| !body.trim().is_empty()),
    })
}

#[derive(Deserialize)]
struct InstallationsPage {
    total_count: u32,
    installations: Vec<InstallationEntry>,
}

#[derive(Deserialize)]
struct InstallationEntry {
    account: InstallationAccount,
    repository_selection: String,
    suspended_at: Option<String>,
}

/// A user or organisation (`login`) or an enterprise (`slug`).
#[derive(Deserialize)]
struct InstallationAccount {
    login: Option<String>,
    slug: Option<String>,
}

/// Installations per page (GitHub's maximum) and the most pages read.
const INSTALLATIONS_PER_PAGE: u32 = 100;
const INSTALLATION_PAGES: u32 = 10;

/// `GET /user/installations` with the user's token: the App's installations the user can access, suspended
/// ones left out.
pub async fn user_installations(
    config: &GithubConfig,
    token: &SecretString,
) -> Result<Vec<AppInstallation>, GithubError> {
    let client = client()?;
    let mut installations = Vec::new();
    let mut seen = 0;
    for page in 1..=INSTALLATION_PAGES {
        let mut url = endpoint(&config.api_url, "user/installations");
        url.query_pairs_mut()
            .append_pair("per_page", &INSTALLATIONS_PER_PAGE.to_string())
            .append_pair("page", &page.to_string());
        let response = client
            .get(url)
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
        let body: InstallationsPage = response.json().await.map_err(GithubError::Http)?;
        let count = u32::try_from(body.installations.len()).unwrap_or(u32::MAX);
        seen += count;
        for entry in body.installations {
            if entry.suspended_at.is_some() {
                continue;
            }
            let all_repositories = match entry.repository_selection.as_str() {
                "all" => true,
                "selected" => false,
                _ => {
                    return Err(GithubError::Unexpected {
                        field: "repository selection",
                    });
                }
            };
            let account =
                entry
                    .account
                    .login
                    .or(entry.account.slug)
                    .ok_or(GithubError::Unexpected {
                        field: "installation account",
                    })?;
            installations.push(AppInstallation {
                account,
                all_repositories,
            });
        }
        if count == 0 || seen >= body.total_count {
            return Ok(installations);
        }
    }
    Err(GithubError::Unexpected {
        field: "installation count",
    })
}

fn commit_sha(sha: String) -> Result<CommitSha, GithubError> {
    let valid = matches!(sha.len(), 40 | 64)
        && sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if valid {
        Ok(CommitSha(sha))
    } else {
        Err(GithubError::Unexpected { field: "commit id" })
    }
}

/// GitHub owner (user or organisation) name: 1–39 letters, digits or hyphens.
fn is_valid_owner(owner: &str) -> bool {
    (1..=39).contains(&owner.len())
        && owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// GitHub repository name: 1–100 letters, digits, `.`, `_` or `-`; never `.` or `..`.
fn is_valid_name(name: &str) -> bool {
    (1..=100).contains(&name.len())
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Owner and name are safe to use in API paths and mirror directories.
pub fn is_valid_repo(repo: &RepoRef) -> bool {
    is_valid_owner(&repo.owner) && is_valid_name(&repo.name)
}

/// Parses `https://github.com/<owner>/<name>/pull/<number>[/…][?…][#…]` (scheme optional); `None` for
/// anything else.
pub fn parse_pr_url(input: &str) -> Option<PrRef> {
    let input = input.trim();
    let url = if input.contains("://") {
        Url::parse(input).ok()?
    } else {
        Url::parse(&format!("https://{input}")).ok()?
    };
    if !matches!(url.scheme(), "https" | "http")
        || !GITHUB_HOSTS.contains(&url.host_str()?)
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let mut segments = url.path_segments()?;
    let repo = RepoRef {
        owner: segments.next()?.to_owned(),
        name: segments.next()?.to_owned(),
    };
    if segments.next()? != "pull" || !is_valid_repo(&repo) {
        return None;
    }
    let number = segments.next()?;
    if number.starts_with('0') || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number: u32 = number.parse().ok()?;
    Some(PrRef { repo, number })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use serde_json::Value;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::config::tests::test_config;

    fn pr(owner: &str, name: &str, number: u32) -> PrRef {
        PrRef {
            repo: RepoRef {
                owner: owner.to_owned(),
                name: name.to_owned(),
            },
            number,
        }
    }

    #[test]
    fn pr_urls_are_parsed_strictly() {
        for (input, expected) in [
            (
                "https://github.com/acme/node-client/pull/482",
                pr("acme", "node-client", 482),
            ),
            (
                " github.com/dashpay/dash-evo-tool/pull/1040/files?w=1#diff ",
                pr("dashpay", "dash-evo-tool", 1040),
            ),
            (
                "http://www.github.com/a-b/c.d_e/pull/7/",
                pr("a-b", "c.d_e", 7),
            ),
        ] {
            assert_eq!(parse_pr_url(input), Some(expected), "{input}");
        }
        for input in [
            "",
            "https://gitlab.com/acme/node-client/pull/482",
            "https://github.com.evil.test/acme/node-client/pull/482",
            "https://user@github.com/acme/node-client/pull/482",
            "https://github.com:8443/acme/node-client/pull/482",
            "https://github.com/acme/node-client/issues/482",
            "https://github.com/acme/node-client/pull/",
            "https://github.com/acme/node-client/pull/0",
            "https://github.com/acme/node-client/pull/-1",
            "https://github.com/acme/node-client/pull/1e3",
            "https://github.com/acme/node-client/pull/99999999999",
            "https://github.com/acme/../pull/1",
            "https://github.com/ac%2Fme/node-client/pull/1",
            "https://github.com/acme/no de/pull/1",
            "file:///etc/passwd",
        ] {
            assert_eq!(parse_pr_url(input), None, "{input}");
        }
    }

    fn fixture(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/pr-482/github/rest")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("JSON")
    }

    fn fixture_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482")
    }

    /// Serves a recorded `{request, status, response}` fixture at its path under `/api`.
    async fn serve(server: &MockServer, name: &str, token: &str) {
        let recorded = fixture(name);
        let url = Url::parse(recorded["request"]["url"].as_str().expect("url")).expect("URL");
        Mock::given(method(
            recorded["request"]["method"].as_str().expect("method"),
        ))
        .and(path(format!("/api{}", url.path())))
        .and(header("authorization", format!("Bearer {token}").as_str()))
        .and(header("x-github-api-version", GITHUB_API_VERSION))
        .respond_with(
            ResponseTemplate::new(
                u16::try_from(recorded["status"].as_u64().expect("status")).expect("u16"),
            )
            .set_body_json(&recorded["response"]),
        )
        .mount(server)
        .await;
    }

    #[tokio::test]
    async fn get_pull_reads_the_recorded_fake_github_answer() {
        let server = MockServer::start().await;
        serve(&server, "pull.json", "ghu_fake_user_token").await;
        let mut config = test_config();
        config.github.api_url = Url::parse(&format!("{}/api", server.uri())).expect("url");
        let commits: Value = serde_json::from_str(
            &std::fs::read_to_string(fixture_dir().join("repo/commits.json")).expect("commits"),
        )
        .expect("JSON");

        let pull = get_pull(
            &config.github,
            &SecretString::from("ghu_fake_user_token"),
            &pr("acme", "node-client", 482),
        )
        .await
        .expect("pull");

        let resolve = &serde_json::from_str::<Value>(
            &std::fs::read_to_string(fixture_dir().join("api-seed/default/pr-resolve.json"))
                .expect("seed"),
        )
        .expect("JSON")["pr"];
        let expected: PrSummary = serde_json::from_value(resolve.clone()).expect("PrSummary");
        assert_eq!(pull.summary, expected);
        assert_eq!(
            pull.summary.base_sha.0,
            commits["base"].as_str().expect("base")
        );
        assert_eq!(
            pull.summary.head_sha.0,
            commits["head"].as_str().expect("head")
        );
        assert!(!pull.private);
        assert_eq!(pull.repo_github_id, 4_821_400);
    }

    #[tokio::test]
    async fn get_pull_maps_not_found_and_rejected_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/repos/acme/node-client/pulls/1"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/repos/acme/node-client/pulls/2"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/repos/acme/node-client/pulls/3"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&server)
            .await;
        let mut config = test_config();
        config.github.api_url = Url::parse(&format!("{}/api", server.uri())).expect("url");
        let token = SecretString::from("t");
        let result = |n| {
            let (github, token) = (&config.github, &token);
            async move { get_pull(github, token, &pr_ref(n)).await }
        };
        assert!(matches!(result(1).await, Err(GithubError::NotFound)));
        assert!(matches!(result(2).await, Err(GithubError::Unauthorized)));
        assert!(matches!(
            result(3).await,
            Err(GithubError::Status(StatusCode::BAD_GATEWAY))
        ));
    }

    fn pr_ref(number: u32) -> PrRef {
        pr("acme", "node-client", number)
    }

    fn installation(account: Value, selection: &str, suspended: bool) -> Value {
        serde_json::json!({
            "id": 1,
            "account": account,
            "repository_selection": selection,
            "suspended_at": suspended.then_some("2026-11-01T00:00:00Z"),
        })
    }

    #[tokio::test]
    async fn user_installations_reads_the_recorded_answer() {
        let server = MockServer::start().await;
        serve(&server, "user-installations.json", "ghu_fake_user_token").await;
        let mut config = test_config();
        config.github.api_url = Url::parse(&format!("{}/api", server.uri())).expect("url");
        let installations =
            user_installations(&config.github, &SecretString::from("ghu_fake_user_token")).await;
        assert_eq!(
            installations.expect("installations"),
            [AppInstallation {
                account: "acme".to_owned(),
                all_repositories: false,
            }]
        );
    }

    #[tokio::test]
    async fn user_installations_pages_and_skips_suspended_ones() {
        let server = MockServer::start().await;
        let first: Vec<Value> = (0..100)
            .map(|n| {
                installation(
                    serde_json::json!({ "login": format!("org-{n}") }),
                    "selected",
                    n == 7,
                )
            })
            .collect();
        let second = vec![installation(
            serde_json::json!({ "slug": "big-corp" }),
            "all",
            false,
        )];
        for (page, installations) in [("1", first), ("2", second)] {
            Mock::given(method("GET"))
                .and(path("/api/user/installations"))
                .and(wiremock::matchers::query_param("per_page", "100"))
                .and(wiremock::matchers::query_param("page", page))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({ "total_count": 101, "installations": installations }),
                ))
                .mount(&server)
                .await;
        }
        let mut config = test_config();
        config.github.api_url = Url::parse(&format!("{}/api", server.uri())).expect("url");
        let installations = user_installations(&config.github, &SecretString::from("t"))
            .await
            .expect("installations");
        assert_eq!(installations.len(), 100, "100 listed, one suspended");
        assert!(!installations.iter().any(|i| i.account == "org-7"));
        assert_eq!(
            installations.last(),
            Some(&AppInstallation {
                account: "big-corp".to_owned(),
                all_repositories: true,
            })
        );
    }

    #[tokio::test]
    async fn user_installations_maps_a_rejected_token_and_bad_answers() {
        let server = MockServer::start().await;
        let mut config = test_config();
        config.github.api_url = Url::parse(&format!("{}/api", server.uri())).expect("url");
        let token = SecretString::from("t");
        Mock::given(method("GET"))
            .and(path("/api/user/installations"))
            .respond_with(ResponseTemplate::new(401))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        assert!(matches!(
            user_installations(&config.github, &token).await,
            Err(GithubError::Unauthorized)
        ));
        Mock::given(method("GET"))
            .and(path("/api/user/installations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "total_count": 1,
                "installations": [installation(serde_json::json!({ "login": "acme" }), "some", false)],
            })))
            .mount(&server)
            .await;
        assert!(matches!(
            user_installations(&config.github, &token).await,
            Err(GithubError::Unexpected { .. })
        ));
    }
}
