//! Alias-batched GraphQL lookups: pull request details (Q2), issue milestones (Q9), history details (Q13) and the
//! reach diagnostic (Q15).

use std::collections::{BTreeMap, HashSet};

use contracts::api::dashboard::{ScanNote, Stack, StackPosition};
use contracts::common::RepoRef;
use contracts::time::Timestamp;
use serde::Deserialize;
use serde_json::Value;

use super::batch::{PR_BATCH, literal, query};
use super::client::{TokenSource, WorkClient, WorkError};
use super::{Fetched, ItemRef, Milestone, note, repo_name, timestamp};

/// Issues per milestone batch (Q9; the skill's size).
const ISSUE_BATCH: usize = 50;
/// Repositories and owners per reach batch (Q15).
const REACH_BATCH: usize = 50;

/// The selection of Q2 (the skill's `PR_DETAIL_QUERY` plus the head and base commits, the repository's privacy and
/// name).
const PULL_FRAGMENT: &str = " fragment Pull on PullRequest { number title url state isDraft \
isCrossRepository body additions deletions createdAt closedAt mergedAt updatedAt mergeable mergeStateStatus reviewDecision \
baseRefName baseRefOid headRefName headRefOid milestone { title number state } \
stackEntry { position stack { id baseRefName size entries(first: 100) { pageInfo { hasNextPage endCursor } \
nodes { position pullRequest { number state } } } } } \
baseRepository { nameWithOwner fullDatabaseId isPrivate viewerPermission defaultBranchRef { name } } \
headRepository { nameWithOwner } author { __typename login } \
labels(first: 20) { nodes { name } } participants { totalCount } \
closingIssuesReferences(first: 5) { nodes { number title labels(first: 10) { nodes { name } } } } \
assignees(first: 10) { nodes { login } } \
reviewRequests(first: 20) { totalCount nodes { requestedReviewer { __typename ... on User { login } \
... on Bot { login } ... on Team { name } } } } \
reviews(last: 20) { totalCount nodes { author { __typename login } state submittedAt commit { oid } } } \
comments(last: 30) { nodes { author { __typename login } body createdAt } } \
reviewThreads(first: 100) { nodes { isResolved isOutdated \
comments(first: 5) { nodes { author { __typename login } body createdAt } } } } \
timelineItems(last: 50, itemTypes: [REVIEW_REQUESTED_EVENT]) { nodes { ... on ReviewRequestedEvent { \
createdAt requestedReviewer { __typename ... on User { login } ... on Bot { login } } } } } \
commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(first: 100) { \
pageInfo { hasNextPage } nodes { __typename ... on CheckRun { name status conclusion startedAt \
checkSuite { workflowRun { workflow { name } } } } ... on StatusContext { context state createdAt } } } } } } } }";

/// `mergeStateStatus` per pull request, as GitHub answered it (Q17).
pub type MergeStates = BTreeMap<ItemRef, String>;

/// A GraphQL connection's nodes.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", bound(deserialize = "N: Deserialize<'de>"))]
pub struct Nodes<N> {
    /// `totalCount`, when selected.
    #[serde(default)]
    pub total_count: Option<u32>,
    /// The nodes.
    #[serde(default)]
    pub nodes: Vec<N>,
}

/// An actor (`User`, `Bot`, `Mannequin`, …).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Actor {
    /// GraphQL type name.
    #[serde(rename = "__typename", default)]
    pub typename: String,
    /// Login.
    #[serde(default)]
    pub login: Option<String>,
}

/// A node with a `login`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Login {
    /// Login.
    pub login: String,
}

/// A node with an `oid`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Oid {
    /// Object id.
    pub oid: String,
}

/// A node with a `name`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Name {
    /// Name.
    pub name: String,
}

/// `{ totalCount }`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Count {
    /// The count.
    pub total_count: u32,
}

/// The base repository of a pull request.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRepository {
    full_database_id: Option<Value>,
    /// `owner/name`.
    pub name_with_owner: String,
    /// Private repository (OD11: shown to this user only).
    pub is_private: bool,
    /// Default branch.
    pub default_branch_ref: Option<Name>,
    /// The signed-in user's permission (`ADMIN`, `MAINTAIN`, `WRITE`, `TRIAGE`, `READ`); null when unknown.
    #[serde(default)]
    pub viewer_permission: Option<String>,
}

