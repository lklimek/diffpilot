//! Executes a parsed dashboard document against the world: root lookups (`repository`, `repositoryOwner`,
//! `rateLimit`), a repository's `pullRequest`/`issue` lookups and `pullRequests`/`issues` listings, an owner's
//! `repositories`, `reviews(author:, states:)`, and GitHub's connection rules (`first`/`last` within 1–100 when
//! `nodes`/`edges` are selected, opaque cursors). Every other field is read from the node; a missing one is `null`.

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};

use super::query::{Document, Field, Fragment, Kind, Selection};
use super::world::{AccountKind, World, name_with_owner};

/// Type names that are concrete objects; a type condition naming anything else (an interface) always applies.
const CONCRETE: &[&str] = &[
    "Query",
    "Repository",
    "PullRequest",
    "Issue",
    "User",
    "Bot",
    "Organization",
    "Team",
    "Mannequin",
    "CheckRun",
    "StatusContext",
    "ReviewRequestedEvent",
    "PullRequestReview",
    "RateLimit",
];
/// Deepest fragment nesting followed (guards against fragment cycles).
const MAX_FRAGMENT_DEPTH: usize = 16;
/// GitHub's page-size limit of a connection.
const MAX_PAGE: i64 = 100;

/// Fields of `selections` that apply to an object of `typename`, fragments expanded, in document order.
pub(crate) fn collect_fields<'a>(
    selections: &'a [Selection],
    typename: Option<&str>,
    fragments: &'a HashMap<String, Fragment>,
) -> Vec<&'a Field> {
    let mut out = Vec::new();
    collect_into(selections, typename, fragments, 0, &mut out);
    out
}

fn collect_into<'a>(
    selections: &'a [Selection],
    typename: Option<&str>,
    fragments: &'a HashMap<String, Fragment>,
    depth: usize,
    out: &mut Vec<&'a Field>,
) {
    if depth > MAX_FRAGMENT_DEPTH {
        return;
    }
    let applies = |on: &str| typename.is_none_or(|t| t == on || !CONCRETE.contains(&on));
    for selection in selections {
        match selection {
            Selection::Field(field) => out.push(field),
            Selection::Inline { on, selections } => {
                if on.as_deref().is_none_or(applies) {
                    collect_into(selections, typename, fragments, depth + 1, out);
                }
            }
            Selection::Spread(name) => {
                if let Some(fragment) = fragments.get(name).filter(|f| applies(&f.on)) {
                    collect_into(&fragment.selections, typename, fragments, depth + 1, out);
                }
            }
        }
    }
}

/// Whether the document is a dashboard document: a query whose top-level fields are only `repository`,
/// `repositoryOwner` and `rateLimit`, with an alias, `rateLimit`, `repositoryOwner`, or a repository
/// `pullRequests`/`issues`/`issue` field. The v1 review documents (one unaliased `repository` or `node`, or a
/// mutation) never match.
pub(crate) fn is_dashboard(doc: &Document) -> bool {
    let root = collect_fields(&doc.selections, Some("Query"), &doc.fragments);
    doc.kind == Kind::Query
        && !root.is_empty()
        && root.iter().all(|f| {
            matches!(
                f.name.as_str(),
                "repository" | "repositoryOwner" | "rateLimit"
            )
        })
        && root.iter().any(|f| {
            f.alias.is_some()
                || f.name != "repository"
                || collect_fields(&f.selections, Some("Repository"), &doc.fragments)
                    .iter()
                    .any(|child| matches!(child.name.as_str(), "pullRequests" | "issues" | "issue"))
        })
}

/// Size of a batch: top-level lookups (without `rateLimit`) and `pullRequest(number:)` lookups among them.
pub(crate) fn batch_size(doc: &Document) -> (usize, usize) {
    let root = collect_fields(&doc.selections, Some("Query"), &doc.fragments);
    let lookups = root.iter().filter(|f| f.name != "rateLimit").count();
    let pulls = root
        .iter()
        .filter(|f| f.name == "repository")
        .map(|f| {
            collect_fields(&f.selections, Some("Repository"), &doc.fragments)
                .iter()
                .filter(|child| child.name == "pullRequest")
                .count()
        })
        .sum();
    (lookups, pulls)
}

/// The pull request whose head moves with `POST /_fake/head`: (`owner/name`, number, current head).
pub(crate) type MovedHead = Option<(String, u64, String)>;

struct Executor<'a> {
    world: &'a World,
    fragments: &'a HashMap<String, Fragment>,
    rate_limit: Value,
    moved_head: &'a MovedHead,
    errors: Vec<Value>,
}

