//! The user's pending review through GraphQL with the user's token (ADR-006, spike S3): a thread is added with
//! `addPullRequestReviewThread` (which creates the pending review or reuses the existing one), a comment is
//! edited with `updatePullRequestReviewComment` and deleted with `deletePullRequestReviewComment`. Nothing here
//! submits a review; [`DOCUMENTS`] lists every document this module sends.

use contracts::api::comments::{CommentKind, PendingComment};
use contracts::common::{LineSide, PrRef};
use secrecy::SecretString;
use serde::Deserialize;
use serde_json::json;
use url::Url;

use crate::config::GithubConfig;
use crate::github::graphql::{self, GraphqlError};
use crate::github::threads::line_side;

/// The pull request's node id and current head.
pub const PULL_REQUEST_HEAD: &str = "query($o:String!,$n:String!,$pr:Int!){repository(owner:$o,name:$n){\
pullRequest(number:$pr){id headRefOid}}}";

/// The user's pending reviews with their comments, and the first page of thread sides.
pub const PENDING_REVIEW: &str = "query($o:String!,$n:String!,$pr:Int!){repository(owner:$o,name:$n){\
pullRequest(number:$pr){id headRefOid \
reviews(states:[PENDING],first:10){nodes{id viewerDidAuthor \
comments(first:100){totalCount nodes{id path line startLine subjectType body replyTo{id}}}}} \
reviewThreads(first:100){pageInfo{hasNextPage endCursor} nodes{id diffSide comments(first:1){nodes{id}}}}}}}";

/// Further pages of thread sides.
pub const THREAD_SIDES: &str = "query($o:String!,$n:String!,$pr:Int!,$after:String!){repository(owner:$o,name:$n){\
pullRequest(number:$pr){reviewThreads(first:100,after:$after){pageInfo{hasNextPage endCursor} \
nodes{id diffSide comments(first:1){nodes{id}}}}}}}";

/// Adds a thread with its first comment to the user's pending review (created when missing).
pub const ADD_THREAD: &str = "mutation($in:AddPullRequestReviewThreadInput!){addPullRequestReviewThread(input:$in){\
thread{id path line startLine diffSide subjectType comments(first:1){nodes{id body}}}}}";

/// Replaces the text of a pending comment.
pub const UPDATE_COMMENT: &str = "mutation($in:UpdatePullRequestReviewCommentInput!){\
updatePullRequestReviewComment(input:$in){pullRequestReviewComment{id body}}}";

/// Deletes a pending comment (the pending review disappears with its last comment).
pub const DELETE_COMMENT: &str = "mutation($in:DeletePullRequestReviewCommentInput!){\
deletePullRequestReviewComment(input:$in){pullRequestReviewComment{id}}}";

/// Every GraphQL document of the pending-review flow.
pub const DOCUMENTS: [&str; 6] = [
    PULL_REQUEST_HEAD,
    PENDING_REVIEW,
    THREAD_SIDES,
    ADD_THREAD,
    UPDATE_COMMENT,
    DELETE_COMMENT,
];

/// Thread-side pages read to place pending comments; at most this many after the first.
const MAX_THREAD_PAGES: usize = 9;

/// A pending-review call failed.
#[derive(Debug, thiserror::Error)]
pub enum PendingReviewError {
    /// The GraphQL call failed.
    #[error("the GitHub GraphQL call failed")]
    Graphql(#[from] GraphqlError),
    /// The pull request is not visible to the user.
    #[error("GitHub did not return the pull request")]
    MissingPullRequest,
    /// GitHub answered `thread: null`: it silently rejected the position.
    #[error("GitHub rejected the comment position")]
    ThreadRejected,
}

/// The pull request as the write path needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequestHead {
    /// GraphQL node id.
    pub id: String,
    /// Current head commit.
    pub head_oid: String,
}