impl PullRepository {
    /// Stable GitHub repository identity from the details read.
    pub(crate) fn github_id(&self) -> Option<i64> {
        let id = self.full_database_id.as_ref()?;
        id.as_i64().or_else(|| id.as_str()?.parse().ok())
    }
}

/// An issue a pull request closes.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct LinkedIssue {
    /// Number.
    pub number: u32,
    /// Title.
    pub title: String,
    /// Labels.
    pub labels: Nodes<Name>,
}

/// A requested reviewer: user, bot (login) or team (name).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRequest {
    /// The reviewer; null when GitHub hides it.
    pub requested_reviewer: Option<Reviewer>,
}

/// A requested reviewer.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Reviewer {
    /// `User`, `Bot`, `Team`, …
    #[serde(rename = "__typename", default)]
    pub typename: String,
    /// Login of a user or bot.
    #[serde(default)]
    pub login: Option<String>,
    /// Name of a team.
    #[serde(default)]
    pub name: Option<String>,
}

/// A submitted review.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewNode {
    /// Text of the review.
    #[serde(default)]
    pub body: String,
    /// GitHub permalink.
    #[serde(default)]
    pub url: String,
    /// Author.
    pub author: Option<Actor>,
    /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, …
    pub state: String,
    /// Submission time (null for a pending review).
    pub submitted_at: Option<Timestamp>,
    /// Commit reviewed (null when GitHub no longer has it or the query does not select it).
    #[serde(default)]
    pub commit: Option<Oid>,
}

/// A comment (conversation or thread).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    /// GitHub permalink.
    #[serde(default)]
    pub url: String,
    /// Author.
    pub author: Option<Actor>,
    /// Text (untrusted).
    #[serde(default)]
    pub body: String,
    /// When.
    pub created_at: Timestamp,
}

/// A review thread with its first comments.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewThread {
    /// Resolved.
    pub is_resolved: bool,
    /// Outdated by later commits.
    pub is_outdated: bool,
    /// First five comments.
    pub comments: Nodes<Comment>,
}

/// A `ReviewRequestedEvent`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineRequest {
    /// When.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
    /// Who was asked.
    #[serde(default)]
    pub requested_reviewer: Option<Reviewer>,
}

/// The last commit's `{ commit { statusCheckRollup } }`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CommitNode {
    /// The commit.
    pub commit: CommitRollup,
}

/// A commit's CI rollup.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRollup {
    /// Null without any check or status.
    pub status_check_rollup: Option<StatusRollup>,
}

/// CI rollup of the head commit.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct StatusRollup {
    /// `SUCCESS`, `FAILURE`, `PENDING`, `ERROR`, `EXPECTED`.
    pub state: String,
    /// The first 100 check runs and statuses.
    pub contexts: Contexts,
}

/// Check runs and statuses of a rollup.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contexts {
    /// More than 100 exist.
    pub page_info: HasNextPage,
    /// The contexts.
    pub nodes: Vec<CheckContext>,
}

/// `pageInfo { hasNextPage }`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HasNextPage {
    /// More pages.
    pub has_next_page: bool,
}

/// A check run (`name`, `status`, `conclusion`) or a commit status (`context`, `state`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckContext {
    /// `CheckRun` or `StatusContext`.
    #[serde(rename = "__typename", default)]
    pub typename: String,
    /// Check run name.
    #[serde(default)]
    pub name: Option<String>,
    /// Check run status.
    #[serde(default)]
    pub status: Option<String>,
    /// Check run conclusion.
    #[serde(default)]
    pub conclusion: Option<String>,
    /// Check run start.
    #[serde(default)]
    pub started_at: Option<Timestamp>,
    /// Workflow of a check run (`checkSuite.workflowRun.workflow.name`).
    #[serde(default, deserialize_with = "workflow_name", rename = "checkSuite")]
    pub workflow: Option<String>,
    /// Status context name.
    #[serde(default)]
    pub context: Option<String>,
    /// Status state.
    #[serde(default)]
    pub state: Option<String>,
    /// Status time.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
}

