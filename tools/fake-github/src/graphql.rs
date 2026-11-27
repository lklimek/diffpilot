//! `POST /graphql`: the review threads of `acme/node-client#482` and the fake user's pending review, kept in
//! memory. Documents are recognised by the field they use (the server's documents, `server::github::review`);
//! answers have GitHub's shape, including the silent `thread: null` for a position outside the diff hunks or a
//! file outside the pull request (spike S3), and the user's Viewed marks per file. Nothing can submit a review.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::StartError;
use crate::fixtures::{RawRecording, load};
use crate::oauth::{Credentials, Oauth};

/// Recorded submitted review threads of the pull request.
const REVIEW_THREADS: &str = "graphql/review-threads.json";
/// The pull request (node id, head) and its files (hunk headers decide which lines take a line thread).
pub(crate) const PULL: &str = "rest/pull.json";
const PULL_FILES: &str = "rest/pull-files.json";
/// Id of the fake user's pending review while it has comments.
const PENDING_REVIEW_ID: &str = "PRR_fake_pending";

/// Lines of one file that take a line thread, per side, from the patch's hunk headers.
#[derive(Debug, Default)]
struct FileLines {
    path: String,
    left: Vec<(u32, u32)>,
    right: Vec<(u32, u32)>,
}

/// A pending comment (each starts its own thread).
#[derive(Clone, Debug)]
struct Pending {
    id: String,
    thread_id: String,
    path: String,
    line: Option<u32>,
    start_line: Option<u32>,
    side: &'static str,
    file_level: bool,
    body: String,
}

/// State of the GraphQL endpoint.
pub(crate) struct Graphql {
    oauth: Arc<Oauth>,
    pull: PullRecording,
    /// The pull request's current head: recorded, or moved with `POST /_fake/head`.
    head: Mutex<String>,
    files: Vec<FileLines>,
    threads: Value,
    pending: Mutex<PendingState>,
    /// The user's Viewed state by path (`VIEWED`, or `DISMISSED` after a head move); absent: `UNVIEWED`.
    viewed: Mutex<BTreeMap<String, &'static str>>,
}

#[derive(Default)]
struct PendingState {
    comments: Vec<Pending>,
    next: u64,
}

#[derive(Deserialize)]
struct PullRecording {
    node_id: String,
    title: String,
    body: Option<String>,
    html_url: String,
    state: String,
    updated_at: String,
    merged: bool,
    draft: bool,
    user: Option<PullAuthor>,
    base: PullBase,
    head: PullRef,
}

#[derive(Deserialize)]
struct PullRef {
    sha: String,
}

#[derive(Deserialize)]
struct PullAuthor {
    login: String,
}

#[derive(Deserialize)]
struct PullBase {
    #[serde(rename = "ref")]
    name: String,
    sha: String,
    repo: PullRepository,
}

#[derive(Deserialize)]
struct PullRepository {
    id: i64,
    private: bool,
}

#[derive(Deserialize)]
struct FileRecording {
    filename: String,
    #[serde(default)]
    patch: Option<String>,
}

/// `@@ -a,b +c,d @@` → ((a, b), (c, d)); a missing count is 1.
fn hunk_header(line: &str) -> Option<((u32, u32), (u32, u32))> {
    let inner = line.strip_prefix("@@ ")?.split(" @@").next()?;
    let mut parts = inner.split(' ');
    let span = |part: Option<&str>, sign: char| -> Option<(u32, u32)> {
        let part = part?.strip_prefix(sign)?;
        let (start, len) = part.split_once(',').unwrap_or((part, "1"));
        Some((start.parse().ok()?, len.parse().ok()?))
    };
    Some((span(parts.next(), '-')?, span(parts.next(), '+')?))
}

fn file_lines(file: FileRecording) -> FileLines {
    let mut lines = FileLines {
        path: file.filename,
        ..FileLines::default()
    };
    for (left, right) in file
        .patch
        .unwrap_or_default()
        .lines()
        .filter_map(hunk_header)
    {
        for (side, (start, len)) in [(&mut lines.left, left), (&mut lines.right, right)] {
            if len > 0 {
                side.push((start, start + len - 1));
            }
        }
    }
    lines
}

