//! Existing review threads of a pull request (GraphQL, user token) for the Tour, and the check that a link
//! GitHub reports points to `https://github.com` before it reaches the browser.

use contracts::api::review_loop::{GithubThreadComment, GithubThreadDetail};
use contracts::api::tour::{GithubThread, ThreadsStatus};
use contracts::common::{LineSide, PrRef};
use contracts::time::Timestamp;
use secrecy::SecretString;
use serde::Deserialize;
use url::Url;

use crate::config::GithubConfig;
use crate::github::graphql::{self, GraphqlError, Page};

/// Threads are read in pages of 100; at most this many pages (1000 threads). More threads: the list is partial.
pub const MAX_PAGES: usize = 10;
/// Characters of the first comment kept as the excerpt.
const EXCERPT_CHARS: usize = 200;

/// Review threads with their first comment; the viewer's own pending threads are read too and dropped.
pub const REVIEW_THREADS: &str = "query($o:String!,$n:String!,$pr:Int!,$after:String){repository(owner:$o,name:$n){\
pullRequest(number:$pr){reviewThreads(first:100,after:$after){pageInfo{hasNextPage endCursor} \
nodes{id path line startLine diffSide subjectType isResolved isOutdated \
comments(first:1){totalCount nodes{state body url author{login}}}}}}}}";

/// Reads one review thread by node ID, including the owning PR on every page.
const THREAD_DETAIL: &str = "query($id:ID!,$after:String){node(id:$id){__typename ... on PullRequestReviewThread{\
path pullRequest{number title repository{nameWithOwner isPrivate}} comments(first:100,after:$after){\
pageInfo{hasNextPage endCursor} nodes{state body url createdAt author{login}}}}}}";

const PR_COMMENT_DETAIL: &str = "query($id:ID!){node(id:$id){__typename ... on IssueComment{body url createdAt author{login} \
pullRequest{number title repository{nameWithOwner isPrivate}}} ... on PullRequestReview{body url \
submittedAt state author{login} pullRequest{number title repository{nameWithOwner isPrivate}}}}}";

/// `raw` if it is an `https://github.com/…` URL (no other host, port or credentials); the UI renders it as a
/// link, so nothing else may pass.
pub fn github_web_url(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    let valid = url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none();
    valid.then(|| url.into())
}

/// The pull request's own page; the link of a thread whose reported URL is not a GitHub URL.
fn pull_url(pr: &PrRef) -> String {
    format!(
        "https://github.com/{}/{}/pull/{}",
        pr.repo.owner, pr.repo.name, pr.number
    )
}

