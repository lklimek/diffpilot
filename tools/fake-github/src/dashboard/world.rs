//! The synthetic GitHub of `fixtures/dashboard/github`: accounts, and repositories, pull requests and issues
//! as GraphQL nodes (camelCase, connections as `{nodes: [...]}`), plus the REST-only data (releases,
//! milestones, file contents, pull request files, user events). Loading fills the derived fields
//! (`__typename`, `id`, `url`, `repository`, `baseRepository`, empty connections), so fixtures stay short.
//! Its type map (`World::schema`) validates documents.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::schema::Schema;
use crate::StartError;
use crate::fixtures::load;

const ACCOUNTS: &str = "accounts.json";
const REPOSITORIES: &str = "repositories.json";
const PULL_REQUESTS: &str = "pull-requests.json";
const ISSUES: &str = "issues.json";
const REST: &str = "rest.json";

/// Connections every pull request answers (empty unless the fixture lists nodes).
const PULL_CONNECTIONS: &[&str] = &[
    "labels",
    "assignees",
    "reviewRequests",
    "reviews",
    "comments",
    "reviewThreads",
    "timelineItems",
    "closingIssuesReferences",
    "commits",
];
/// Connections every issue answers.
const ISSUE_CONNECTIONS: &[&str] = &["labels", "assignees", "comments"];

/// Kind of a GitHub account.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub(crate) enum AccountKind {
    User,
    Bot,
    Organization,
}

/// A login known to the fake.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Account {
    pub(crate) login: String,
    pub(crate) id: u64,
    #[serde(rename = "type")]
    pub(crate) kind: AccountKind,
    /// `false`: GitHub search refuses the login ("cannot be searched").
    #[serde(default = "searchable_default")]
    pub(crate) searchable: bool,
}

fn searchable_default() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Accounts {
    accounts: Vec<Account>,
}

/// REST-only data, keyed by `owner/name` (files by `owner/name#number`, events by login).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestData {
    #[serde(default)]
    pub(crate) releases: HashMap<String, Vec<Value>>,
    #[serde(default)]
    pub(crate) milestones: HashMap<String, Vec<Value>>,
    /// Repository → path → content (a JSON string is served as is, other JSON pretty-printed).
    #[serde(default)]
    pub(crate) contents: HashMap<String, HashMap<String, Value>>,
    #[serde(default)]
    pub(crate) files: HashMap<String, Vec<Value>>,
    #[serde(default)]
    pub(crate) events: HashMap<String, Vec<Value>>,
}

/// The loaded world.
pub(crate) struct World {
    pub(crate) accounts: Vec<Account>,
    pub(crate) repositories: Vec<Value>,
    pub(crate) pull_requests: Vec<Value>,
    pub(crate) issues: Vec<Value>,
    pub(crate) rest: RestData,
    /// Types and fields of the nodes, for document validation.
    pub(crate) schema: Schema,
}

/// `owner/name` of a repository, pull request or issue node.
pub(crate) fn name_with_owner(node: &Value) -> &str {
    node["nameWithOwner"]
        .as_str()
        .or_else(|| node["repository"]["nameWithOwner"].as_str())
        .unwrap_or_default()
}

/// The node's `number`.
pub(crate) fn number(node: &Value) -> u64 {
    node["number"].as_u64().unwrap_or_default()
}

impl World {
    /// Loads and normalises the fixtures of `dir`, every date moved by `shift_seconds`.
    pub(crate) fn load(dir: &Path, shift_seconds: i64) -> Result<Self, StartError> {
        if !dir.is_dir() {
            return Err(StartError::FixtureDirMissing {
                path: dir.to_path_buf(),
            });
        }
        let accounts: Accounts = load_shifted(dir, ACCOUNTS, shift_seconds)?;
        let mut repositories: Vec<Value> = load_shifted(dir, REPOSITORIES, shift_seconds)?;
        let mut pull_requests: Vec<Value> = load_shifted(dir, PULL_REQUESTS, shift_seconds)?;
        let mut issues: Vec<Value> = load_shifted(dir, ISSUES, shift_seconds)?;
        let rest: RestData = load_shifted(dir, REST, shift_seconds)?;
        for repository in &mut repositories {
            let Some(object) = repository.as_object_mut().filter(|r| {
                r.get("nameWithOwner")
                    .and_then(Value::as_str)
                    .is_some_and(|n| n.contains('/'))
            }) else {
                return Err(bad(dir, REPOSITORIES, "nameWithOwner"));
            };
            object
                .entry("__typename")
                .or_insert_with(|| json!("Repository"));
        }
        for (file, nodes, typename, path, connections) in [
            (
                PULL_REQUESTS,
                &mut pull_requests,
                "PullRequest",
                "pull",
                PULL_CONNECTIONS,
            ),
            (ISSUES, &mut issues, "Issue", "issues", ISSUE_CONNECTIONS),
        ] {
            for node in nodes.iter_mut() {
                let repository = repositories
                    .iter()
                    .find(|r| r["nameWithOwner"] == node["repository"]["nameWithOwner"])
                    .ok_or_else(|| bad(dir, file, "repository.nameWithOwner"))?;
                if node["number"].as_u64().is_none() {
                    return Err(bad(dir, file, "number"));
                }
                normalise(node, repository, typename, path, connections);
            }
        }
        let schema = Schema::build(repositories.iter().chain(&pull_requests).chain(&issues));
        Ok(Self {
            accounts: accounts.accounts,
            repositories,
            pull_requests,
            issues,
            rest,
            schema,
        })
    }

