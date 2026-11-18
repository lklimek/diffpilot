//! The dashboard's REST reads besides search: latest release, releases, milestones, file contents (raw or
//! base64 JSON), pull request files and user events, with GitHub's pagination (`per_page` ≤ 100, `Link`
//! header; events end after 300, search after 1000 results).

use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::form_urlencoded::Serializer;

use super::world::World;

/// A REST path the dashboard world answers.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Endpoint {
    Search,
    LatestRelease { repository: String },
    Releases { repository: String },
    Milestones { repository: String },
    Contents { repository: String, path: String },
    Files { repository: String, number: u64 },
    Events { login: String },
}

impl Endpoint {
    /// The endpoint of a `GET` path, if the dashboard world serves it.
    pub(crate) fn of(path: &str) -> Option<Self> {
        let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let repository = |owner: &str, name: &str| format!("{owner}/{name}");
        Some(match segments.as_slice() {
            ["search", "issues"] => Self::Search,
            ["repos", o, n, "releases", "latest"] => Self::LatestRelease {
                repository: repository(o, n),
            },
            ["repos", o, n, "releases"] => Self::Releases {
                repository: repository(o, n),
            },
            ["repos", o, n, "milestones"] => Self::Milestones {
                repository: repository(o, n),
            },
            ["repos", o, n, "contents", rest @ ..]
                if !rest.is_empty() && rest.iter().all(|s| !s.is_empty()) =>
            {
                Self::Contents {
                    repository: repository(o, n),
                    path: rest.join("/"),
                }
            }
            ["repos", o, n, "pulls", number, "files"] => Self::Files {
                repository: repository(o, n),
                number: number.parse().ok()?,
            },
            ["users", login, "events"] => Self::Events {
                login: (*login).to_owned(),
            },
            _ => return None,
        })
    }

    /// The repository the endpoint reads, if any.
    pub(crate) fn repository(&self) -> Option<&str> {
        match self {
            Self::LatestRelease { repository }
            | Self::Releases { repository }
            | Self::Milestones { repository }
            | Self::Contents { repository, .. }
            | Self::Files { repository, .. } => Some(repository),
            Self::Search | Self::Events { .. } => None,
        }
    }
}

/// An answer of a REST endpoint before rate-limit headers are added.
pub(crate) enum Answer {
    /// A JSON body, with a `Link` header when paginated.
    Json {
        status: StatusCode,
        body: Value,
        link: Option<String>,
    },
    /// Raw file content (`Accept: application/vnd.github.raw`).
    Raw(String),
}

fn error(status: StatusCode, message: &str) -> Answer {
    Answer::Json {
        status,
        body: json!({"message": message, "documentation_url": "https://docs.github.com/rest", "status": status.as_str()}),
        link: None,
    }
}

/// 404 as GitHub answers for missing and invisible resources alike.
pub(crate) fn not_found() -> Answer {
    error(StatusCode::NOT_FOUND, "Not Found")
}

/// `per_page` (default 30, at most 100) and `page` (default 1) of a request.
pub(crate) fn page_params(query: &[(String, String)]) -> (usize, usize) {
    let number = |key: &str| {
        query
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.parse::<usize>().ok())
    };
    let per_page = number("per_page").filter(|n| *n > 0).unwrap_or(30).min(100);
    (per_page, number("page").filter(|n| *n > 0).unwrap_or(1))
}

/// One page of `items` (of which at most `cap` are reachable) with GitHub's `Link` header; `base` is the
/// request URL without its query.
pub(crate) fn page(
    items: &[Value],
    cap: usize,
    base: &str,
    query: &[(String, String)],
) -> (Vec<Value>, Option<String>) {
    let (per_page, page) = page_params(query);
    let reachable = items.len().min(cap);
    let last = reachable.div_ceil(per_page).max(1);
    let start = (page - 1).saturating_mul(per_page).min(reachable);
    let slice = items[start..(start + per_page).min(reachable)].to_vec();
    let link_to = |target: usize, rel: &str| {
        let query = Serializer::new(String::new())
            .extend_pairs(query.iter().filter(|(k, _)| k != "page"))
            .append_pair("page", &target.to_string())
            .finish();
        format!("<{base}?{query}>; rel=\"{rel}\"")
    };
    let mut links = Vec::new();
    if page > 1 {
        links.push(link_to(page - 1, "prev"));
    }
    if page < last {
        links.push(link_to(page + 1, "next"));
        links.push(link_to(last, "last"));
    }
    if page > 1 {
        links.push(link_to(1, "first"));
    }
    (slice, (!links.is_empty()).then(|| links.join(", ")))
}

