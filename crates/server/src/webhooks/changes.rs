//! Selects subscribed actions and projects verified JSON into durable dispatch hints.

use db::repo::github::{ChangeKind, Effect, InstallationUpdate, Repository, WebhookChange};
use serde_json::Value;

use crate::github::work::parse_repo;

const PR_ACTIONS: &[&str] = &[
    "opened",
    "closed",
    "reopened",
    "synchronize",
    "edited",
    "ready_for_review",
    "converted_to_draft",
    "review_requested",
    "review_request_removed",
    "labeled",
    "unlabeled",
    "assigned",
    "unassigned",
    "milestoned",
    "demilestoned",
    "auto_merge_enabled",
    "auto_merge_disabled",
    "enqueued",
    "dequeued",
];
const ISSUE_ACTIONS: &[&str] = &[
    "opened",
    "edited",
    "closed",
    "reopened",
    "assigned",
    "unassigned",
    "labeled",
    "unlabeled",
    "milestoned",
    "demilestoned",
    "transferred",
    "deleted",
];
const COMMENT_ACTIONS: &[&str] = &["created", "edited", "deleted"];
const FAILED_CHECKS: &[&str] = &["failure", "timed_out", "cancelled", "action_required"];

#[derive(Debug, thiserror::Error)]
#[error("the webhook payload does not contain the required GitHub fields")]
pub(super) struct InvalidPayload;

pub(super) enum Changes {
    Ignore,
    Ping {
        hook_id: Option<i64>,
        events: Vec<String>,
    },
    Signal(Repository, Vec<WebhookChange>),
    Installation(i64, Lifecycle),
}

pub(super) enum Lifecycle {
    Add(Vec<Repository>),
    Remove(Vec<Repository>),
    Delete,
    Suspend(bool),
}

impl Changes {
    pub(super) fn effect(&self) -> Effect<'_> {
        match self {
            Self::Ignore | Self::Ping { .. } => Effect::Ignore,
            Self::Signal(repo, changes) => Effect::Signal(repo, changes),
            Self::Installation(id, action) => Effect::Installation(
                *id,
                match action {
                    Lifecycle::Add(repos) => InstallationUpdate::Add(repos),
                    Lifecycle::Remove(repos) => InstallationUpdate::Remove(repos),
                    Lifecycle::Delete => InstallationUpdate::Delete,
                    Lifecycle::Suspend(suspended) => InstallationUpdate::Suspend(*suspended),
                },
            ),
        }
    }
}

pub(super) fn code(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
}

pub(super) fn positive_id(value: &Value) -> Option<i64> {
    value.as_i64().filter(|id| *id > 0)
}

fn repository(value: &Value) -> Option<Repository> {
    // A malformed object cannot downgrade numeric matching to matching a name.
    let id = positive_id(&value["id"])?;
    let name = value["full_name"]
        .as_str()
        .and_then(parse_repo)
        .or_else(|| repository_url(&value["url"]))?;
    Some(Repository { id: Some(id), name })
}

fn repository_url(value: &Value) -> Option<contracts::common::RepoRef> {
    let url = url::Url::parse(value.as_str()?).ok()?;
    let (_, path) = url.path().rsplit_once("/repos/")?;
    parse_repo(path)
}

pub(super) fn event_repository(body: &Value) -> Option<Repository> {
    match body.get("repository") {
        Some(value) if !value.is_null() => repository(value),
        _ => {
            let base = &body["pull_request"]["base"]["repo"];
            if base.is_object() {
                return repository(base);
            }
            let name = repository_url(&body["issue"]["repository_url"])?;
            Some(Repository { id: None, name })
        }
    }
}

fn number(value: &Value) -> Option<u32> {
    u32::try_from(positive_id(value)?).ok()
}

fn sha(value: &Value) -> Option<String> {
    let sha = value.as_str()?;
    (sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| sha.to_ascii_lowercase())
}

