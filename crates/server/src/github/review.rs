//! The user's pending review through GraphQL with the user's token (ADR-006, spike S3): a thread is added with
//! `addPullRequestReviewThread` (which creates the pending review or reuses the existing one), a comment is
//! edited with `updatePullRequestReviewComment` and deleted with `deletePullRequestReviewComment`. Nothing here
//! submits a review; [`DOCUMENTS`] lists every document this module sends.

use contracts::api::comments::{CommentKind, PendingComment};
use contracts::common::{LineSide, PrRef};
use secrecy::SecretString;
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

use crate::config::GithubConfig;
use crate::github::graphql::{self, GraphqlError, GraphqlErrorType};
use crate::github::threads::line_side;

/// The pull request's node id and current head.
pub const PULL_REQUEST_HEAD: &str = "query($o:String!,$n:String!,$pr:Int!){repository(owner:$o,name:$n){\
pullRequest(number:$pr){id headRefOid}}}";

/// The user's pending reviews with their first comments, and the first page of thread sides.
pub const PENDING_REVIEW: &str = "query($o:String!,$n:String!,$pr:Int!){repository(owner:$o,name:$n){\
pullRequest(number:$pr){id headRefOid \
reviews(states:[PENDING],first:10){pageInfo{hasNextPage endCursor} nodes{id viewerDidAuthor \
comments(first:100){pageInfo{hasNextPage endCursor} nodes{id path line startLine subjectType body replyTo{id}}}}} \
reviewThreads(first:100){pageInfo{hasNextPage endCursor} nodes{id diffSide comments(first:1){nodes{id}}}}}}}";

/// Further pages of the user's pending reviews.
pub const PENDING_REVIEWS_PAGE: &str = "query($o:String!,$n:String!,$pr:Int!,$after:String!){\
repository(owner:$o,name:$n){pullRequest(number:$pr){\
reviews(states:[PENDING],first:10,after:$after){pageInfo{hasNextPage endCursor} nodes{id viewerDidAuthor \
comments(first:100){pageInfo{hasNextPage endCursor} nodes{id path line startLine subjectType body replyTo{id}}}}}}}}";

/// Further pages of one pending review's comments.
pub const REVIEW_COMMENTS_PAGE: &str = "query($id:ID!,$after:String!){node(id:$id){\
... on PullRequestReview{comments(first:100,after:$after){pageInfo{hasNextPage endCursor} \
nodes{id path line startLine subjectType body replyTo{id}}}}}}";

/// One review comment by id, with its state and where it belongs.
pub const COMMENT_BY_ID: &str = "query($id:ID!){node(id:$id){__typename \
... on PullRequestReviewComment{id path line startLine subjectType body state replyTo{id} \
pullRequestReview{state viewerDidAuthor} pullRequest{number repository{name owner{login}}}}}}";

/// Further pages of thread sides.
pub const THREAD_SIDES: &str = "query($o:String!,$n:String!,$pr:Int!,$after:String!){repository(owner:$o,name:$n){\
pullRequest(number:$pr){reviewThreads(first:100,after:$after){pageInfo{hasNextPage endCursor} \
nodes{id diffSide comments(first:1){nodes{id}}}}}}}";

/// Adds a thread with its first comment to the user's pending review (created when missing); the comment's
/// commits show which head GitHub placed it on.
pub const ADD_THREAD: &str = "mutation($in:AddPullRequestReviewThreadInput!){addPullRequestReviewThread(input:$in){\
thread{id path line startLine diffSide subjectType \
comments(first:1){nodes{id body commit{oid} originalCommit{oid}}}}}}";

/// Replaces the text of a comment; answers whether it was still pending.
pub const UPDATE_COMMENT: &str = "mutation($in:UpdatePullRequestReviewCommentInput!){\
updatePullRequestReviewComment(input:$in){pullRequestReviewComment{id body state pullRequestReview{state}}}}";

/// Deletes a comment (a pending review disappears with its last comment); answers the review's state.
pub const DELETE_COMMENT: &str = "mutation($in:DeletePullRequestReviewCommentInput!){\
deletePullRequestReviewComment(input:$in){pullRequestReview{state} pullRequestReviewComment{id}}}";

