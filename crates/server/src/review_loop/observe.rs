//! From one GitHub read to items (plan §2.1 I1–I3), rounds (§2.6) and the PR snapshot the views use. Pure. The
//! bot rule (RQ17): GitHub `Bot` type, a `[bot]` login or the reader's `known_bots`; thepastaclaw's markers are read
//! from bot-authored text only, so a human cannot claim another item's key.

use std::collections::{BTreeMap, HashSet};

use contracts::api::review_loop::{
    AuthorKind, CommentItemKind, ItemGithubFacts, ItemLocation, Written,
};
use contracts::api::tour::CiStatus;
use contracts::common::LineRange;
use contracts::ids::CommitSha;
use contracts::time::Timestamp;
use db::repo::review_loop::ObservedItem;
use serde::{Deserialize, Serialize};

use super::markers::{self, GateState};
use crate::dashboard::score::is_bot;
use crate::github::review_loop::{Author, PrRead, Review};
use crate::github::threads::github_web_url;

/// The bot rule of one reader: the dashboard's (`dashboard::score::is_bot`).
#[derive(Clone, Debug, Default)]
pub struct BotRule {
    known: Vec<String>,
}

impl BotRule {
    /// The rule with these known bots (Settings `known_bots`, case-insensitive).
    pub fn new(known: &[String]) -> Self {
        Self {
            known: known.iter().map(|login| login.to_lowercase()).collect(),
        }
    }

    /// The kind of `author`; a deleted account counts as a person.
    pub fn kind(&self, author: Option<&Author>) -> AuthorKind {
        match author {
            Some(author) if is_bot(&author.login, author.typename == "Bot", &self.known) => {
                AuthorKind::Bot
            }
            _ => AuthorKind::Human,
        }
    }
}

/// A bot round (plan §2.6, spike S13-1 R1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundFact {
    /// 1-based.
    pub number: u32,
    /// The round review's node id.
    pub review_id: String,
    /// Its GitHub link.
    pub url: String,
    /// Head validated.
    pub head: String,
    /// Submitted.
    pub finished_at: Timestamp,
    /// The review's state.
    pub verdict: String,
}

/// A bot whose latest decisive review requests changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotChangeRequest {
    /// The bot's login.
    pub login: String,
    /// The review's node id.
    pub review_id: String,
    /// Its GitHub link.
    pub url: String,
}

/// The gate comment's state (a status, not an item).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateFact {
    /// A review of the bot runs.
    InProgress,
    /// A review of the bot is queued.
    Queued,
    /// The bot's review finished.
    Complete,
}

/// What the server keeps of the last successful read (`comment_prs.github_snapshot`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Title.
    pub title: String,
    /// Description as its author wrote it (untrusted); none when empty.
    #[serde(default)]
    pub description: Option<String>,
    /// GitHub link.
    pub url: String,
    /// `OPEN`, `CLOSED`, `MERGED`.
    pub state: String,
    /// Draft.
    pub draft: bool,
    /// Base branch.
    pub base: String,
    /// Head.
    pub head: String,
    /// Base commit; none in a snapshot stored before it was kept.
    #[serde(default)]
    pub base_sha: Option<String>,
    /// Rounds, oldest first.
    pub rounds: Vec<RoundFact>,
    /// The gate comment's state and the head (8 hex) it names.
    pub gate: Option<(GateFact, Option<String>)>,
    /// Bots whose latest decisive review requests changes.
    pub bot_change_requests: Vec<BotChangeRequest>,
    /// Authors (any) whose latest decisive review requests changes.
    pub changes_requested_by: Vec<String>,
    /// CI of the head; `None` when not read.
    pub ci: Option<CiStatus>,
    /// Names of the check runs and statuses pending at the head when [`Snapshot::ci`] was read.
    #[serde(default)]
    pub pending_checks: Vec<String>,
    /// Threads, reviews and comments read.
    pub counts: (u32, u32, u32),
    /// Every page was read.
    pub complete: bool,
    /// Unresolved threads on GitHub.
    pub unresolved_threads: u32,
    /// Thread node id by item key.
    pub thread_ids: BTreeMap<String, String>,
    /// Review node id by key of a review-body item.
    pub review_ids: BTreeMap<String, String>,
    /// Where GitHub says each item was written, by item key; empty in a snapshot stored before it was kept.
    #[serde(default)]
    pub written: BTreeMap<String, Written>,
}

