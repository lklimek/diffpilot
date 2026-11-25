//! The world's type map and static validation of dashboard documents against it, before execution (GitHub
//! refuses an invalid document as a whole, whatever the data). Types come from the fixtures: an object with a
//! `__typename` has that type, any other object the pseudo-type `<parent type>.<field>` (list items too), so
//! `PullRequest.milestone` and `PullRequest.labels.nodes` have field sets; resolver fields, connections
//! (`totalCount`, `edges`, `pageInfo`) and `RateLimit` are added. A type's fields are the keys seen on it.

use std::collections::{BTreeSet, HashMap};

use serde_json::{Value, json};

use super::execute::{collect_fields, cursor_index};
use super::query::{Document, Field, Fragment, Selection};

/// GitHub's page-size limit of a connection.
const MAX_PAGE: i64 = 100;
/// Resolver-listed node types and their connection types.
const LISTED: &[&str] = &["PullRequest", "Issue", "Repository"];

/// What a field returns: a scalar, objects of these types, or (neither) only `null` was seen.
#[derive(Debug, Default)]
struct FieldInfo {
    scalar: bool,
    types: BTreeSet<String>,
}

/// Fields per type name.
#[derive(Debug, Default)]
pub(crate) struct Schema {
    types: HashMap<String, HashMap<String, FieldInfo>>,
}

/// Expected argument type.
#[derive(Clone, Copy)]
enum Expect {
    Str,
    Int,
    Bool,
    /// An enum value or a list of them.
    Enums,
    /// An order object `{field: ENUM, direction: ASC|DESC}`.
    Order,
}

impl Expect {
    fn accepts(self, value: &Value) -> bool {
        match self {
            Self::Str => value.is_string(),
            Self::Int => value.is_i64(),
            Self::Bool => value.is_boolean(),
            Self::Enums => {
                value.is_string()
                    || value
                        .as_array()
                        .is_some_and(|items| items.iter().all(Value::is_string))
            }
            Self::Order => {
                value["field"].is_string()
                    && matches!(value["direction"].as_str(), Some("ASC" | "DESC"))
            }
        }
    }

    fn name(self, required: bool) -> String {
        let name = match self {
            Self::Str => "String",
            Self::Int => "Int",
            Self::Bool => "Boolean",
            Self::Enums => "[Enum!]",
            Self::Order => "IssueOrder",
        };
        format!("{name}{}", if required { "!" } else { "" })
    }
}

/// Implemented arguments of a field: name, type, required.
type ArgSpec = &'static [(&'static str, Expect, bool)];

const STATES: (&str, Expect, bool) = ("states", Expect::Enums, false);
const FIRST: (&str, Expect, bool) = ("first", Expect::Int, false);
const LAST: (&str, Expect, bool) = ("last", Expect::Int, false);
const AFTER: (&str, Expect, bool) = ("after", Expect::Str, false);

/// The arguments the fake implements for a field; `connection` marks a field returning a connection.
fn arg_spec(typename: &str, field: &str, connection: bool) -> ArgSpec {
    match (typename, field) {
        ("Query", "repository") => &[("owner", Expect::Str, true), ("name", Expect::Str, true)],
        ("Query", "repositoryOwner") => &[("login", Expect::Str, true)],
        ("Repository", "pullRequest" | "issue") => &[("number", Expect::Int, true)],
        ("Repository", "pullRequests") => &[
            STATES,
            FIRST,
            LAST,
            AFTER,
            ("baseRefName", Expect::Str, false),
            ("headRefName", Expect::Str, false),
            ("orderBy", Expect::Order, false),
        ],
        ("Repository", "issues") => &[STATES, FIRST, LAST, AFTER],
        ("Organization" | "User", "repositories") => {
            &[FIRST, LAST, AFTER, ("isArchived", Expect::Bool, false)]
        }
        ("PullRequest", "reviews") => &[("author", Expect::Str, false), STATES, FIRST, LAST, AFTER],
        (_, "timelineItems") if connection => {
            &[FIRST, LAST, AFTER, ("itemTypes", Expect::Enums, false)]
        }
        _ if connection => &[FIRST, LAST, AFTER],
        _ => &[],
    }
}

