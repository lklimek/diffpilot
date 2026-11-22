//! The scan of one refresh (plan §A.2–§A.4): searches, enumeration fallback, details with the ref listing,
//! context (ownership, milestones, reach), history and TODOs, assembled into a [`Snapshot`]. Only errors for which
//! `WorkError::ends_refresh` holds fail it; every other failure is a scan note.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use contracts::api::dashboard::{
    DashboardSettings, KindCounts, MentionCoverage, PolicyRef, ScanNote, ScanStepKey, StepState,
    TodosState, TodosStatus,
};
use contracts::common::RepoRef;
use contracts::time::Timestamp;
use futures::future::join_all;
use jiff::tz::TimeZone;
use tokio::time::{Instant, sleep_until};

use super::RunError;
use super::facts::{self, PrContext};
use super::owned;
use super::progress::{Ended, Progress};
use super::{history, mentions};
use crate::dashboard::score::{self, MilestoneContext, OwnedChange, ScoreRules, TodoFacts};
use crate::dashboard::settings::memcan_projects;
use crate::dashboard::snapshot::{SNAPSHOT_VERSION, Snapshot, StoredPr, StoredTodo};
use crate::github::work::{
    Fetched, ItemRef, PR_BATCH, PullDetail, TokenSource, WorkClient, WorkError, repo_name,
};
use crate::memcan::todos::{self, TodosRead};

/// Pull requests per details step: four batches, read concurrently by the client.
const DETAIL_CHUNK: usize = PR_BATCH * 4;
/// The user's pull requests whose merge state is read again (Q17): one batch.
const MERGE_RECHECK_LIMIT: usize = PR_BATCH;

/// What a scan reads besides GitHub.
pub(super) struct ScanInput<'a> {
    /// Rules of the user's settings.
    pub rules: &'a ScoreRules,
    /// The signed-in login.
    pub own_login: &'a str,
    /// The user's settings.
    pub settings: &'a DashboardSettings,
    /// The user's time zone (UTC until one is stored).
    pub zone: &'a TimeZone,
    /// The current snapshot (TODO change times, TODOs kept when MemCan is unavailable).
    pub previous: Option<&'a Snapshot>,
    /// MemCan is configured.
    pub memcan: bool,
}

fn note(text: String) -> ScanNote {
    ScanNote { text, item: None }
}