impl Graphql {
    pub(crate) fn load(config: &crate::Config, oauth: Arc<Oauth>) -> Result<Self, StartError> {
        let parse = |file: &str, recording: RawRecording| {
            serde_json::from_str::<Value>(recording.response.get()).map_err(|source| {
                StartError::ParseFixture {
                    path: config.fixture_dir.join(file),
                    source,
                }
            })
        };
        let pull: PullRecording =
            serde_json::from_value(parse(PULL, load(&config.fixture_dir, PULL)?)?).map_err(
                |source| StartError::ParseFixture {
                    path: config.fixture_dir.join(PULL),
                    source,
                },
            )?;
        let files: Vec<FileRecording> =
            serde_json::from_value(parse(PULL_FILES, load(&config.fixture_dir, PULL_FILES)?)?)
                .map_err(|source| StartError::ParseFixture {
                    path: config.fixture_dir.join(PULL_FILES),
                    source,
                })?;
        let mut threads = parse(REVIEW_THREADS, load(&config.fixture_dir, REVIEW_THREADS)?)?
            ["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"]
            .take();
        for thread in threads.as_array_mut().into_iter().flatten() {
            for comment in thread["comments"]["nodes"]
                .as_array_mut()
                .into_iter()
                .flatten()
            {
                comment["state"] = json!("SUBMITTED");
            }
        }
        Ok(Self {
            oauth,
            head: Mutex::new(pull.head.sha.clone()),
            pull,
            files: files.into_iter().map(file_lines).collect(),
            threads,
            pending: Mutex::default(),
            viewed: Mutex::default(),
        })
    }