pub(super) fn extract(
    event: &str,
    action: Option<&str>,
    body: &Value,
    installation: Option<i64>,
) -> Result<Changes, InvalidPayload> {
    let action_name = action.unwrap_or("");
    let kind = match event {
        "ping" => {
            let hook_id = positive_id(&body["hook_id"]);
            let events = body["hook"]["events"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|value| code(value))
                .take(64)
                .map(str::to_owned)
                .collect();
            return Ok(Changes::Ping { hook_id, events });
        }
        "installation" | "installation_repositories" => {
            return lifecycle(event, action_name, body, installation);
        }
        "pull_request" if PR_ACTIONS.contains(&action_name) => ChangeKind::PullRequest,
        "pull_request_review" if matches!(action_name, "submitted" | "edited" | "dismissed") => {
            ChangeKind::PullRequestReview
        }
        "pull_request_review_comment" if COMMENT_ACTIONS.contains(&action_name) => {
            ChangeKind::PullRequestReview
        }
        "pull_request_review_thread" if matches!(action_name, "resolved" | "unresolved") => {
            ChangeKind::PullRequestReview
        }
        "issue_comment" if COMMENT_ACTIONS.contains(&action_name) => {
            if body["issue"]["pull_request"].is_object() {
                ChangeKind::PullRequestReview
            } else {
                ChangeKind::Issue
            }
        }
        "issues" if ISSUE_ACTIONS.contains(&action_name) => ChangeKind::Issue,
        "check_suite" if action_name == "completed" => ChangeKind::Check,
        "check_run"
            if action_name == "completed"
                && body["check_run"]["conclusion"]
                    .as_str()
                    .is_some_and(|conclusion| FAILED_CHECKS.contains(&conclusion)) =>
        {
            ChangeKind::Check
        }
        "status" => ChangeKind::Check,
        _ => return Ok(Changes::Ignore),
    };
    let repo = event_repository(body).ok_or(InvalidPayload)?;
    let make = |number, head_repository, head_sha| WebhookChange {
        kind,
        repository: repo.clone(),
        head_repository,
        number,
        head_sha,
        action: action.map(str::to_owned),
        installation_id: installation,
    };
    let changes = if matches!(kind, ChangeKind::Check) {
        let check = &body[event];
        let head_sha = sha(if event == "status" {
            &body["sha"]
        } else {
            &check["head_sha"]
        });
        let mut changes: Vec<_> = check["pull_requests"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|pr| {
                let number = number(&pr["number"])?;
                let head = repository(&pr["head"]["repo"]);
                Some(make(
                    Some(number),
                    head,
                    sha(&pr["head"]["sha"]).or_else(|| head_sha.clone()),
                ))
            })
            .collect();
        if changes.is_empty() {
            changes.push(make(None, None, Some(head_sha.ok_or(InvalidPayload)?)));
        }
        changes
    } else {
        let pr = &body["pull_request"];
        let number = number(&body["number"])
            .or_else(|| number(&pr["number"]))
            .or_else(|| number(&body["issue"]["number"]))
            .ok_or(InvalidPayload)?;
        vec![make(
            Some(number),
            repository(&pr["head"]["repo"]),
            sha(&pr["head"]["sha"]),
        )]
    };
    Ok(Changes::Signal(repo, changes))
}

fn lifecycle(
    event: &str,
    action: &str,
    body: &Value,
    installation: Option<i64>,
) -> Result<Changes, InvalidPayload> {
    let repos = |field: &str| -> Result<Vec<Repository>, InvalidPayload> {
        body[field]
            .as_array()
            .ok_or(InvalidPayload)?
            .iter()
            .map(|value| repository(value).ok_or(InvalidPayload))
            .collect()
    };
    let update = match (event, action) {
        ("installation", "created") => Lifecycle::Add(repos("repositories")?),
        ("installation", "new_permissions_accepted") if body["repositories"].is_array() => {
            Lifecycle::Add(repos("repositories")?)
        }
        ("installation", "new_permissions_accepted") => return Ok(Changes::Ignore),
        ("installation", "deleted") => Lifecycle::Delete,
        ("installation", "suspend") => Lifecycle::Suspend(true),
        ("installation", "unsuspend") => Lifecycle::Suspend(false),
        ("installation_repositories", "added") => Lifecycle::Add(repos("repositories_added")?),
        ("installation_repositories", "removed") => {
            Lifecycle::Remove(repos("repositories_removed")?)
        }
        _ => return Ok(Changes::Ignore),
    };
    Ok(Changes::Installation(
        installation.ok_or(InvalidPayload)?,
        update,
    ))
}
