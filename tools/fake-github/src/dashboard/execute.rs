//! Executes a dashboard document `Schema::validate` accepted against the world: root lookups (`repository`,
//! `repositoryOwner`, `rateLimit`), a repository's `pullRequest`/`issue` lookups and `pullRequests`/`issues`
//! listings, an owner's `repositories`, `reviews(author:, states:)`, `timelineItems(itemTypes:)`, and connection
//! pages (`first`/`last`/`after`, opaque cursors). Other fields are read from the node; lookups that find nothing
//! are `null` with a `NOT_FOUND` error next to the data.

use std::collections::{HashMap, HashSet};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};

use super::query::{Document, Field, Fragment, Kind, Selection};
use super::world::{AccountKind, World, name_with_owner, number};

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
/// Deepest fragment nesting followed (resolved documents have no fragment cycles).
const MAX_FRAGMENT_DEPTH: usize = 64;

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

/// Whether a resolved document belongs to the v1 review endpoint: a mutation or subscription, a `node` lookup,
/// or one unaliased `repository` lookup of the recorded pull request (`recorded`: `owner/name`, number) only.
pub(crate) fn routes_to_v1(doc: &Document, recorded: Option<&(String, u64)>) -> bool {
    if doc.kind != Kind::Query {
        return true;
    }
    let root = collect_fields(&doc.selections, Some("Query"), &doc.fragments);
    if root.iter().any(|f| f.name == "node") {
        return true;
    }
    let (Some((repository, number)), [only]) = (recorded, root.as_slice()) else {
        return false;
    };
    let text = |name: &str| {
        only.args
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    let children = collect_fields(&only.selections, Some("Repository"), &doc.fragments);
    only.name == "repository"
        && only.alias.is_none()
        && format!("{}/{}", text("owner"), text("name")).eq_ignore_ascii_case(repository)
        && children.iter().any(|c| c.name == "pullRequest")
        && children.iter().all(|c| match c.name.as_str() {
            "pullRequest" => c.args.get("number").and_then(Value::as_u64) == Some(*number),
            "databaseId" | "isPrivate" => c.alias.is_none(),
            _ => false,
        })
}

/// Size of a batch: top-level lookups (without `rateLimit`), `pullRequest(number:)` lookups among them, and
/// whether `rateLimit` is selected.
pub(crate) fn batch_size(doc: &Document) -> (usize, usize, bool) {
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
    (lookups, pulls, root.len() > lookups)
}

/// The pull request whose head moves with `POST /_fake/head`: (`owner/name`, number, current head).
pub(crate) type MovedHead = Option<(String, u64, String)>;

/// `ReviewRequestedEvent` → `REVIEW_REQUESTED_EVENT`.
fn item_type(typename: &str) -> String {
    let mut out = String::new();
    for (index, c) in typename.chars().enumerate() {
        if c.is_ascii_uppercase() && index > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_uppercase());
    }
    out
}

struct Executor<'a> {
    world: &'a World,
    fragments: &'a HashMap<String, Fragment>,
    rate_limit: Value,
    moved_head: &'a MovedHead,
    /// Pull requests whose lazily computed merge state was read once (see `World::computed_merge_states`).
    merge_reads: &'a mut HashSet<(String, u64)>,
    /// Lookups that found nothing (answered next to the data).
    errors: Vec<Value>,
}

/// Runs a document `Schema::validate` accepted and returns GitHub's response body: `data`, and `errors` when a
/// lookup failed.
pub(crate) fn execute(
    doc: &Document,
    world: &World,
    rate_limit: Value,
    moved_head: &MovedHead,
    merge_reads: &mut HashSet<(String, u64)>,
) -> Value {
    let mut executor = Executor {
        world,
        fragments: &doc.fragments,
        rate_limit,
        moved_head,
        merge_reads,
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

/// The index an opaque cursor names.
pub(crate) fn cursor_index(cursor: &str) -> Option<usize> {
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
        let typename = parent["__typename"].as_str();
        let fields = collect_fields(selections, typename, self.fragments);
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

    /// A lazily computed merge state: the first lookup selecting `mergeStateStatus` keeps the fixture's value,
    /// later lookups answer the computed one.
    fn lazy_merge_state(&mut self, node: &mut Value, field: &Field) {
        let key = (name_with_owner(node).to_ascii_lowercase(), number(node));
        let Some(computed) = self.world.computed_merge_states.get(&key) else {
            return;
        };
        let selected = collect_fields(&field.selections, Some("PullRequest"), self.fragments)
            .iter()
            .any(|f| f.name == "mergeStateStatus");
        if selected && !self.merge_reads.insert(key) {
            node["mergeStateStatus"] = computed.clone();
        }
    }

    fn resolve(&mut self, parent: &Value, field: &Field, path: &[String]) -> Value {
        let typename = parent["__typename"].as_str().unwrap_or_default();
        let arg = |name: &str| field.args.get(name);
        let text = |name: &str| {
            arg(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
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
                    world.pull_request(repository, number).map(|node| {
                        let mut node = self.pull(node);
                        self.lazy_merge_state(&mut node, field);
                        node
                    })
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
                    .map(|node| if field.name == "issues" { node.clone() } else { self.pull(node) })
                    .collect();
                items.sort_by_key(|node| node["number"].as_u64());
                if let Some(order) = arg("orderBy") {
                    let key = match order["field"].as_str() {
                        Some("UPDATED_AT") => "updatedAt",
                        _ => "createdAt",
                    };
                    // RFC 3339 UTC strings order as their instants.
                    items.sort_by(|a, b| a[key].as_str().cmp(&b[key].as_str()));
                    if order["direction"] == "DESC" {
                        items.reverse();
                    }
                }
                connection(items, field)
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
                connection(items, field)
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
                connection(items, field)
            }
            _ => {
                let value = parent.get(&field.name).cloned().unwrap_or(Value::Null);
                match value.get("nodes").and_then(Value::as_array).cloned() {
                    Some(nodes) => {
                        let types: Option<Vec<String>> = arg("itemTypes")
                            .and_then(Value::as_array)
                            .map(|t| t.iter().filter_map(Value::as_str).map(str::to_owned).collect());
                        let nodes = nodes
                            .into_iter()
                            .filter(|node| {
                                types.as_ref().is_none_or(|types| {
                                    types.contains(&item_type(node["__typename"].as_str().unwrap_or_default()))
                                })
                            })
                            .collect();
                        connection(nodes, field)
                    }
                    None => value,
                }
            }
        }
    }
}

/// A page of `items` following a connection's `first`/`last`/`after` (validated by `Schema::validate`).
fn connection(items: Vec<Value>, field: &Field) -> Value {
    let first = field.args.get("first").and_then(Value::as_i64);
    let last = field.args.get("last").and_then(Value::as_i64);
    let total = items.len();
    let start = field
        .args
        .get("after")
        .and_then(Value::as_str)
        .and_then(cursor_index)
        .and_then(|i| i.checked_add(1))
        .unwrap_or(0)
        .min(total);
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
            .map(|(offset, node)| json!({"__typename": "Edge", "cursor": cursor(begin + offset), "node": node}))
            .collect();
    let (start_cursor, end_cursor) = if nodes.is_empty() {
        (Value::Null, Value::Null)
    } else {
        (json!(cursor(begin)), json!(cursor(end - 1)))
    };
    json!({
        "__typename": "Connection",
        "totalCount": total,
        "nodes": nodes,
        "edges": edges,
        "pageInfo": {"__typename": "PageInfo", "hasNextPage": end < total, "hasPreviousPage": begin > 0,
            "startCursor": start_cursor, "endCursor": end_cursor},
    })
}