/// The user's pending comments on a pull request, merged over all their pending reviews.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingComments {
    /// The pull request.
    pub pull_request: PullRequestHead,
    /// The comments in review and comment order.
    pub comments: Vec<PendingComment>,
}

/// Where a new thread goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadTarget {
    /// A line thread inside the diff.
    Line {
        /// Path of the file in the pull request.
        path: String,
        /// Diff side of all lines.
        side: LineSide,
        /// First line of a multi-line thread.
        start_line: Option<u32>,
        /// Last line.
        line: u32,
    },
    /// A file-level thread; the body links to the lines.
    File {
        /// Path of the file in the pull request.
        path: String,
    },
}

fn diff_side(side: LineSide) -> &'static str {
    match side {
        LineSide::Head => "RIGHT",
        LineSide::Base => "LEFT",
    }
}

/// The lines a file-level comment links to, as `(start_line, line)`: the last `blob/<sha>/<path>#L<a>[-L<b>]`
/// permalink into `pr`'s repository in `body` (the server appends it on creation).
fn linked_lines(body: &str, pr: &PrRef) -> (Option<u32>, Option<u32>) {
    let parsed = body
        .split_whitespace()
        .rev()
        .filter_map(|word| Url::parse(word).ok())
        .find_map(|url| {
            let segments: Vec<&str> = url.path_segments()?.collect();
            let [owner, name, "blob", _sha, _file, ..] = segments.as_slice() else {
                return None;
            };
            let same_repo = url.host_str() == Some("github.com")
                && owner.eq_ignore_ascii_case(&pr.repo.owner)
                && name.eq_ignore_ascii_case(&pr.repo.name);
            if !same_repo {
                return None;
            }
            let fragment = url.fragment()?;
            let (start, end) = match fragment.split_once('-') {
                Some((start, end)) => (start, Some(end)),
                None => (fragment, None),
            };
            let number = |part: &str| part.strip_prefix('L')?.parse::<u32>().ok();
            let start = number(start)?;
            match end {
                Some(end) => Some((Some(start), Some(number(end)?))),
                None => Some((None, Some(start))),
            }
        });
    parsed.unwrap_or((None, None))
}

fn variables(pr: &PrRef) -> serde_json::Value {
    json!({"o": pr.repo.owner, "n": pr.repo.name, "pr": pr.number})
}