    /// The account of `login` (case-insensitive).
    pub(crate) fn account(&self, login: &str) -> Option<&Account> {
        self.accounts
            .iter()
            .find(|account| account.login.eq_ignore_ascii_case(login))
    }

    /// The repository `owner/name` (case-insensitive) if the caller can see it (private ones need a token).
    pub(crate) fn repository(&self, name: &str, authenticated: bool) -> Option<&Value> {
        self.repositories.iter().find(|repository| {
            name_with_owner(repository).eq_ignore_ascii_case(name)
                && (authenticated || repository["isPrivate"] != true)
        })
    }

    /// The pull request `repository#number`.
    pub(crate) fn pull_request(&self, repository: &str, number: u64) -> Option<&Value> {
        find(&self.pull_requests, repository, number)
    }

    /// The issue `repository#number`.
    pub(crate) fn issue(&self, repository: &str, number: u64) -> Option<&Value> {
        find(&self.issues, repository, number)
    }
}

/// The fixture `file` of `dir` with every date moved by `shift_seconds`.
fn load_shifted<T: DeserializeOwned>(
    dir: &Path,
    file: &str,
    shift_seconds: i64,
) -> Result<T, StartError> {
    let mut value: Value = load(dir, file)?;
    if shift_seconds != 0 {
        shift_dates(&mut value, shift_seconds);
    }
    serde_json::from_value(value).map_err(|source| StartError::ParseFixture {
        path: dir.join(file),
        source,
    })
}

/// Moves every string that is a whole-second UTC date (`2026-11-18T09:50:00Z`, the fixtures' only date form).
fn shift_dates(value: &mut Value, shift_seconds: i64) {
    match value {
        Value::String(text) => {
            let moved = (text.len() == 20 && text.ends_with('Z'))
                .then(|| text.parse::<jiff::Timestamp>().ok())
                .flatten()
                .and_then(|at| {
                    at.checked_add(jiff::SignedDuration::from_secs(shift_seconds))
                        .ok()
                });
            if let Some(moved) = moved {
                *text = moved.to_string();
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| shift_dates(item, shift_seconds)),
        Value::Object(fields) => fields
            .values_mut()
            .for_each(|field| shift_dates(field, shift_seconds)),
        _ => {}
    }
}

fn find<'a>(nodes: &'a [Value], repository: &str, wanted: u64) -> Option<&'a Value> {
    nodes.iter().find(|node| {
        name_with_owner(node).eq_ignore_ascii_case(repository) && number(node) == wanted
    })
}

fn bad(dir: &Path, file: &str, field: &'static str) -> StartError {
    StartError::BadRecording {
        path: dir.join(file),
        field,
    }
}

/// Fills the derived fields of a pull request or issue node.
fn normalise(
    node: &mut Value,
    repository: &Value,
    typename: &str,
    path: &str,
    connections: &[&str],
) {
    let nwo = name_with_owner(repository).to_owned();
    let number = number(node);
    let defaults = [
        ("__typename", json!(typename)),
        (
            "id",
            json!(format!(
                "{}_{}_{number}",
                if typename == "Issue" { "I" } else { "PR" },
                nwo.replace('/', "_")
            )),
        ),
        (
            "url",
            json!(format!("https://github.com/{nwo}/{path}/{number}")),
        ),
        ("body", json!("")),
        ("closedAt", Value::Null),
        ("mergedAt", Value::Null),
        ("isDraft", json!(false)),
        ("isCrossRepository", json!(false)),
    ];
    let Some(object) = node.as_object_mut() else {
        return;
    };
    for (key, value) in defaults {
        if typename == "Issue" && matches!(key, "mergedAt" | "isDraft" | "isCrossRepository") {
            continue;
        }
        object.entry(key).or_insert(value);
    }
    let base = format!("https://github.com/{nwo}/{path}/{number}");
    for (connection, prefix) in [
        ("comments", "issuecomment-"),
        ("reviews", "pullrequestreview-"),
    ] {
        if let Some(posts) = object
            .get_mut(connection)
            .and_then(|v| v["nodes"].as_array_mut())
        {
            for (index, post) in posts.iter_mut().enumerate() {
                if let Some(post) = post.as_object_mut() {
                    post.entry("url")
                        .or_insert_with(|| json!(format!("{base}#{prefix}{}", index + 1)));
                    post.entry("body").or_insert_with(|| json!(""));
                }
            }
        }
    }
    if let Some(threads) = object
        .get_mut("reviewThreads")
        .and_then(|v| v["nodes"].as_array_mut())
    {
        for (thread_index, thread) in threads.iter_mut().enumerate() {
            if let Some(posts) = thread["comments"]["nodes"].as_array_mut() {
                for (index, post) in posts.iter_mut().enumerate() {
                    if let Some(post) = post.as_object_mut() {
                        post.entry("url").or_insert_with(|| {
                            json!(format!(
                                "{base}#discussion_r{}",
                                thread_index * 1000 + index + 1
                            ))
                        });
                    }
                }
            }
        }
    }
    object.insert("repository".to_owned(), repository.clone());
    if typename == "PullRequest" {
        object.insert("baseRepository".to_owned(), repository.clone());
    }
    for connection in connections {
        object
            .entry(*connection)
            .or_insert_with(|| json!({"nodes": []}));
    }
}
