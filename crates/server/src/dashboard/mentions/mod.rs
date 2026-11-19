//! Deterministic unanswered mentions, independent of scores and judgement.

use super::score::ScoreRules;
use crate::github::work::{Actor, Comment, ItemRef, mentions};
use contracts::api::dashboard::{GithubKind, Mention};
use sha2::{Digest, Sha256};

#[cfg(test)]
mod tests;

/// Selects posts strictly newer than every scanned login's latest comment/review.
pub(crate) fn unanswered(
    rules: &ScoreRules,
    logins: &[String],
    item: &ItemRef,
    kind: GithubKind,
    title: &str,
    posts: &[Comment],
) -> Vec<Mention> {
    let latest = posts
        .iter()
        .filter(|p| {
            p.url != "#body"
                && p.author
                    .as_ref()
                    .and_then(|a| a.login.as_deref())
                    .is_some_and(|login| rules.is_scanned(login))
        })
        .map(|p| p.created_at)
        .max();
    let key = match kind {
        GithubKind::Pr => item.pr_key(),
        GithubKind::Issue => item.issue_key(),
    };
    let Some(key) = key else {
        return Vec::new();
    };
    posts
        .iter()
        .filter_map(|post| {
            if latest.is_some_and(|at| post.created_at <= at)
                || !logins.iter().any(|login| mentions(&post.body, login))
            {
                return None;
            }
            let author = post
                .author
                .as_ref()
                .and_then(|a| a.login.as_deref())
                .unwrap_or("ghost");
            if rules.is_scanned(author) || rules.is_skipped_author(author) {
                return None;
            }
            let anchor = safe_anchor(&post.url)?;
            let path = if kind == GithubKind::Pr {
                "pull"
            } else {
                "issues"
            };
            let mut url = format!(
                "https://github.com/{}/{}/{path}/{}",
                item.repo.owner, item.repo.name, item.number
            );
            if anchor != "body" {
                url.push_str(&format!("#{anchor}"));
            }
            let id = Sha256::digest(format!("{key}|{anchor}").as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let text = post
                .body
                .lines()
                .find(|line| logins.iter().any(|login| mentions(line, login)))
                .unwrap_or("")
                .chars()
                .take(300)
                .collect();
            Some(Mention {
                id,
                item_key: key.clone(),
                kind,
                repo: item.repo.clone(),
                number: item.number,
                title: title.chars().take(300).collect(),
                author: author.to_owned(),
                is_bot: rules.is_bot(
                    author,
                    post.author.as_ref().is_some_and(|a| a.typename == "Bot"),
                ),
                created_at: post.created_at,
                text,
                url,
            })
        })
        .collect()
}

/// Only GitHub's fixed anchors enter server-built links, never an upstream URL.
fn safe_anchor(url: &str) -> Option<&str> {
    let (_, anchor) = url.rsplit_once('#')?;
    if anchor == "body" {
        return Some(anchor);
    }
    ["issuecomment-", "pullrequestreview-", "discussion_r"]
        .iter()
        .find_map(|prefix| {
            let number = anchor.strip_prefix(prefix)?;
            (!number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())).then_some(anchor)
        })
}

/// A description participates in mention discovery, but is never an answer.
pub(crate) fn body(
    author: Option<Actor>,
    text: &str,
    created_at: contracts::time::Timestamp,
) -> Comment {
    Comment {
        author,
        body: text.to_owned(),
        created_at,
        url: "#body".to_owned(),
    }
}