fn problem(kind: &str, path: &[String], message: String) -> Value {
    json!({"type": kind, "path": path, "locations": [], "message": message, "extensions": {"code": kind}})
}

impl Schema {
    /// The type map of the world's repository, pull request and issue nodes.
    pub(crate) fn build<'a>(nodes: impl Iterator<Item = &'a Value>) -> Self {
        let mut schema = Self::default();
        for node in nodes {
            if let Some(typename) = node["__typename"].as_str() {
                schema.learn(node, typename);
            }
        }
        let connections: Vec<(String, BTreeSet<String>)> = schema
            .types
            .iter()
            .filter_map(|(ty, fields)| fields.get("nodes").map(|n| (ty.clone(), n.types.clone())))
            .collect();
        for (ty, items) in connections {
            schema.connection(&ty, items);
        }
        for ty in LISTED {
            schema.connection(
                &format!("Connection:{ty}"),
                BTreeSet::from([(*ty).to_owned()]),
            );
        }
        let object = |name: &str| FieldInfo {
            scalar: false,
            types: BTreeSet::from([name.to_owned()]),
        };
        let scalar = || FieldInfo {
            scalar: true,
            types: BTreeSet::new(),
        };
        let mut add = |ty: &str, field: &str, info: FieldInfo| {
            let entry = schema
                .types
                .entry(ty.to_owned())
                .or_default()
                .entry(field.to_owned())
                .or_default();
            entry.scalar |= info.scalar;
            entry.types.extend(info.types);
        };
        add("Query", "repository", object("Repository"));
        add("Query", "repositoryOwner", object("Organization"));
        add("Query", "repositoryOwner", object("User"));
        add("Query", "rateLimit", object("RateLimit"));
        add("Repository", "pullRequest", object("PullRequest"));
        add("Repository", "issue", object("Issue"));
        add(
            "Repository",
            "pullRequests",
            object("Connection:PullRequest"),
        );
        add("Repository", "issues", object("Connection:Issue"));
        for owner in ["Organization", "User"] {
            add(owner, "id", scalar());
            add(owner, "login", scalar());
            add(owner, "repositories", object("Connection:Repository"));
        }
        for field in ["cost", "limit", "remaining", "used", "resetAt", "nodeCount"] {
            add("RateLimit", field, scalar());
        }
        for field in ["hasNextPage", "hasPreviousPage", "startCursor", "endCursor"] {
            add("PageInfo", field, scalar());
        }
        // Stack membership is nullable even in worlds that contain no native stacks.
        let entry = "PullRequest.stackEntry";
        let stack = "PullRequest.stackEntry.stack";
        let entries = "PullRequest.stackEntry.stack.entries";
        let member = "PullRequest.stackEntry.stack.entries.nodes";
        add("PullRequest", "stackEntry", object(entry));
        add(entry, "position", scalar());
        add(entry, "stack", object(stack));
        add(stack, "id", scalar());
        add(stack, "baseRefName", scalar());
        add(stack, "size", scalar());
        add(stack, "entries", object(entries));
        add(member, "position", scalar());
        add(member, "pullRequest", object("PullRequest"));
        add("PullRequest", "number", scalar());
        add("PullRequest", "state", scalar());
        schema.connection(entries, BTreeSet::from([member.to_owned()]));
        schema
    }

    /// Records the fields of `value` (of type `ty`) and of everything below it.
    fn learn(&mut self, value: &Value, ty: &str) {
        let Value::Object(object) = value else {
            return;
        };
        self.types.entry(ty.to_owned()).or_default();
        for (key, child) in object {
            let items: Vec<&Value> = match child {
                Value::Array(items) => items.iter().collect(),
                single => vec![single],
            };
            let mut info = FieldInfo::default();
            let mut nested = Vec::new();
            for item in items {
                match item {
                    Value::Null => {}
                    Value::Object(inner) => {
                        let child_ty = inner
                            .get("__typename")
                            .and_then(Value::as_str)
                            .map_or_else(|| format!("{ty}.{key}"), str::to_owned);
                        info.types.insert(child_ty.clone());
                        nested.push((item, child_ty));
                    }
                    _ => info.scalar = true,
                }
            }
            let entry = self
                .types
                .entry(ty.to_owned())
                .or_default()
                .entry(key.clone())
                .or_default();
            entry.scalar |= info.scalar;
            entry.types.extend(info.types);
            for (item, child_ty) in nested {
                self.learn(item, &child_ty);
            }
        }
    }

    /// Makes `ty` a connection over `items`: `totalCount`, `nodes`, `edges { cursor node }`, `pageInfo`.
    fn connection(&mut self, ty: &str, items: BTreeSet<String>) {
        let edge = format!("Edge:{ty}");
        let fields = self.types.entry(ty.to_owned()).or_default();
        fields.entry("totalCount".to_owned()).or_default().scalar = true;
        fields
            .entry("nodes".to_owned())
            .or_default()
            .types
            .extend(items.clone());
        fields
            .entry("edges".to_owned())
            .or_default()
            .types
            .insert(edge.clone());
        fields
            .entry("pageInfo".to_owned())
            .or_default()
            .types
            .insert("PageInfo".to_owned());
        let edges = self.types.entry(edge).or_default();
        edges.entry("cursor".to_owned()).or_default().scalar = true;
        edges
            .entry("node".to_owned())
            .or_default()
            .types
            .extend(items);
    }

    fn is_connection(&self, types: &BTreeSet<String>) -> bool {
        types.iter().any(|t| {
            self.types
                .get(t)
                .is_some_and(|f| f.contains_key("nodes") && f.contains_key("edges"))
        })
    }

    /// Every problem GitHub would refuse `doc` for, as GraphQL error objects; empty when valid.
    pub(crate) fn validate(&self, doc: &Document) -> Vec<Value> {
        let mut problems = Vec::new();
        self.selections(
            &doc.selections,
            &BTreeSet::from(["Query".to_owned()]),
            &doc.fragments,
            &mut Vec::new(),
            &mut problems,
        );
        problems
    }

    fn selections(
        &self,
        selections: &[Selection],
        types: &BTreeSet<String>,
        fragments: &HashMap<String, Fragment>,
        path: &mut Vec<String>,
        problems: &mut Vec<Value>,
    ) {
        for selection in selections {
            let (on, inner) = match selection {
                Selection::Field(field) => {
                    path.push(field.key().to_owned());
                    self.field(field, types, fragments, path, problems);
                    path.pop();
                    continue;
                }
                Selection::Inline { on, selections } => (on.as_deref(), selections.as_slice()),
                Selection::Spread(name) => match fragments.get(name) {
                    Some(fragment) => (Some(fragment.on.as_str()), fragment.selections.as_slice()),
                    None => continue,
                },
            };
            match on {
                None => self.selections(inner, types, fragments, path, problems),
                Some(on) if types.contains(on) || self.types.contains_key(on) => {
                    self.selections(
                        inner,
                        &BTreeSet::from([on.to_owned()]),
                        fragments,
                        path,
                        problems,
                    );
                }
                Some(_) => {}
            }
        }
    }

    fn field(
        &self,
        field: &Field,
        types: &BTreeSet<String>,
        fragments: &HashMap<String, Fragment>,
        path: &[String],
        problems: &mut Vec<Value>,
    ) {
        let name = &field.name;
        let known_types = types.iter().all(|t| self.types.contains_key(t));
        let (info, parent) = if name == "__typename" {
            (
                FieldInfo {
                    scalar: true,
                    types: BTreeSet::new(),
                },
                String::new(),
            )
        } else {
            let matching: Vec<(&String, &FieldInfo)> = types
                .iter()
                .filter_map(|t| self.types.get(t)?.get(name).map(|info| (t, info)))
                .collect();
            let Some((parent, _)) = matching.first() else {
                if known_types && let Some(ty) = types.iter().next() {
                    problems.push(problem(
                        "undefinedField",
                        path,
                        format!("Field '{name}' doesn't exist on type '{ty}'"),
                    ));
                }
                return;
            };
            let mut merged = FieldInfo::default();
            for (_, info) in &matching {
                merged.scalar |= info.scalar;
                merged.types.extend(info.types.iter().cloned());
            }
            (merged, (*parent).clone())
        };
        let connection = self.is_connection(&info.types);
        if let Some(message) = args_problem(&parent, field, connection) {
            problems.push(message(path));
            return;
        }
        if connection {
            let selects_nodes = collect_fields(&field.selections, None, fragments)
                .iter()
                .any(|f| matches!(f.name.as_str(), "nodes" | "edges"));
            if let Some(found) = page_problem(field, selects_nodes, path) {
                problems.push(found);
                return;
            }
        }
        match (info.types.is_empty(), field.selections.is_empty()) {
            (false, true) => problems.push(problem(
                "selectionMismatch",
                path,
                format!("Field must have selections (field '{name}' returns an object but has no selections)"),
            )),
            (true, false) if info.scalar => problems.push(problem(
                "selectionMismatch",
                path,
                format!("Selections can't be made on scalars (field '{name}' returns a scalar)"),
            )),
            (false, false) => {
                let mut path = path.to_vec();
                self.selections(&field.selections, &info.types, fragments, &mut path, problems);
            }
            _ => {}
        }
    }
}

