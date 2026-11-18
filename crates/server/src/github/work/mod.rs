//! Read-only GitHub scan of a user's work (plan v1.1 §A.2, §A.3; T18), with the user's token: REST searches
//! (Q1, Q10, Q12) one at a time and paced, GraphQL alias batches (Q2, Q9, Q13, Q15), one open-PR ref listing per
//! repository (Q5/Q6), owner enumeration for logins search refuses (Q11) and REST reads (Q3, Q4, Q7, Q8, Q14).
//!
//! Only `GET` and GraphQL `query` documents are sent. The token stays inside [`WorkClient`]: it is never logged
//! and no error carries it. Errors for which [`WorkError::ends_refresh`] holds fail the refresh; every other
//! failure of one item, repository or owner becomes a [`ScanNote`] and the scan goes on.

mod batch;
mod client;
mod details;
mod enumerate;
mod reads;
mod refs;
mod search;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, Utc};
use contracts::api::dashboard::{ItemKey, ItemTarget, ScanNote};
use contracts::common::RepoRef;
use serde::Deserialize;

use crate::github::rest::is_valid_repo;

pub use batch::{MAX_PR_BATCH, PR_BATCH};
pub use client::{TokenSource, UserTokens, WorkClient, WorkError};
pub use details::{
    Actor, CheckContext, Comment, CommitNode, CommitRollup, Contexts, Count, HasNextPage,
    HistoryPr, LinkedIssue, Login, Name, Nodes, PullDetail, PullRepository, Reach, ReviewNode,
    ReviewRequest, ReviewThread, Reviewer, StatusRollup, TimelineRequest,
};
pub use enumerate::Enumeration;
pub use reads::{PullFile, Release, RestMilestone, UserEvent};
pub use refs::{Blockers, OpenPrRef, RefListing};
pub use search::{
    Candidates, HistorySearch, IssueCandidate, IssueRelation, IssueSearch, PrCandidate, PrRelation,
    PrSearch,
};

/// A pull request or issue: repository and number.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemRef {
    /// Repository.
    pub repo: RepoRef,
    /// Number.
    pub number: u32,
}

impl ItemRef {
    /// `owner/name` and number; `None` for names GitHub does not allow or number 0.
    pub fn new(owner: &str, name: &str, number: u32) -> Option<Self> {
        let repo = RepoRef {
            owner: owner.to_owned(),
            name: name.to_owned(),
        };
        (is_valid_repo(&repo) && number > 0).then_some(Self { repo, number })
    }

    /// `owner/name` plus a number.
    pub fn parse(name_with_owner: &str, number: u32) -> Option<Self> {
        let (owner, name) = name_with_owner.split_once('/')?;
        Self::new(owner, name, number)
    }

    /// The item key of this pull request.
    pub fn pr_key(&self) -> Option<ItemKey> {
        ItemKey::new(&ItemTarget::PullRequest {
            repo: self.repo.clone(),
            number: self.number,
        })
        .ok()
    }

    /// The item key of this issue.
    pub fn issue_key(&self) -> Option<ItemKey> {
        ItemKey::new(&ItemTarget::Issue {
            repo: self.repo.clone(),
            number: self.number,
        })
        .ok()
    }
}

impl fmt::Display for ItemRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}#{}", self.repo.owner, self.repo.name, self.number)
    }
}

/// `owner/name` of a repository.
pub fn repo_name(repo: &RepoRef) -> String {
    format!("{}/{}", repo.owner, repo.name)
}

/// Parses `owner/name`; `None` for names GitHub does not allow.
pub fn parse_repo(name_with_owner: &str) -> Option<RepoRef> {
    ItemRef::parse(name_with_owner, 1).map(|item| item.repo)
}

/// A milestone as GraphQL and REST search answer it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Milestone {
    /// Title.
    pub title: String,
    /// Number.
    pub number: u32,
    /// `OPEN`/`CLOSED` (GraphQL) or `open`/`closed` (REST).
    pub state: String,
}

/// What a batched read found: the items read, and one note per item left out.
#[derive(Clone, Debug, PartialEq)]
pub struct Fetched<T> {
    /// Items read.
    pub found: BTreeMap<ItemRef, T>,
    /// Items left out, one note each.
    pub notes: Vec<ScanNote>,
}

impl<T> Default for Fetched<T> {
    fn default() -> Self {
        Self {
            found: BTreeMap::new(),
            notes: Vec::new(),
        }
    }
}

/// A login (optionally with GitHub's `[bot]` suffix) is safe in an API path and a search query.
fn valid_login(login: &str) -> bool {
    let bare = login.strip_suffix("[bot]").unwrap_or(login);
    (1..=39).contains(&bare.len())
        && bare
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn note(text: String, item: Option<ItemKey>) -> ScanNote {
    ScanNote { text, item }
}

/// An RFC 3339 timestamp GitHub sent.
fn timestamp(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    value
        .as_str()
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|time| time.with_timezone(&Utc))
}