/// An observed item with what the server needs beside the stored fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observed {
    /// The item as stored (`outside` and `edited_after_decision` set later).
    pub item: ObservedItem,
    /// Its thread: node id, resolved, resolved by.
    pub thread: Option<(String, bool, Option<String>)>,
}

fn parse_time(text: &str) -> Option<Timestamp> {
    text.parse().ok()
}

fn login(author: Option<&Author>) -> String {
    author.map_or_else(|| "ghost".to_owned(), |author| author.login.clone())
}

/// The round number an entry written at `at` (or in review `review`) belongs to: the round of that review, else
/// the rounds finished before it; `None` before the first round.
fn round_of(rounds: &[RoundFact], review: Option<&str>, at: Option<Timestamp>) -> Option<u32> {
    if let Some(round) = review.and_then(|id| rounds.iter().find(|round| round.review_id == id)) {
        return Some(round.number);
    }
    let at = at?;
    let before = rounds
        .iter()
        .filter(|round| round.finished_at <= at)
        .count();
    u32::try_from(before).ok().filter(|count| *count > 0)
}

/// Where an entry of review `review` (if any) written at `at` was written: a round's own review names that round with
/// its head and end, any other review its commit with `at`; `None` without a time.
fn written_on(
    rounds: &[RoundFact],
    reviews: &[Review],
    review: Option<&str>,
    at: Option<Timestamp>,
) -> Option<Written> {
    if let Some(round) = review.and_then(|id| rounds.iter().find(|round| round.review_id == id)) {
        return Some(Written {
            round: Some(round.number),
            commit: Some(CommitSha(round.head.clone())),
            at: round.finished_at,
        });
    }
    let commit = review
        .and_then(|id| reviews.iter().find(|candidate| candidate.id == id))
        .and_then(|review| review.commit.as_ref())
        .map(|commit| CommitSha(commit.oid.clone()));
    Some(Written {
        round: None,
        commit,
        at: at?,
    })
}

fn facts(replies: u32) -> ItemGithubFacts {
    ItemGithubFacts {
        resolved: None,
        resolved_by: None,
        outdated: None,
        bot_status: None,
        edited_after_decision: false,
        replies,
        marker: None,
    }
}

fn rounds(read: &PrRead, bots: &BotRule) -> Vec<RoundFact> {
    let mut rounds: Vec<(Timestamp, &Review, String)> = read
        .reviews
        .iter()
        .filter(|review| bots.kind(review.author.as_ref()) == AuthorKind::Bot)
        .filter_map(|review| {
            let sha = markers::round_sha(&review.body)?;
            let at = parse_time(review.submitted_at.as_deref()?)?;
            Some((at, review, sha))
        })
        .collect();
    rounds.sort_by_key(|(at, _, _)| *at);
    rounds
        .into_iter()
        .enumerate()
        .map(|(index, (at, review, sha))| RoundFact {
            number: u32::try_from(index + 1).unwrap_or(u32::MAX),
            review_id: review.id.clone(),
            url: github_web_url(&review.url).unwrap_or_else(|| read.pr.url.clone()),
            head: sha,
            finished_at: at,
            verdict: review.state.clone(),
        })
        .collect()
}