/// A problem with a field's arguments against `arg_spec`, as a builder of the error at a path.
fn args_problem(
    parent: &str,
    field: &Field,
    connection: bool,
) -> Option<impl FnOnce(&[String]) -> Value> {
    let spec = arg_spec(parent, &field.name, connection);
    let name = field.name.clone();
    let failure = field.args.iter().find_map(|(arg, value)| {
        let Some((_, expect, required)) = spec.iter().find(|(known, _, _)| known == arg) else {
            let implemented: Vec<&str> = spec.iter().map(|(known, _, _)| *known).collect();
            let hint = if implemented.is_empty() {
                String::new()
            } else {
                format!(" (fake-github implements: {})", implemented.join(", "))
            };
            return Some((
                "argumentNotAccepted",
                format!("Field '{name}' doesn't accept argument '{arg}'{hint}"),
            ));
        };
        (!value.is_null() && !expect.accepts(value)).then(|| {
            (
                "argumentLiteralsIncompatible",
                format!(
                    "Argument '{arg}' on Field '{name}' has an invalid value ({value}). Expected type '{}'.",
                    expect.name(*required)
                ),
            )
        })
    });
    let failure = failure.or_else(|| {
        let missing: Vec<&str> = spec
            .iter()
            .filter(|(arg, _, required)| {
                *required && field.args.get(*arg).is_none_or(Value::is_null)
            })
            .map(|(arg, _, _)| *arg)
            .collect();
        (!missing.is_empty()).then(|| {
            (
                "missingRequiredArguments",
                format!(
                    "Field '{name}' is missing required arguments: {}",
                    missing.join(", ")
                ),
            )
        })
    });
    failure.map(|(kind, message)| move |path: &[String]| problem(kind, path, message))
}