/// Every GraphQL document of the pending-review flow.
pub const DOCUMENTS: [&str; 9] = [
    PULL_REQUEST_HEAD,
    PENDING_REVIEW,
    PENDING_REVIEWS_PAGE,
    REVIEW_COMMENTS_PAGE,
    COMMENT_BY_ID,
    THREAD_SIDES,
    ADD_THREAD,
    UPDATE_COMMENT,
    DELETE_COMMENT,
];

/// Read caps: pages of 10 pending reviews, of 100 comments per review, of 100 threads (for sides).
pub const MAX_REVIEW_PAGES: usize = 5;
/// See [`MAX_REVIEW_PAGES`]: up to 1000 comments per pending review.
pub const MAX_COMMENT_PAGES: usize = 10;
/// See [`MAX_REVIEW_PAGES`]: up to 1000 threads are searched for the sides of pending comments.
pub const MAX_THREAD_PAGES: usize = 10;

/// A pending-review call failed.
#[derive(Debug, thiserror::Error)]
pub enum PendingReviewError {
    /// The GraphQL call failed.
    #[error("the GitHub GraphQL call failed")]
    Graphql(#[from] GraphqlError),
    /// The pull request is not visible to the user.
    #[error("GitHub did not return the pull request")]
    MissingPullRequest,
    /// GitHub answered `thread: null` without errors: it silently rejected the position.
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
    /// A read cap was hit: more pending reviews or comments exist than listed.
    pub incomplete: bool,
}

/// A comment just added, with the commits GitHub placed it on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddedComment {
    /// The comment.
    pub comment: PendingComment,
    /// `commit` and `originalCommit` of the comment, as far as GitHub reported them.
    pub commits: Vec<String>,
}

/// Whether an edited or deleted comment still belonged to a pending review when GitHub applied the change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewState {
    /// The review was still pending.
    Pending,
    /// The review had been published meanwhile; the change applied to the published comment.
    Published,
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

