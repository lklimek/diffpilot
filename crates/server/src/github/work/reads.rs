//! REST reads: ownership policy contents (Q3), pull request files (Q4), latest release (Q7), releases and
//! milestones (Q8) and a login's public events (Q14).

use chrono::{DateTime, Utc};
use contracts::common::RepoRef;
use serde::Deserialize;

use super::client::{TokenSource, WorkClient, WorkError};
use super::{ItemRef, valid_login};
use crate::github::rest::is_valid_repo;

/// GitHub serves at most 3000 files of a pull request.
const FILES_CAP: usize = 3000;
/// GitHub serves at most 300 events of a user.
const EVENTS_CAP: usize = 300;
/// Event types about pull requests (Q14: pull request throughput of unsearchable logins).
const PR_EVENT_TYPES: [&str; 2] = ["PullRequestEvent", "PullRequestReviewEvent"];

/// A changed file of a pull request.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PullFile {
    /// Path.
    pub filename: String,
    /// `added`, `modified`, `removed`, `renamed`, …
    pub status: String,
    /// Added lines.
    pub additions: u64,
    /// Deleted lines.
    pub deletions: u64,
    /// The diff; absent for binary or very large files.
    #[serde(default)]
    pub patch: Option<String>,
}

/// A release.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Release {
    /// Tag.
    pub tag_name: String,
    /// Draft.
    #[serde(default)]
    pub draft: bool,
    /// Prerelease.
    #[serde(default)]
    pub prerelease: bool,
    /// Publication time.
    #[serde(default)]
    pub published_at: Option<DateTime<Utc>>,
}

/// A milestone as REST lists it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RestMilestone {
    /// Number.
    pub number: u32,
    /// Title.
    pub title: String,
    /// `open` or `closed`.
    pub state: String,
    /// Due date.
    #[serde(default)]
    pub due_on: Option<DateTime<Utc>>,
}

/// A pull request event of a login.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserEvent {
    /// `PullRequestEvent` or `PullRequestReviewEvent`.
    pub kind: String,
    /// The pull request.
    pub item: ItemRef,
    /// `opened`, `closed`, `created`, …
    pub action: Option<String>,
    /// For a closing `PullRequestEvent`: merged.
    pub merged: Option<bool>,
    /// When.
    pub created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct RawEvent {
    #[serde(rename = "type")]
    kind: Option<String>,
    repo: RawRepo,
    #[serde(default)]
    payload: serde_json::Value,
    created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct RawRepo {
    name: String,
}

/// `repos/<owner>/<name>/<rest>`; names GitHub does not allow never reach a path.
fn repo_path(repo: &RepoRef, rest: &str) -> Result<String, WorkError> {
    if !is_valid_repo(repo) {
        return Err(WorkError::NotFound);
    }
    Ok(format!("repos/{}/{}/{rest}", repo.owner, repo.name))
}

/// A relative path whose segments use only `[A-Za-z0-9._-]` and are not `.` or `..` (settings G6). `%` and `\`
/// are refused, so nothing the URL parser decodes or normalises can climb out of the repository.
fn valid_path(path: &str) -> bool {
    path.split('/').all(|segment| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    })
}

impl<T: TokenSource> WorkClient<T> {
    /// The raw text of `path` in `repo` (Q3, ownership policy); [`WorkError::NotFound`] when absent. `path` must
    /// be relative without `..` (settings G6).
    pub async fn file_contents(&self, repo: &RepoRef, path: &str) -> Result<String, WorkError> {
        if !valid_path(path) {
            return Err(WorkError::NotFound);
        }
        self.rest_raw(&repo_path(repo, &format!("contents/{path}"))?)
            .await
    }

    /// Changed files of a pull request (Q4), every page.
    pub async fn pull_files(&self, pr: &ItemRef) -> Result<Vec<PullFile>, WorkError> {
        self.rest_pages(
            &repo_path(&pr.repo, &format!("pulls/{}/files", pr.number))?,
            &[],
            FILES_CAP,
        )
        .await
    }

    /// The latest published release (Q7); `None` when the repository has none.
    pub async fn latest_release(&self, repo: &RepoRef) -> Result<Option<Release>, WorkError> {
        match self
            .rest_json(&repo_path(repo, "releases/latest")?, &[])
            .await
        {
            Ok(release) => Ok(Some(release)),
            Err(WorkError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Every release (Q8).
    pub async fn releases(&self, repo: &RepoRef) -> Result<Vec<Release>, WorkError> {
        self.rest_pages(&repo_path(repo, "releases")?, &[], usize::MAX)
            .await
    }

    /// Every milestone, open and closed (Q8).
    pub async fn milestones(&self, repo: &RepoRef) -> Result<Vec<RestMilestone>, WorkError> {
        self.rest_pages(
            &repo_path(repo, "milestones")?,
            &[("state", "all".to_owned())],
            usize::MAX,
        )
        .await
    }

    /// Pull request events among the last 300 public events of `login` (Q14; coverage is partial).
    pub async fn user_events(&self, login: &str) -> Result<Vec<UserEvent>, WorkError> {
        if !valid_login(login) {
            return Err(WorkError::NotFound);
        }
        let raw: Vec<RawEvent> = self
            .rest_pages(&format!("users/{login}/events"), &[], EVENTS_CAP)
            .await?;
        Ok(raw
            .into_iter()
            .filter_map(|event| {
                let kind = event
                    .kind
                    .filter(|kind| PR_EVENT_TYPES.contains(&kind.as_str()))?;
                let pull = &event.payload["pull_request"];
                let number = pull["number"]
                    .as_u64()
                    .or_else(|| event.payload["number"].as_u64())
                    .and_then(|n| u32::try_from(n).ok())?;
                Some(UserEvent {
                    kind,
                    item: ItemRef::parse(&event.repo.name, number)?,
                    action: event.payload["action"].as_str().map(str::to_owned),
                    merged: pull["merged"].as_bool(),
                    created_at: event.created_at,
                })
            })
            .collect())
    }
}
