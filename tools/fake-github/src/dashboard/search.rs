//! `GET /search/issues`: the qualifiers of the dashboard scan (plan §A.2 Q1, Q10, Q12) evaluated over the world.
//! Supported: `is:pr`/`is:pull-request`/`type:pr`, `is:issue`/`type:issue`, `is:open`/`is:closed`/`state:`,
//! `archived:`, `author:`, `assignee:`, `review-requested:`, `reviewed-by:`, `mentions:`, `repo:` (several are
//! OR-ed), and `created:`/`updated:`/`closed:`/`merged:` with `>=D`, `>D`, `<=D`, `<D`, `D`, `A..B`, `*` (dates or
//! RFC 3339 times, UTC). Anything else is refused with 422, so a client never relies on an unsupported term.

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use serde_json::{Value, json};

use super::world::{World, name_with_owner, number};

/// A search GitHub refuses with 422.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SearchError {
    /// Neither `is:issue` nor `is:pr`.
    #[error("Query must include 'is:issue' or 'is:pull-request'")]
    MissingType,
    /// A login or repository the token cannot search.
    #[error(
        "The listed users and repositories cannot be searched either because the resources do not exist or you \
         do not have permission to view them."
    )]
    CannotBeSearched,
    /// A term or qualifier value the fake does not implement.
    #[error("fake-github does not support the search term `{0}`")]
    Unsupported(String),
}

/// A user relation qualifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Relation {
    Author,
    Assignee,
    ReviewRequested,
    ReviewedBy,
    Mentions,
}

/// A half-open time range `[from, until)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Range {
    from: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
}

/// A parsed search query.
#[derive(Debug)]
pub(crate) struct Search {
    /// `true`: pull requests; `false`: issues.
    pub(crate) pull_requests: bool,
    open: Option<bool>,
    archived: Option<bool>,
    relations: Vec<(Relation, String)>,
    repositories: Vec<String>,
    /// GraphQL field of the timestamp and its range.
    dates: Vec<(&'static str, Range)>,
}

/// The first and the last instant (exclusive) a date or time names.
fn instant(text: &str) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        let start = date.and_hms_opt(0, 0, 0)?.and_utc();
        return Some((start, start + TimeDelta::days(1)));
    }
    let time = DateTime::parse_from_rfc3339(text).ok()?.with_timezone(&Utc);
    Some((time, time + TimeDelta::seconds(1)))
}

fn range(value: &str) -> Option<Range> {
    let bound = |text: &str| -> Option<Option<(DateTime<Utc>, DateTime<Utc>)>> {
        if text == "*" {
            Some(None)
        } else {
            instant(text).map(Some)
        }
    };
    if let Some((low, high)) = value.split_once("..") {
        return Some(Range {
            from: bound(low)?.map(|(start, _)| start),
            until: bound(high)?.map(|(_, end)| end),
        });
    }
    for (prefix, lower, first_instant) in [
        (">=", true, true),
        ("<=", false, false),
        (">", true, false),
        ("<", false, true),
    ] {
        if let Some(rest) = value.strip_prefix(prefix) {
            let (start, end) = instant(rest)?;
            let at = if first_instant { start } else { end };
            return Some(if lower {
                Range {
                    from: Some(at),
                    until: None,
                }
            } else {
                Range {
                    from: None,
                    until: Some(at),
                }
            });
        }
    }
    let (start, end) = instant(value)?;
    Some(Range {
        from: Some(start),
        until: Some(end),
    })
}

fn boolean(term: &str, value: &str) -> Result<bool, SearchError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(SearchError::Unsupported(term.to_owned())),
    }
}

