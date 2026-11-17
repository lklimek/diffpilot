//! The reviewer's GitHub side of an analysis: the review threads and CI status read at ingest (the GitHub
//! snapshot the Tour shows) and the comments in the user's pending review ([`comments`]). GitHub calls live in
//! `github::{threads, checks, review}`, SQL in `db`.

use chrono::Utc;
use contracts::api::analyses::AnalysisSummary;
use contracts::api::tour::{CiStatus, GithubThread, ThreadsStatus};
use contracts::common::PrRef;
use contracts::ids::AnalysisId;
use db::error::DbError;
use db::repo::documents::DocumentKind;
use serde::{Deserialize, Serialize};

use crate::auth::{user_access_token, user_token_rejected};
use crate::github::graphql::GraphqlError;
use crate::github::rest::{self, GithubError};
use crate::github::{checks, threads};
use crate::state::AppState;

pub mod comments;

/// Threads and CI status of the pull request, read once at ingest with the analysis creator's token; stored as
/// the analysis's `github_snapshot` document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubSnapshot {
    /// CI status of the analysed head; `not_checked` when the check runs could not be read.
    pub ci: CiStatus,
    /// Submitted review threads, without step or LLM status (empty when not checked).
    pub threads: Vec<GithubThread>,
    /// Whether `threads` is every thread, the first ones up to the read cap, or nothing could be read.
    pub threads_status: ThreadsStatus,
    /// The pull request description (author-written, untrusted); none when empty or not readable.
    #[serde(default)]
    pub description: Option<String>,
}

impl GithubSnapshot {
    /// Nothing was read.
    pub fn not_checked() -> Self {
        Self {
            ci: checks::NOT_CHECKED,
            threads: Vec::new(),
            threads_status: ThreadsStatus::NotChecked,
            description: None,
        }
    }
}

/// Reads the pull request's threads, description and the head's check runs with `user_id`'s token. Never fails:
/// what could not be read stays "not checked" (logged).
pub async fn read_github_snapshot(
    state: &AppState,
    summary: &AnalysisSummary,
    user_id: &str,
) -> GithubSnapshot {
    let token = match user_access_token(state, user_id).await {
        Ok(token) => token,
        Err(error) => {
            tracing::warn!(analysis = %summary.id, ?error,
                "no GitHub token for the snapshot; threads and CI stay not checked");
            return GithubSnapshot::not_checked();
        }
    };
    let config = &state.config.github;
    let pr = PrRef {
        repo: summary.pr.repo.clone(),
        number: summary.pr.number,
    };
    let (ci, threads, pull) = tokio::join!(
        checks::fetch(config, &token, &pr.repo, &summary.pr.head_sha),
        threads::fetch(config, &token, &pr),
        rest::get_pull(config, &token, &pr),
    );
    if matches!(ci, Err(GithubError::Unauthorized))
        || matches!(threads, Err(GraphqlError::Unauthorized))
        || matches!(pull, Err(GithubError::Unauthorized))
    {
        user_token_rejected(state, user_id, &token).await;
    }
    let description = pull.map_or_else(
        |error| {
            tracing::warn!(analysis = %summary.id, ?error,
                "the pull request description could not be read; the agent works without it");
            None
        },
        |pull| pull.description,
    );
    let ci = ci.unwrap_or_else(|error| {
        tracing::warn!(analysis = %summary.id, ?error,
            "check runs could not be read; CI stays not checked");
        checks::NOT_CHECKED
    });
    let (threads, threads_status) = threads.unwrap_or_else(|error| {
        tracing::warn!(analysis = %summary.id, ?error,
            "review threads could not be read; threads stay not checked");
        (Vec::new(), ThreadsStatus::NotChecked)
    });
    GithubSnapshot {
        ci,
        threads,
        threads_status,
        description,
    }
}

/// Reads the snapshot ([`read_github_snapshot`]) and stores it for the analysis (replacing an earlier one).
pub async fn ingest_github_snapshot(
    state: &AppState,
    summary: &AnalysisSummary,
    user_id: &str,
) -> Result<GithubSnapshot, DbError> {
    let snapshot = read_github_snapshot(state, summary, user_id).await;
    state
        .db
        .documents()
        .put(
            summary.id,
            DocumentKind::GithubSnapshot,
            "",
            &snapshot,
            Utc::now(),
        )
        .await?;
    Ok(snapshot)
}