/// GitHub's connection rules: `first`/`last` within 1–100, one of them when `nodes`/`edges` are selected, a
/// valid `after` cursor.
fn page_problem(field: &Field, selects_nodes: bool, path: &[String]) -> Option<Value> {
    let name = &field.name;
    let first = field.args.get("first").and_then(Value::as_i64);
    let last = field.args.get("last").and_then(Value::as_i64);
    if selects_nodes && first.is_none() && last.is_none() {
        return Some(problem(
            "MISSING_PAGINATION_BOUNDARIES",
            path,
            format!(
                "You must provide a `first` or `last` value to properly paginate the `{name}` connection."
            ),
        ));
    }
    for (bound, value) in [("first", first), ("last", last)] {
        if let Some(n) = value.filter(|n| !(1..=MAX_PAGE).contains(n)) {
            return Some(problem(
                "EXCESSIVE_PAGE_SIZE",
                path,
                format!(
                    "Requesting {n} records on the `{name}` connection exceeds the `{bound}` limit of {MAX_PAGE} records."
                ),
            ));
        }
    }
    match field.args.get("after") {
        None | Some(Value::Null) => None,
        Some(after) => after
            .as_str()
            .and_then(cursor_index)
            .and_then(|i| i.checked_add(1))
            .is_none()
            .then(|| {
                problem(
                    "INVALID_CURSOR_ARGUMENTS",
                    path,
                    format!(
                        "`{}` does not appear to be a valid cursor.",
                        after.as_str().unwrap_or_default()
                    ),
                )
            }),
    }
}