/// Parses the `q` parameter.
pub(crate) fn parse(q: &str) -> Result<Search, SearchError> {
    let mut kind = None;
    let mut search = Search {
        pull_requests: false,
        open: None,
        archived: None,
        relations: Vec::new(),
        repositories: Vec::new(),
        dates: Vec::new(),
    };
    for term in q.split_whitespace() {
        let unsupported = || SearchError::Unsupported(term.to_owned());
        let (key, value) = term.split_once(':').ok_or_else(unsupported)?;
        if value.is_empty() {
            return Err(unsupported());
        }
        match (key, value) {
            ("is" | "type", "pr" | "pull-request") => kind = Some(true),
            ("is" | "type", "issue") => kind = Some(false),
            ("is" | "state", "open") => search.open = Some(true),
            ("is" | "state", "closed") => search.open = Some(false),
            ("archived", _) => search.archived = Some(boolean(term, value)?),
            ("repo", _) if value.contains('/') => search.repositories.push(value.to_owned()),
            ("author" | "assignee" | "review-requested" | "reviewed-by" | "mentions", _) => {
                let relation = match key {
                    "author" => Relation::Author,
                    "assignee" => Relation::Assignee,
                    "review-requested" => Relation::ReviewRequested,
                    "reviewed-by" => Relation::ReviewedBy,
                    _ => Relation::Mentions,
                };
                search.relations.push((relation, value.to_owned()));
            }
            ("created" | "updated" | "closed" | "merged", _) => {
                let field = match key {
                    "created" => "createdAt",
                    "updated" => "updatedAt",
                    "closed" => "closedAt",
                    _ => "mergedAt",
                };
                search
                    .dates
                    .push((field, range(value).ok_or_else(unsupported)?));
            }
            _ => return Err(unsupported()),
        }
    }
    search.pull_requests = kind.ok_or(SearchError::MissingType)?;
    Ok(search)
}

fn logins(connection: &Value, pointer: &str) -> Vec<String> {
    connection["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| {
            node.pointer(pointer)
                .and_then(Value::as_str)
                .map(str::to_lowercase)
        })
        .collect()
}

/// Whether `text` mentions `@login` (not as part of a longer login).
fn mentions(text: &str, login: &str) -> bool {
    let text = text.to_lowercase();
    let needle = format!("@{}", login.to_lowercase());
    text.match_indices(&needle).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + needle.len()..].chars().next();
        before.is_none_or(|c| !c.is_alphanumeric())
            && after.is_none_or(|c| !(c.is_alphanumeric() || c == '-'))
    })
}

impl Search {
    /// Every login and repository the query names must be searchable by the caller.
    pub(crate) fn check(&self, world: &World, authenticated: bool) -> Result<(), SearchError> {
        let logins_ok = self.relations.iter().all(|(_, login)| {
            world
                .account(login)
                .is_some_and(|account| account.searchable)
        });
        let repositories_ok = self
            .repositories
            .iter()
            .all(|name| world.repository(name, authenticated).is_some());
        if logins_ok && repositories_ok {
            Ok(())
        } else {
            Err(SearchError::CannotBeSearched)
        }
    }

    /// Whether a pull request or issue node matches.
    pub(crate) fn matches(&self, node: &Value) -> bool {
        let open = node["state"] == "OPEN";
        let relation = |(relation, login): &(Relation, String)| {
            let login = login.to_lowercase();
            match relation {
                Relation::Author => node["author"]["login"]
                    .as_str()
                    .is_some_and(|author| author.eq_ignore_ascii_case(&login)),
                Relation::Assignee => logins(&node["assignees"], "/login").contains(&login),
                Relation::ReviewRequested => {
                    logins(&node["reviewRequests"], "/requestedReviewer/login").contains(&login)
                }
                Relation::ReviewedBy => node["reviews"]["nodes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|review| {
                        review["state"] != "PENDING"
                            && review["author"]["login"]
                                .as_str()
                                .is_some_and(|author| author.eq_ignore_ascii_case(&login))
                    }),
                Relation::Mentions => std::iter::once(&node["body"])
                    .chain(
                        node["comments"]["nodes"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|c| &c["body"]),
                    )
                    .filter_map(Value::as_str)
                    .any(|text| mentions(text, &login)),
            }
        };
        let in_range = |(field, range): &(&str, Range)| {
            node[*field]
                .as_str()
                .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
                .map(|time| time.with_timezone(&Utc))
                .is_some_and(|time| {
                    range.from.is_none_or(|from| time >= from)
                        && range.until.is_none_or(|until| time < until)
                })
        };
        node["__typename"]
            == if self.pull_requests {
                "PullRequest"
            } else {
                "Issue"
            }
            && self.open.is_none_or(|wanted| wanted == open)
            && self
                .archived
                .is_none_or(|wanted| node["repository"]["isArchived"] == wanted)
            && (self.repositories.is_empty()
                || self
                    .repositories
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(name_with_owner(node))))
            && self.relations.iter().all(relation)
            && self.dates.iter().all(in_range)
    }
}

