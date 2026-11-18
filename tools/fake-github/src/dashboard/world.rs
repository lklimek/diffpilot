//! The synthetic GitHub of `fixtures/dashboard/github`: accounts, and repositories, pull requests and issues
//! as GraphQL nodes (camelCase, connections as `{nodes: [...]}`), plus the REST-only data (releases,
//! milestones, file contents, pull request files, user events). Loading fills the derived fields
//! (`__typename`, `id`, `url`, `repository`, `baseRepository`, empty connections), so fixtures stay short.
//! The fields a type has are the keys its nodes carry anywhere in the world plus the resolver fields
//! (`World::fields`); a query selecting any other field is refused.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};

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
/// Fields answered by resolvers or built by the executor, per type.
const BUILT_FIELDS: &[(&str, &[&str])] = &[
    ("Query", &["repository", "repositoryOwner", "rateLimit"]),
    (
        "Repository",
        &["pullRequest", "issue", "pullRequests", "issues"],
    ),
    ("Organization", &["id", "login", "repositories"]),
    ("User", &["id", "login", "repositories"]),
    (
        "RateLimit",
        &["cost", "limit", "remaining", "used", "resetAt", "nodeCount"],
    ),
    ("Connection", &["totalCount", "nodes", "edges", "pageInfo"]),
    ("Edge", &["cursor", "node"]),
    (
        "PageInfo",
        &["hasNextPage", "hasPreviousPage", "startCursor", "endCursor"],
    ),
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
    /// Known fields per `__typename`.
    pub(crate) fields: HashMap<String, HashSet<String>>,
}

/// Adds the keys of every object with a `__typename` in `value` to `fields`.
fn collect_fields(value: &Value, fields: &mut HashMap<String, HashSet<String>>) {
    match value {
        Value::Object(object) => {
            if let Some(typename) = object.get("__typename").and_then(Value::as_str) {
                fields
                    .entry(typename.to_owned())
                    .or_default()
                    .extend(object.keys().cloned());
            }
            object
                .values()
                .for_each(|item| collect_fields(item, fields));
        }
        Value::Array(items) => items.iter().for_each(|item| collect_fields(item, fields)),
        _ => {}
    }
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
    /// Loads and normalises the fixtures of `dir`.
    pub(crate) fn load(dir: &Path) -> Result<Self, StartError> {
        if !dir.is_dir() {
            return Err(StartError::FixtureDirMissing {
                path: dir.to_path_buf(),
            });
        }
        let accounts: Accounts = load(dir, ACCOUNTS)?;
        let mut repositories: Vec<Value> = load(dir, REPOSITORIES)?;
        let mut pull_requests: Vec<Value> = load(dir, PULL_REQUESTS)?;
        let mut issues: Vec<Value> = load(dir, ISSUES)?;
        let rest: RestData = load(dir, REST)?;
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
        let mut fields: HashMap<String, HashSet<String>> = HashMap::new();
        for node in repositories.iter().chain(&pull_requests).chain(&issues) {
            collect_fields(node, &mut fields);
        }
        for (typename, built) in BUILT_FIELDS {
            fields
                .entry((*typename).to_owned())
                .or_default()
                .extend(built.iter().map(|f| (*f).to_owned()));
        }
        Ok(Self {
            accounts: accounts.accounts,
            repositories,
            pull_requests,
            issues,
            rest,
            fields,
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