    fn pending(&self) -> MutexGuard<'_, PendingState> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The pull request's current head commit.
    pub(crate) fn head(&self) -> String {
        self.head
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Moves the pull request's head (as a push by the author would).
    pub(crate) fn move_head(&self, sha: String) {
        tracing::info!(%sha, "fake-github moved the pull request head");
        *self.head.lock().unwrap_or_else(PoisonError::into_inner) = sha;
        // GitHub dismisses Viewed marks of files a later commit changes; the fake treats every file as changed.
        for state in self.viewed().values_mut() {
            *state = "DISMISSED";
        }
    }

    fn viewed(&self) -> MutexGuard<'_, BTreeMap<String, &'static str>> {
        self.viewed.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every file of the pull request with the user's Viewed state, on one page.
    fn files_viewed(&self) -> Value {
        let viewed = self.viewed();
        let nodes: Vec<Value> = self
            .files
            .iter()
            .map(|file| {
                let state = viewed.get(&file.path).copied().unwrap_or("UNVIEWED");
                json!({"path": file.path, "viewerViewedState": state})
            })
            .collect();
        self.pull_request(json!({"id": self.pull.node_id, "headRefOid": self.head(),
            "files": {"pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": nodes}}))
    }

    /// `markFileAsViewed` / `unmarkFileAsViewed`; an unknown pull request or path is `NOT_FOUND`.
    fn set_viewed(&self, field: &str, input: &Value, viewed: bool) -> Value {
        let path = input["path"].as_str().unwrap_or_default();
        let known = input["pullRequestId"] == json!(self.pull.node_id)
            && self.files.iter().any(|file| file.path == path);
        if !known {
            return json!({"data": {field: null}, "errors": [{"type": "NOT_FOUND", "path": [field],
                "message": "Could not resolve the pull request or the file."}]});
        }
        if viewed {
            self.viewed().insert(path.to_owned(), "VIEWED");
        } else {
            self.viewed().remove(path);
        }
        tracing::info!(path, viewed, "fake-github changed a Viewed mark");
        json!({"data": {field: {"clientMutationId": null}}})
    }

    fn pull_request(&self, fields: Value) -> Value {
        json!({"data": {"repository": {"pullRequest": fields}}})
    }

    /// Review-loop facts for the ordinary fixture, using the same PR as the REST endpoint.
    fn pull_facts(&self) -> Value {
        let pull = &self.pull;
        let state = if pull.merged {
            "MERGED".to_owned()
        } else {
            pull.state.to_ascii_uppercase()
        };
        let author = pull
            .user
            .as_ref()
            .map(|author| json!({"login": author.login}));
        json!({"data": {"repository": {
            "databaseId": pull.base.repo.id,
            "isPrivate": pull.base.repo.private,
            "pullRequest": {
                "title": pull.title, "body": pull.body.as_deref().unwrap_or_default(),
                "url": pull.html_url, "state": state, "isDraft": pull.draft, "author": author,
                "baseRefName": pull.base.name, "baseRefOid": pull.base.sha, "headRefOid": self.head(),
                "reviewDecision": null, "updatedAt": pull.updated_at,
            }
        }}})
    }

    /// Submitted threads plus one per pending comment.
    fn thread_nodes(&self, pending: &[Pending]) -> Vec<Value> {
        let mut nodes: Vec<Value> = self.threads.as_array().cloned().unwrap_or_default();
        for (index, node) in nodes.iter_mut().enumerate() {
            node["comments"]["nodes"][0]["id"] = json!(format!("PRRC_fixture482_t{}", index + 1));
        }
        nodes.extend(pending.iter().map(|comment| {
            json!({"id": comment.thread_id, "path": comment.path,
                "line": if comment.file_level { Some(1) } else { comment.line },
                "startLine": comment.start_line.or(comment.line), "diffSide": comment.side,
                "subjectType": if comment.file_level { "FILE" } else { "LINE" },
                "isResolved": false, "isOutdated": false,
                "comments": {"totalCount": 1, "nodes": [{"id": comment.id, "state": "PENDING",
                    "body": comment.body, "createdAt": "2026-11-16T10:12:00Z",
                    "url": format!("https://github.com/acme/node-client/pull/482#discussion_{}", comment.id),
                    "author": {"login": "alice"}}]}})
        }));
        nodes
    }

    fn review_threads(&self) -> Value {
        let nodes = self.thread_nodes(&self.pending().comments);
        self.pull_request(json!({"reviewThreads": {"totalCount": nodes.len(),
            "pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": nodes}}))
    }

    fn pending_review(&self) -> Value {
        let state = self.pending();
        let reviews = if state.comments.is_empty() {
            Vec::new()
        } else {
            let comments: Vec<Value> = state
                .comments
                .iter()
                .map(|comment| {
                    json!({"id": comment.id, "path": comment.path,
                        "line": if comment.file_level { None } else { comment.line },
                        "startLine": comment.start_line,
                        "subjectType": if comment.file_level { "FILE" } else { "LINE" },
                        "state": "PENDING", "body": comment.body, "replyTo": null})
                })
                .collect();
            vec![
                json!({"id": PENDING_REVIEW_ID, "state": "PENDING", "viewerDidAuthor": true,
                "comments": {"totalCount": comments.len(),
                    "pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": comments}}),
            ]
        };
        let threads = self.thread_nodes(&state.comments);
        self.pull_request(json!({"id": self.pull.node_id, "headRefOid": self.head(),
            "reviews": {"totalCount": reviews.len(),
                "pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": reviews},
            "reviewThreads": {"totalCount": threads.len(),
                "pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": threads}}))
    }

    /// Whether GitHub would accept the thread: the file is in the pull request and, for a line thread, every
    /// line lies in one hunk of its side.
    fn accepts(&self, input: &Value) -> bool {
        let Some(file) = self
            .files
            .iter()
            .find(|file| Some(file.path.as_str()) == input["path"].as_str())
        else {
            return false;
        };
        if input["subjectType"] == "FILE" {
            return true;
        }
        let Some(line) = input["line"].as_u64().and_then(|n| u32::try_from(n).ok()) else {
            return false;
        };
        let start = input["startLine"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(line);
        let hunks = if input["side"] == "LEFT" {
            &file.left
        } else {
            &file.right
        };
        hunks.iter().any(|(a, b)| *a <= start && line <= *b)
    }

    fn add_thread(&self, input: &Value) -> Value {
        if input["pullRequestId"] != json!(self.pull.node_id) || !self.accepts(input) {
            tracing::info!(path = ?input["path"], line = ?input["line"],
                "fake-github rejects the thread silently (thread: null), as GitHub does");
            return json!({"data": {"addPullRequestReviewThread": {"thread": null}}});
        }
        let mut state = self.pending();
        state.next += 1;
        let file_level = input["subjectType"] == "FILE";
        let line = input["line"].as_u64().and_then(|n| u32::try_from(n).ok());
        let comment = Pending {
            id: format!("PRRC_fake_{}", state.next),
            thread_id: format!("PRRT_fake_{}", state.next),
            path: input["path"].as_str().unwrap_or_default().to_owned(),
            line,
            start_line: input["startLine"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok()),
            side: if input["side"] == "LEFT" {
                "LEFT"
            } else {
                "RIGHT"
            },
            file_level,
            body: input["body"].as_str().unwrap_or_default().to_owned(),
        };
        state.comments.push(comment.clone());
        tracing::info!(id = %comment.id, "fake-github added a pending comment");
        json!({"data": {"addPullRequestReviewThread": {"thread": {
            "id": comment.thread_id, "path": comment.path,
            "line": if file_level { Some(1) } else { line },
            "startLine": comment.start_line.or(line), "diffSide": comment.side,
            "subjectType": if file_level { "FILE" } else { "LINE" },
            "comments": {"nodes": [{"id": comment.id, "body": comment.body, "state": "PENDING",
                "commit": {"oid": self.head()}, "originalCommit": {"oid": self.head()}}]}}}}})
    }

    fn not_found(field: &str, id: &Value) -> Value {
        json!({"data": {field: null}, "errors": [{"type": "NOT_FOUND", "path": [field],
            "message": format!("Could not resolve to a node with the global id of '{}'.", id.as_str().unwrap_or_default())}]})
    }

    fn update_comment(&self, input: &Value) -> Value {
        let id = &input["pullRequestReviewCommentId"];
        let mut state = self.pending();
        let Some(comment) = state.comments.iter_mut().find(|c| json!(c.id) == *id) else {
            return Self::not_found("updatePullRequestReviewComment", id);
        };
        comment.body = input["body"].as_str().unwrap_or_default().to_owned();
        json!({"data": {"updatePullRequestReviewComment": {"pullRequestReviewComment":
            {"id": comment.id, "body": comment.body, "state": "PENDING",
                "pullRequestReview": {"state": "PENDING"}}}}})
    }

    fn delete_comment(&self, input: &Value) -> Value {
        let id = &input["id"];
        let mut state = self.pending();
        let Some(index) = state.comments.iter().position(|c| json!(c.id) == *id) else {
            return Self::not_found("deletePullRequestReviewComment", id);
        };
        let removed = state.comments.remove(index);
        tracing::info!(id = %removed.id, left = state.comments.len(), "fake-github deleted a pending comment");
        // A pending review disappears with its last comment.
        let review = (!state.comments.is_empty()).then(|| json!({"state": "PENDING"}));
        json!({"data": {"deletePullRequestReviewComment": {"pullRequestReview": review,
            "pullRequestReviewComment": {"id": removed.id}}}})
    }
}

/// The JSON envelope shared by the GraphQL fixture routes.
#[derive(Deserialize)]
pub(crate) struct GraphqlRequest {
    pub(crate) query: String,
    #[serde(default)]
    pub(crate) variables: Value,
}

fn graphql_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({"message": message, "documentation_url": "https://docs.github.com/graphql"})),
    )
        .into_response()
}