/// The REST search item of a pull request or issue node.
pub(crate) fn item(node: &Value, world: &World) -> Value {
    let repository = name_with_owner(node);
    let number = number(node);
    let index = world
        .repositories
        .iter()
        .position(|r| name_with_owner(r) == repository)
        .unwrap_or_default();
    let user = |login: &Value| {
        let login = login.as_str().unwrap_or_default();
        let account = world.account(login);
        json!({"login": login, "id": account.map(|a| a.id), "type": account.map(|a| format!("{:?}", a.kind))})
    };
    let assignees: Vec<Value> = node["assignees"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| user(&a["login"]))
        .collect();
    let labels: Vec<Value> = node["labels"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|label| json!({"name": label["name"]}))
        .collect();
    let milestone = match &node["milestone"] {
        Value::Object(m) => json!({"number": m.get("number"), "title": m.get("title"),
            "state": m.get("state").and_then(Value::as_str).map(str::to_lowercase)}),
        _ => Value::Null,
    };
    let api = format!("https://api.github.com/repos/{repository}");
    let mut item = json!({
        "url": format!("{api}/issues/{number}"),
        "repository_url": api,
        "html_url": node["url"],
        "id": (index as u64 + 1) * 1_000_000 + number,
        "node_id": node["id"],
        "number": number,
        "title": node["title"],
        "user": user(&node["author"]["login"]),
        "labels": labels,
        "state": if node["state"] == "OPEN" { "open" } else { "closed" },
        "locked": false,
        "assignee": assignees.first(),
        "assignees": assignees,
        "milestone": milestone,
        "comments": node["comments"]["nodes"].as_array().map_or(0, Vec::len),
        "created_at": node["createdAt"],
        "updated_at": node["updatedAt"],
        "closed_at": node["closedAt"],
        "body": node["body"],
        "score": 1.0,
    });
    if node["__typename"] == "PullRequest" {
        item["draft"] = node["isDraft"].clone();
        item["pull_request"] = json!({
            "url": format!("{api}/pulls/{number}"),
            "html_url": node["url"],
            "merged_at": node["mergedAt"],
        });
    }
    item
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn date_ranges_are_half_open_and_date_only_bounds_cover_whole_days() {
        let day = |d: &str| range(d).unwrap();
        assert_eq!(
            day(">=2026-11-01"),
            Range {
                from: Some(at("2026-11-01T00:00:00Z")),
                until: None
            }
        );
        assert_eq!(
            day(">2026-11-01"),
            Range {
                from: Some(at("2026-11-02T00:00:00Z")),
                until: None
            }
        );
        assert_eq!(
            day("<=2026-11-01"),
            Range {
                from: None,
                until: Some(at("2026-11-02T00:00:00Z"))
            }
        );
        assert_eq!(
            day("<2026-11-01"),
            Range {
                from: None,
                until: Some(at("2026-11-01T00:00:00Z"))
            }
        );
        assert_eq!(
            day("2026-11-01..2026-11-03"),
            Range {
                from: Some(at("2026-11-01T00:00:00Z")),
                until: Some(at("2026-11-04T00:00:00Z"))
            }
        );
        assert_eq!(
            day("*..2026-11-03"),
            Range {
                from: None,
                until: Some(at("2026-11-04T00:00:00Z"))
            }
        );
        assert_eq!(
            day(">=2026-11-01T12:00:00Z"),
            Range {
                from: Some(at("2026-11-01T12:00:00Z")),
                until: None
            }
        );
        assert_eq!(range(">=yesterday"), None);
    }

    #[test]
    fn queries_need_a_type_and_supported_terms() {
        assert_eq!(
            parse("is:open author:alice").unwrap_err(),
            SearchError::MissingType
        );
        assert!(
            parse("is:pr is:open archived:false author:alice")
                .unwrap()
                .pull_requests
        );
        assert!(!parse("type:issue mentions:alice").unwrap().pull_requests);
        for term in [
            "sync",
            "label:bug",
            "-author:alice",
            "archived:maybe",
            "created:>=soon",
            "repo:acme",
            "author:",
        ] {
            assert_eq!(
                parse(&format!("is:pr {term}")).unwrap_err(),
                SearchError::Unsupported(term.to_owned()),
                "{term}"
            );
        }
    }

    #[test]
    fn mentions_respect_login_boundaries() {
        assert!(mentions("ping @alice, please", "alice"));
        assert!(mentions("@Alice.", "alice"));
        assert!(!mentions("ping @alice-helper", "alice"));
        assert!(!mentions("mail bob@alice.dev", "alice"));
        assert!(!mentions("no mention", "alice"));
    }
}