pub(super) fn same_repo(a: &RepoRef, b: &RepoRef) -> bool {
    a.owner.eq_ignore_ascii_case(&b.owner) && a.name.eq_ignore_ascii_case(&b.name)
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Runs the scan; `Err` for an error that ends the refresh, when no candidate search was answered (GitHub
/// down: a failed scan, never an empty snapshot), or when the refresh ended meanwhile.
pub(super) async fn scan<T: TokenSource>(
    client: &WorkClient<T>,
    input: &ScanInput<'_>,
    progress: &mut Progress,
) -> Result<Snapshot, RunError> {
    let settings = input.settings;
    let mut notes = Vec::new();
    let logins: Vec<String> = std::iter::once(input.own_login.to_owned())
        .chain(settings.linked_logins.iter().cloned())
        .collect();

    progress
        .step(ScanStepKey::SearchPrs, StepState::Running, None, None)
        .await?;
    let found = client.search_pull_requests(&logins).await?;
    notes.extend(found.notes);
    let mut candidates = found.candidates;
    let searchable: Vec<String> = logins
        .iter()
        .filter(|login| !found.unsearchable.contains(login))
        .cloned()
        .collect();
    if !found.unsearchable.is_empty() {
        if settings.enumerate_owners.is_empty() {
            notes.push(note(format!(
                "GitHub search refuses {}. Their work is missing from this list. In Settings → Dashboard, add the \
                 organisations or users that hold it under \"When GitHub search refuses an account\".",
                found.unsearchable.join(", ")
            )));
        } else {
            let listed = client
                .enumerate_owners(&settings.enumerate_owners, &found.unsearchable)
                .await?;
            notes.extend(listed.notes);
            candidates.merge(listed.candidates);
        }
    }
    let prs = count(
        candidates
            .pull_requests
            .values()
            .filter(|pr| pr.ranked())
            .count(),
    );
    progress.log(format!("search PRs: {prs} found ({} logins)", logins.len()));
    progress
        .step(
            ScanStepKey::SearchPrs,
            StepState::Done,
            Some(prs),
            Some(prs),
        )
        .await?;

    progress
        .step(ScanStepKey::Issues, StepState::Running, None, None)
        .await?;
    let watched: Vec<RepoRef> = settings
        .repositories
        .iter()
        .filter(|row| row.watch_issues)
        .map(|row| row.repo.clone())
        .collect();
    let issues = client.search_issues(&watched, &searchable).await?;
    let attempted = !searchable.is_empty() || !watched.is_empty();
    if attempted && found.answered == 0 && issues.answered == 0 {
        return Err(RunError::SearchUnavailable);
    }
    notes.extend(issues.notes);
    candidates.merge(issues.candidates);
    let issue_count = count(candidates.issues.values().filter(|i| i.ranked()).count());
    progress.log(format!("issues: {issue_count} found"));
    progress
        .step(
            ScanStepKey::Issues,
            StepState::Done,
            Some(issue_count),
            Some(issue_count),
        )
        .await?;

    let pulls: Vec<ItemRef> = candidates
        .pull_requests
        .iter()
        .filter(|(_, c)| c.ranked())
        .map(|(item, _)| item.clone())
        .collect();
    let total = count(pulls.len());
    progress
        .step(
            ScanStepKey::PrDetails,
            StepState::Running,
            Some(0),
            Some(total),
        )
        .await?;
    let repos: Vec<RepoRef> = pulls
        .iter()
        .map(|pr| pr.repo.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let details = async {
        let mut all = Fetched::default();
        let mut done = 0;
        for chunk in pulls.chunks(DETAIL_CHUNK) {
            let fetched = client.pull_requests(chunk).await?;
            all.found.extend(fetched.found);
            all.notes.extend(fetched.notes);
            done += count(chunk.len());
            if let Some(last) = chunk.last() {
                progress.detail(ScanStepKey::PrDetails, last.to_string());
            }
            progress
                .step(
                    ScanStepKey::PrDetails,
                    StepState::Running,
                    Some(done),
                    Some(total),
                )
                .await?;
        }
        Ok::<_, RunError>(all)
    };
    let refs = async { client.open_pr_refs(&repos).await.map_err(RunError::from) };
    let (mut details, listing) = tokio::try_join!(details, refs)?;
    let details_read = Instant::now();
    notes.append(&mut details.notes);
    notes.extend(listing.notes.iter().cloned());
    details.found.retain(|_, detail| detail.state == "OPEN");
    progress.log(format!(
        "PR details: {} of {total} read",
        details.found.len()
    ));
    progress
        .step(
            ScanStepKey::PrDetails,
            StepState::Done,
            Some(total),
            Some(total),
        )
        .await?;

    progress
        .step(ScanStepKey::Context, StepState::Running, None, None)
        .await?;
    let owned = owned_changes(client, input, &details.found, &mut notes).await?;
    let (mentions, mention_notes, unchecked_items) = mentions::collect(
        client,
        input.rules,
        &searchable,
        &candidates,
        &details.found,
        &owned,
    )
    .await?;
    notes.extend(mention_notes);
    candidates.issues.retain(|_, issue| issue.ranked());
    let with_milestone: BTreeSet<RepoRef> = details
        .found
        .iter()
        .filter(|(_, d)| d.milestone.is_some())
        .map(|(item, _)| item.repo.clone())
        .chain(
            candidates
                .issues
                .iter()
                .filter(|(_, i)| i.milestone.is_some())
                .map(|(item, _)| item.repo.clone()),
        )
        .collect();
    let milestones = milestone_contexts(client, &with_milestone, &mut notes).await?;
    // The settings' rows and the issues' repositories (their privacy: issue search does not tell it).
    let mut looked_up: Vec<RepoRef> = Vec::new();
    for repo in settings
        .repositories
        .iter()
        .map(|row| &row.repo)
        .chain(candidates.issues.keys().map(|item| &item.repo))
    {
        if !looked_up.iter().any(|seen| same_repo(seen, repo)) {
            looked_up.push(repo.clone());
        }
    }
    let reach = client.reach(&looked_up, &settings.enumerate_owners).await?;
    notes.extend(reach.notes);
    let rows: Vec<&RepoRef> = settings.repositories.iter().map(|row| &row.repo).collect();
    let unreachable: Vec<String> = reach
        .invisible_repositories
        .iter()
        .filter(|repo| rows.iter().any(|row| same_repo(row, repo)))
        .map(repo_name)
        .chain(reach.invisible_owners.iter().cloned())
        .collect();
    progress.log(format!(
        "context: {} ownership verdicts, {} milestone lists, {} not visible",
        owned.verdicts.len(),
        milestones.len(),
        unreachable.len()
    ));
    progress
        .step(ScanStepKey::Context, StepState::Done, None, None)
        .await?;

    progress
        .step(ScanStepKey::History, StepState::Running, None, None)
        .await?;
    let (first, start) = history::window_start(progress.now(), input.zone);
    let configured: Vec<RepoRef> = settings
        .repositories
        .iter()
        .map(|row| row.repo.clone())
        .collect();
    let refused_from = history::Refused {
        repos: &configured,
        owners: &settings.enumerate_owners,
    };
    let (history, history_notes) =
        history::collect(client, input.rules, &logins, &refused_from, first, start).await?;
    notes.extend(history_notes);
    progress.log(format!("history: {} events since {first}", history.len()));
    let events = count(history.len());
    progress
        .step(
            ScanStepKey::History,
            StepState::Done,
            Some(events),
            Some(events),
        )
        .await?;

    let (todos, todos_state) = read_todos(input, progress, &mut notes).await?;

    let private: Vec<&RepoRef> = details
        .found
        .iter()
        .filter(|(_, d)| d.base_repository.as_ref().is_some_and(|r| r.is_private))
        .map(|(item, _)| &item.repo)
        .chain(&reach.private_repositories)
        .collect();
    let mut pull_requests: Vec<_> = details
        .found
        .iter()
        .filter_map(|(item, detail)| {
            let candidate = candidates.pull_requests.get(item)?;
            Some(facts::pr(
                input.rules,
                item,
                detail,
                PrContext {
                    candidate,
                    blockers: listing.blockers(item),
                    owned: owned
                        .verdicts
                        .get(item)
                        .copied()
                        .unwrap_or(OwnedChange::NoPolicy),
                    milestones: milestones.get(&item.repo).cloned().unwrap_or_default(),
                },
            ))
        })
        .collect();
    recheck_merge_states(
        client,
        input.rules,
        &mut pull_requests,
        details_read,
        progress,
    )
    .await?;
    let issues: Vec<_> = candidates
        .issues
        .iter()
        .map(|(item, candidate)| {
            facts::issue(
                item,
                candidate,
                milestones.get(&item.repo).cloned().unwrap_or_default(),
                private.iter().any(|repo| same_repo(repo, &item.repo)),
            )
        })
        .collect();
    let checked = KindCounts {
        pr: count(pull_requests.len()),
        issue: count(issues.len()),
        todo: count(todos.len()),
    };
    Ok(Snapshot {
        mention_coverage: MentionCoverage {
            unchecked_items,
            unchecked_logins: found.unsearchable,
            timed_out_logins: found.timed_out_mention_logins,
        },
        mentions,
        version: SNAPSHOT_VERSION,
        own_login: input.own_login.to_owned(),
        settings: settings.clone(),
        scanned_at: progress.now(),
        pull_requests,
        issues,
        todos,
        todos_state,
        history,
        notes,
        unreachable,
        checked,
        merge_states_recorded: true,
        prs_reread_at: None,
        todos_hidden: false,
        snoozes: Vec::new(),
    })
}

/// Reads once more the merge state of the user's pull requests GitHub had not computed at the details read (it
/// computes it in the background; plan §A.2 Q17), at most [`MERGE_RECHECK_LIMIT`], one pacing interval after that
/// read at the earliest. What stays unknown is counted where Ready to merge is shown.
async fn recheck_merge_states<T: TokenSource>(
    client: &WorkClient<T>,
    rules: &ScoreRules,
    pulls: &mut [StoredPr],
    details_read: Instant,
    progress: &mut Progress,
) -> Result<(), RunError> {
    let item = |pr: &StoredPr| ItemRef {
        repo: pr.facts.repo.clone(),
        number: pr.facts.number,
    };
    let pending: Vec<ItemRef> = pulls
        .iter()
        .filter(|pr| score::merge_state_pending(rules, &pr.facts))
        .take(MERGE_RECHECK_LIMIT)
        .map(item)
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    sleep_until(details_read + client.search_interval).await;
    let states = client.merge_states(&pending).await?;
    for pr in pulls.iter_mut() {
        if let Some(state) = states.get(&item(pr)) {
            pr.facts.merge_state = facts::merge_state(state);
        }
    }
    let unknown = pulls
        .iter()
        .filter(|pr| score::merge_state_pending(rules, &pr.facts))
        .count();
    progress.log(format!(
        "merge states: {} read again, {unknown} not computed by GitHub yet",
        pending.len()
    ));
    Ok(())
}

/// Ownership verdicts of the pull requests in repositories with an ownership policy (Q3, Q4); a policy or file
/// list that cannot be read leaves the verdict unknown (eligibility to confirm) with a note.
async fn owned_changes<T: TokenSource>(
    client: &WorkClient<T>,
    input: &ScanInput<'_>,
    details: &BTreeMap<ItemRef, PullDetail>,
    notes: &mut Vec<ScanNote>,
) -> Result<owned::Ownership, WorkError> {
    let mut ownership = owned::Ownership::new(input.settings);
    let rows: Vec<(RepoRef, PolicyRef, Vec<ItemRef>)> = input
        .settings
        .repositories
        .iter()
        .filter_map(|row| {
            let policy = row.ownership_policy.clone()?;
            let pulls: Vec<ItemRef> = details
                .keys()
                .filter(|item| same_repo(&item.repo, &row.repo))
                .cloned()
                .collect();
            (!pulls.is_empty()).then(|| (row.repo.clone(), policy, pulls))
        })
        .collect();
    for (repo, policy, pulls) in rows {
        let where_ = format!("{}:{}", repo_name(&policy.repo), policy.path);
        let paths = ownership
            .paths(client, input.rules, &repo)
            .await?
            .map(<[String]>::to_vec);
        let Some(paths) = paths else {
            notes.push(note(format!(
                "The ownership policy {where_} could not be read; pull requests of {} are ranked with \
                 ownership unverified",
                repo_name(&repo)
            )));
            ownership
                .verdicts
                .extend(pulls.into_iter().map(|pr| (pr, OwnedChange::Unknown)));
            continue;
        };
        let mut reads = Vec::with_capacity(pulls.len());
        for pr in &pulls {
            reads.push(client.pull_files(pr));
        }
        let files = join_all(reads).await;
        for (pr, files) in pulls.into_iter().zip(files) {
            let verdict = match files {
                Ok(files) => owned::verdict(&files, &paths),
                Err(error) if error.ends_refresh() => return Err(error),
                Err(error) => {
                    notes.push(ScanNote {
                        text: format!(
                            "The changed files of {pr} could not be read ({error}); its ownership is unverified"
                        ),
                        item: pr.pr_key(),
                    });
                    OwnedChange::Unknown
                }
            };
            ownership.verdicts.insert(pr, verdict);
        }
    }
    Ok(ownership)
}

/// Release tags and milestone titles of each repository (Q8); a list that cannot be read stays unknown.
async fn milestone_contexts<T: TokenSource>(
    client: &WorkClient<T>,
    repos: &BTreeSet<RepoRef>,
    notes: &mut Vec<ScanNote>,
) -> Result<BTreeMap<RepoRef, MilestoneContext>, WorkError> {
    let mut pending = Vec::with_capacity(repos.len());
    for repo in repos {
        pending.push(async move { tokio::join!(client.releases(repo), client.milestones(repo)) });
    }
    let reads = join_all(pending).await;
    let mut contexts = BTreeMap::new();
    for (repo, (releases, milestones)) in repos.iter().zip(reads) {
        let mut unread = |what: &str, error: &WorkError| {
            notes.push(note(format!(
                "The {what} of {} could not be read ({error}); milestone bonuses there are left to the LLM",
                repo_name(repo)
            )));
        };
        let release_tags = match releases {
            Ok(releases) => Some(
                releases
                    .into_iter()
                    .filter(|r| !r.draft && !r.prerelease)
                    .map(|r| facts::capped(&r.tag_name, 100))
                    .collect(),
            ),
            Err(error) if error.ends_refresh() => return Err(error),
            Err(error) => {
                unread("releases", &error);
                None
            }
        };
        let milestone_titles = match milestones {
            Ok(milestones) => Some(
                milestones
                    .into_iter()
                    .map(|m| facts::capped(&m.title, 100))
                    .collect(),
            ),
            Err(error) if error.ends_refresh() => return Err(error),
            Err(error) => {
                unread("milestones", &error);
                None
            }
        };
        contexts.insert(
            repo.clone(),
            MilestoneContext {
                release_tags,
                milestone_titles,
            },
        );
    }
    Ok(contexts)
}

/// The MemCan TODOs (plan §G): read when shown and configured; an unavailable MemCan keeps the previous
/// snapshot's rows, marked not refreshed. Never fails the refresh.
async fn read_todos(
    input: &ScanInput<'_>,
    progress: &mut Progress,
    notes: &mut Vec<ScanNote>,
) -> Result<(Vec<StoredTodo>, TodosState), Ended> {
    let skipped = |status| TodosState {
        status,
        read_at: None,
        failed_at: None,
    };
    if !input.settings.include_todos {
        progress
            .step(ScanStepKey::Todos, StepState::Skipped, None, None)
            .await?;
        return Ok((Vec::new(), skipped(TodosStatus::Off)));
    }
    if !input.memcan {
        progress
            .step(ScanStepKey::Todos, StepState::Skipped, None, None)
            .await?;
        return Ok((Vec::new(), skipped(TodosStatus::NotConfigured)));
    }
    progress
        .step(ScanStepKey::Todos, StepState::Running, None, None)
        .await?;
    let projects = memcan_projects(input.settings);
    let now = progress.now();
    let previous: HashMap<_, _> = input
        .previous
        .map(|snapshot| {
            snapshot
                .todos
                .iter()
                .map(|t| (t.facts.id, (t.change_key.clone(), t.facts.changed_at)))
                .collect()
        })
        .unwrap_or_default();
    let read = todos::read(&progress.state().config.memcan, &projects).await;
    match read {
        TodosRead::Read { todos, notes: read } => {
            notes.extend(read.into_iter().map(note));
            let rows: Vec<StoredTodo> = todos
                .into_iter()
                .map(|todo| {
                    let before = previous.get(&todo.id).map(|(key, at)| (key.as_str(), *at));
                    StoredTodo {
                        facts: TodoFacts {
                            id: todo.id,
                            title: todo.title.clone(),
                            status: todo.status,
                            priority: Some(todo.priority.clone()).filter(|p| !p.is_empty()),
                            changed_at: todo.changed_at(before, now),
                        },
                        project: todo.project.clone(),
                        description: todo.description.clone(),
                        owner: todo.owner.clone(),
                        blocked_by: todo.blocked_by.len(),
                        change_key: todo.change_key.clone(),
                    }
                })
                .collect();
            let read_count = count(rows.len());
            progress.log(format!(
                "TODOs: {read_count} read from {} MemCan projects",
                projects.len()
            ));
            progress
                .step(
                    ScanStepKey::Todos,
                    StepState::Done,
                    Some(read_count),
                    Some(read_count),
                )
                .await?;
            let state = TodosState {
                status: TodosStatus::Read,
                read_at: Some(now),
                failed_at: None,
            };
            Ok((rows, state))
        }
        TodosRead::Unavailable { note: text } => {
            notes.push(note(text));
            progress.log("TODOs: MemCan unavailable, previous rows kept");
            progress
                .step(ScanStepKey::Todos, StepState::Failed, None, None)
                .await?;
            let (rows, read_at) = kept_todos(input.previous);
            let state = TodosState {
                status: TodosStatus::NotRefreshed,
                read_at,
                failed_at: Some(now),
            };
            Ok((rows, state))
        }
    }
}

/// The previous snapshot's TODO rows and when they were read.
fn kept_todos(previous: Option<&Snapshot>) -> (Vec<StoredTodo>, Option<Timestamp>) {
    previous.map_or((Vec::new(), None), |snapshot| {
        (snapshot.todos.clone(), snapshot.todos_state.read_at)
    })
}
