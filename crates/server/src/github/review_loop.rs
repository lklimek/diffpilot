//! Review-loop reads (plan §4.1 Q-L1–Q-L4) with the reader's user token, queries only: the pull request's facts,
//! its review threads with every comment, its reviews and its PR-level comments. Pages of 100 (threads: 50
//! comments each), at most [`MAX_PAGES`] per connection; every page costs one GraphQL point (spike S13-2).

use contracts::common::PrRef;
use contracts::time::Timestamp;
use secrecy::SecretString;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::GithubConfig;
use crate::github::graphql::{self, GraphqlError, Page, ReadBudget};

/// Pages read per connection (1000 threads, reviews or comments); more: the read is incomplete.
pub const MAX_PAGES: usize = 10;

/// Q-L1: pull request facts and the repository's id and visibility.
pub const PR_FACTS: &str = "query($o:String!,$n:String!,$pr:Int!){repository(owner:$o,name:$n){databaseId isPrivate \
pullRequest(number:$pr){title body url state isDraft author{login} baseRefName baseRefOid headRefOid \
reviewDecision updatedAt}}}";

/// Q-L2: review threads with their comments (the first is the finding).
pub const REVIEW_THREADS: &str = "query($o:String!,$n:String!,$pr:Int!,$after:String){repository(owner:$o,name:$n){\
pullRequest(number:$pr){reviewThreads(first:100,after:$after){totalCount pageInfo{hasNextPage endCursor} \
nodes{id isResolved isOutdated path line startLine diffSide resolvedBy{login} comments(first:50){totalCount \
nodes{databaseId state author{login __typename ... on User{databaseId} ... on Bot{databaseId}} body createdAt lastEditedAt url commit{oid} originalCommit{oid} pullRequestReview{id}}}}}}}}";

/// Q-L3: reviews.
pub const REVIEWS: &str = "query($o:String!,$n:String!,$pr:Int!,$after:String){repository(owner:$o,name:$n){\
pullRequest(number:$pr){reviews(first:100,after:$after){totalCount pageInfo{hasNextPage endCursor} \
nodes{id databaseId author{login __typename ... on User{databaseId} ... on Bot{databaseId}} state submittedAt lastEditedAt commit{oid} body url}}}}}";

/// Q-L4: PR-level comments.
pub const PR_COMMENTS: &str = "query($o:String!,$n:String!,$pr:Int!,$after:String){repository(owner:$o,name:$n){\
pullRequest(number:$pr){comments(first:100,after:$after){totalCount pageInfo{hasNextPage endCursor} \
nodes{id databaseId author{login __typename ... on User{databaseId} ... on Bot{databaseId}} body createdAt lastEditedAt url}}}}}";

/// A comment's or review's author; `None` for a deleted account (`ghost`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    /// Numeric account id, when GitHub exposes it for this actor.
    #[serde(rename = "databaseId", default)]
    pub database_id: Option<u64>,
    /// Login.
    pub login: String,
    /// `User`, `Bot`, `Organization`, …
    #[serde(rename = "__typename", default)]
    pub typename: String,
}

/// Q-L1 facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrFacts {
    /// Title.
    pub title: String,
    /// Description (untrusted).
    #[serde(default)]
    pub body: String,
    /// GitHub page.
    pub url: String,
    /// `OPEN`, `CLOSED`, `MERGED`.
    pub state: String,
    /// GitHub's last update of the pull request.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
    /// Draft.
    pub is_draft: bool,
    /// Author.
    pub author: Option<Author>,
    /// Base branch.
    pub base_ref_name: String,
    /// Base commit.
    pub base_ref_oid: String,
    /// Head commit.
    pub head_ref_oid: String,
    /// `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED` or null.
    pub review_decision: Option<String>,
}

/// One comment of a thread.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadComment {
    /// Commit where GitHub places this inline comment.
    #[serde(default)]
    pub commit: Option<CommitRef>,
    /// Original commit when GitHub remapped the comment.
    #[serde(default)]
    pub original_commit: Option<CommitRef>,
    /// REST id.
    pub database_id: Option<i64>,
    /// `PENDING` for the viewer's unsubmitted comment.
    #[serde(default)]
    pub state: Option<String>,
    /// Author.
    pub author: Option<Author>,
    /// Text (untrusted).
    pub body: String,
    /// Written.
    pub created_at: String,
    /// Last edit.
    pub last_edited_at: Option<String>,
    /// GitHub link.
    pub url: String,
    /// The review it belongs to.
    pub pull_request_review: Option<NodeRef>,
}

