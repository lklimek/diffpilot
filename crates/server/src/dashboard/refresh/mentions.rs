//! Bounded conversation orchestration for the separate mentions strip.

use super::{facts, owned};
use crate::dashboard::mentions::{body, unanswered};
use crate::dashboard::score::{OwnedChange, Relation, ScoreRules, pull_eligibility};
use crate::github::work::{
    Candidates, ItemRef, PullDetail, TokenSource, WorkClient, WorkError, incomplete,
};
use contracts::api::dashboard::{GithubKind, Mention, ScanNote};
use futures::{StreamExt, stream::FuturesUnordered};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::time::{Instant, timeout_at};

/// At most 40 uncached conversations; the client limits HTTP calls to four in flight.
const MAX_DISCUSSIONS: usize = 40;
const MENTION_LIMIT: Duration = Duration::from_secs(30);

/// Recency across both kinds determines the read budget; completed results survive the deadline.
pub(crate) async fn collect<T: TokenSource>(
    client: &WorkClient<T>,
    rules: &ScoreRules,
    logins: &[String],
    candidates: &Candidates,
    details: &BTreeMap<ItemRef, PullDetail>,
    ownership: &owned::Ownership,
) -> Result<(Vec<Mention>, Vec<ScanNote>, u32), WorkError> {
    let mut wanted: Vec<_> = candidates
        .pull_requests
        .iter()
        .filter(|(_, pr)| pr.mentioned())
        .map(|(item, pr)| (item, GithubKind::Pr, pr.updated_at))
        .chain(
            candidates
                .issues
                .iter()
                .filter(|(_, i)| {
                    i.mention_candidate
                        && !i
                            .author
                            .as_deref()
                            .is_some_and(|a| rules.is_skipped_author(a))
                })
                .map(|(item, i)| (item, GithubKind::Issue, Some(i.updated_at))),
        )
        .collect();
    wanted.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(b.0)));
    let total = wanted.len();
    wanted.truncate(MAX_DISCUSSIONS);
    let deadline = Instant::now() + MENTION_LIMIT;
    let mut pending: FuturesUnordered<_> = wanted
        .into_iter()
        .map(|(item, kind, _)| {
            read_one(
                client, rules, logins, candidates, details, ownership, item, kind,
            )
        })
        .collect();
    let mut unchecked = total;
    let mut found = Vec::new();
    let mut notes = Vec::new();
    while let Ok(Some(result)) = timeout_at(deadline, pending.next()).await {
        if let Some((mentions, note)) = result? {
            unchecked -= 1;
            found.extend(mentions);
            notes.extend(note);
        }
    }
    found.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    found.dedup_by(|a, b| a.id == b.id);
    Ok((found, notes, u32::try_from(unchecked).unwrap_or(u32::MAX)))
}

#[expect(
    clippy::too_many_arguments,
    reason = "one refresh context and one item; no additional state"
)]
async fn read_one<T: TokenSource>(
    client: &WorkClient<T>,
    rules: &ScoreRules,
    logins: &[String],
    candidates: &Candidates,
    details: &BTreeMap<ItemRef, PullDetail>,
    ownership: &owned::Ownership,
    item: &ItemRef,
    kind: GithubKind,
) -> Result<Option<(Vec<Mention>, Option<ScanNote>)>, WorkError> {
    let empty = || Some((Vec::new(), None));
    let mut note = None;
    let (mut posts, discussion, partly_read, title) = match kind {
        GithubKind::Pr => {
            let Some(pr) = client
                .pull_discussions(std::slice::from_ref(item))
                .await?
                .found
                .remove(item)
            else {
                return Ok(None);
            };
            let author = pr.issue.author.as_ref().and_then(|a| a.login.as_deref());
            let mut owned = ownership
                .verdicts
                .get(item)
                .copied()
                .unwrap_or(OwnedChange::NoPolicy);
            let (assigned, pending) = if let Some(detail) = details.get(item) {
                let facts = facts::pr(
                    rules,
                    item,
                    detail,
                    facts::PrContext {
                        candidate: &candidates.pull_requests[item],
                        blockers: None,
                        owned,
                        milestones: Default::default(),
                    },
                )
                .facts;
                (
                    facts.relations.contains(&Relation::Assigned),
                    facts.review_pending_for_us,
                )
            } else {
                (false, false)
            };
            if pr.issue.state != "OPEN"
                || pull_eligibility(rules, author, pr.is_draft, assigned, pending, owned)
                    .excluded_by
                    .is_some()
            {
                return Ok(empty());
            }
            if !ownership.verdicts.contains_key(item) && ownership.has_policy(&item.repo) {
                owned = match ownership.paths(client, rules, &item.repo).await? {
                    Some(paths) => match client.mention_files(item).await {
                        Ok(files) if files.len() < 100 => owned::verdict(&files, paths),
                        Err(error) if error.ends_refresh() => return Err(error),
                        _ => OwnedChange::Unknown,
                    },
                    None => OwnedChange::Unknown,
                };
            }
            if pull_eligibility(rules, author, pr.is_draft, assigned, pending, owned)
                .excluded_by
                .is_some()
            {
                return Ok(empty());
            }
            if owned == OwnedChange::Unknown {
                note = Some(ScanNote {
                    text: format!("Ownership of {item} is unverified; its mentions are included"),
                    item: item.pr_key(),
                });
            }
            let partly_read = pr.partly_read();
            (
                pr.posts(),
                pr.issue,
                partly_read,
                &candidates.pull_requests[item].title,
            )
        }
        GithubKind::Issue => {
            let candidate = &candidates.issues[item];
            let Some(discussion) = client
                .issue_discussions(std::slice::from_ref(item))
                .await?
                .found
                .remove(item)
            else {
                return Ok(None);
            };
            if discussion.state != "OPEN"
                || discussion
                    .author
                    .as_ref()
                    .and_then(|a| a.login.as_deref())
                    .is_some_and(|a| rules.is_skipped_author(a))
            {
                return Ok(empty());
            }
            let partly_read = incomplete(&discussion.comments);
            (
                discussion.comments.nodes.clone(),
                discussion,
                partly_read,
                &candidate.title,
            )
        }
    };
    posts.push(body(
        discussion.author,
        &discussion.body,
        discussion.created_at,
    ));
    let mut found = unanswered(rules, logins, item, kind, title, &posts);
    for mention in &mut found {
        mention.partly_read = partly_read;
    }
    Ok(Some((found, note)))
}
