//! REST searches: open pull request candidates (Q1), issue candidates (Q10) and pull request history (Q12). A
//! login GitHub search refuses is reported as unsearchable (enumeration fallback, Q11).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, NaiveDate, Utc};
use contracts::api::dashboard::ScanNote;
use serde::Deserialize;

use super::client::{TokenSource, WorkClient, WorkError};
use super::{ItemRef, Milestone, note, parse_repo, repo_name};
use contracts::common::RepoRef;

/// How a scanned login relates to an open pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PrRelation {
    /// Author.
    Authored,
    /// Assignee.
    Assigned,
    /// Review requested from the login (directly; GitHub search also counts team requests).
    ReviewRequested,
}

impl PrRelation {
    const ALL: [Self; 3] = [Self::Authored, Self::Assigned, Self::ReviewRequested];

    fn qualifier(self) -> &'static str {
        match self {
            Self::Authored => "author",
            Self::Assigned => "assignee",
            Self::ReviewRequested => "review-requested",
        }
    }
}

/// Why an open issue is a candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IssueRelation {
    /// Open in a repository whose issues the user watches.
    RepoOpen,
    /// Assigned to a scanned login.
    Assigned,
    /// Mentions a scanned login.
    Mentioned,
}

/// An open pull request found by search or enumeration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrCandidate {
    /// Title.
    pub title: String,
    /// Web URL.
    pub url: String,
    /// Relations per scanned login.
    pub relations: BTreeMap<String, BTreeSet<PrRelation>>,
}

/// An open issue found by search or enumeration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueCandidate {
    /// Title.
    pub title: String,
    /// Web URL.
    pub url: String,
    /// Author login.
    pub author: Option<String>,
    /// `OWNER`, `MEMBER`, `CONTRIBUTOR`, `NONE`, …
    pub author_association: Option<String>,
    /// Assignee logins.
    pub assignees: Vec<String>,
    /// Label names.
    pub labels: Vec<String>,
    /// Milestone.
    pub milestone: Option<Milestone>,
    /// Description (untrusted).
    pub body: String,
    /// Opened.
    pub created_at: DateTime<Utc>,
    /// Last update.
    pub updated_at: DateTime<Utc>,
    /// Comment count.
    pub comments: u32,
    /// Why it is a candidate.
    pub relations: BTreeSet<IssueRelation>,
}

/// Candidates of the scan, keyed by item, relations merged across searches and enumeration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Candidates {
    /// Open pull requests.
    pub pull_requests: BTreeMap<ItemRef, PrCandidate>,
    /// Open issues.
    pub issues: BTreeMap<ItemRef, IssueCandidate>,
}

impl Candidates {
    /// Adds a pull request relation of `login`.
    pub fn add_pr(
        &mut self,
        item: ItemRef,
        title: &str,
        url: &str,
        login: &str,
        relation: PrRelation,
    ) {
        self.pull_requests
            .entry(item)
            .or_insert_with(|| PrCandidate {
                title: title.to_owned(),
                url: url.to_owned(),
                relations: BTreeMap::new(),
            })
            .relations
            .entry(login.to_owned())
            .or_default()
            .insert(relation);
    }

    /// Adds an issue, merging its relations into an already found one.
    pub fn add_issue(&mut self, item: ItemRef, issue: IssueCandidate) {
        match self.issues.get_mut(&item) {
            Some(found) => found.relations.extend(issue.relations),
            None => {
                self.issues.insert(item, issue);
            }
        }
    }

    /// Merges `other` into these candidates.
    pub fn merge(&mut self, other: Self) {
        for (item, pr) in other.pull_requests {
            for (login, relations) in pr.relations {
                for relation in relations {
                    self.add_pr(item.clone(), &pr.title, &pr.url, &login, relation);
                }
            }
        }
        for (item, issue) in other.issues {
            self.add_issue(item, issue);
        }
    }
}

/// Pull request candidates of the searchable logins (Q1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrSearch {
    /// Candidates found (pull requests only).
    pub candidates: Candidates,
    /// Logins search refused; the caller enumerates the configured owners for them (Q11).
    pub unsearchable: Vec<String>,
    /// Searches that failed otherwise.
    pub notes: Vec<ScanNote>,
}

/// Issue candidates (Q10).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssueSearch {
    /// Candidates found (issues only).
    pub candidates: Candidates,
    /// Searches that failed.
    pub notes: Vec<ScanNote>,
}

/// Pull requests of a login's history window (Q12).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistorySearch {
    /// Authored and opened in the window.
    pub opened: BTreeSet<ItemRef>,
    /// Authored and closed (merged or not) in the window.
    pub closed: BTreeSet<ItemRef>,
    /// Reviewed by the login and updated in the window.
    pub reviewed: BTreeSet<ItemRef>,
}

/// A REST search item (issues and pull requests share the shape).
#[derive(Deserialize)]
struct SearchItem {
    number: u32,
    title: String,
    html_url: String,
    repository_url: String,
    #[serde(default)]
    user: Option<SearchUser>,
    #[serde(default)]
    author_association: Option<String>,
    #[serde(default)]
    assignees: Vec<SearchUser>,
    #[serde(default)]
    labels: Vec<SearchLabel>,
    #[serde(default)]
    milestone: Option<Milestone>,
    #[serde(default)]
    body: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    comments: u32,
}