/// A node id.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct NodeRef {
    /// Node id.
    pub id: String,
}

/// A login.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct LoginRef {
    /// Login.
    pub login: String,
}

/// One review thread.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    /// Node id.
    pub id: String,
    /// Resolved on GitHub.
    pub is_resolved: bool,
    /// Its lines changed since it was written.
    pub is_outdated: bool,
    /// File.
    pub path: String,
    /// Last line at the head (null when outdated).
    pub line: Option<u32>,
    /// First line of a multi-line thread.
    pub start_line: Option<u32>,
    /// `LEFT` (base) or `RIGHT` (head).
    pub diff_side: Option<String>,
    /// Who resolved it.
    pub resolved_by: Option<LoginRef>,
    /// Comments, first is the finding.
    pub comments: CountedNodes<ThreadComment>,
}

/// Nodes with their total count.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CountedNodes<T> {
    /// Total.
    pub total_count: u32,
    /// Nodes read.
    pub nodes: Vec<T>,
}

/// A commit id.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CommitRef {
    /// SHA.
    pub oid: String,
}

/// One review.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    /// REST review ID, preserved for inline reply-chain metadata.
    #[serde(default)]
    pub database_id: Option<i64>,
    /// Node id.
    pub id: String,
    /// Author.
    pub author: Option<Author>,
    /// `CHANGES_REQUESTED`, `APPROVED`, `COMMENTED`, `DISMISSED`, `PENDING`.
    pub state: String,
    /// Submitted; null while pending.
    pub submitted_at: Option<String>,
    /// Last edit.
    pub last_edited_at: Option<String>,
    /// Commit reviewed.
    pub commit: Option<CommitRef>,
    /// Text (untrusted).
    pub body: String,
    /// GitHub link.
    pub url: String,
}

/// One PR-level comment.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrComment {
    /// Node id.
    pub id: String,
    /// REST id.
    pub database_id: Option<i64>,
    /// Author.
    pub author: Option<Author>,
    /// Text (untrusted).
    pub body: String,
    /// Written.
    pub created_at: String,
    /// Last edit.
    pub last_edited_at: Option<String>,
    /// GitHub link.
    pub url: String,
}

/// Everything one read found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrRead {
    /// GitHub repository id.
    pub repo_github_id: i64,
    /// Private repository.
    pub private: bool,
    /// Q-L1.
    pub pr: PrFacts,
    /// Q-L2.
    pub threads: Vec<Thread>,
    /// Q-L3.
    pub reviews: Vec<Review>,
    /// Q-L4.
    pub comments: Vec<PrComment>,
    /// Every page of every connection was read.
    pub complete: bool,
}

/// A read failed.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    /// The repository or pull request does not exist or is not visible to the user.
    #[error("the pull request was not found on GitHub")]
    NotFound,
    /// A GraphQL call failed.
    #[error("a GitHub read failed")]
    Graphql(#[from] GraphqlError),
}

