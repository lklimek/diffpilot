//! Bounded conversation orchestration for the separate mentions strip.

use super::owned;
use crate::dashboard::score::{OwnedChange, ScoreRules};
use crate::github::work::{
    Candidates, IssueRelation, ItemRef, PullDetail, TokenSource, WorkClient, WorkError, incomplete,
};
use contracts::api::dashboard::{DashboardSettings, GithubKind, Mention, ScanNote};
use std::collections::BTreeMap;
use std::time::Duration;

use crate::dashboard::mentions::{body, unanswered};

/// At most 40 conversation reads and 80 ownership reads; four HTTP calls in flight.
const MAX_DISCUSSIONS: usize = 40;
/// Mention work cannot consume the five-minute ranking refresh budget.
const MENTION_LIMIT: Duration = Duration::from_secs(30);

/// Bounded mention collection; an exhausted budget leaves a visible note, never a failed ranking.
pub(crate) async fn collect<T: TokenSource>(
    client: &WorkClient<T>,
    rules: &ScoreRules,
    logins: &[String],
    candidates: &Candidates,
    details: &BTreeMap<ItemRef, PullDetail>,
    settings: &DashboardSettings,
    owned_changes: &BTreeMap<ItemRef, OwnedChange>,
) -> Result<(Vec<Mention>, Vec<ScanNote>), WorkError> {
    match tokio::time::timeout(
        MENTION_LIMIT,
        collect_window(
            client,
            rules,
            logins,
            candidates,
            details,
            settings,
            owned_changes,
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Ok((
            Vec::new(),
            vec![ScanNote {
                text: "Mention reads exceeded 30 seconds; mentions are omitted for this refresh"
                    .into(),
                item: None,
            }],
        )),
    }
}

async fn collect_window<T: TokenSource>(
    client: &WorkClient<T>,
    rules: &ScoreRules,
    logins: &[String],
    candidates: &Candidates,
    details: &BTreeMap<ItemRef, PullDetail>,
    settings: &DashboardSettings,
    owned_changes: &BTreeMap<ItemRef, OwnedChange>,
) -> Result<(Vec<Mention>, Vec<ScanNote>), WorkError> {
    let mut found = Vec::new();
    let mut notes = Vec::new();
    let pulls: Vec<_> = candidates
        .pull_requests
        .iter()
        .filter(|(_, p)| p.mentioned())
        .map(|(item, _)| item.clone())
        .collect();
    let issues: Vec<_> = candidates
        .issues
        .iter()
        .filter(|(_, i)| {
            i.relations.contains(&IssueRelation::Mentioned)
                && !i
                    .author
                    .as_deref()
                    .is_some_and(|a| rules.is_skipped_author(a))
        })
        .map(|(item, _)| item.clone())
        .collect();
    if pulls.len() + issues.len() > MAX_DISCUSSIONS {
        notes.push(ScanNote {
            text: "Mention reads are limited to 40 conversations per refresh; \
                   remaining conversations are omitted"
                .into(),
            item: None,
        });
    }
    let pulls = &pulls[..pulls.len().min(MAX_DISCUSSIONS)];
    let issues = &issues[..issues.len().min(MAX_DISCUSSIONS - pulls.len())];
    let missing: Vec<_> = issues
        .iter()
        .filter(|item| candidates.issues[*item].discussion.is_none())
        .cloned()
        .collect();
    let (pr_reads, issue_reads) = tokio::try_join!(
        client.pull_discussions(pulls),
        client.issue_discussions(&missing)
    )?;
    notes.extend(pr_reads.notes);
    notes.extend(issue_reads.notes);
    for (item, discussion) in pr_reads.found {
        let author = discussion
            .issue
            .author
            .as_ref()
            .and_then(|a| a.login.as_deref())
            .unwrap_or("ghost");
        if discussion.issue.state != "OPEN"
            || rules.is_skipped_author(author)
            || (discussion.is_draft && !rules.is_scanned(author))
        {
            continue;
        }
        let Some(mut posts) = discussion.posts() else {
            notes.push(ScanNote {
                text: format!("The discussion of {item} is incomplete; its mentions are omitted"),
                item: item.pr_key(),
            });
            continue;
        };
        let directed = details.get(&item).is_some_and(|d| {
            rules.is_scanned(author)
                || d.assignees.nodes.iter().any(|a| rules.is_scanned(&a.login))
                || d.review_requests
                    .nodes
                    .iter()
                    .filter_map(|r| r.requested_reviewer.as_ref())
                    .any(|r| r.login.as_deref().is_some_and(|a| rules.is_scanned(a)))
        });
        if !directed {
            if matches!(
                owned_changes.get(&item),
                Some(OwnedChange::None | OwnedChange::Minor)
            ) {
                continue;
            }
            if !details.contains_key(&item)
                && let Some(policy) = settings
                    .repositories
                    .iter()
                    .find(|r| {
                        r.repo.owner.eq_ignore_ascii_case(&item.repo.owner)
                            && r.repo.name.eq_ignore_ascii_case(&item.repo.name)
                    })
                    .and_then(|r| r.ownership_policy.as_ref())
            {
                let read = async {
                    let text = client.file_contents(&policy.repo, &policy.path).await?;
                    let files = client.mention_files(&item).await?;
                    Ok::<_, WorkError>((text, files))
                }
                .await;
                match read {
                    Ok((text, files)) => match owned::owned_paths(rules, &text) {
                        Ok(paths) if files.len() < 100 => {
                            if matches!(
                                owned::verdict(&files, &paths),
                                OwnedChange::None | OwnedChange::Minor
                            ) {
                                continue;
                            }
                        }
                        _ => {
                            notes.push(ScanNote {
                                text: format!(
                                    "Ownership of {item} could not be established; \
                                               its mentions are omitted"
                                ),
                                item: item.pr_key(),
                            });
                            continue;
                        }
                    },
                    Err(e) if e.ends_refresh() => return Err(e),
                    Err(_) => {
                        notes.push(ScanNote {
                            text: format!(
                                "Ownership of {item} could not be read; its mentions are omitted"
                            ),
                            item: item.pr_key(),
                        });
                        continue;
                    }
                }
            }
        }
        posts.push(body(
            discussion.issue.author,
            &discussion.issue.body,
            discussion.issue.created_at,
        ));
        found.extend(unanswered(
            rules,
            logins,
            &item,
            GithubKind::Pr,
            &candidates.pull_requests[&item].title,
            &posts,
        ));
    }
    for item in issues {
        let discussion = candidates.issues[item]
            .discussion
            .as_ref()
            .or_else(|| issue_reads.found.get(item));
        let Some(discussion) = discussion else {
            continue;
        };
        if discussion.state != "OPEN"
            || discussion
                .author
                .as_ref()
                .and_then(|a| a.login.as_deref())
                .is_some_and(|a| rules.is_skipped_author(a))
        {
            continue;
        }
        if incomplete(&discussion.comments) {
            notes.push(ScanNote {
                text: format!("The discussion of {item} is incomplete; its mentions are omitted"),
                item: item.issue_key(),
            });
            continue;
        }
        let mut posts = discussion.comments.nodes.clone();
        posts.push(body(
            discussion.author.clone(),
            &discussion.body,
            discussion.created_at,
        ));
        found.extend(unanswered(
            rules,
            logins,
            item,
            GithubKind::Issue,
            &candidates.issues[item].title,
            &posts,
        ));
    }
    found.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    found.dedup_by(|a, b| a.id == b.id);
    Ok((found, notes))
}