#[derive(Deserialize)]
struct SearchUser {
    login: String,
}

#[derive(Deserialize)]
struct SearchLabel {
    name: String,
}

impl SearchItem {
    /// The item; `repository_url` is `<api>/repos/<owner>/<name>`.
    fn item(&self) -> Option<ItemRef> {
        let (_, name) = self.repository_url.rsplit_once("/repos/")?;
        let repo = parse_repo(name)?;
        Some(ItemRef {
            repo,
            number: self.number,
        })
        .filter(|item| item.number > 0)
    }

    fn issue(self, relation: IssueRelation) -> IssueCandidate {
        IssueCandidate {
            title: self.title,
            url: self.html_url,
            author: self.user.map(|user| user.login),
            author_association: self.author_association,
            assignees: self.assignees.into_iter().map(|user| user.login).collect(),
            labels: self.labels.into_iter().map(|label| label.name).collect(),
            milestone: self.milestone,
            body: self.body.unwrap_or_default(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            comments: self.comments,
            relations: BTreeSet::from([relation]),
        }
    }
}

fn search_failed(what: &str, error: &WorkError) -> ScanNote {
    note(
        format!(
            "GitHub search for {what} failed ({error}); its results are missing from this refresh"
        ),
        None,
    )
}

impl<T: TokenSource> WorkClient<T> {
    /// Searches open, non-archived pull requests each login authored, is assigned or asked to review (Q1). The
    /// first refusal marks a login unsearchable and skips its remaining searches.
    pub async fn search_pull_requests(&self, logins: &[String]) -> Result<PrSearch, WorkError> {
        let mut found = PrSearch::default();
        for login in logins {
            for relation in PrRelation::ALL {
                let q = format!(
                    "is:pr is:open archived:false {}:{login}",
                    relation.qualifier()
                );
                let items = match self.search::<SearchItem>(&q).await {
                    Ok(items) => items,
                    Err(WorkError::Unsearchable) => {
                        tracing::info!(
                            login,
                            "GitHub search refuses a scanned login; its owners are enumerated instead"
                        );
                        found.unsearchable.push(login.clone());
                        break;
                    }
                    Err(error) if error.ends_refresh() => return Err(error),
                    Err(error) => {
                        found
                            .notes
                            .push(search_failed(&format!("pull requests of {login}"), &error));
                        continue;
                    }
                };
                for item in items {
                    if let Some(key) = item.item() {
                        found
                            .candidates
                            .add_pr(key, &item.title, &item.html_url, login, relation);
                    }
                }
            }
        }
        Ok(found)
    }

    /// Searches open, non-archived issues of the watched repositories and those assigned to or mentioning a
    /// searchable login (Q10).
    pub async fn search_issues(
        &self,
        watched: &[RepoRef],
        searchable_logins: &[String],
    ) -> Result<IssueSearch, WorkError> {
        let mut searches: Vec<(String, IssueRelation, String)> = watched
            .iter()
            .map(|repo| {
                let name = repo_name(repo);
                (
                    format!("repo:{name}"),
                    IssueRelation::RepoOpen,
                    format!("issues of {name}"),
                )
            })
            .collect();
        for login in searchable_logins {
            searches.push((
                format!("assignee:{login}"),
                IssueRelation::Assigned,
                format!("issues assigned to {login}"),
            ));
            searches.push((
                format!("mentions:{login}"),
                IssueRelation::Mentioned,
                format!("issues mentioning {login}"),
            ));
        }
        let mut found = IssueSearch::default();
        for (qualifier, relation, what) in searches {
            let q = format!("is:issue is:open archived:false {qualifier}");
            match self.search::<SearchItem>(&q).await {
                Ok(items) => {
                    for item in items {
                        if let Some(key) = item.item() {
                            found.candidates.add_issue(key, item.issue(relation));
                        }
                    }
                }
                Err(error) if error.ends_refresh() => return Err(error),
                Err(error) => found.notes.push(search_failed(&what, &error)),
            }
        }
        Ok(found)
    }

    /// Pull requests `login` opened, closed or reviewed since `since` (Q12; `reviewed-by:` needs `updated:`).
    /// [`WorkError::Unsearchable`] when search refuses the login: its events are the fallback (Q14).
    pub async fn search_history(
        &self,
        login: &str,
        since: NaiveDate,
    ) -> Result<HistorySearch, WorkError> {
        let mut history = HistorySearch::default();
        let HistorySearch {
            opened,
            closed,
            reviewed,
        } = &mut history;
        for (qualifiers, set) in [
            (format!("author:{login} created:>={since}"), opened),
            (format!("author:{login} closed:>={since}"), closed),
            (format!("reviewed-by:{login} updated:>={since}"), reviewed),
        ] {
            let items = self
                .search::<SearchItem>(&format!("is:pr {qualifiers}"))
                .await?;
            set.extend(items.iter().filter_map(SearchItem::item));
        }
        Ok(history)
    }
}