#[derive(Deserialize)]
struct Data<T> {
    repository: Option<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactsRepository {
    database_id: i64,
    is_private: bool,
    pull_request: Option<PrFacts>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageRepository<T> {
    pull_request: Option<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadsPull {
    review_threads: Page<Thread>,
}

#[derive(Deserialize)]
struct ReviewsPull {
    reviews: Page<Review>,
}

#[derive(Deserialize)]
struct CommentsPull {
    comments: Page<PrComment>,
}

/// Every page of one connection, at most [`MAX_PAGES`]; `false` when more remain.
async fn pages<P: DeserializeOwned, T>(
    config: &GithubConfig,
    token: &SecretString,
    pr: &PrRef,
    document: &str,
    budget: Option<ReadBudget<'_>>,
    page_of: impl Fn(P) -> Page<T>,
) -> Result<(Vec<T>, bool), ReadError> {
    graphql::pr_pages(
        config,
        token,
        pr,
        document,
        MAX_PAGES,
        budget,
        |data: Data<PageRepository<P>>| {
            data.repository
                .and_then(|repository| repository.pull_request)
                .map(&page_of)
        },
    )
    .await?
    .ok_or(ReadError::NotFound)
}

/// Reads Q-L1–Q-L4 of `pr` with the user's token.
pub(crate) async fn read(
    config: &GithubConfig,
    token: &SecretString,
    pr: &PrRef,
    budget: Option<ReadBudget<'_>>,
) -> Result<PrRead, ReadError> {
    let facts: Data<FactsRepository> = graphql::execute_read(
        config,
        token,
        PR_FACTS,
        json!({"o": pr.repo.owner, "n": pr.repo.name, "pr": pr.number}),
        budget,
    )
    .await?;
    let repository = facts.repository.ok_or(ReadError::NotFound)?;
    let pull = repository.pull_request.ok_or(ReadError::NotFound)?;
    let (threads, threads_complete) = pages(
        config,
        token,
        pr,
        REVIEW_THREADS,
        budget,
        |p: ThreadsPull| p.review_threads,
    )
    .await?;
    let (reviews, reviews_complete) =
        pages(config, token, pr, REVIEWS, budget, |p: ReviewsPull| {
            p.reviews
        })
        .await?;
    let (comments, comments_complete) =
        pages(config, token, pr, PR_COMMENTS, budget, |p: CommentsPull| {
            p.comments
        })
        .await?;
    Ok(PrRead {
        repo_github_id: repository.database_id,
        private: repository.is_private,
        pr: pull,
        threads,
        reviews,
        comments,
        complete: threads_complete && reviews_complete && comments_complete,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::{Path, PathBuf};

    use contracts::common::RepoRef;
    use serde_json::Value;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::github::graphql::tests::github_config;

    pub(crate) fn fixture_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/review-loop/github")
    }

    /// The recorded answer of `file` in `fixtures/review-loop/github`.
    pub(crate) fn recorded(file: &str) -> Value {
        let text = std::fs::read_to_string(fixture_dir().join(file)).expect("fixture");
        serde_json::from_str::<Value>(&text).expect("JSON")["response"].clone()
    }

    /// Serves Q-L1–Q-L4 of fixture PR 482 (each answer recognised by a field only its document uses).
    pub(crate) async fn mount_fixtures(server: &MockServer) {
        for (marker, file) in [
            ("reviewDecision", "pr.json"),
            ("reviewThreads(first:100", "review-threads.json"),
            ("reviews(first:100", "reviews.json"),
            ("comments(first:100", "comments.json"),
        ] {
            Mock::given(method("POST"))
                .and(path("/api/graphql"))
                .and(body_string_contains(marker))
                .respond_with(ResponseTemplate::new(200).set_body_json(recorded(file)))
                .mount(server)
                .await;
        }
    }

    pub(crate) fn pr482() -> PrRef {
        PrRef {
            repo: RepoRef {
                owner: "acme".into(),
                name: "node-client".into(),
            },
            number: 482,
        }
    }

    /// The recorded documents are the ones the server sends (the fixtures stay a faithful record).
    #[test]
    fn fixture_requests_record_the_documents_sent() {
        for (file, document) in [
            ("pr.json", PR_FACTS),
            ("review-threads.json", REVIEW_THREADS),
            ("reviews.json", REVIEWS),
            ("comments.json", PR_COMMENTS),
        ] {
            let text = std::fs::read_to_string(fixture_dir().join(file)).expect("fixture");
            let recording: Value = serde_json::from_str(&text).expect("JSON");
            assert_eq!(
                recording["request"]["body"]["query"],
                json!(document),
                "{file}"
            );
        }
    }

    #[tokio::test]
    async fn read_returns_every_connection_of_the_fixture() {
        let server = MockServer::start().await;
        mount_fixtures(&server).await;
        let read = read(
            &github_config(&server),
            &SecretString::from("ghu_token"),
            &pr482(),
            None,
        )
        .await
        .expect("read");
        assert_eq!(read.pr.head_ref_oid, crate::test_support::github::HEAD);
        assert_eq!(
            read.pr.author.as_ref().map(|a| a.login.as_str()),
            Some("alice")
        );
        assert!(!read.private);
        assert_eq!(
            (read.threads.len(), read.reviews.len(), read.comments.len()),
            (8, 9, 5)
        );
        assert!(read.complete);
    }

    #[tokio::test]
    async fn a_missing_pull_request_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"data": {"repository": {"databaseId": 1, "isPrivate": false,
                        "pullRequest": null}}}),
            ))
            .mount(&server)
            .await;
        let result = read(
            &github_config(&server),
            &SecretString::from("ghu_token"),
            &pr482(),
            None,
        )
        .await;
        assert!(matches!(result, Err(ReadError::NotFound)), "{result:?}");
    }
}
