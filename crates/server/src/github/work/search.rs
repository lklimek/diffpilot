//! REST searches: open pull request candidates (Q1), issue candidates (Q10) and pull request history (Q12). A
//! login GitHub search refuses is reported as unsearchable (enumeration fallback, Q11).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use tokio::time::{Instant, timeout};

use contracts::api::dashboard::ScanNote;
use contracts::time::Timestamp;
use jiff::civil::Date;
use serde::Deserialize;

use super::client::{Page, SEARCH_CAP, TokenSource, WorkClient, WorkError};
use super::{ItemRef, Milestone, note, parse_repo, repo_name, valid_login};
use crate::github::rest::is_valid_repo;
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
    /// Mentions a scanned login; discovery only, never a ranking relation.
    Mentioned,
}

impl PrRelation {
    const ALL: [Self; 4] = [
        Self::Authored,
        Self::Assigned,
        Self::ReviewRequested,
        Self::Mentioned,
    ];

    fn qualifier(self) -> &'static str {
        match self {
            Self::Authored => "author",
            Self::Assigned => "assignee",
            Self::ReviewRequested => "review-requested",
            Self::Mentioned => "mentions",
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

impl PrCandidate {
    /// Has a ranking relation beyond mention discovery.
    pub fn ranked(&self) -> bool {
        self.relations
            .values()
            .flatten()
            .any(|r| *r != PrRelation::Mentioned)
    }

    /// At least one scanned login was mentioned.
    pub fn mentioned(&self) -> bool {
        self.relations
            .values()
            .any(|r| r.contains(&PrRelation::Mentioned))
    }
}

/// An open issue found by search or enumeration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueCandidate {
    /// Complete discussion already read by enumeration, when available.
    pub discussion: Option<super::IssueDiscussion>,
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
    pub created_at: Timestamp,
    /// Last update.
    pub updated_at: Timestamp,
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
            Some(found) => {
                found.relations.extend(issue.relations);
                if found.discussion.is_none() {
                    found.discussion = issue.discussion;
                }
            }
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
    /// Searches GitHub answered.
    pub answered: u32,
}

/// Issue candidates (Q10).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssueSearch {
    /// Candidates found (issues only).
    pub candidates: Candidates,
    /// Searches that failed.
    pub notes: Vec<ScanNote>,
    /// Searches GitHub answered.
    pub answered: u32,
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
    /// Searches skipped or cut short.
    pub notes: Vec<ScanNote>,
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
    created_at: Timestamp,
    updated_at: Timestamp,
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
            discussion: None,
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

/// Notes for a search GitHub answered only in part.
fn cut_short(what: &str, page: &Page<SearchItem>) -> Vec<ScanNote> {
    let mut notes = Vec::new();
    if page.total_count > SEARCH_CAP as u64 {
        notes.push(note(
            format!("GitHub found more than 1000 matches for {what}; the rest is not shown"),
            None,
        ));
    }
    if page.incomplete_results {
        notes.push(note(
            format!("GitHub returned an incomplete result for {what}; some items may be missing"),
            None,
        ));
    }
    notes
}

fn invalid_login(login: &str) -> ScanNote {
    note(
        format!("`{login}` is not a valid GitHub login; its searches are skipped"),
        None,
    )
}

impl<T: TokenSource> WorkClient<T> {
    /// Searches open, non-archived PRs each login authored, is assigned, asked to review or mentioned in (Q1). The
    /// first refusal marks a login unsearchable and skips its remaining searches. An invalid login is skipped
    /// with a note (it never reaches `q`).
    pub async fn search_pull_requests(&self, logins: &[String]) -> Result<PrSearch, WorkError> {
        let mut found = PrSearch::default();
        let mut mention_budget = Duration::from_secs(15);
        for login in logins {
            if !valid_login(login) {
                found.notes.push(invalid_login(login));
                continue;
            }
            for relation in PrRelation::ALL {
                let q = format!(
                    "is:pr is:open archived:false {}:{login}",
                    relation.qualifier()
                );
                let what = format!("pull requests of {login}");
                let read = if relation == PrRelation::Mentioned {
                    let started = Instant::now();
                    let read =
                        timeout(mention_budget, self.search_capped::<SearchItem>(&q, 100)).await;
                    mention_budget = mention_budget.saturating_sub(started.elapsed());
                    match read {
                        Ok(read) => read,
                        Err(_) => {
                            found.notes.push(note(
                                "PR mention search exceeded its 15-second budget; \
                                 some mentions may be missing"
                                    .into(),
                                None,
                            ));
                            continue;
                        }
                    }
                } else {
                    self.search::<SearchItem>(&q).await
                };
                let page = match read {
                    Ok(page) => page,
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
                        found.notes.push(search_failed(&what, &error));
                        continue;
                    }
                };
                found.answered += 1;
                if relation == PrRelation::Mentioned && page.total_count > 100 {
                    found.notes.push(note(
                        format!(
                            "Only the first 100 PR mention matches for {login} were read; \
                                 remaining matches are omitted"
                        ),
                        None,
                    ));
                }
                found.notes.extend(cut_short(&what, &page));
                for item in page.items {
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
    /// searchable login (Q10). Invalid repository names and logins are skipped with a note.
    pub async fn search_issues(
        &self,
        watched: &[RepoRef],
        searchable_logins: &[String],
    ) -> Result<IssueSearch, WorkError> {
        let mut found = IssueSearch::default();
        let mut searches: Vec<(String, IssueRelation, String)> = Vec::new();
        for repo in watched {
            let name = repo_name(repo);
            if !is_valid_repo(repo) {
                found.notes.push(note(
                    format!("`{name}` is not a valid repository name; its searches are skipped"),
                    None,
                ));
                continue;
            }
            searches.push((
                format!("repo:{name}"),
                IssueRelation::RepoOpen,
                format!("issues of {name}"),
            ));
        }
        for login in searchable_logins {
            if !valid_login(login) {
                found.notes.push(invalid_login(login));
                continue;
            }
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
        for (qualifier, relation, what) in searches {
            let q = format!("is:issue is:open archived:false {qualifier}");
            match self.search::<SearchItem>(&q).await {
                Ok(page) => {
                    found.answered += 1;
                    found.notes.extend(cut_short(&what, &page));
                    for item in page.items {
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
    /// [`WorkError::Unsearchable`] when search refuses the login: its events are the fallback (Q14). An invalid
    /// login is skipped with a note.
    pub async fn search_history(
        &self,
        login: &str,
        since: Date,
    ) -> Result<HistorySearch, WorkError> {
        let mut history = HistorySearch::default();
        if !valid_login(login) {
            history.notes.push(invalid_login(login));
            return Ok(history);
        }
        let HistorySearch {
            opened,
            closed,
            reviewed,
            notes,
        } = &mut history;
        for (qualifiers, set, what) in [
            (
                format!("author:{login} created:>={since}"),
                opened,
                "pull requests opened by",
            ),
            (
                format!("author:{login} closed:>={since}"),
                closed,
                "pull requests closed by",
            ),
            (
                format!("reviewed-by:{login} updated:>={since}"),
                reviewed,
                "pull requests reviewed by",
            ),
        ] {
            let page = self
                .search::<SearchItem>(&format!("is:pr {qualifiers}"))
                .await?;
            notes.extend(cut_short(&format!("{what} {login}"), &page));
            set.extend(page.items.iter().filter_map(SearchItem::item));
        }
        Ok(history)
    }
}
