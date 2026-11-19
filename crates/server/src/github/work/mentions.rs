//! Discussion reads for mentions; PRs reuse their detail query, issues use alias batches.

use super::batch::{PR_BATCH, literal};
use super::{
    Actor, Comment, Fetched, ItemRef, Nodes, PullDetail, TokenSource, WorkClient, WorkError,
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

#[derive(Deserialize)]
struct RestPost {
    id: u64,
    user: Option<RestActor>,
    #[serde(default)]
    body: Option<String>,
    created_at: Option<Timestamp>,
    submitted_at: Option<Timestamp>,
}
#[derive(Deserialize)]
struct RestActor {
    login: String,
    #[serde(rename = "type")]
    kind: String,
}

fn incomplete<N>(nodes: &Nodes<N>) -> bool {
    nodes
        .total_count
        .is_some_and(|n| n as usize > nodes.nodes.len())
}

impl<T: TokenSource> WorkClient<T> {
    /// Issues with their last 50 comments; any older comments are read only when needed.
    pub async fn issue_discussions(
        &self,
        items: &[ItemRef],
    ) -> Result<Fetched<IssueDiscussion>, WorkError> {
        let answers = self.batched(items, PR_BATCH, "", |item| format!(
            "repository(owner: {}, name: {}) {{ issue(number: {}) {{ state body createdAt author {{ __typename login }} comments(last: 50) {{ totalCount nodes {{ author {{ __typename login }} body createdAt url }} }} }} }}",
            literal(&item.repo.owner), literal(&item.repo.name), item.number
        )).await?;
        let mut result = Fetched::default();
        for (item, value) in answers.resolved {
            match serde_json::from_value::<IssueDiscussion>(value["issue"].clone()) {
                Ok(mut discussion) => {
                    if incomplete(&discussion.comments) {
                        match self
                            .discussion_posts(&item, "issues", "comments", "issuecomment")
                            .await
                        {
                            Ok(posts) => discussion.comments.nodes = posts,
                            Err(e) if e.ends_refresh() => return Err(e),
                            Err(_) => {
                                result.notes.push(super::note(format!("The complete discussion of {item} could not be read; its mentions are omitted"), item.issue_key()));
                                continue;
                            }
                        }
                    }
                    result.found.insert(item, discussion);
                }
                Err(_) => result.notes.push(super::note(
                    format!("The discussion of {item} could not be read; its mentions are omitted"),
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

    /// All posts of a PR, refilling only truncated connections; leaves ranking facts untouched.
    pub async fn pull_discussion(
        &self,
        item: &ItemRef,
        detail: &PullDetail,
    ) -> Result<Vec<Comment>, WorkError> {
        let mut posts = if incomplete(&detail.comments) {
            self.discussion_posts(item, "issues", "comments", "issuecomment")
                .await?
        } else {
            detail.comments.nodes.clone()
        };
        if incomplete(&detail.reviews) {
            posts.extend(
                self.discussion_posts(item, "pulls", "reviews", "pullrequestreview")
                    .await?,
            );
        } else {
            posts.extend(detail.reviews.nodes.iter().filter_map(|r| {
                Some(Comment {
                    author: r.author.clone(),
                    body: r.body.clone(),
                    created_at: r.submitted_at?,
                    url: r.url.clone(),
                })
            }));
        }
        if incomplete(&detail.review_threads)
            || detail
                .review_threads
                .nodes
                .iter()
                .any(|t| incomplete(&t.comments))
        {
            posts.extend(
                self.discussion_posts(item, "pulls", "comments", "discussion_r")
                    .await?,
            );
        } else {
            posts.extend(
                detail
                    .review_threads
                    .nodes
                    .iter()
                    .flat_map(|t| t.comments.nodes.iter().cloned()),
            );
        }
        Ok(posts)
    }

    async fn discussion_posts(
        &self,
        item: &ItemRef,
        group: &str,
        connection: &str,
        anchor: &str,
    ) -> Result<Vec<Comment>, WorkError> {
        let path = format!(
            "repos/{}/{}/{group}/{}/{connection}",
            item.repo.owner, item.repo.name, item.number
        );
        let posts: Vec<RestPost> = self.rest_pages(&path, &[], usize::MAX).await?;
        Ok(posts
            .into_iter()
            .filter_map(|p| {
                Some(Comment {
                    author: p.user.map(|u| Actor {
                        login: Some(u.login),
                        typename: u.kind,
                    }),
                    body: p.body.unwrap_or_default(),
                    created_at: if connection == "reviews" {
                        p.submitted_at?
                    } else {
                        p.created_at?
                    },
                    url: format!(
                        "https://github.com/{}/{}/pull/{}#{anchor}{}{}",
                        item.repo.owner,
                        item.repo.name,
                        item.number,
                        if anchor == "discussion_r" { "" } else { "-" },
                        p.id
                    ),
                })
            })
            .collect())
    }
}