#[derive(Deserialize)]
struct Data {
    repository: Option<Repository>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    pull_request: Option<PullRequest>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequest {
    review_threads: Page<ThreadNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadNode {
    id: String,
    path: String,
    line: Option<u32>,
    start_line: Option<u32>,
    diff_side: Option<String>,
    subject_type: Option<String>,
    is_resolved: bool,
    is_outdated: bool,
    comments: Comments,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Comments {
    total_count: u32,
    nodes: Vec<CommentNode>,
}

#[derive(Deserialize)]
struct CommentNode {
    state: Option<String>,
    body: String,
    url: String,
    author: Option<Author>,
}

#[derive(Deserialize)]
struct Author {
    login: String,
}

#[derive(Deserialize)]
struct DetailData {
    node: Option<DetailNode>,
}

#[derive(Deserialize)]
#[serde(tag = "__typename", rename_all_fields = "camelCase")]
enum DetailNode {
    PullRequestReviewThread {
        path: String,
        pull_request: DetailPr,
        comments: Page<DetailComment>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct DetailPr {
    number: u32,
    title: String,
    repository: DetailRepo,
}

impl DetailPr {
    fn matches(&self, pr: &PrRef) -> bool {
        self.number == pr.number
            && !self.repository.is_private
            && self
                .repository
                .name_with_owner
                .eq_ignore_ascii_case(&format!("{}/{}", pr.repo.owner, pr.repo.name))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailRepo {
    name_with_owner: String,
    is_private: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailComment {
    state: String,
    body: String,
    url: String,
    created_at: Timestamp,
    author: Option<Author>,
}

#[derive(Deserialize)]
struct PrCommentData {
    node: Option<PrCommentNode>,
}

#[derive(Deserialize)]
#[serde(tag = "__typename", rename_all_fields = "camelCase")]
enum PrCommentNode {
    IssueComment {
        body: String,
        url: String,
        created_at: Timestamp,
        author: Option<Author>,
        pull_request: Option<DetailPr>,
    },
    PullRequestReview {
        body: String,
        url: String,
        submitted_at: Option<Timestamp>,
        state: String,
        author: Option<Author>,
        pull_request: DetailPr,
    },
    #[serde(other)]
    Other,
}

/// One exact submitted PR-level comment or review body.
pub async fn pr_comment_detail(
    config: &GithubConfig,
    token: &SecretString,
    pr: &PrRef,
    id: &str,
) -> Result<Option<GithubThreadDetail>, GraphqlError> {
    let Some((kind, id)) = id.split_once(':') else {
        return Ok(None);
    };
    let data: PrCommentData = graphql::execute(
        config,
        token,
        PR_COMMENT_DETAIL,
        serde_json::json!({"id": id}),
    )
    .await?;
    let (body, url, at, author, pull) = match (kind, data.node) {
        (
            "issue",
            Some(PrCommentNode::IssueComment {
                body,
                url,
                created_at,
                author,
                pull_request: Some(pull),
            }),
        ) => (body, url, created_at, author, pull),
        (
            "review",
            Some(PrCommentNode::PullRequestReview {
                body,
                url,
                submitted_at: Some(at),
                state,
                author,
                pull_request,
            }),
        ) if state != "PENDING" => (body, url, at, author, pull_request),
        _ => return Ok(None),
    };
    if !pull.matches(pr) {
        return Ok(None);
    }
    Ok(Some(GithubThreadDetail {
        title: pull.title,
        path: String::new(),
        pr_level: true,
        agent_annotations: Vec::new(),
        item_no: None,
        complete: true,
        comments: vec![GithubThreadComment {
            author: author.map_or_else(|| "ghost".to_owned(), |author| author.login),
            body,
            created_at: at,
            url: github_web_url(&url),
        }],
    }))
}

/// One submitted conversation from the given PR, read with the viewer's GitHub token.
pub async fn detail(
    config: &GithubConfig,
    token: &SecretString,
    pr: &PrRef,
    id: &str,
) -> Result<Option<GithubThreadDetail>, GraphqlError> {
    let mut after = None;
    let mut result: Option<GithubThreadDetail> = None;
    for _ in 0..MAX_PAGES {
        let data: DetailData = graphql::execute(
            config,
            token,
            THREAD_DETAIL,
            serde_json::json!({"id": id, "after": after}),
        )
        .await?;
        let Some(DetailNode::PullRequestReviewThread {
            path,
            pull_request,
            comments,
        }) = data.node
        else {
            return Ok(None);
        };
        if !pull_request.matches(pr) {
            return Ok(None);
        }
        if result.is_none()
            && comments
                .nodes
                .first()
                .is_none_or(|comment| comment.state == "PENDING")
        {
            return Ok(None);
        }
        let thread = result.get_or_insert_with(|| GithubThreadDetail {
            title: pull_request.title,
            path,
            pr_level: false,
            agent_annotations: Vec::new(),
            item_no: None,
            complete: false,
            comments: Vec::new(),
        });
        thread.comments.extend(
            comments
                .nodes
                .into_iter()
                .filter(|comment| comment.state != "PENDING")
                .map(|comment| GithubThreadComment {
                    author: comment
                        .author
                        .map_or_else(|| "ghost".to_owned(), |author| author.login),
                    body: comment.body,
                    created_at: comment.created_at,
                    url: github_web_url(&comment.url),
                }),
        );
        if !comments.page_info.has_next_page {
            thread.complete = true;
            break;
        }
        let next = comments.page_info.end_cursor;
        if next.is_none() || next == after {
            break;
        }
        after = next;
    }
    Ok(result)
}

/// GitHub's `diffSide` as a line side.
pub(crate) fn line_side(diff_side: Option<&str>) -> Option<LineSide> {
    match diff_side {
        Some("LEFT") => Some(LineSide::Base),
        Some("RIGHT") => Some(LineSide::Head),
        _ => None,
    }
}

/// The start of `body` on one line: whitespace collapsed, at most [`EXCERPT_CHARS`] characters.
fn excerpt(body: &str) -> String {
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    match collapsed.char_indices().nth(EXCERPT_CHARS) {
        Some((cut, _)) => format!("{}…", &collapsed[..cut]),
        None => collapsed,
    }
}

fn thread(pr: &PrRef, node: ThreadNode) -> Option<GithubThread> {
    let first = node.comments.nodes.into_iter().next()?;
    if first.state.as_deref() == Some("PENDING") {
        return None;
    }
    let file_level = node.subject_type.as_deref() == Some("FILE");
    let line = node.line.filter(|_| !file_level);
    Some(GithubThread {
        id: node.id,
        path: node.path,
        line,
        start_line: node
            .start_line
            .filter(|start| line.is_some_and(|line| *start < line)),
        side: line_side(node.diff_side.as_deref()).filter(|_| !file_level),
        author: first
            .author
            .map_or_else(|| "ghost".to_owned(), |author| author.login),
        excerpt: excerpt(&first.body),
        comment_count: node.comments.total_count,
        url: github_web_url(&first.url).unwrap_or_else(|| pull_url(pr)),
        is_resolved: node.is_resolved,
        is_outdated: node.is_outdated,
        step_id: None,
        llm_status: None,
        llm_reason: None,
    })
}

/// Every submitted review thread of the pull request (at most 1000), read with the user's token. Step, LLM
/// status and reason are left empty for the Tour projection.
pub async fn fetch(
    config: &GithubConfig,
    token: &SecretString,
    pr: &PrRef,
) -> Result<(Vec<GithubThread>, ThreadsStatus), GraphqlError> {
    let (nodes, complete) = graphql::pr_pages(
        config,
        token,
        pr,
        REVIEW_THREADS,
        MAX_PAGES,
        None,
        |data: Data| {
            data.repository
                .and_then(|repository| repository.pull_request)
                .map(|pull| pull.review_threads)
        },
    )
    .await?
    .ok_or(GraphqlError::NoData)?;
    let threads = nodes
        .into_iter()
        .filter_map(|node| thread(pr, node))
        .collect();
    let status = if complete {
        ThreadsStatus::Checked
    } else {
        ThreadsStatus::Partial
    };
    Ok((threads, status))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use contracts::api::tour::Tour;
    use contracts::common::RepoRef;
    use serde_json::{Value, json};
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::github::graphql::tests::github_config;

    fn fixture_pr() -> PrRef {
        PrRef {
            repo: RepoRef {
                owner: "acme".to_owned(),
                name: "node-client".to_owned(),
            },
            number: 482,
        }
    }

    fn fixture(relative: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/pr-482")
            .join(relative);
        serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("JSON")
    }

    /// The recorded answer with `pageInfo` (the recording predates pagination).
    fn recorded_page() -> Value {
        let mut response = fixture("github/graphql/review-threads.json")["response"].clone();
        response["data"]["repository"]["pullRequest"]["reviewThreads"]["pageInfo"] =
            json!({"hasNextPage": false, "endCursor": null});
        response
    }

    #[test]
    fn only_github_web_urls_pass() {
        for raw in [
            "https://github.com/acme/node-client/pull/482#discussion_r1",
            "https://github.com/acme/node-client/runs/3",
        ] {
            assert_eq!(github_web_url(raw).as_deref(), Some(raw), "{raw}");
        }
        for raw in [
            "http://github.com/acme/node-client",
            "javascript:alert(1)",
            "https://github.com.evil.test/x",
            "https://evil.test/github.com",
            "https://user@github.com/x",
            "https://github.com:8443/x",
            "https://api.github.com/repos/acme/node-client",
            "https://ci.example.test/run/1",
            "data:text/html,<script>",
            "/relative/path",
            "",
        ] {
            assert_eq!(github_web_url(raw), None, "{raw}");
        }
    }

    #[tokio::test]
    async fn threads_match_the_tour_seed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .and(body_partial_json(
                json!({"variables": {"o": "acme", "n": "node-client", "pr": 482}}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(recorded_page()))
            .expect(1)
            .mount(&server)
            .await;
        let threads = fetch(
            &github_config(&server),
            &SecretString::from("ghu_t"),
            &fixture_pr(),
        )
        .await
        .expect("threads");
        assert_eq!(threads.1, ThreadsStatus::Checked);
        let threads = threads.0;
        let tour: Tour =
            serde_json::from_value(fixture("api-seed/default/tour.json")).expect("tour");
        let expected: Vec<GithubThread> = tour
            .threads
            .into_iter()
            .map(|thread| GithubThread {
                step_id: None,
                llm_status: None,
                llm_reason: None,
                ..thread
            })
            .collect();
        assert_eq!(threads, expected);
    }

    fn thread_node(id: &str, extra: Value) -> Value {
        let mut node = json!({
            "id": id, "path": "src/a.rs", "line": 7, "startLine": 7, "diffSide": "LEFT",
            "subjectType": "LINE", "isResolved": true, "isOutdated": true,
            "comments": {"totalCount": 3, "nodes": [{
                "state": "SUBMITTED", "body": "  first\n\nline  ",
                "url": "https://github.com/acme/node-client/pull/482#discussion_r9",
                "author": null
            }]}
        });
        if let (Value::Object(node), Value::Object(extra)) = (&mut node, extra) {
            node.extend(extra);
        }
        node
    }

    fn thread_page(nodes: Vec<Value>, next: Option<&str>) -> Value {
        json!({"data": {"repository": {"pullRequest": {"reviewThreads": {
            "pageInfo": {"hasNextPage": next.is_some(), "endCursor": next},
            "nodes": nodes
        }}}}})
    }

    #[tokio::test]
    async fn pages_are_followed_and_pending_file_and_foreign_link_threads_are_mapped() {
        let server = MockServer::start().await;
        let pending = json!({"comments": {"totalCount": 1, "nodes": [{"state": "PENDING", "body": "mine",
            "url": "https://github.com/x", "author": {"login": "alice"}}]}});
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .and(body_partial_json(json!({"variables": {"after": null}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(thread_page(
                vec![thread_node("T1", json!({})), thread_node("T2", pending)],
                Some("c1"),
            )))
            .expect(1)
            .mount(&server)
            .await;
        let file =
            json!({"subjectType": "FILE", "line": 1, "startLine": null, "diffSide": "RIGHT"});
        let foreign = json!({"comments": {"totalCount": 1, "nodes": [{"state": "SUBMITTED",
            "body": "b", "url": "javascript:alert(1)", "author": {"login": "eve"}}]}});
        let empty = json!({"comments": {"totalCount": 0, "nodes": []}});
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .and(body_partial_json(json!({"variables": {"after": "c1"}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(thread_page(
                vec![
                    thread_node("T3", file),
                    thread_node("T4", foreign),
                    thread_node("T5", empty),
                ],
                None,
            )))
            .expect(1)
            .mount(&server)
            .await;
        let threads = fetch(
            &github_config(&server),
            &SecretString::from("ghu_t"),
            &fixture_pr(),
        )
        .await
        .expect("threads");
        assert_eq!(threads.1, ThreadsStatus::Checked);
        let threads = threads.0;
        let ids: Vec<&str> = threads.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["T1", "T3", "T4"], "pending and empty threads dropped");
        let t1 = &threads[0];
        assert_eq!(
            (t1.line, t1.start_line, t1.side),
            (Some(7), None, Some(LineSide::Base))
        );
        assert_eq!(t1.author, "ghost");
        assert_eq!(t1.excerpt, "first line");
        assert_eq!(t1.comment_count, 3);
        assert!(t1.is_resolved && t1.is_outdated);
        let t3 = &threads[1];
        assert_eq!((t3.line, t3.start_line, t3.side), (None, None, None));
        assert_eq!(
            threads[2].url, "https://github.com/acme/node-client/pull/482",
            "a non-GitHub link is replaced by the pull request page"
        );
    }

    #[tokio::test]
    async fn threads_beyond_the_cap_are_partial() {
        let server = MockServer::start().await;
        let nodes: Vec<Value> = (0..100)
            .map(|n| thread_node(&format!("T{n}"), json!({})))
            .collect();
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(thread_page(nodes, Some("more"))),
            )
            .expect(u64::try_from(MAX_PAGES).expect("pages"))
            .mount(&server)
            .await;
        let (threads, status) = fetch(
            &github_config(&server),
            &SecretString::from("ghu_t"),
            &fixture_pr(),
        )
        .await
        .expect("threads");
        assert_eq!(threads.len(), 100 * MAX_PAGES);
        assert_eq!(status, ThreadsStatus::Partial, "never reported as complete");
    }

    #[tokio::test]
    async fn a_next_page_without_a_cursor_is_partial() {
        let server = MockServer::start().await;
        let mut page = thread_page(vec![thread_node("T1", json!({}))], None);
        page["data"]["repository"]["pullRequest"]["reviewThreads"]["pageInfo"]["hasNextPage"] =
            json!(true);
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page))
            .expect(1)
            .mount(&server)
            .await;
        let (threads, status) = fetch(
            &github_config(&server),
            &SecretString::from("ghu_t"),
            &fixture_pr(),
        )
        .await
        .expect("threads");
        assert_eq!(threads.len(), 1);
        assert_eq!(status, ThreadsStatus::Partial);
    }

    #[test]
    fn excerpt_is_cut_at_a_character_boundary() {
        let long = "ä".repeat(EXCERPT_CHARS + 5);
        let cut = excerpt(&long);
        assert_eq!(cut.chars().count(), EXCERPT_CHARS + 1);
        assert!(cut.ends_with('…'));
        assert_eq!(excerpt("short"), "short");
    }

    #[tokio::test]
    async fn missing_pull_request_is_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"data": {"repository": {"pullRequest": null}}})),
            )
            .mount(&server)
            .await;
        let result = fetch(
            &github_config(&server),
            &SecretString::from("ghu_t"),
            &fixture_pr(),
        )
        .await;
        assert!(matches!(result, Err(GraphqlError::NoData)), "{result:?}");
    }
}