/// Runs `doc` and returns GitHub's response body (`data`, and `errors` when a lookup failed).
pub(crate) fn execute(
    doc: &Document,
    world: &World,
    rate_limit: Value,
    moved_head: &MovedHead,
) -> Value {
    let mut executor = Executor {
        world,
        fragments: &doc.fragments,
        rate_limit,
        moved_head,
        errors: Vec::new(),
    };
    let data = executor.object(
        &json!({"__typename": "Query"}),
        &doc.selections,
        &mut Vec::new(),
    );
    if executor.errors.is_empty() {
        json!({ "data": data })
    } else {
        json!({ "data": data, "errors": executor.errors })
    }
}

fn cursor(index: usize) -> String {
    STANDARD.encode(format!("cursor:{index}"))
}

fn cursor_index(cursor: &str) -> Option<usize> {
    let decoded = STANDARD.decode(cursor).ok()?;
    std::str::from_utf8(&decoded)
        .ok()?
        .strip_prefix("cursor:")?
        .parse()
        .ok()
}

/// Whether `value` (a string or a list of strings, e.g. `states: [OPEN]`) allows `candidate`; `null` allows all.
fn allows(filter: Option<&Value>, candidate: &Value) -> bool {
    match filter {
        None | Some(Value::Null) => true,
        Some(Value::Array(values)) => values.contains(candidate),
        Some(value) => value == candidate,
    }
}

