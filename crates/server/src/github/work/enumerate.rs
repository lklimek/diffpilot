//! Enumeration fallback for logins GitHub search refuses (Q11): every non-archived repository of the configured
//! owners with open items, its open pull requests and issues, matched against the logins locally. Mentions are
//! matched in issue bodies and the last 50 comments only.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use contracts::api::dashboard::ScanNote;
use futures::future::join_all;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::batch::{literal, query};
use super::client::{TokenSource, WorkClient, WorkError};
use super::details::{Count, Login, Name, Nodes, ReviewRequest};
use super::search::{Candidates, IssueCandidate, IssueRelation, PrRelation};
use super::{ItemRef, Milestone, note, parse_repo, repo_name};
use contracts::common::RepoRef;

/// What the enumeration found for the unsearchable logins.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Enumeration {
    /// Open pull requests and issues involving the logins.
    pub candidates: Candidates,
    /// One note per owner or repository that could not be listed.
    pub notes: Vec<ScanNote>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OwnerRepo {
    name_with_owner: String,
    pull_requests: Count,
    issues: Count,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenPull {
    number: u32,
    title: String,
    url: String,
    author: Option<Login>,
    assignees: Nodes<Login>,
    review_requests: Nodes<ReviewRequest>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenIssue {
    number: u32,
    title: String,
    url: String,
    #[serde(default)]
    body: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    author_association: Option<String>,
    author: Option<Login>,
    assignees: Nodes<Login>,
    labels: Nodes<Name>,
    comments: Nodes<IssueComment>,
    milestone: Option<Milestone>,
}

#[derive(Deserialize)]
struct IssueComment {
    #[serde(default)]
    body: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    page_info: PageInfo,
    nodes: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

const PULL_NODES: &str = "number title url author { login } assignees(first: 20) { nodes { login } } \
reviewRequests(first: 20) { nodes { requestedReviewer { ... on User { login } } } }";
const ISSUE_NODES: &str = "number title url body createdAt updatedAt authorAssociation author { login } \
assignees(first: 20) { nodes { login } } labels(first: 20) { nodes { name } } \
comments(last: 50) { totalCount nodes { body } } milestone { title number state }";

/// `@login` in `text`, not part of a longer login or an e-mail address (case-insensitive).
pub(crate) fn mentions(text: &str, login: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    let lower = text.to_ascii_lowercase();
    let wanted = format!("@{}", login.to_ascii_lowercase());
    lower.match_indices(&wanted).any(|(at, _)| {
        let before = lower[..at].chars().next_back();
        let after = lower[at + wanted.len()..].chars().next();
        !before.is_some_and(word) && !after.is_some_and(word)
    })
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl<T: TokenSource> WorkClient<T> {
    /// Every node of a connection, paged by cursor; `lookup` builds the top-level field for a cursor and
    /// `path` leads from it to the connection.
    async fn all_nodes<N: DeserializeOwned>(
        &self,
        lookup: impl Fn(&str) -> String,
        path: &[&str],
    ) -> Result<Vec<N>, WorkError> {
        let mut nodes = Vec::new();
        let mut after = String::new();
        loop {
            let mut data = self.graphql(&query("", &[lookup(&after)])).await?;
            let mut connection = data.remove("a0").unwrap_or(Value::Null);
            for key in path {
                connection = connection[*key].take();
            }
            let page: Page = serde_json::from_value(connection).map_err(WorkError::Malformed)?;
            for node in page.nodes {
                nodes.push(serde_json::from_value(node).map_err(WorkError::Malformed)?);
            }
            match page.page_info.end_cursor {
                Some(cursor) if page.page_info.has_next_page => {
                    after = format!(", after: {}", literal(&cursor));
                }
                _ => return Ok(nodes),
            }
        }
    }

    /// Open pull requests and issues of `owners`' repositories that involve `logins` (Q11).
    pub async fn enumerate_owners(
        &self,
        owners: &[String],
        logins: &[String],
    ) -> Result<Enumeration, WorkError> {
        let mut enumeration = Enumeration::default();
        let mut repos = Vec::new();
        for owner in owners {
            let listed = self
                .all_nodes::<OwnerRepo>(
                    |after| {
                        format!(
                            "repositoryOwner(login: {}) {{ repositories(first: 100, isArchived: false{after}) {{ \
                             pageInfo {{ hasNextPage endCursor }} nodes {{ nameWithOwner \
                             pullRequests(states: OPEN) {{ totalCount }} issues(states: OPEN) {{ totalCount }} }} }} }}",
                            literal(owner)
                        )
                    },
                    &["repositories"],
                )
                .await;
            match listed {
                Ok(listed) => repos.extend(listed.into_iter().filter_map(|repo| {
                    let prs = repo.pull_requests.total_count > 0;
                    let issues = repo.issues.total_count > 0;
                    parse_repo(&repo.name_with_owner)
                        .filter(|_| prs || issues)
                        .map(|name| (name, prs, issues))
                })),
                Err(error) if error.ends_refresh() => return Err(error),
                Err(error) => enumeration.notes.push(note(
                    format!("Repositories of {owner} could not be listed ({error}); items of unsearchable logins there are missing"),
                    None,
                )),
            }
        }
        let results = join_all(
            repos
                .iter()
                .map(|(repo, prs, issues)| self.enumerate_repo(repo, *prs, *issues, logins)),
        )
        .await;
        for (result, (repo, _, _)) in results.into_iter().zip(&repos) {
            match result {
                Ok(found) => enumeration.candidates.merge(found),
                Err(error) if error.ends_refresh() => return Err(error),
                Err(error) => enumeration.notes.push(note(
                    format!("Open items of {} could not be listed ({error}); items of unsearchable logins there are missing", repo_name(repo)),
                    None,
                )),
            }
        }
        Ok(enumeration)
    }

    async fn enumerate_repo(
        &self,
        repo: &RepoRef,
        has_prs: bool,
        has_issues: bool,
        logins: &[String],
    ) -> Result<Candidates, WorkError> {
        let mut found = Candidates::default();
        let lookup = |connection: &'static str, nodes: &'static str| {
            move |after: &str| {
                format!(
                    "repository(owner: {}, name: {}) {{ {connection}(states: OPEN, first: 50{after}) {{ \
                     pageInfo {{ hasNextPage endCursor }} nodes {{ {nodes} }} }} }}",
                    literal(&repo.owner),
                    literal(&repo.name)
                )
            }
        };
        if has_prs {
            let pulls: Vec<OpenPull> = self
                .all_nodes(lookup("pullRequests", PULL_NODES), &["pullRequests"])
                .await?;
            for pull in pulls {
                let Some(item) = ItemRef::new(&repo.owner, &repo.name, pull.number) else {
                    continue;
                };
                for login in logins {
                    let relations = [
                        (
                            pull.author.as_ref().is_some_and(|a| same(&a.login, login)),
                            PrRelation::Authored,
                        ),
                        (
                            pull.assignees.nodes.iter().any(|a| same(&a.login, login)),
                            PrRelation::Assigned,
                        ),
                        (
                            pull.review_requests.nodes.iter().any(|request| {
                                request
                                    .requested_reviewer
                                    .as_ref()
                                    .and_then(|r| r.login.as_deref())
                                    .is_some_and(|reviewer| same(reviewer, login))
                            }),
                            PrRelation::ReviewRequested,
                        ),
                    ];
                    for (hit, relation) in relations {
                        if hit {
                            found.add_pr(item.clone(), &pull.title, &pull.url, login, relation);
                        }
                    }
                }
            }
        }
        if has_issues {
            let issues: Vec<OpenIssue> = self
                .all_nodes(lookup("issues", ISSUE_NODES), &["issues"])
                .await?;
            for issue in issues {
                let mut relations = BTreeSet::new();
                for login in logins {
                    if issue.assignees.nodes.iter().any(|a| same(&a.login, login)) {
                        relations.insert(IssueRelation::Assigned);
                    }
                    let mut texts = std::iter::once(issue.body.as_str())
                        .chain(issue.comments.nodes.iter().map(|c| c.body.as_str()));
                    if texts.any(|text| mentions(text, login)) {
                        relations.insert(IssueRelation::Mentioned);
                    }
                }
                let Some(item) = ItemRef::new(&repo.owner, &repo.name, issue.number) else {
                    continue;
                };
                if relations.is_empty() {
                    continue;
                }
                found.add_issue(
                    item,
                    IssueCandidate {
                        title: issue.title,
                        url: issue.url,
                        author: issue.author.map(|a| a.login),
                        author_association: issue.author_association,
                        assignees: issue.assignees.nodes.into_iter().map(|a| a.login).collect(),
                        labels: issue.labels.nodes.into_iter().map(|l| l.name).collect(),
                        milestone: issue.milestone,
                        body: issue.body,
                        created_at: issue.created_at,
                        updated_at: issue.updated_at,
                        comments: issue.comments.total_count.unwrap_or_default(),
                        relations,
                    },
                );
            }
        }
        Ok(found)
    }
}