fn variables(pr: &PrRef) -> Value {
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
    reviews: Page<ReviewNode>,
    review_threads: Page<ThreadSide>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewsNode {
    reviews: Page<ReviewNode>,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    page_info: PageInfo,
    nodes: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

impl PageInfo {
    /// The cursor of the next page, if there is one.
    fn next(self) -> Option<String> {
        self.end_cursor.filter(|_| self.has_next_page)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewNode {
    id: String,
    viewer_did_author: bool,
    comments: Page<CommentNode>,
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

impl CommentNode {
    /// The id of the thread's first comment.
    fn root(&self) -> &str {
        self.reply_to
            .as_ref()
            .map_or(&self.id, |reply_to| &reply_to.id)
    }
}

#[derive(Deserialize)]
struct IdNode {
    id: String,
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
    review_threads: Page<ThreadSide>,
}

#[derive(Deserialize)]
struct NodeData<N> {
    node: Option<N>,
}

#[derive(Deserialize)]
struct ReviewComments {
    comments: Option<Page<CommentNode>>,
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
    let (thread_id, side) = threads.get(comment.root()).unwrap_or(("", None));
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

/// Every comment of the user's pending reviews in `reviews`, following comment pages up to
/// [`MAX_COMMENT_PAGES`]; returns whether a cap was hit.
async fn review_comments(
    config: &GithubConfig,
    token: &SecretString,
    reviews: Vec<ReviewNode>,
    comments: &mut Vec<CommentNode>,
) -> Result<bool, PendingReviewError> {
    let mut capped = false;
    for review in reviews
        .into_iter()
        .filter(|review| review.viewer_did_author)
    {
        comments.extend(review.comments.nodes);
        let mut next = review.comments.page_info.next();
        let mut pages = 1;
        while let Some(after) = next {
            if pages == MAX_COMMENT_PAGES {
                capped = true;
                break;
            }
            let data: NodeData<ReviewComments> = graphql::execute(
                config,
                token,
                REVIEW_COMMENTS_PAGE,
                json!({"id": review.id, "after": after}),
            )
            .await?;
            let page = data
                .node
                .and_then(|node| node.comments)
                .ok_or(GraphqlError::NoData)?;
            comments.extend(page.nodes);
            next = page.page_info.next();
            pages += 1;
        }
    }
    Ok(capped)
}

/// The user's pending comments on `pr`, merged over every pending review the user has there (concurrent first
/// comments can create several, S3 run 80), with the pull request's node id and head. Reads at most
/// [`MAX_REVIEW_PAGES`] × 10 reviews with [`MAX_COMMENT_PAGES`] × 100 comments each; beyond that the result
/// is marked incomplete.
pub async fn pending_comments(
    config: &GithubConfig,
    token: &SecretString,
    pr: &PrRef,
) -> Result<PendingComments, PendingReviewError> {
    let data: RepositoryData<PendingNode> =
        graphql::execute(config, token, PENDING_REVIEW, variables(pr)).await?;
    let node = data.pull_request()?;
    let mut comments: Vec<CommentNode> = Vec::new();
    let mut incomplete = review_comments(config, token, node.reviews.nodes, &mut comments).await?;
    let mut next = node.reviews.page_info.next();
    let mut pages = 1;
    while let Some(after) = next {
        if pages == MAX_REVIEW_PAGES {
            incomplete = true;
            break;
        }
        let mut vars = variables(pr);
        vars["after"] = json!(after);
        let data: RepositoryData<ReviewsNode> =
            graphql::execute(config, token, PENDING_REVIEWS_PAGE, vars).await?;
        let reviews = data.pull_request()?.reviews;
        incomplete |= review_comments(config, token, reviews.nodes, &mut comments).await?;
        next = reviews.page_info.next();
        pages += 1;
    }
    if incomplete {
        tracing::warn!(
            read = comments.len(),
            "the user has more pending comments than are read; the list is marked incomplete"
        );
    }
    let mut threads = ThreadIndex::default();
    threads.add(node.review_threads.nodes);
    let mut next = node.review_threads.page_info.next();
    let mut pages = 1;
    while let Some(after) = next {
        let unplaced = comments
            .iter()
            .any(|comment| threads.get(comment.root()).is_none());
        if !unplaced || pages == MAX_THREAD_PAGES {
            break;
        }
        let mut vars = variables(pr);
        vars["after"] = json!(after);
        let data: RepositoryData<ThreadsNode> =
            graphql::execute(config, token, THREAD_SIDES, vars).await?;
        let page = data.pull_request()?.review_threads;
        threads.add(page.nodes);
        next = page.page_info.next();
        pages += 1;
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
        incomplete,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentById {
    #[serde(flatten)]
    comment: CommentNode,
    state: String,
    pull_request_review: Option<ReviewOfComment>,
    pull_request: PullOfComment,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewOfComment {
    viewer_did_author: bool,
}

#[derive(Deserialize)]
struct PullOfComment {
    number: u32,
    repository: RepositoryName,
}

#[derive(Deserialize)]
struct RepositoryName {
    name: String,
    owner: IdLogin,
}

#[derive(Deserialize)]
struct IdLogin {
    login: String,
}

/// Comment `comment_id` when it is a pending comment of the user's on `pr` (used when the list hit its cap);
/// its thread and side are not looked up.
pub async fn pending_comment_by_id(
    config: &GithubConfig,
    token: &SecretString,
    pr: &PrRef,
    comment_id: &str,
) -> Result<Option<PendingComment>, PendingReviewError> {
    let data: NodeData<Value> =
        match graphql::execute(config, token, COMMENT_BY_ID, json!({"id": comment_id})).await {
            Ok(data) => data,
            Err(GraphqlError::Rejected(GraphqlErrorType::NotFound)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
    let Some(node) = data
        .node
        .filter(|node| node["__typename"] == "PullRequestReviewComment")
    else {
        return Ok(None);
    };
    let found: CommentById = serde_json::from_value(node).map_err(GraphqlError::Malformed)?;
    let own_pending = found.state == "PENDING"
        && found
            .pull_request_review
            .is_some_and(|review| review.viewer_did_author)
        && found.pull_request.number == pr.number
        && found
            .pull_request
            .repository
            .owner
            .login
            .eq_ignore_ascii_case(&pr.repo.owner)
        && found
            .pull_request
            .repository
            .name
            .eq_ignore_ascii_case(&pr.repo.name);
    Ok(own_pending.then(|| pending_comment(pr, found.comment, &ThreadIndex::default())))
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
    comments: Nodes<AddedNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddedNode {
    id: String,
    body: String,
    #[serde(default)]
    commit: Option<OidNode>,
    #[serde(default)]
    original_commit: Option<OidNode>,
}

#[derive(Deserialize)]
struct OidNode {
    oid: String,
}

/// Adds `body` as a new thread at `target` to the user's pending review on the pull request with node id
/// `pull_request_id`; `thread: null` without errors (GitHub's silent rejection) is
/// [`PendingReviewError::ThreadRejected`].
pub async fn add_thread(
    config: &GithubConfig,
    token: &SecretString,
    pr: &PrRef,
    pull_request_id: &str,
    target: &ThreadTarget,
    body: &str,
) -> Result<AddedComment, PendingReviewError> {
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
    // A thread without its comment is an answer GitHub never gives; whether anything was written is unknown.
    let added = thread
        .comments
        .nodes
        .into_iter()
        .next()
        .ok_or(GraphqlError::NoData)?;
    let commits = [added.commit, added.original_commit]
        .into_iter()
        .flatten()
        .map(|commit| commit.oid)
        .collect();
    let mut threads = ThreadIndex::default();
    threads.0.push((
        added.id.clone(),
        thread.id,
        line_side(thread.diff_side.as_deref()),
    ));
    let comment = CommentNode {
        id: added.id,
        path: thread.path,
        line: thread.line,
        start_line: thread
            .start_line
            .filter(|start| Some(*start) != thread.line),
        subject_type: thread.subject_type,
        body: added.body,
        reply_to: None,
    };
    Ok(AddedComment {
        comment: pending_comment(pr, comment, &threads),
        commits,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateData {
    update_pull_request_review_comment: Option<UpdatePayload>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdatePayload {
    pull_request_review_comment: Option<UpdatedComment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdatedComment {
    body: String,
    state: Option<String>,
    pull_request_review: Option<StateNode>,
}

#[derive(Deserialize)]
struct StateNode {
    state: Option<String>,
}

/// Published unless GitHub reports the comment and its review as pending.
fn review_state(comment_state: Option<&str>, review: Option<&StateNode>) -> ReviewState {
    let review_pending = review.is_none_or(|review| review.state.as_deref() == Some("PENDING"));
    if comment_state == Some("PENDING") && review_pending {
        ReviewState::Pending
    } else {
        ReviewState::Published
    }
}

/// Replaces the text of comment `comment_id`; returns the stored text and whether the comment was still
/// pending (GitHub has no conditional mutation: a review published meanwhile receives the edit).
pub async fn update_comment(
    config: &GithubConfig,
    token: &SecretString,
    comment_id: &str,
    body: &str,
) -> Result<(String, ReviewState), PendingReviewError> {
    let data: UpdateData = graphql::execute(
        config,
        token,
        UPDATE_COMMENT,
        json!({"in": {"pullRequestReviewCommentId": comment_id, "body": body}}),
    )
    .await?;
    let comment = data
        .update_pull_request_review_comment
        .and_then(|payload| payload.pull_request_review_comment)
        .ok_or(PendingReviewError::Graphql(GraphqlError::NoData))?;
    let state = review_state(
        comment.state.as_deref(),
        comment.pull_request_review.as_ref(),
    );
    Ok((comment.body, state))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteData {
    delete_pull_request_review_comment: Option<DeletePayload>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeletePayload {
    pull_request_review: Option<StateNode>,
}

/// Deletes comment `comment_id`; returns whether its review was still pending (a pending review that lost its
/// last comment is gone, which counts as pending).
pub async fn delete_comment(
    config: &GithubConfig,
    token: &SecretString,
    comment_id: &str,
) -> Result<ReviewState, PendingReviewError> {
    let data: DeleteData = graphql::execute(
        config,
        token,
        DELETE_COMMENT,
        json!({"in": {"id": comment_id}}),
    )
    .await?;
    let payload = data
        .delete_pull_request_review_comment
        .ok_or(PendingReviewError::Graphql(GraphqlError::NoData))?;
    Ok(review_state(
        Some("PENDING"),
        payload.pull_request_review.as_ref(),
    ))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use contracts::common::RepoRef;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
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
        assert_eq!(comment.commits, [HEAD], "the comment's commit is reported");
        let comment = comment.comment;
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
        .expect("comment")
        .comment;
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
    async fn thread_null_with_an_execution_error_is_not_a_rejection() {
        let server = MockServer::start().await;
        answer(
            &server,
            json!({}),
            json!({"data": {"addPullRequestReviewThread": {"thread": null}},
                "errors": [{"type": "SOMETHING_NEW", "message": "field failed"}]}),
        )
        .await;
        let target = ThreadTarget::File {
            path: "src/rpc/client.rs".to_owned(),
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
            matches!(
                result,
                Err(PendingReviewError::Graphql(GraphqlError::Rejected(
                    GraphqlErrorType::Other
                )))
            ),
            "the comment may exist: {result:?}"
        );
    }

    fn comment_node(id: &str, extra: Value) -> Value {
        let mut node = json!({"id": id, "path": "src/a.rs", "line": 5, "startLine": null,
            "subjectType": "LINE", "body": "x", "replyTo": null});
        if let (Value::Object(node), Value::Object(extra)) = (&mut node, extra) {
            node.extend(extra);
        }
        node
    }

    fn page(nodes: Vec<Value>, next: Option<&str>) -> Value {
        json!({"pageInfo": {"hasNextPage": next.is_some(), "endCursor": next}, "nodes": nodes})
    }

    fn comments(range: std::ops::Range<u32>) -> Vec<Value> {
        range
            .map(|n| comment_node(&format!("C{n}"), json!({})))
            .collect()
    }

    fn pending_answer(reviews: Value) -> Value {
        json!({"data": {"repository": {"pullRequest": {"id": "PR_1", "headRefOid": HEAD,
            "reviews": reviews, "reviewThreads": page(vec![], None)}}}})
    }

    #[tokio::test]
    async fn pending_comments_follow_comment_and_review_pages() {
        let server = MockServer::start().await;
        let review = |id: &str, comments: Value| json!({"id": id, "viewerDidAuthor": true, "comments": comments});
        answer(
            &server,
            json!({"query": PENDING_REVIEW}),
            pending_answer(page(
                vec![review("R1", page(comments(0..100), Some("c1")))],
                Some("r1"),
            )),
        )
        .await;
        answer(
            &server,
            json!({"query": REVIEW_COMMENTS_PAGE, "variables": {"id": "R1", "after": "c1"}}),
            json!({"data": {"node": {"comments": page(comments(100..101), None)}}}),
        )
        .await;
        answer(
            &server,
            json!({"query": PENDING_REVIEWS_PAGE, "variables": {"after": "r1"}}),
            json!({"data": {"repository": {"pullRequest": {"reviews": page(
                vec![review("R2", page(comments(101..102), None))], None)}}}}),
        )
        .await;
        let pending = pending_comments(&github_config(&server), &token(), &pr())
            .await
            .expect("pending");
        assert_eq!(pending.comments.len(), 102);
        assert_eq!(
            pending.comments[100].id, "C100",
            "the 101st comment is listed"
        );
        assert!(!pending.incomplete);
    }

    #[tokio::test]
    async fn pending_comments_beyond_the_cap_are_marked_incomplete() {
        let server = MockServer::start().await;
        let review = json!({"id": "R1", "viewerDidAuthor": true, "comments": page(comments(0..100), Some("c"))});
        answer(
            &server,
            json!({"query": PENDING_REVIEW}),
            pending_answer(page(vec![review], None)),
        )
        .await;
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .and(body_partial_json(json!({"query": REVIEW_COMMENTS_PAGE})))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"data": {"node": {"comments": page(comments(0..100), Some("c"))}}}),
            ))
            .expect(u64::try_from(MAX_COMMENT_PAGES - 1).expect("pages"))
            .mount(&server)
            .await;
        let pending = pending_comments(&github_config(&server), &token(), &pr())
            .await
            .expect("pending");
        assert_eq!(pending.comments.len(), 100 * MAX_COMMENT_PAGES);
        assert!(pending.incomplete, "a capped list is never complete");
    }

    #[tokio::test]
    async fn one_comment_is_found_by_id_only_when_it_is_the_users_pending_one() {
        let server = MockServer::start().await;
        let node = |state: &str, own: bool, number: u32| {
            let mut node = comment_node(
                "C7",
                json!({"__typename": "PullRequestReviewComment",
                "state": state, "pullRequestReview": {"state": state, "viewerDidAuthor": own},
                "pullRequest": {"number": number, "repository": {"name": "node-client", "owner": {"login": "acme"}}}}),
            );
            node["path"] = json!("src/a.rs");
            json!({"data": {"node": node}})
        };
        let cases = [
            ("C7", node("PENDING", true, 482), true),
            ("C8", node("SUBMITTED", true, 482), false),
            ("C9", node("PENDING", false, 482), false),
            ("C10", node("PENDING", true, 481), false),
            ("C11", json!({"data": {"node": null}}), false),
        ];
        for (id, response, _) in &cases {
            answer(
                &server,
                json!({"query": COMMENT_BY_ID, "variables": {"id": id}}),
                response.clone(),
            )
            .await;
        }
        for (id, _, own_pending) in cases {
            let found = pending_comment_by_id(&github_config(&server), &token(), &pr(), id)
                .await
                .expect("lookup");
            assert_eq!(found.is_some(), own_pending, "{id}");
        }
    }

    #[tokio::test]
    async fn pending_comments_merge_reviews_and_place_sides() {
        let server = MockServer::start().await;
        let comment = comment_node;
        let none = json!({"hasNextPage": false, "endCursor": null});
        let link = format!("see https://github.com/acme/node-client/blob/{HEAD}/src/a.rs#L3-L4");
        let response = json!({"data": {"repository": {"pullRequest": {
            "id": "PR_1", "headRefOid": HEAD,
            "reviews": {"pageInfo": none, "nodes": [
                {"id": "R1", "viewerDidAuthor": true, "comments": {"pageInfo": none, "nodes": [
                    comment("C1", json!({"startLine": 3, "line": 4})),
                    comment("C2", json!({"subjectType": "FILE", "line": null, "body": link})),
                ]}},
                {"id": "R2", "viewerDidAuthor": true, "comments": {"pageInfo": none, "nodes": [
                    comment("C3", json!({"replyTo": {"id": "X9"}})),
                ]}},
                {"id": "R3", "viewerDidAuthor": false, "comments": {"pageInfo": none, "nodes": [
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
        let none = json!({"hasNextPage": false, "endCursor": null});
        response["data"]["repository"]["pullRequest"]["reviewThreads"]["pageInfo"] = none.clone();
        response["data"]["repository"]["pullRequest"]["reviews"]["pageInfo"] = none.clone();
        for review in response["data"]["repository"]["pullRequest"]["reviews"]["nodes"]
            .as_array_mut()
            .expect("reviews")
        {
            review["comments"]["pageInfo"] = none.clone();
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
                {"id": "PRRC_fixture482_c1", "body": "new", "state": "PENDING",
                    "pullRequestReview": {"state": "PENDING"}}}}}),
        )
        .await;
        answer(
            &server,
            json!({"variables": {"in": {"id": "PRRC_fixture482_c1"}}}),
            recorded("delete-comment.json"),
        )
        .await;
        let config = github_config(&server);
        let updated = update_comment(&config, &token(), "PRRC_fixture482_c1", "new")
            .await
            .expect("update");
        assert_eq!(updated, ("new".to_owned(), ReviewState::Pending));
        let deleted = delete_comment(&config, &token(), "PRRC_fixture482_c1")
            .await
            .expect("delete");
        assert_eq!(deleted, ReviewState::Pending);
    }

    #[tokio::test]
    async fn edit_or_delete_of_a_comment_published_meanwhile_reports_it() {
        let server = MockServer::start().await;
        answer(
            &server,
            json!({"query": UPDATE_COMMENT}),
            json!({"data": {"updatePullRequestReviewComment": {"pullRequestReviewComment":
                {"id": "C1", "body": "new", "state": "SUBMITTED", "pullRequestReview": {"state": "COMMENTED"}}}}}),
        )
        .await;
        answer(
            &server,
            json!({"query": DELETE_COMMENT}),
            json!({"data": {"deletePullRequestReviewComment": {
                "pullRequestReview": {"state": "COMMENTED"}, "pullRequestReviewComment": {"id": "C1"}}}}),
        )
        .await;
        let config = github_config(&server);
        let (_, state) = update_comment(&config, &token(), "C1", "new")
            .await
            .expect("update");
        assert_eq!(state, ReviewState::Published);
        let state = delete_comment(&config, &token(), "C1")
            .await
            .expect("delete");
        assert_eq!(state, ReviewState::Published);
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