/// Items, rounds and the snapshot of one read (CI added by the caller).
pub fn observe(read: &PrRead, bots: &BotRule) -> (Vec<Observed>, Snapshot) {
    let head = CommitSha(read.pr.head_ref_oid.clone());
    let rounds = rounds(read, bots);
    let mut observed = Vec::new();
    let mut keys = HashSet::new();
    let mut thread_ids = BTreeMap::new();
    let mut review_ids = BTreeMap::new();
    let mut written = BTreeMap::new();
    let mut unresolved = 0u32;

    for thread in &read.threads {
        let Some(first) = thread.comments.nodes.first() else {
            continue;
        };
        if first.state.as_deref() == Some("PENDING") {
            continue;
        }
        if !thread.is_resolved {
            unresolved += 1;
        }
        let author_kind = bots.kind(first.author.as_ref());
        let bot = author_kind == AuthorKind::Bot;
        if bot && markers::is_thread_answer(&first.body) {
            continue; // S13-1 M5: a bot's thread answer is a reply, never an item
        }
        let text = markers::normalise(&first.body);
        let marker = bot.then(|| markers::finding_marker(&first.body)).flatten();
        let dedupe = marker
            .and_then(|marker| marker.dedupe)
            .filter(|key| !keys.contains(key));
        let stable_key = dedupe.clone().unwrap_or_else(|| thread.id.clone());
        keys.insert(stable_key.clone());
        let heading = bot.then(|| markers::heading_of(&text)).flatten();
        let head_side = thread.diff_side.as_deref() != Some("LEFT");
        let lines = thread
            .line
            .filter(|_| head_side && !thread.is_outdated)
            .map(|line| LineRange {
                start_line: thread
                    .start_line
                    .filter(|start| *start <= line)
                    .unwrap_or(line),
                end_line: line,
            });
        let resolved_by = thread.resolved_by.as_ref().map(|by| by.login.clone());
        let mut github = facts(thread.comments.total_count.saturating_sub(1));
        github.resolved = Some(thread.is_resolved);
        github.resolved_by = resolved_by.clone();
        github.outdated = Some(thread.is_outdated);
        github.bot_status = bot.then(|| markers::bot_status(&first.body)).flatten();
        github.marker = dedupe;
        thread_ids.insert(stable_key.clone(), thread.id.clone());
        let review_id = first.pull_request_review.as_ref().map(|r| r.id.as_str());
        let at = parse_time(&first.created_at);
        if let Some(on) = written_on(&rounds, &read.reviews, review_id, at) {
            written.insert(stable_key.clone(), on);
        }
        observed.push(Observed {
            item: ObservedItem {
                kind: CommentItemKind::Thread,
                stable_key,
                author: login(first.author.as_ref()),
                author_kind,
                url: github_web_url(&first.url),
                location: Some(ItemLocation {
                    path: thread.path.clone(),
                    lines,
                }),
                title: heading
                    .as_ref()
                    .map_or_else(|| markers::title_of(&text), |(_, title)| title.clone()),
                body_hash: markers::body_hash(&text),
                body: text,
                head: head.clone(),
                round: round_of(&rounds, review_id, at),
                bot_severity: heading.map(|(severity, _)| severity),
                github,
                outside: None,
            },
            thread: Some((thread.id.clone(), thread.is_resolved, resolved_by)),
        });
    }

    let decisive = |review: &&Review| {
        matches!(
            review.state.as_str(),
            "CHANGES_REQUESTED" | "APPROVED" | "DISMISSED"
        )
    };
    let mut latest: BTreeMap<String, &Review> = BTreeMap::new();
    let mut latest_any: BTreeMap<String, &Review> = BTreeMap::new();
    for review in read.reviews.iter().filter(|r| r.submitted_at.is_some()) {
        let key = login(review.author.as_ref()).to_lowercase();
        if decisive(&review) {
            latest.insert(key.clone(), review);
        }
        if !review.body.trim().is_empty() {
            latest_any.insert(key, review);
        }
    }
    let changes_requested_by: Vec<String> = latest
        .values()
        .filter(|review| review.state == "CHANGES_REQUESTED")
        .map(|review| login(review.author.as_ref()))
        .collect();
    let bot_change_requests = latest
        .values()
        .filter(|review| {
            review.state == "CHANGES_REQUESTED"
                && bots.kind(review.author.as_ref()) == AuthorKind::Bot
        })
        .map(|review| BotChangeRequest {
            login: login(review.author.as_ref()),
            review_id: review.id.clone(),
            url: github_web_url(&review.url).unwrap_or_else(|| read.pr.url.clone()),
        })
        .collect();

    for review in &read.reviews {
        if review.submitted_at.is_none() || review.state == "DISMISSED" {
            continue;
        }
        let author_kind = bots.kind(review.author.as_ref());
        let at = review.submitted_at.as_deref().and_then(parse_time);
        let round = round_of(&rounds, Some(&review.id), at);
        let on = written_on(&rounds, &read.reviews, Some(&review.id), at);
        let url = github_web_url(&review.url);
        let mut push = |ordinal: usize, title: String, text: String, severity, location| {
            let stable_key = format!("review:{}:{ordinal}", review.id);
            review_ids.insert(stable_key.clone(), review.id.clone());
            if let Some(on) = &on {
                written.insert(stable_key.clone(), on.clone());
            }
            observed.push(Observed {
                item: ObservedItem {
                    kind: CommentItemKind::ReviewBody,
                    stable_key,
                    author: login(review.author.as_ref()),
                    author_kind,
                    url: url.clone(),
                    location,
                    title,
                    body_hash: markers::body_hash(&text),
                    body: text,
                    head: head.clone(),
                    round,
                    bot_severity: severity,
                    github: facts(0),
                    outside: None,
                },
                thread: None,
            });
        };
        if author_kind == AuthorKind::Bot {
            if review.body.contains("<!-- thepastaclaw-review v1") {
                for (index, finding) in markers::body_findings(&review.body).into_iter().enumerate()
                {
                    let location = finding.location.map(|(path, lines)| ItemLocation {
                        path,
                        lines: Some(lines),
                    });
                    push(
                        index + 1,
                        finding.title,
                        finding.text,
                        Some(finding.severity),
                        location,
                    );
                }
            } else if review.state == "CHANGES_REQUESTED" && !review.body.trim().is_empty() {
                let text = markers::normalise(&review.body);
                push(1, markers::title_of(&text), text, None, None);
            }
            continue;
        }
        let is_latest = latest_any
            .get(&login(review.author.as_ref()).to_lowercase())
            .is_some_and(|latest| latest.id == review.id);
        let asks =
            review.state == "CHANGES_REQUESTED" || (is_latest && review.state == "COMMENTED");
        if asks && !review.body.trim().is_empty() {
            let text = review.body.trim().to_owned();
            push(1, markers::title_of(&text), text, None, None);
        }
    }

    let mut gate = None;
    for comment in &read.comments {
        let author_kind = bots.kind(comment.author.as_ref());
        if author_kind == AuthorKind::Bot {
            if let Some((state, commit)) = markers::gate(&comment.body) {
                let state = match state {
                    GateState::InProgress => GateFact::InProgress,
                    GateState::Queued => GateFact::Queued,
                    GateState::Complete => GateFact::Complete,
                };
                gate = Some((state, commit));
            }
            continue;
        }
        let text = comment.body.trim().to_owned();
        if text.is_empty() {
            continue;
        }
        let Some(id) = comment.database_id else {
            continue;
        };
        let at = parse_time(&comment.created_at);
        if let Some(on) = written_on(&rounds, &read.reviews, None, at) {
            written.insert(format!("comment:{id}:1"), on);
        }
        observed.push(Observed {
            item: ObservedItem {
                kind: CommentItemKind::PrComment,
                stable_key: format!("comment:{id}:1"),
                author: login(comment.author.as_ref()),
                author_kind,
                url: github_web_url(&comment.url),
                location: None,
                title: markers::title_of(&text),
                body_hash: markers::body_hash(&text),
                body: text,
                head: head.clone(),
                round: round_of(&rounds, None, at),
                bot_severity: None,
                github: facts(0),
                outside: None,
            },
            thread: None,
        });
    }

    let snapshot = Snapshot {
        title: read.pr.title.clone(),
        description: Some(read.pr.body.trim().to_owned()).filter(|body| !body.is_empty()),
        url: github_web_url(&read.pr.url).unwrap_or_default(),
        state: read.pr.state.clone(),
        draft: read.pr.is_draft,
        base: read.pr.base_ref_name.clone(),
        head: read.pr.head_ref_oid.clone(),
        base_sha: Some(read.pr.base_ref_oid.clone()),
        rounds,
        gate,
        bot_change_requests,
        changes_requested_by,
        ci: None,
        pending_checks: Vec::new(),
        counts: (
            u32::try_from(read.threads.len()).unwrap_or(u32::MAX),
            u32::try_from(read.reviews.len()).unwrap_or(u32::MAX),
            u32::try_from(read.comments.len()).unwrap_or(u32::MAX),
        ),
        complete: read.complete,
        unresolved_threads: unresolved,
        thread_ids,
        review_ids,
        written,
    };
    (observed, snapshot)
}

#[cfg(test)]
mod tests;