#[derive(Deserialize)]
struct RepositoryData<P> {
    repository: Option<Repository<P>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository<P> {
    pull_request: Option<P>,
}

impl<P> RepositoryData<P> {
    fn pull_request(self) -> Result<P, PendingReviewError> {
        self.repository
            .and_then(|repository| repository.pull_request)
            .ok_or(PendingReviewError::MissingPullRequest)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadNode {
    id: String,
    head_ref_oid: String,
}

/// The pull request's node id and current head.
pub async fn pull_request_head(
    config: &GithubConfig,
    token: &SecretString,
    pr: &PrRef,
) -> Result<PullRequestHead, PendingReviewError> {
    let data: RepositoryData<HeadNode> =
        graphql::execute(config, token, PULL_REQUEST_HEAD, variables(pr)).await?;
    let node = data.pull_request()?;
    Ok(PullRequestHead {
        id: node.id,
        head_oid: node.head_ref_oid,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingNode {
    id: String,
    head_ref_oid: String,
    reviews: Nodes<ReviewNode>,
    review_threads: ThreadPage,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewNode {
    viewer_did_author: bool,
    comments: CommentConnection,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentConnection {
    total_count: usize,
    nodes: Vec<CommentNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentNode {
    id: String,
    path: String,
    line: Option<u32>,
    start_line: Option<u32>,
    subject_type: Option<String>,
    body: String,
    reply_to: Option<IdNode>,
}

#[derive(Deserialize)]
struct IdNode {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadPage {
    page_info: PageInfo,
    nodes: Vec<ThreadSide>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadSide {
    id: String,
    diff_side: Option<String>,
    comments: Nodes<IdNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadsNode {
    review_threads: ThreadPage,
}

/// Thread id and diff side by the id of the thread's first comment.
#[derive(Default)]
struct ThreadIndex(Vec<(String, String, Option<LineSide>)>);

impl ThreadIndex {
    fn add(&mut self, page: Vec<ThreadSide>) {
        for thread in page {
            if let Some(first) = thread.comments.nodes.into_iter().next() {
                self.0
                    .push((first.id, thread.id, line_side(thread.diff_side.as_deref())));
            }
        }
    }

    fn get(&self, root_comment: &str) -> Option<(&str, Option<LineSide>)> {
        self.0
            .iter()
            .find(|(first, _, _)| first == root_comment)
            .map(|(_, thread, side)| (thread.as_str(), *side))
    }
}

fn pending_comment(pr: &PrRef, comment: CommentNode, threads: &ThreadIndex) -> PendingComment {
    let root = comment
        .reply_to
        .as_ref()
        .map_or(comment.id.as_str(), |reply_to| reply_to.id.as_str());
    let (thread_id, side) = threads.get(root).unwrap_or(("", None));
    let thread_id = thread_id.to_owned();
    if comment.subject_type.as_deref() == Some("FILE") {
        let (start_line, line) = linked_lines(&comment.body, pr);
        return PendingComment {
            id: comment.id,
            thread_id,
            path: comment.path,
            kind: CommentKind::File,
            side: None,
            start_line,
            line,
            body: comment.body,
        };
    }
    PendingComment {
        id: comment.id,
        thread_id,
        path: comment.path,
        kind: CommentKind::Line,
        side,
        start_line: comment.start_line,
        line: comment.line,
        body: comment.body,
    }
}

/// The user's pending comments on `pr`, merged over every pending review the user has there (concurrent first
/// comments can create several, S3 run 80), with the pull request's node id and head.
pub async fn pending_comments(
    config: &GithubConfig,
    token: &SecretString,
    pr: &PrRef,
) -> Result<PendingComments, PendingReviewError> {
    let data: RepositoryData<PendingNode> =
        graphql::execute(config, token, PENDING_REVIEW, variables(pr)).await?;
    let node = data.pull_request()?;
    let mut comments: Vec<CommentNode> = Vec::new();
    for review in node
        .reviews
        .nodes
        .into_iter()
        .filter(|r| r.viewer_did_author)
    {
        if review.comments.total_count > review.comments.nodes.len() {
            tracing::warn!(
                total = review.comments.total_count,
                read = review.comments.nodes.len(),
                "a pending review has more comments than are read; the list shows the first ones"
            );
        }
        comments.extend(review.comments.nodes);
    }
    let mut threads = ThreadIndex::default();
    let mut page_info = node.review_threads.page_info;
    threads.add(node.review_threads.nodes);
    for _ in 0..MAX_THREAD_PAGES {
        let unplaced = comments.iter().any(|comment| {
            let root = comment.reply_to.as_ref().map_or(&comment.id, |r| &r.id);
            threads.get(root).is_none()
        });
        let (true, true, Some(after)) = (unplaced, page_info.has_next_page, page_info.end_cursor)
        else {
            break;
        };
        let mut vars = variables(pr);
        vars["after"] = json!(after);
        let data: RepositoryData<ThreadsNode> =
            graphql::execute(config, token, THREAD_SIDES, vars).await?;
        let page = data.pull_request()?.review_threads;
        page_info = page.page_info;
        threads.add(page.nodes);
    }
    Ok(PendingComments {
        pull_request: PullRequestHead {
            id: node.id,
            head_oid: node.head_ref_oid,
        },
        comments: comments
            .into_iter()
            .map(|comment| pending_comment(pr, comment, &threads))
            .collect(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddThreadData {
    add_pull_request_review_thread: Option<AddThreadPayload>,
}

#[derive(Deserialize)]
struct AddThreadPayload {
    thread: Option<AddedThread>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddedThread {
    id: String,
    path: String,
    line: Option<u32>,
    start_line: Option<u32>,
    diff_side: Option<String>,
    subject_type: Option<String>,
    comments: Nodes<AddedComment>,
}

#[derive(Deserialize)]
struct AddedComment {
    id: String,
    body: String,
}

/// Adds `body` as a new thread at `target` to the user's pending review on the pull request with node id
/// `pull_request_id`; `thread: null` (GitHub's silent rejection) is [`PendingReviewError::ThreadRejected`].
pub async fn add_thread(
    config: &GithubConfig,
    token: &SecretString,
    pr: &PrRef,
    pull_request_id: &str,
    target: &ThreadTarget,
    body: &str,
) -> Result<PendingComment, PendingReviewError> {
    let input = match target {
        ThreadTarget::Line {
            path,
            side,
            start_line,
            line,
        } => {
            let mut input = json!({"pullRequestId": pull_request_id, "path": path, "line": line,
                "side": diff_side(*side), "body": body});
            if let Some(start) = start_line.filter(|start| start < line) {
                input["startLine"] = json!(start);
                input["startSide"] = json!(diff_side(*side));
            }
            input
        }
        ThreadTarget::File { path } => {
            json!({"pullRequestId": pull_request_id, "path": path, "subjectType": "FILE", "body": body})
        }
    };
    let data: AddThreadData =
        graphql::execute(config, token, ADD_THREAD, json!({"in": input})).await?;
    let thread = data
        .add_pull_request_review_thread
        .and_then(|payload| payload.thread)
        .ok_or(PendingReviewError::ThreadRejected)?;
    let comment = thread
        .comments
        .nodes
        .into_iter()
        .next()
        .ok_or(PendingReviewError::ThreadRejected)?;
    let mut threads = ThreadIndex::default();
    threads.0.push((
        comment.id.clone(),
        thread.id,
        line_side(thread.diff_side.as_deref()),
    ));
    let comment = CommentNode {
        id: comment.id,
        path: thread.path,
        line: thread.line,
        start_line: thread
            .start_line
            .filter(|start| Some(*start) != thread.line),
        subject_type: thread.subject_type,
        body: comment.body,
        reply_to: None,
    };
    Ok(pending_comment(pr, comment, &threads))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateData {
    update_pull_request_review_comment: Option<UpdatePayload>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdatePayload {
    pull_request_review_comment: Option<AddedComment>,
}

/// Replaces the text of comment `comment_id`; returns the stored text.
pub async fn update_comment(
    config: &GithubConfig,
    token: &SecretString,
    comment_id: &str,
    body: &str,
) -> Result<String, PendingReviewError> {
    let data: UpdateData = graphql::execute(
        config,
        token,
        UPDATE_COMMENT,
        json!({"in": {"pullRequestReviewCommentId": comment_id, "body": body}}),
    )
    .await?;
    data.update_pull_request_review_comment
        .and_then(|payload| payload.pull_request_review_comment)
        .map(|comment| comment.body)
        .ok_or(PendingReviewError::Graphql(GraphqlError::NoData))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteData {
    delete_pull_request_review_comment: Option<serde_json::Value>,
}

/// Deletes comment `comment_id`.
pub async fn delete_comment(
    config: &GithubConfig,
    token: &SecretString,
    comment_id: &str,
) -> Result<(), PendingReviewError> {
    let data: DeleteData = graphql::execute(
        config,
        token,
        DELETE_COMMENT,
        json!({"in": {"id": comment_id}}),
    )
    .await?;
    data.delete_pull_request_review_comment
        .map(|_| ())
        .ok_or(PendingReviewError::Graphql(GraphqlError::NoData))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use contracts::common::RepoRef;
    use serde_json::Value;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::github::graphql::GraphqlErrorType;
    use crate::github::graphql::tests::github_config;
    use crate::github::threads::REVIEW_THREADS;

    const HEAD: &str = "52d2da140191a9d47becf6a446310be803c007fc";

    fn pr() -> PrRef {
        PrRef {
            repo: RepoRef {
                owner: "acme".to_owned(),
                name: "node-client".to_owned(),
            },
            number: 482,
        }
    }

    fn token() -> SecretString {
        SecretString::from("ghu_review")
    }

    fn recorded(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/pr-482/github/graphql")
            .join(name);
        let recording: Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("JSON");
        recording["response"].clone()
    }

    async fn answer(server: &MockServer, request: Value, response: Value) {
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .and(body_partial_json(request))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(server)
            .await;
    }

    #[test]
    fn no_document_submits_a_review() {
        let submit = concat!("submit", "PullRequestReview");
        let add_review = concat!("addPullRequest", "Review(");
        for document in DOCUMENTS.iter().chain([&REVIEW_THREADS]) {
            assert!(!document.contains(submit), "{document}");
            assert!(!document.contains(add_review), "{document}");
            assert!(!document.contains("event"), "{document}");
        }
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut pending = vec![dir];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).expect("src dir") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let source = std::fs::read_to_string(&path).expect("source");
                    assert!(!source.contains(submit), "{}", path.display());
                    assert!(
                        !source.contains(concat!("/rev", "iews\"")),
                        "REST reviews endpoint in {}",
                        path.display()
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn line_thread_is_added_and_mapped() {
        let server = MockServer::start().await;
        answer(
            &server,
            json!({"variables": {"in": {"pullRequestId": "PR_fixture482", "path": "src/rpc/client.rs",
                "line": 96, "side": "RIGHT", "body": "b"}}}),
            recorded("add-thread-line.json"),
        )
        .await;
        let target = ThreadTarget::Line {
            path: "src/rpc/client.rs".to_owned(),
            side: LineSide::Head,
            start_line: None,
            line: 96,
        };
        let comment = add_thread(
            &github_config(&server),
            &token(),
            &pr(),
            "PR_fixture482",
            &target,
            "b",
        )
        .await
        .expect("comment");
        assert_eq!(comment.id, "PRRC_fixture482_c1");
        assert_eq!(comment.thread_id, "PRRT_fixture482_c1");
        assert_eq!(comment.kind, CommentKind::Line);
        assert_eq!(
            (comment.side, comment.start_line, comment.line),
            (Some(LineSide::Head), None, Some(96))
        );
    }

    #[tokio::test]
    async fn multi_line_thread_sends_start_line_and_side() {
        let server = MockServer::start().await;
        answer(
            &server,
            json!({"variables": {"in": {"startLine": 10, "startSide": "LEFT", "line": 12, "side": "LEFT"}}}),
            recorded("add-thread-left-removed-line.json"),
        )
        .await;
        let target = ThreadTarget::Line {
            path: "src/rpc/client.rs".to_owned(),
            side: LineSide::Base,
            start_line: Some(10),
            line: 12,
        };
        add_thread(
            &github_config(&server),
            &token(),
            &pr(),
            "PR_fixture482",
            &target,
            "b",
        )
        .await
        .expect("comment");
    }

    #[tokio::test]
    async fn file_thread_reports_the_linked_lines() {
        let server = MockServer::start().await;
        answer(
            &server,
            json!({"variables": {"in": {"subjectType": "FILE", "path": "src/rpc/client.rs"}}}),
            recorded("add-thread-file.json"),
        )
        .await;
        let target = ThreadTarget::File {
            path: "src/rpc/client.rs".to_owned(),
        };
        let comment = add_thread(
            &github_config(&server),
            &token(),
            &pr(),
            "PR_fixture482",
            &target,
            "b",
        )
        .await
        .expect("comment");
        assert_eq!(comment.kind, CommentKind::File);
        assert_eq!(
            (comment.side, comment.start_line, comment.line),
            (None, Some(60), Some(62))
        );
    }

    #[tokio::test]
    async fn thread_null_is_an_error() {
        let server = MockServer::start().await;
        answer(
            &server,
            json!({}),
            recorded("add-thread-null-line-outside-diff.json"),
        )
        .await;
        let target = ThreadTarget::Line {
            path: "src/rpc/client.rs".to_owned(),
            side: LineSide::Head,
            start_line: None,
            line: 60,
        };
        let result = add_thread(
            &github_config(&server),
            &token(),
            &pr(),
            "PR_fixture482",
            &target,
            "b",
        )
        .await;
        assert!(
            matches!(result, Err(PendingReviewError::ThreadRejected)),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn pending_comments_merge_reviews_and_place_sides() {
        let server = MockServer::start().await;
        let comment = |id: &str, extra: Value| {
            let mut node = json!({"id": id, "path": "src/a.rs", "line": 5, "startLine": null,
                "subjectType": "LINE", "body": "x", "replyTo": null});
            if let (Value::Object(node), Value::Object(extra)) = (&mut node, extra) {
                node.extend(extra);
            }
            node
        };
        let link = format!("see https://github.com/acme/node-client/blob/{HEAD}/src/a.rs#L3-L4");
        let response = json!({"data": {"repository": {"pullRequest": {
            "id": "PR_1", "headRefOid": HEAD,
            "reviews": {"nodes": [
                {"id": "R1", "viewerDidAuthor": true, "comments": {"totalCount": 2, "nodes": [
                    comment("C1", json!({"startLine": 3, "line": 4})),
                    comment("C2", json!({"subjectType": "FILE", "line": null, "body": link})),
                ]}},
                {"id": "R2", "viewerDidAuthor": true, "comments": {"totalCount": 1, "nodes": [
                    comment("C3", json!({"replyTo": {"id": "X9"}})),
                ]}},
                {"id": "R3", "viewerDidAuthor": false, "comments": {"totalCount": 1, "nodes": [
                    comment("C4", json!({})),
                ]}}
            ]},
            "reviewThreads": {"pageInfo": {"hasNextPage": true, "endCursor": "p1"}, "nodes": [
                {"id": "T1", "diffSide": "LEFT", "comments": {"nodes": [{"id": "C1"}]}},
                {"id": "T2", "diffSide": "RIGHT", "comments": {"nodes": [{"id": "C2"}]}}
            ]}
        }}}});
        answer(&server, json!({"query": PENDING_REVIEW}), response).await;
        let second = json!({"data": {"repository": {"pullRequest": {"reviewThreads": {
            "pageInfo": {"hasNextPage": false, "endCursor": null},
            "nodes": [{"id": "T9", "diffSide": "RIGHT", "comments": {"nodes": [{"id": "X9"}]}}]
        }}}}});
        answer(
            &server,
            json!({"query": THREAD_SIDES, "variables": {"after": "p1"}}),
            second,
        )
        .await;
        let pending = pending_comments(&github_config(&server), &token(), &pr())
            .await
            .expect("pending");
        assert_eq!(
            pending.pull_request,
            PullRequestHead {
                id: "PR_1".to_owned(),
                head_oid: HEAD.to_owned()
            }
        );
        let summary: Vec<_> = pending
            .comments
            .iter()
            .map(|c| {
                (
                    c.id.as_str(),
                    c.thread_id.as_str(),
                    c.kind,
                    c.side,
                    c.start_line,
                    c.line,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "C1",
                    "T1",
                    CommentKind::Line,
                    Some(LineSide::Base),
                    Some(3),
                    Some(4)
                ),
                ("C2", "T2", CommentKind::File, None, Some(3), Some(4)),
                (
                    "C3",
                    "T9",
                    CommentKind::Line,
                    Some(LineSide::Head),
                    None,
                    Some(5)
                ),
            ],
            "other users' reviews are skipped; replies placed by their root comment"
        );
    }

    #[tokio::test]
    async fn recorded_pending_list_is_read() {
        let server = MockServer::start().await;
        let mut response = recorded("pending-list-one.json");
        response["data"]["repository"]["pullRequest"]["reviewThreads"]["pageInfo"] =
            json!({"hasNextPage": false, "endCursor": null});
        for review in response["data"]["repository"]["pullRequest"]["reviews"]["nodes"]
            .as_array_mut()
            .expect("reviews")
        {
            for comment in review["comments"]["nodes"]
                .as_array_mut()
                .expect("comments")
            {
                comment["replyTo"] = Value::Null;
            }
        }
        answer(&server, json!({}), response).await;
        let pending = pending_comments(&github_config(&server), &token(), &pr())
            .await
            .expect("pending");
        let seed: Value = serde_json::from_str(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../fixtures/pr-482/api-states/missed.json"),
            )
            .expect("state"),
        )
        .expect("JSON");
        let expected: Vec<PendingComment> =
            serde_json::from_value(seed["pending-review"]["comments"].clone()).expect("comments");
        assert_eq!(pending.comments, expected, "the `missed` fixture state");
        assert_eq!(pending.pull_request.head_oid, HEAD);
    }

    #[tokio::test]
    async fn update_and_delete_send_the_comment_id() {
        let server = MockServer::start().await;
        answer(
            &server,
            json!({"variables": {"in": {"pullRequestReviewCommentId": "PRRC_fixture482_c1", "body": "new"}}}),
            json!({"data": {"updatePullRequestReviewComment": {"pullRequestReviewComment":
                {"id": "PRRC_fixture482_c1", "body": "new"}}}}),
        )
        .await;
        answer(
            &server,
            json!({"variables": {"in": {"id": "PRRC_fixture482_c1"}}}),
            recorded("delete-comment.json"),
        )
        .await;
        let config = github_config(&server);
        let body = update_comment(&config, &token(), "PRRC_fixture482_c1", "new")
            .await
            .expect("update");
        assert_eq!(body, "new");
        delete_comment(&config, &token(), "PRRC_fixture482_c1")
            .await
            .expect("delete");
    }

    #[tokio::test]
    async fn deleting_a_deleted_comment_is_not_found() {
        let server = MockServer::start().await;
        answer(
            &server,
            json!({}),
            recorded("delete-comment-not-found.json"),
        )
        .await;
        let result = delete_comment(&github_config(&server), &token(), "PRRC_fixture482_c1").await;
        assert!(
            matches!(
                result,
                Err(PendingReviewError::Graphql(GraphqlError::Rejected(
                    GraphqlErrorType::NotFound
                )))
            ),
            "{result:?}"
        );
    }

    #[test]
    fn linked_lines_come_from_the_last_permalink_into_this_repository() {
        let pr = pr();
        let url = |repo: &str, fragment: &str| {
            format!("https://github.com/{repo}/blob/{HEAD}/src/a.rs#{fragment}")
        };
        let own = |fragment: &str| url("acme/node-client", fragment);
        assert_eq!(linked_lines(&own("L7"), &pr), (None, Some(7)));
        assert_eq!(linked_lines(&own("L7-L9"), &pr), (Some(7), Some(9)));
        assert_eq!(
            linked_lines(&format!("{} then {}", own("L1"), own("L5-L6")), &pr),
            (Some(5), Some(6))
        );
        assert_eq!(
            linked_lines(&format!("{} {}", own("L2"), url("other/repo", "L9")), &pr),
            (None, Some(2)),
            "links into other repositories are ignored"
        );
        assert_eq!(linked_lines(&own("x"), &pr), (None, None));
        assert_eq!(linked_lines("no link", &pr), (None, None));
    }
}