impl Executor<'_> {
    fn error(&mut self, kind: &str, path: &[String], message: String) -> Value {
        self.errors
            .push(json!({"type": kind, "path": path, "locations": [], "message": message}));
        Value::Null
    }

    fn object(
        &mut self,
        parent: &Value,
        selections: &[Selection],
        path: &mut Vec<String>,
    ) -> Value {
        let fields = collect_fields(selections, parent["__typename"].as_str(), self.fragments);
        let mut out = Map::new();
        for field in fields {
            if out.contains_key(field.key()) {
                continue;
            }
            path.push(field.key().to_owned());
            let value = self.resolve(parent, field, path);
            let value = self.project(value, &field.selections, path);
            path.pop();
            out.insert(field.key().to_owned(), value);
        }
        Value::Object(out)
    }

    fn project(&mut self, value: Value, selections: &[Selection], path: &mut Vec<String>) -> Value {
        if selections.is_empty() {
            return value;
        }
        match value {
            Value::Array(items) => Value::Array(
                items
                    .into_iter()
                    .map(|item| self.project(item, selections, path))
                    .collect(),
            ),
            object @ Value::Object(_) => self.object(&object, selections, path),
            other => other,
        }
    }

    /// The pull request with its current head when it is the one `POST /_fake/head` moves.
    fn pull(&self, node: &Value) -> Value {
        let mut node = node.clone();
        if let Some((repository, number, head)) = self.moved_head
            && name_with_owner(&node).eq_ignore_ascii_case(repository)
            && node["number"] == *number
        {
            node["headRefOid"] = json!(head);
        }
        node
    }

    fn resolve(&mut self, parent: &Value, field: &Field, path: &[String]) -> Value {
        let arg = |name: &str| field.args.get(name);
        let text = |name: &str| {
            arg(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let typename = parent["__typename"].as_str().unwrap_or_default();
        let world = self.world;
        match (typename, field.name.as_str()) {
            ("Query", "repository") => {
                let name = format!("{}/{}", text("owner"), text("name"));
                match world.repository(&name, true) {
                    Some(repository) => repository.clone(),
                    None => self.error(
                        "NOT_FOUND",
                        path,
                        format!("Could not resolve to a Repository with the name '{name}'."),
                    ),
                }
            }
            ("Query", "repositoryOwner") => world
                .account(&text("login"))
                .filter(|account| account.kind != AccountKind::Bot)
                .map_or(Value::Null, |account| {
                    json!({"__typename": format!("{:?}", account.kind), "id": format!("O_{}", account.id),
                        "login": account.login})
                }),
            ("Query", "rateLimit") => self.rate_limit.clone(),
            ("Repository", "pullRequest" | "issue") => {
                let number = arg("number").and_then(Value::as_u64).unwrap_or_default();
                let repository = name_with_owner(parent);
                let found = if field.name == "issue" {
                    world.issue(repository, number).cloned()
                } else {
                    world.pull_request(repository, number).map(|node| self.pull(node))
                };
                found.unwrap_or_else(|| {
                    let what = if field.name == "issue" { "an Issue" } else { "a PullRequest" };
                    self.error(
                        "NOT_FOUND",
                        path,
                        format!("Could not resolve to {what} with the number of {number}."),
                    )
                })
            }
            ("Repository", "pullRequests" | "issues") => {
                let repository = name_with_owner(parent);
                let nodes = if field.name == "issues" { &world.issues } else { &world.pull_requests };
                let mut items: Vec<Value> = nodes
                    .iter()
                    .filter(|node| name_with_owner(node).eq_ignore_ascii_case(repository))
                    .filter(|node| allows(arg("states"), &node["state"]))
                    .filter(|node| allows(arg("baseRefName"), &node["baseRefName"]))
                    .filter(|node| allows(arg("headRefName"), &node["headRefName"]))
                    .map(|node| {
                        if field.name == "issues" {
                            node.clone()
                        } else {
                            self.pull(node)
                        }
                    })
                    .collect();
                items.sort_by_key(|node| node["number"].as_u64());
                self.connection(items, field, path)
            }
            ("Organization" | "User", "repositories") => {
                let owner = parent["login"].as_str().unwrap_or_default();
                let items: Vec<Value> = world
                    .repositories
                    .iter()
                    .filter(|r| r["owner"]["login"].as_str().is_some_and(|l| l.eq_ignore_ascii_case(owner)))
                    .filter(|r| allows(arg("isArchived"), &r["isArchived"]))
                    .cloned()
                    .collect();
                self.connection(items, field, path)
            }
            ("PullRequest", "reviews") => {
                let author = arg("author").and_then(Value::as_str);
                let items: Vec<Value> = parent["reviews"]["nodes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|review| {
                        author.is_none_or(|a| {
                            review["author"]["login"].as_str().is_some_and(|l| l.eq_ignore_ascii_case(a))
                        })
                    })
                    .filter(|review| allows(arg("states"), &review["state"]))
                    .cloned()
                    .collect();
                self.connection(items, field, path)
            }
            _ => {
                let value = parent.get(&field.name).cloned().unwrap_or(Value::Null);
                match value.get("nodes").and_then(Value::as_array) {
                    Some(nodes) => {
                        let nodes = nodes.clone();
                        self.connection(nodes, field, path)
                    }
                    None => value,
                }
            }
        }
    }

    /// A page of `items` following GitHub's connection arguments and limits.
    fn connection(&mut self, items: Vec<Value>, field: &Field, path: &[String]) -> Value {
        let name = &field.name;
        let first = field.args.get("first").and_then(Value::as_i64);
        let last = field.args.get("last").and_then(Value::as_i64);
        let pages_nodes = collect_fields(&field.selections, None, self.fragments)
            .iter()
            .any(|f| matches!(f.name.as_str(), "nodes" | "edges"));
        if pages_nodes && first.is_none() && last.is_none() {
            return self.error(
                "MISSING_PAGINATION_BOUNDARIES",
                path,
                format!("You must provide a `first` or `last` value to properly paginate the `{name}` connection."),
            );
        }
        for (bound, value) in [("first", first), ("last", last)] {
            if let Some(n) = value.filter(|n| !(1..=MAX_PAGE).contains(n)) {
                return self.error(
                    "EXCESSIVE_PAGE_SIZE",
                    path,
                    format!("Requesting {n} records on the `{name}` connection exceeds the `{bound}` limit of {MAX_PAGE} records."),
                );
            }
        }
        let start = match field.args.get("after") {
            None | Some(Value::Null) => 0,
            Some(after) => match after.as_str().and_then(cursor_index) {
                Some(index) => index + 1,
                None => {
                    return self.error(
                        "INVALID_CURSOR_ARGUMENTS",
                        path,
                        format!(
                            "`{}` does not appear to be a valid cursor.",
                            after.as_str().unwrap_or_default()
                        ),
                    );
                }
            },
        };
        let total = items.len();
        let start = start.min(total);
        let (begin, end) = match (first, last) {
            (Some(n), _) => (
                start,
                (start + usize::try_from(n).unwrap_or_default()).min(total),
            ),
            (None, Some(n)) => (
                total
                    .saturating_sub(usize::try_from(n).unwrap_or_default())
                    .max(start),
                total,
            ),
            (None, None) => (start, total),
        };
        let nodes = &items[begin..end];
        let edges: Vec<Value> = nodes
            .iter()
            .enumerate()
            .map(|(offset, node)| json!({"cursor": cursor(begin + offset), "node": node}))
            .collect();
        let (start_cursor, end_cursor) = if nodes.is_empty() {
            (Value::Null, Value::Null)
        } else {
            (json!(cursor(begin)), json!(cursor(end - 1)))
        };
        json!({
            "totalCount": total,
            "nodes": nodes,
            "edges": edges,
            "pageInfo": {"hasNextPage": end < total, "hasPreviousPage": begin > 0,
                "startCursor": start_cursor, "endCursor": end_cursor},
        })
    }
}