/// Answers one GraphQL request; a valid access token is required (GitHub has no anonymous GraphQL).
pub(crate) fn serve(
    graphql: &Graphql,
    headers: &HeaderMap,
    request: Option<&GraphqlRequest>,
) -> Response {
    if graphql.oauth.credentials(headers) != Credentials::Valid {
        return graphql_error(StatusCode::UNAUTHORIZED, "Bad credentials");
    }
    let Some(request) = request else {
        return graphql_error(StatusCode::BAD_REQUEST, "Problems parsing JSON");
    };
    let query = request.query.as_str();
    let input = &request.variables["in"];
    let answer = if query.contains("submitPullRequestReview")
        || query.contains("addPullRequestReview(")
    {
        tracing::warn!(
            "fake-github refuses to create or submit a review; the server must never do this"
        );
        json!({"data": null, "errors": [{"type": "FORBIDDEN", "message": "not supported by fake-github"}]})
    } else if query.contains("unmarkFileAsViewed") {
        graphql.set_viewed("unmarkFileAsViewed", input, false)
    } else if query.contains("markFileAsViewed") {
        graphql.set_viewed("markFileAsViewed", input, true)
    } else if query.contains("viewerViewedState") {
        graphql.files_viewed()
    } else if query.contains("addPullRequestReviewThread") {
        graphql.add_thread(input)
    } else if query.contains("updatePullRequestReviewComment") {
        graphql.update_comment(input)
    } else if query.contains("deletePullRequestReviewComment") {
        graphql.delete_comment(input)
    } else if query.contains("reviews(states:[PENDING]") {
        graphql.pending_review()
    } else if query.contains("reviewThreads") && request.variables["after"].is_string() {
        // Every thread fits on the first page.
        graphql.pull_request(json!({"reviewThreads": empty_page()}))
    } else if query.contains("reviewThreads") {
        graphql.review_threads()
    } else if query.contains("repository(")
        && query.contains("baseRefOid")
        && query.contains("reviewDecision")
    {
        graphql.pull_facts()
    } else if query.contains("pullRequest(number:$pr){reviews(first:100") {
        graphql.pull_request(json!({"reviews": empty_page()}))
    } else if query.contains("pullRequest(number:$pr){comments(first:100") {
        graphql.pull_request(json!({"comments": empty_page()}))
    } else if query.contains("headRefOid") {
        graphql.pull_request(json!({"id": graphql.pull.node_id, "headRefOid": graphql.head()}))
    } else {
        tracing::warn!("fake-github has no answer for this GraphQL document");
        json!({"data": null, "errors": [{"type": "UNKNOWN_DOCUMENT", "message": "not supported by fake-github"}]})
    };
    (StatusCode::OK, Json(answer)).into_response()
}