/// The stored snapshot of the analysis; "not checked" when none was stored.
pub async fn stored_github_snapshot(
    state: &AppState,
    analysis: AnalysisId,
) -> Result<GithubSnapshot, DbError> {
    Ok(state
        .db
        .documents()
        .get(analysis, DocumentKind::GithubSnapshot, "")
        .await?
        .unwrap_or_else(GithubSnapshot::not_checked))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;
    use std::sync::LazyLock;

    use contracts::api::tour::CiState;
    use db::repo::Db;
    use db::repo::analyses::testing::seed_analysis;
    use secrecy::SecretString;
    use serde_json::json;
    use url::Url;
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::auth::{lock_user, store_sign_in_tokens};
    use crate::config::Config;
    use crate::config::tests::test_config;
    use crate::github::app_token::tests::{mount_installation, write_key};
    use crate::github::oauth::TokenGrant;
    use crate::test_support::logs::LogCapture;

    /// User token of the seeded user `u1` in [`github_state`].
    pub(crate) const USER_TOKEN: &str = "ghu_review_secret_token_u1";

    /// The throwaway App key, written once per test process.
    static APP_KEY: LazyLock<(tempfile::TempDir, PathBuf)> = LazyLock::new(|| {
        let dir = tempfile::tempdir().expect("key dir");
        let file = write_key(dir.path());
        (dir, file)
    });

    /// State whose GitHub API is `server` (under `/api`), with an analysis seeded for `u1` and the GitHub App
    /// installed on `acme/node-client`; `with_token` stores [`USER_TOKEN`] as u1's sign-in.
    pub(crate) async fn github_state(
        server: &MockServer,
        with_token: bool,
    ) -> (AppState, AnalysisId) {
        github_state_with(server, with_token, |_| {}).await
    }

    /// [`github_state`] with further configuration changes.
    pub(crate) async fn github_state_with(
        server: &MockServer,
        with_token: bool,
        configure: impl FnOnce(&mut Config),
    ) -> (AppState, AnalysisId) {
        let mut config = test_config();
        config.github.api_url = Url::parse(&format!("{}/api", server.uri())).expect("url");
        config.github.private_key_file = APP_KEY.1.clone();
        configure(&mut config);
        mount_installation(server).await;
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        let state = AppState::with_queue(config, db);
        if with_token {
            let lock = lock_user("u1").await;
            let grant = TokenGrant {
                access_token: SecretString::from(USER_TOKEN),
                expires_in: 8 * 3600,
                refresh_token: SecretString::from("ghr_review_refresh_u1"),
                refresh_token_expires_in: 180 * 86_400,
            };
            store_sign_in_tokens(&state, &lock, db::repo::DEFAULT_TENANT, &grant, Utc::now())
                .await
                .expect("tokens");
        }
        (state, analysis)
    }

    async fn summary(state: &AppState, analysis: AnalysisId) -> AnalysisSummary {
        state
            .db
            .analyses()
            .summary(analysis)
            .await
            .expect("summary")
    }

    #[tokio::test]
    async fn snapshot_without_token_is_not_checked_and_calls_nothing() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let (state, analysis) = github_state(&server, false).await;
        let snapshot = read_github_snapshot(&state, &summary(&state, analysis).await, "u1").await;
        assert_eq!(snapshot, GithubSnapshot::not_checked());
        assert_eq!(snapshot.ci.state, CiState::NotChecked);
    }

    #[tokio::test]
    async fn unreadable_checks_are_not_checked_and_never_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(
                "^/api/repos/acme/node-client/commits/[0-9a-f]+/check-runs$",
            ))
            .respond_with(ResponseTemplate::new(403).insert_header("x-ratelimit-remaining", "0"))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"repository":
                {"pullRequest": {"reviewThreads": {"pageInfo": {"hasNextPage": false, "endCursor": null},
                "nodes": []}}}}})))
            .expect(1)
            .mount(&server)
            .await;
        let (state, analysis) = github_state(&server, true).await;
        let capture = LogCapture::start();
        let snapshot = read_github_snapshot(&state, &summary(&state, analysis).await, "u1").await;
        assert_eq!(snapshot.ci, checks::NOT_CHECKED);
        assert_eq!(
            (snapshot.threads, snapshot.threads_status),
            (Vec::new(), ThreadsStatus::Checked),
            "threads read: none exist"
        );
        let logs = capture.text();
        assert!(logs.contains("CI source could not be read"), "{logs}");
        assert!(!logs.contains(USER_TOKEN), "{logs}");
    }

    /// A GitHub 401 for the user token makes its next use refresh the pair (ADR-007 addendum).
    #[tokio::test]
    async fn github_401_marks_the_user_token_expired() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(
                "^/api/repos/acme/node-client/commits/[0-9a-f]+/check-runs$",
            ))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let (state, analysis) = github_state(&server, true).await;
        let snapshot = read_github_snapshot(&state, &summary(&state, analysis).await, "u1").await;
        assert_eq!(snapshot, GithubSnapshot::not_checked());
        let stored = state
            .db
            .user_tokens()
            .get("u1")
            .await
            .expect("lookup")
            .expect("pair");
        assert!(stored.tokens.access_expires_at <= Utc::now(), "{stored:?}");
    }

    #[tokio::test]
    async fn ingested_snapshot_is_stored_and_read_back() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(
                "^/api/repos/acme/node-client/commits/[0-9a-f]+/check-runs$",
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"total_count": 1,
                "check_runs": [{"name": "ci", "status": "completed", "conclusion": "success",
                "html_url": "https://github.com/acme/node-client/runs/1", "output": null}]})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&server)
            .await;
        let (state, analysis) = github_state(&server, true).await;
        assert_eq!(
            stored_github_snapshot(&state, analysis)
                .await
                .expect("none stored"),
            GithubSnapshot::not_checked()
        );
        let capture = LogCapture::start();
        let ingested = ingest_github_snapshot(&state, &summary(&state, analysis).await, "u1")
            .await
            .expect("ingest");
        assert_eq!(ingested.ci.state, CiState::NoFailures);
        assert_eq!(
            ingested.threads_status,
            ThreadsStatus::NotChecked,
            "threads not read"
        );
        assert_eq!(
            stored_github_snapshot(&state, analysis)
                .await
                .expect("stored"),
            ingested
        );
        assert!(!capture.text().contains(USER_TOKEN));
    }
}