/// Answers one non-search endpoint; `raw` is set when the caller asked for raw content.
pub(crate) fn answer(
    endpoint: &Endpoint,
    world: &World,
    authenticated: bool,
    base: &str,
    query: &[(String, String)],
    raw: bool,
) -> Answer {
    if let Some(repository) = endpoint.repository()
        && world.repository(repository, authenticated).is_none()
    {
        return not_found();
    }
    let list = |items: Option<&Vec<Value>>, cap: usize| {
        let items = items.cloned().unwrap_or_default();
        let (slice, link) = page(&items, cap, base, query);
        Answer::Json {
            status: StatusCode::OK,
            body: Value::Array(slice),
            link,
        }
    };
    let param = |key: &str| {
        query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    let rest = &world.rest;
    match endpoint {
        Endpoint::LatestRelease { repository } => rest
            .releases
            .get(repository)
            .into_iter()
            .flatten()
            .filter(|r| r["draft"] != true && r["prerelease"] != true)
            .max_by(|a, b| a["published_at"].as_str().cmp(&b["published_at"].as_str()))
            .map_or_else(not_found, |release| Answer::Json {
                status: StatusCode::OK,
                body: release.clone(),
                link: None,
            }),
        Endpoint::Releases { repository } => list(rest.releases.get(repository), usize::MAX),
        Endpoint::Milestones { repository } => {
            let state = param("state").unwrap_or("open");
            let milestones: Vec<Value> = rest
                .milestones
                .get(repository)
                .into_iter()
                .flatten()
                .filter(|m| state == "all" || m["state"] == state)
                .cloned()
                .collect();
            list(Some(&milestones), usize::MAX)
        }
        Endpoint::Contents { repository, path } => {
            let Some(content) = rest
                .contents
                .get(repository)
                .and_then(|files| files.get(path))
            else {
                return not_found();
            };
            let text = match content {
                Value::String(text) => text.clone(),
                other => serde_json::to_string_pretty(other).unwrap_or_default(),
            };
            if raw {
                return Answer::Raw(text);
            }
            let encoded = STANDARD.encode(&text);
            let wrapped: Vec<&str> = encoded
                .as_bytes()
                .chunks(60)
                .filter_map(|chunk| std::str::from_utf8(chunk).ok())
                .collect();
            let sha: String = Sha256::digest(text.as_bytes())
                .iter()
                .take(20)
                .map(|byte| format!("{byte:02x}"))
                .collect();
            Answer::Json {
                status: StatusCode::OK,
                body: json!({"type": "file", "encoding": "base64", "size": text.len(),
                    "name": path.rsplit('/').next(), "path": path, "sha": sha,
                    "content": format!("{}\n", wrapped.join("\n"))}),
                link: None,
            }
        }
        Endpoint::Files { repository, number } => {
            if world.pull_request(repository, *number).is_none() {
                return not_found();
            }
            list(rest.files.get(&format!("{repository}#{number}")), 3000)
        }
        Endpoint::Events { login } => {
            if world.account(login).is_none() {
                return not_found();
            }
            let (per_page, page_number) = page_params(query);
            if (page_number - 1).saturating_mul(per_page) >= 300 {
                return error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "In order to keep the API fast for everyone, pagination is limited for this resource. Check \
                     the rel=last link relation in the Link response header to see how far back you can traverse.",
                );
            }
            list(rest.events.get(login), 300)
        }
        Endpoint::Search => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_map_to_endpoints() {
        assert_eq!(Endpoint::of("/search/issues"), Some(Endpoint::Search));
        assert_eq!(
            Endpoint::of("/repos/acme/platform/contents/policies/platform.json"),
            Some(Endpoint::Contents {
                repository: "acme/platform".into(),
                path: "policies/platform.json".into()
            })
        );
        assert_eq!(
            Endpoint::of("/repos/acme/platform/pulls/12/files"),
            Some(Endpoint::Files {
                repository: "acme/platform".into(),
                number: 12
            })
        );
        assert_eq!(Endpoint::of("/repos/acme/platform/pulls/x/files"), None);
        assert_eq!(Endpoint::of("/repos/acme/platform/contents/"), None);
        assert_eq!(Endpoint::of("/repos/acme/platform/pulls/12"), None);
    }

    #[test]
    fn pages_link_next_last_prev_first() {
        let items: Vec<Value> = (0..5).map(Value::from).collect();
        let url = "http://fake.test/users/alice/events";
        let query = |page: &str| {
            vec![
                ("per_page".to_owned(), "2".to_owned()),
                ("page".to_owned(), page.to_owned()),
            ]
        };
        let (slice, link) = page(&items, usize::MAX, url, &query("1"));
        assert_eq!(slice, vec![json!(0), json!(1)]);
        assert_eq!(
            link.unwrap(),
            "<http://fake.test/users/alice/events?per_page=2&page=2>; rel=\"next\", \
             <http://fake.test/users/alice/events?per_page=2&page=3>; rel=\"last\""
        );
        let (slice, link) = page(&items, usize::MAX, url, &query("3"));
        assert_eq!(slice, vec![json!(4)]);
        assert!(link.unwrap().ends_with("page=1>; rel=\"first\""));
        let (slice, _) = page(&items, 3, url, &query("2"));
        assert_eq!(
            slice,
            vec![json!(2)],
            "items beyond the cap are unreachable"
        );
        assert_eq!(page_params(&[("per_page".into(), "500".into())]), (100, 1));
    }
}
