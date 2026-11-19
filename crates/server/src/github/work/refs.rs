//! One open-PR ref listing per repository (Q5/Q6): the branches of every open pull request, five repositories
//! per query, paged by cursor. Stacks are computed locally, within one repository (the skill's 131 `gh pr list`
//! calls, S11-2 C3).

use std::collections::{BTreeMap, BTreeSet};

use contracts::api::dashboard::ScanNote;
use contracts::common::RepoRef;
use serde::Deserialize;

use super::batch::literal;
use super::client::{TokenSource, WorkClient, WorkError};
use super::{ItemRef, note, repo_name};

/// Repositories per listing query.
const REPOS_PER_QUERY: usize = 5;

/// An open pull request's branches.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenPrRef {
    /// Number.
    pub number: u32,
    /// Base branch.
    pub base_ref_name: String,
    /// Head branch (in the fork when cross-repository).
    pub head_ref_name: String,
    /// Draft.
    pub is_draft: bool,
    /// Head in a fork.
    pub is_cross_repository: bool,
}

/// Open pull requests in the same repository a pull request waits for or that wait for it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blockers {
    /// Open pull requests based on this one's head branch (they wait for it).
    pub dependents: Vec<u32>,
    /// Open same-repository pull requests whose head is this one's base branch (it waits for them). The caller
    /// ignores this when the base is a long-lived branch (settings K9).
    pub stacked_on: Vec<u32>,
}

/// The listing of the scanned repositories.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefListing {
    /// Open pull requests per listed repository.
    pub repositories: BTreeMap<RepoRef, Vec<OpenPrRef>>,
    /// One note per repository whose listing failed.
    pub notes: Vec<ScanNote>,
}

impl RefListing {
    /// The relations of `pr`; `None` when its repository's listing failed or it is not open (lookup failed).
    pub fn blockers(&self, pr: &ItemRef) -> Option<Blockers> {
        let open = self.repositories.get(&pr.repo)?;
        let this = open.iter().find(|other| other.number == pr.number)?;
        let others = || open.iter().filter(|other| other.number != pr.number);
        // A fork's head branch is no branch of this repository: nothing here is based on it.
        if this.is_cross_repository {
            return Some(Blockers::default());
        }
        let mut blockers = Blockers {
            dependents: others()
                .filter(|other| other.base_ref_name == this.head_ref_name)
                .map(|other| other.number)
                .collect(),
            stacked_on: others()
                .filter(|other| {
                    !other.is_cross_repository && other.head_ref_name == this.base_ref_name
                })
                .map(|other| other.number)
                .collect(),
        };
        blockers.dependents.sort_unstable();
        blockers.stacked_on.sort_unstable();
        Some(blockers)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    page_info: PageInfo,
    nodes: Vec<OpenPrRef>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

impl<T: TokenSource> WorkClient<T> {
    /// Lists the open pull requests of `repos` (Q5/Q6); a repository whose listing fails gets a note and no
    /// entry, so [`RefListing::blockers`] reports its lookups as failed.
    pub async fn open_pr_refs(&self, repos: &[RepoRef]) -> Result<RefListing, WorkError> {
        let mut listing = RefListing::default();
        let mut pending: Vec<(RepoRef, Option<String>)> =
            repos.iter().map(|repo| (repo.clone(), None)).collect();
        let mut failed = BTreeSet::new();
        while !pending.is_empty() {
            let answers = self
                .batched(&pending, REPOS_PER_QUERY, "", |(repo, after)| {
                    let after = after
                        .as_deref()
                        .map(|cursor| format!(", after: {}", literal(cursor)))
                        .unwrap_or_default();
                    format!(
                        "repository(owner: {}, name: {}) {{ pullRequests(states: OPEN, first: 100{after}) {{ \
                         pageInfo {{ hasNextPage endCursor }} nodes {{ number baseRefName headRefName isDraft \
                         isCrossRepository }} }} }}",
                        literal(&repo.owner),
                        literal(&repo.name)
                    )
                })
                .await?;
            failed.extend(answers.failed.into_iter().map(|(repo, _)| repo));
            pending = Vec::new();
            for ((repo, _), value) in answers.resolved {
                match serde_json::from_value::<Page>(value["pullRequests"].clone()) {
                    Ok(page) => {
                        listing
                            .repositories
                            .entry(repo.clone())
                            .or_default()
                            .extend(page.nodes);
                        if page.page_info.has_next_page
                            && let Some(cursor) = page.page_info.end_cursor
                        {
                            pending.push((repo, Some(cursor)));
                        }
                    }
                    Err(error) => {
                        tracing::debug!(repo = %repo_name(&repo), %error,
                            "an open pull request listing did not parse");
                        failed.insert(repo);
                    }
                }
            }
        }
        for repo in failed {
            listing.repositories.remove(&repo);
            listing.notes.push(note(
                format!(
                    "Open pull requests of {} could not be listed; stacked pull requests there are not detected",
                    repo_name(&repo)
                ),
                None,
            ));
        }
        Ok(listing)
    }
}
