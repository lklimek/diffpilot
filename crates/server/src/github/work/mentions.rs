//! Bounded conversation windows, isolated from the large ranking detail query.

use super::batch::literal;
use super::{
    Actor, Comment, Fetched, ItemRef, Nodes, ReviewNode, ReviewThread, TokenSource, WorkClient,
    WorkError,
};
use contracts::time::Timestamp;
use serde::Deserialize;

/// An issue's discussion, including the description.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueDiscussion {
    /// Current state.
    pub state: String,
    /// Description.
    pub body: String,
    /// Author.
    pub author: Option<Actor>,
    /// Opening time.
    pub created_at: Timestamp,
    /// Conversation comments.
    pub comments: Nodes<Comment>,
}

/// Conversation-only PR projection; no checks, timeline, labels or ranking context.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullDiscussion {
    /// Drafts by others remain excluded.
    pub is_draft: bool,
    /// Description and ordinary comments.
    #[serde(flatten)]
    pub issue: IssueDiscussion,
    /// Submitted reviews.
    pub reviews: Nodes<ReviewNode>,
    /// Inline review discussions.
    pub review_threads: Nodes<ReviewThread>,
}

/// Missing counts also mean completeness cannot be established.
pub(crate) fn incomplete<N>(nodes: &Nodes<N>) -> bool {
    nodes
        .total_count
        .is_none_or(|n| n as usize != nodes.nodes.len())
}

impl PullDiscussion {
    /// Whether any bounded connection omits part of the conversation.
    pub fn partly_read(&self) -> bool {
        incomplete(&self.issue.comments)
            || incomplete(&self.reviews)
            || incomplete(&self.review_threads)
            || self
                .review_threads
                .nodes
                .iter()
                .any(|t| incomplete(&t.comments))
    }

    /// Comments and submitted reviews in the read windows.
    pub fn posts(&self) -> Vec<Comment> {
        let mut posts = self.issue.comments.nodes.clone();
        posts.extend(self.reviews.nodes.iter().filter_map(|r| {
            Some(Comment {
                author: r.author.clone(),
                body: r.body.clone(),
                created_at: r.submitted_at?,
                url: r.url.clone(),
            })
        }));
        posts.extend(
            self.review_threads
                .nodes
                .iter()
                .flat_map(|t| t.comments.nodes.iter().cloned()),
        );
        posts
    }
}

const DISCUSSION: &str = "state body createdAt author { __typename login } \
comments(last: 50) { totalCount nodes { author { __typename login } body createdAt url } }";
const REVIEWS: &str = "reviews(last: 20) { totalCount nodes { author { __typename login } state submittedAt body url } } \
reviewThreads(last: 20) { totalCount nodes { isResolved isOutdated \
comments(last: 5) { totalCount nodes { author { __typename login } body createdAt url } } } }";

impl<T: TokenSource> WorkClient<T> {
    /// One bounded query per issue; no REST refills or batch-split amplification.
    pub async fn issue_discussions(
        &self,
        items: &[ItemRef],
    ) -> Result<Fetched<IssueDiscussion>, WorkError> {
        let answers = self
            .batched(items, 1, "", |item| {
                format!(
                    "repository(owner: {}, name: {}) {{ issue(number: {}) {{ {DISCUSSION} }} }}",
                    literal(&item.repo.owner),
                    literal(&item.repo.name),
                    item.number
                )
            })
            .await?;
        let mut result = Fetched::default();
        for (item, value) in answers.resolved {
            match serde_json::from_value::<IssueDiscussion>(value["issue"].clone()) {
                Ok(discussion) => {
                    result.found.insert(item, discussion);
                }
                _ => result.notes.push(super::note(
                    format!("The discussion of {item} could not be read"),
                    item.issue_key(),
                )),
            }
        }
        for item in answers.failed {
            result.notes.push(super::note(
                format!("The discussion of {item} could not be read; its mentions are omitted"),
                item.issue_key(),
            ));
        }
        Ok(result)
    }

    /// One bounded query per mentioned PR, independently of Q2.
    pub async fn pull_discussions(
        &self,
        items: &[ItemRef],
    ) -> Result<Fetched<PullDiscussion>, WorkError> {
        let answers = self.batched(items, 1, "", |item| format!(
            "repository(owner: {}, name: {}) {{ pullRequest(number: {}) {{ isDraft {DISCUSSION} {REVIEWS} }} }}",
            literal(&item.repo.owner), literal(&item.repo.name), item.number
        )).await?;
        let mut result = Fetched::default();
        for (item, value) in answers.resolved {
            match serde_json::from_value::<PullDiscussion>(value["pullRequest"].clone()) {
                Ok(discussion) => {
                    result.found.insert(item, discussion);
                }
                Err(_) => result.notes.push(super::note(
                    format!("The discussion of {item} could not be read; its mentions are omitted"),
                    item.pr_key(),
                )),
            }
        }
        for item in answers.failed {
            result.notes.push(super::note(
                format!("The discussion of {item} could not be read; its mentions are omitted"),
                item.pr_key(),
            ));
        }
        Ok(result)
    }

    /// A bounded ownership read for a mention-only PR (100 files, one page).
    pub async fn mention_files(&self, item: &ItemRef) -> Result<Vec<super::PullFile>, WorkError> {
        self.rest_pages(
            &format!(
                "repos/{}/{}/pulls/{}/files",
                item.repo.owner, item.repo.name, item.number
            ),
            &[],
            100,
        )
        .await
    }
}