fn workflow_name<'de, De: serde::Deserializer<'de>>(
    deserializer: De,
) -> Result<Option<String>, De::Error> {
    let suite = Value::deserialize(deserializer)?;
    Ok(suite["workflowRun"]["workflow"]["name"]
        .as_str()
        .map(str::to_owned))
}

/// A pull request's head repository.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadRepository {
    /// Repository owner and name.
    pub name_with_owner: String,
}

/// Current facts of an issue.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IssueDetail {
    pub(crate) state: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) author: Option<Actor>,
    pub(crate) labels: Nodes<Name>,
    pub(crate) milestone: Option<Milestone>,
    pub(crate) created_at: Timestamp,
    pub(crate) updated_at: Timestamp,
    pub(crate) comments: Count,
    #[serde(skip)]
    pub(crate) private: Option<bool>,
}

/// Details of one open pull request (Q2).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullDetail {
    /// Number.
    pub number: u32,
    /// Title.
    pub title: String,
    /// Web URL.
    pub url: String,
    /// `OPEN`, `CLOSED`, `MERGED`.
    pub state: String,
    /// Draft.
    pub is_draft: bool,
    /// Head in a fork.
    pub is_cross_repository: bool,
    /// Description (untrusted).
    #[serde(default)]
    pub body: String,
    /// Added lines.
    pub additions: u64,
    /// Deleted lines.
    pub deletions: u64,
    /// Opened.
    pub created_at: Timestamp,
    /// Closed (merged or not).
    pub closed_at: Option<Timestamp>,
    /// Merged.
    pub merged_at: Option<Timestamp>,
    /// Last update.
    pub updated_at: Timestamp,
    /// `MERGEABLE`, `CONFLICTING`, `UNKNOWN`.
    pub mergeable: String,
    /// `CLEAN`, `BLOCKED`, `BEHIND`, …
    pub merge_state_status: String,
    /// `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED` or null.
    pub review_decision: Option<String>,
    /// Base branch.
    pub base_ref_name: String,
    /// Head branch.
    pub head_ref_name: String,
    /// Head commit.
    pub head_ref_oid: String,
    /// Base commit.
    #[serde(default)]
    pub base_ref_oid: Option<String>,
    stack_entry: Option<PullStackEntry>,
    /// Milestone.
    pub milestone: Option<Milestone>,
    /// Base repository.
    pub base_repository: Option<PullRepository>,
    /// Head repository, absent if the fork was deleted.
    #[serde(default)]
    pub head_repository: Option<HeadRepository>,
    /// Author (null for a deleted account).
    pub author: Option<Actor>,
    /// Labels (first 20).
    pub labels: Nodes<Name>,
    /// Participants.
    pub participants: Count,
    /// Issues it closes (first 5).
    pub closing_issues_references: Nodes<LinkedIssue>,
    /// Assignees (first 10).
    pub assignees: Nodes<Login>,
    /// Pending review requests (first 20).
    pub review_requests: Nodes<ReviewRequest>,
    /// Last 20 reviews.
    pub reviews: Nodes<ReviewNode>,
    /// Last 30 conversation comments.
    pub comments: Nodes<Comment>,
    /// Review threads (first 100).
    pub review_threads: Nodes<ReviewThread>,
    /// Last 50 review requests.
    pub timeline_items: Nodes<TimelineRequest>,
    /// The head commit's CI.
    pub commits: Nodes<CommitNode>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct PullStackEntry {
    position: u32,
    stack: Option<PullStack>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullStack {
    id: String,
    base_ref_name: String,
    size: u32,
    entries: StackEntries,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StackEntries {
    nodes: Option<Vec<Option<StackMember>>>,
    page_info: Option<StackPageInfo>,
}

impl StackEntries {
    fn next_cursor(&self) -> Option<&str> {
        let info = self.page_info.as_ref().filter(|page| page.has_next_page)?;
        info.end_cursor.as_deref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StackPageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StackMember {
    position: u32,
    pull_request: Option<StackPull>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct StackPull {
    number: u32,
    state: String,
}

impl PullDetail {
    /// The GitHub-native stack and its open layers above this pull request.
    pub(crate) fn stack(&self) -> Option<Stack> {
        let entry = self.stack_entry.as_ref()?;
        let stack = entry.stack.as_ref()?;
        if stack.size < 2 || entry.position == 0 || entry.position > stack.size {
            return None;
        }
        let mut dependents: Vec<_> = stack
            .entries
            .nodes
            .iter()
            .flatten()
            .flatten()
            .filter(|member| member.position > entry.position)
            .filter_map(|member| {
                member
                    .pull_request
                    .as_ref()
                    .filter(|pr| pr.state == "OPEN")
                    .map(|pr| (member.position, pr.number))
            })
            .collect();
        dependents.sort_unstable();
        let position = if dependents.is_empty() {
            StackPosition::Top
        } else if self.base_ref_name == stack.base_ref_name {
            // GitHub retargets the next open layer to the stack's base when the bottom merges.
            StackPosition::Bottom
        } else {
            StackPosition::Middle
        };
        Some(Stack {
            position,
            size: stack.size,
            dependents: dependents.into_iter().map(|(_, number)| number).collect(),
        })
    }
}

/// History facts of one pull request (Q13).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryPr {
    /// `OPEN`, `CLOSED`, `MERGED`.
    pub state: String,
    /// Opened.
    pub created_at: Timestamp,
    /// Closed (merged or not).
    pub closed_at: Option<Timestamp>,
    /// Merged.
    pub merged_at: Option<Timestamp>,
    /// Author login.
    pub author: Option<String>,
    /// Submission times of the last 20 reviews per scanned login that reviewed it.
    pub reviews: BTreeMap<String, Vec<Timestamp>>,
}

/// What the reach diagnostic (Q15) found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reach {
    /// Repositories the token cannot see.
    pub invisible_repositories: Vec<RepoRef>,
    /// Owners the token cannot see.
    pub invisible_owners: Vec<String>,
    /// Visible repositories that are private.
    pub private_repositories: Vec<RepoRef>,
    /// One note per invisible repository or owner, and per lookup that failed.
    pub notes: Vec<ScanNote>,
}

#[derive(Clone)]
enum ReachKey {
    Repo(RepoRef),
    Owner(String),
}

fn repository(repo: &RepoRef, inner: &str) -> String {
    format!(
        "repository(owner: {}, name: {}) {{ {inner} }}",
        literal(&repo.owner),
        literal(&repo.name)
    )
}

fn left_out(item: &ItemRef, what: &str, issue: bool) -> ScanNote {
    let key = if issue {
        item.issue_key()
    } else {
        item.pr_key()
    };
    note(
        format!("{what} of {item} could not be read from GitHub; left out of this refresh"),
        key,
    )
}

impl<T: TokenSource> WorkClient<T> {
    /// Details of open pull requests (Q2), ten per query. A pull request GitHub does not resolve, or whose
    /// batch and half both fail, is left out with a note.
    pub async fn pull_requests(&self, pulls: &[ItemRef]) -> Result<Fetched<PullDetail>, WorkError> {
        let answers = self
            .batched(pulls, PR_BATCH, PULL_FRAGMENT, |pr| {
                repository(
                    &pr.repo,
                    &format!("pullRequest(number: {}) {{ ...Pull }}", pr.number),
                )
            })
            .await?;
        let mut fetched = Fetched::default();
        for (pr, value) in answers.resolved {
            match serde_json::from_value::<Option<PullDetail>>(value["pullRequest"].clone()) {
                Ok(Some(mut detail)) => {
                    if let Some(stack) = detail
                        .stack_entry
                        .as_mut()
                        .and_then(|entry| entry.stack.as_mut())
                    {
                        self.read_remaining_stack_entries(&pr, stack).await?;
                    }
                    fetched.found.insert(pr, detail);
                }
                Ok(None) => fetched.notes.push(left_out(&pr, "Details", false)),
                Err(error) => {
                    tracing::debug!(%pr, %error, "GitHub pull request details did not parse");
                    fetched.notes.push(left_out(&pr, "Details", false));
                }
            }
        }
        for pr in answers.failed {
            fetched.notes.push(left_out(&pr, "Details", false));
        }
        Ok(fetched)
    }

    async fn read_remaining_stack_entries(
        &self,
        pr: &ItemRef,
        stack: &mut PullStack,
    ) -> Result<(), WorkError> {
        let mut cursors = HashSet::new();
        while let Some(cursor) = stack.entries.next_cursor() {
            if !cursors.insert(cursor.to_owned()) {
                break;
            }
            let lookup = repository(
                &pr.repo,
                &format!(
                    "pullRequest(number: {}) {{ stackEntry {{ stack {{ id entries(first: 100, after: {}) {{ \
                     pageInfo {{ hasNextPage endCursor }} nodes {{ position pullRequest {{ number state }} }} \
                     }} }} }} }}",
                    pr.number,
                    literal(cursor)
                ),
            );
            let data = match self.graphql(&query("", &[lookup])).await {
                Ok(data) => data,
                Err(error) if error.ends_refresh() => return Err(error),
                Err(error) => {
                    tracing::debug!(%pr, ?error, "could not read the next GitHub stack page");
                    break;
                }
            };
            let page =
                &data.get("a0").unwrap_or(&Value::Null)["pullRequest"]["stackEntry"]["stack"];
            if page["id"].as_str() != Some(stack.id.as_str()) {
                break;
            }
            let Ok(StackEntries {
                nodes: Some(nodes),
                page_info,
            }) = serde_json::from_value(page["entries"].clone())
            else {
                break;
            };
            if nodes.is_empty() {
                break;
            }
            let members = stack.entries.nodes.get_or_insert_default();
            members.extend(nodes);
            if members.len() >= stack.size as usize {
                break;
            }
            stack.entries.page_info = page_info;
        }
        Ok(())
    }

    /// `mergeStateStatus` of open pull requests, read again (Q17), ten per query. A pull request GitHub does not
    /// resolve, or whose batch fails, is absent (its first answer stands).
    pub async fn merge_states(&self, pulls: &[ItemRef]) -> Result<MergeStates, WorkError> {
        let answers = self
            .batched(pulls, PR_BATCH, "", |pr| {
                repository(
                    &pr.repo,
                    &format!("pullRequest(number: {}) {{ mergeStateStatus }}", pr.number),
                )
            })
            .await?;
        Ok(answers
            .resolved
            .into_iter()
            .filter_map(|(pr, value)| {
                let state = value["pullRequest"]["mergeStateStatus"]
                    .as_str()?
                    .to_owned();
                Some((pr, state))
            })
            .collect())
    }

    /// Issue facts, with a null issue in a visible repository treated as absent.
    pub(crate) async fn issues(
        &self,
        issues: &[ItemRef],
    ) -> Result<Fetched<Option<IssueDetail>>, WorkError> {
        let answers = self
            .batched(issues, PR_BATCH, "", |issue| {
                repository(
                    &issue.repo,
                    &format!(
                        "isPrivate issue(number: {}) {{ state title body
                 author {{ login }} labels(first: 20) {{ nodes {{ name }} }}
                 comments {{ totalCount }}
                 milestone {{ title number state }} createdAt updatedAt }}",
                        issue.number
                    ),
                )
            })
            .await?;
        let mut fetched = Fetched::default();
        for (issue, value) in answers.resolved {
            match value
                .get("issue")
                .cloned()
                .map(serde_json::from_value::<Option<IssueDetail>>)
            {
                Some(Ok(mut detail)) => {
                    if let Some(detail) = &mut detail {
                        detail.private = value["isPrivate"].as_bool();
                    }
                    fetched.found.insert(issue, detail);
                }
                _ => fetched.notes.push(left_out(&issue, "Details", true)),
            }
        }
        for issue in answers.failed {
            fetched.notes.push(left_out(&issue, "Details", true));
        }
        Ok(fetched)
    }

    /// Milestones of issues (Q9), fifty per query; `None` for an issue without one.
    pub async fn issue_milestones(
        &self,
        issues: &[ItemRef],
    ) -> Result<Fetched<Option<Milestone>>, WorkError> {
        let answers = self
            .batched(issues, ISSUE_BATCH, "", |issue| {
                repository(
                    &issue.repo,
                    &format!(
                        "issue(number: {}) {{ milestone {{ title number state }} }}",
                        issue.number
                    ),
                )
            })
            .await?;
        let mut fetched = Fetched::default();
        for (issue, value) in answers.resolved {
            let parsed = value["issue"].as_object().map(|found| {
                serde_json::from_value::<Option<Milestone>>(found["milestone"].clone())
            });
            match parsed {
                Some(Ok(milestone)) => {
                    fetched.found.insert(issue, milestone);
                }
                _ => fetched.notes.push(left_out(&issue, "The milestone", true)),
            }
        }
        for issue in answers.failed {
            fetched.notes.push(left_out(&issue, "The milestone", true));
        }
        Ok(fetched)
    }

    /// Creation, close and merge times of pull requests plus the review times of each of `logins` (Q13).
    pub async fn history_details(
        &self,
        pulls: &[ItemRef],
        logins: &[String],
    ) -> Result<Fetched<HistoryPr>, WorkError> {
        let reviews: String = logins
            .iter()
            .enumerate()
            .map(|(index, login)| {
                format!(
                    " r{index}: reviews(author: {}, last: 20) {{ nodes {{ submittedAt }} }}",
                    literal(login)
                )
            })
            .collect();
        let answers = self
            .batched(pulls, PR_BATCH, "", |pr| {
                repository(
                    &pr.repo,
                    &format!(
                        "pullRequest(number: {}) {{ state createdAt closedAt mergedAt author {{ login }}{reviews} }}",
                        pr.number
                    ),
                )
            })
            .await?;
        let mut fetched = Fetched::default();
        for (pr, value) in answers.resolved {
            let found = &value["pullRequest"];
            let (Some(state), Some(created_at)) =
                (found["state"].as_str(), timestamp(&found["createdAt"]))
            else {
                fetched.notes.push(left_out(&pr, "The history", false));
                continue;
            };
            let reviews = logins
                .iter()
                .enumerate()
                .map(|(index, login)| {
                    let times: Vec<Timestamp> = found[format!("r{index}")]["nodes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|review| timestamp(&review["submittedAt"]))
                        .collect();
                    (login.clone(), times)
                })
                .filter(|(_, times)| !times.is_empty())
                .collect();
            fetched.found.insert(
                pr,
                HistoryPr {
                    state: state.to_owned(),
                    created_at,
                    closed_at: timestamp(&found["closedAt"]),
                    merged_at: timestamp(&found["mergedAt"]),
                    author: found["author"]["login"].as_str().map(str::to_owned),
                    reviews,
                },
            );
        }
        for pr in answers.failed {
            fetched.notes.push(left_out(&pr, "The history", false));
        }
        Ok(fetched)
    }

    /// Looks up each repository and owner of the user's settings with the user token (Q15), with each visible
    /// repository's privacy; the invisible ones get the note "Not visible to your GitHub token: …".
    pub async fn reach(&self, repos: &[RepoRef], owners: &[String]) -> Result<Reach, WorkError> {
        let keys: Vec<ReachKey> = repos
            .iter()
            .cloned()
            .map(ReachKey::Repo)
            .chain(owners.iter().cloned().map(ReachKey::Owner))
            .collect();
        let answers = self
            .batched(&keys, REACH_BATCH, "", |key| match key {
                ReachKey::Repo(repo) => repository(repo, "id isPrivate"),
                ReachKey::Owner(login) => {
                    format!("repositoryOwner(login: {}) {{ id }}", literal(login))
                }
            })
            .await?;
        let mut reach = Reach::default();
        for (key, value) in answers.resolved {
            if !value.is_null() {
                if let ReachKey::Repo(repo) = key
                    && value["isPrivate"].as_bool() == Some(true)
                {
                    reach.private_repositories.push(repo);
                }
                continue;
            }
            let name = match key {
                ReachKey::Repo(repo) => {
                    let name = repo_name(&repo);
                    reach.invisible_repositories.push(repo);
                    name
                }
                ReachKey::Owner(login) => {
                    reach.invisible_owners.push(login.clone());
                    login
                }
            };
            reach.notes.push(note(
                format!(
                    "Not visible to your GitHub token: {name} — install the GitHub App there or grant it access"
                ),
                None,
            ));
        }
        for key in answers.failed {
            let name = match key {
                ReachKey::Repo(repo) => repo_name(&repo),
                ReachKey::Owner(login) => login,
            };
            reach.notes.push(note(
                format!("Whether {name} is visible to your GitHub token could not be checked"),
                None,
            ));
        }
        Ok(reach)
    }
}