fn empty_page() -> Value {
    json!({"totalCount": 0, "pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": []})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_facts_preserve_updated_at_and_report_a_merged_pr_with_a_deleted_author() {
        let config = crate::Config {
            fixture_dir: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/pr-482/github"),
            app_url: url::Url::parse("http://app.test:3000").unwrap(),
            user_login: None,
            user_id: None,
            app_id: None,
            app_public_key_file: None,
            dashboard_fixture_dir: None,
            dashboard_world_at: None,
            review_loop_fixture_dir: None,
        };
        let oauth = Arc::new(Oauth::load(&config).unwrap());
        let mut graphql = Graphql::load(&config, oauth).unwrap();
        graphql.pull.merged = true;
        graphql.pull.user = None;
        let head = "1".repeat(40);
        graphql.move_head(head.clone());

        let facts = graphql.pull_facts();
        let repository = &facts["data"]["repository"];
        let pull = &repository["pullRequest"];
        assert_eq!(repository["databaseId"], 4821400);
        assert_eq!(repository["isPrivate"], false);
        assert_eq!(pull["updatedAt"], "2026-11-12T08:00:00Z");
        assert_eq!(pull["state"], "MERGED");
        assert!(pull.get("author").is_some_and(Value::is_null));
        assert_eq!(pull["headRefOid"], head);
    }

    #[test]
    fn hunk_headers_give_both_sides() {
        assert_eq!(
            hunk_header("@@ -32,13 +32,14 @@ fn x"),
            Some(((32, 13), (32, 14)))
        );
        assert_eq!(hunk_header("@@ -0,0 +1 @@"), Some(((0, 0), (1, 1))));
        assert_eq!(hunk_header("+added line"), None);
    }
}
