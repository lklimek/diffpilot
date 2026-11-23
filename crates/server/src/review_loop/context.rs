//! What a `verify_comments` input carries of an item beyond its stored row (plan §4.2), kept with the read's
//! snapshot: the replies of its open thread (bot replies without markup and cut, people's whole: the fingerprint sees
//! every edit, the input cuts them), which of them record a maintainer's decision and which concede. Pure.

use std::collections::BTreeMap;

use contracts::agent::verify_comments::{
    MAX_REPLIES, MAX_REPLY_CHARS, ReplyAuthorKind, ThreadReply,
};
use contracts::api::review_loop::AuthorKind;
use serde::{Deserialize, Serialize};

use super::clean;
use super::markers;
use super::observe::BotRule;
use crate::github::review_loop::{PrRead, Thread};

/// Words of a reply that records a maintainer's decision ("Per maintainer decision, …", thepastaclaw's
/// "**Deferred**"), lowercase.
const DECISION_WORDS: [&str; 4] = [
    "maintainer decision",
    "deferred",
    "won't fix",
    "will not fix",
];

/// Words of a bot reply's first line that withdraw or accept the point (CodeRabbit's "you're right"), lowercase.
const CONCESSION_WORDS: [&str; 10] = [
    "you're right",
    "you are right",
    "you're absolutely right",
    "you are absolutely right",
    "you're correct",
    "you are correct",
    "thanks for the clarification",
    "thank you for the clarification",
    "i stand corrected",
    "my apologies",
];

/// An item's context from the read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemContext {
    /// The latest replies of its open thread, oldest first; people's are not cut (see [`cut`]).
    #[serde(default)]
    pub replies: Vec<ThreadReply>,
    /// People and concessions from the full thread, before the input window; absent in older snapshots.
    #[serde(default)]
    pub fingerprint_replies: Option<Vec<ThreadReply>>,
    /// Earlier replies left out of `replies`.
    #[serde(default)]
    pub earlier_replies: u32,
}

/// The context of the items of open threads, by stable key (`thread_ids`: stable key to thread node id).
pub fn of(
    read: &PrRead,
    bots: &BotRule,
    thread_ids: &BTreeMap<String, String>,
) -> BTreeMap<String, ItemContext> {
    let owner = read.pr.author.as_ref().map(|author| author.login.as_str());
    thread_ids
        .iter()
        .filter_map(|(key, id)| {
            let thread = read.threads.iter().find(|thread| thread.id == *id)?;
            let context = thread_context(thread, bots, owner);
            (!thread.is_resolved && !context.replies.is_empty()).then(|| (key.clone(), context))
        })
        .collect()
}

/// The replies of `thread` after its first comment; `owner` is the pull request author's login.
fn thread_context(thread: &Thread, bots: &BotRule, owner: Option<&str>) -> ItemContext {
    let mut replies: Vec<ThreadReply> = thread
        .comments
        .nodes
        .iter()
        .skip(1)
        .filter(|comment| comment.state.as_deref() != Some("PENDING"))
        .filter_map(|comment| {
            let author_kind = match bots.kind(comment.author.as_ref()) {
                AuthorKind::Bot => ReplyAuthorKind::Bot,
                _ if comment
                    .author
                    .as_ref()
                    .zip(owner)
                    .is_some_and(|(author, owner)| author.login.eq_ignore_ascii_case(owner)) =>
                {
                    ReplyAuthorKind::Owner
                }
                _ => ReplyAuthorKind::Human,
            };
            let text = match author_kind {
                ReplyAuthorKind::Bot => cut(&clean::bot_body(&markers::normalise(&comment.body))),
                _ => comment.body.trim().to_owned(),
            };
            (!text.is_empty()).then(|| ThreadReply {
                author: comment
                    .author
                    .as_ref()
                    .map_or_else(|| "ghost".to_owned(), |author| author.login.clone()),
                author_kind,
                body: text,
            })
        })
        .collect();
    let fingerprint_replies = replies
        .iter()
        .filter(|reply| reply.author_kind != ReplyAuthorKind::Bot || concedes(reply))
        .cloned()
        .collect();
    let left_out = replies.len().saturating_sub(MAX_REPLIES);
    replies.drain(..left_out);
    ItemContext {
        fingerprint_replies: Some(fingerprint_replies),
        replies,
        earlier_replies: u32::try_from(left_out).unwrap_or(u32::MAX),
    }
}

/// `text` cut to [`MAX_REPLY_CHARS`] characters, ending with `…` when cut: a reply as an input carries it.
pub fn cut(text: &str) -> String {
    match text.char_indices().nth(MAX_REPLY_CHARS - 1) {
        Some((at, _)) if text[at..].chars().nth(1).is_some() => format!("{}…", &text[..at]),
        _ => text.to_owned(),
    }
}

/// The latest reply that records a maintainer's decision on the item, if any.
pub fn decision_reply(context: &ItemContext) -> Option<&ThreadReply> {
    context.replies.iter().rev().find(|reply| {
        let text = reply.body.to_lowercase().replace('’', "'");
        DECISION_WORDS.iter().any(|words| text.contains(words))
    })
}

/// Whether `reply` is a bot's that concedes: its first line withdraws or accepts the point. Bots without a status
/// marker (CodeRabbit) say so only in words.
pub fn concedes(reply: &ThreadReply) -> bool {
    reply.author_kind == ReplyAuthorKind::Bot
        && reply
            .body
            .lines()
            .find(|line| !line.trim().is_empty())
            .is_some_and(|line| {
                let line = line.to_lowercase().replace('’', "'");
                CONCESSION_WORDS.iter().any(|words| line.contains(words))
            })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::review_loop::{Author, CountedNodes, PrFacts, ThreadComment};

    fn comment(login: &str, typename: &str, body: &str) -> ThreadComment {
        ThreadComment {
            database_id: Some(1),
            state: None,
            author: Some(Author {
                login: login.to_owned(),
                typename: typename.to_owned(),
            }),
            body: body.to_owned(),
            created_at: "2026-11-20T10:00:00Z".to_owned(),
            last_edited_at: None,
            url: "https://github.com/acme/node-client/pull/482#discussion_r1".to_owned(),
            pull_request_review: None,
        }
    }

    fn thread(id: &str, resolved: bool, comments: Vec<ThreadComment>) -> Thread {
        Thread {
            id: id.to_owned(),
            is_resolved: resolved,
            is_outdated: false,
            path: "src/rpc/client.rs".to_owned(),
            line: Some(42),
            start_line: None,
            diff_side: Some("RIGHT".to_owned()),
            resolved_by: None,
            comments: CountedNodes {
                total_count: u32::try_from(comments.len()).expect("count"),
                nodes: comments,
            },
        }
    }

    fn read(threads: Vec<Thread>) -> PrRead {
        PrRead {
            repo_github_id: 1,
            private: false,
            pr: PrFacts {
                title: "Retry RPC calls".to_owned(),
                body: String::new(),
                url: "https://github.com/acme/node-client/pull/482".to_owned(),
                state: "OPEN".to_owned(),
                is_draft: false,
                author: Some(Author {
                    login: "Alice".to_owned(),
                    typename: "User".to_owned(),
                }),
                base_ref_name: "main".to_owned(),
                base_ref_oid: "92f2274efbb93570cc8f820a49c058cf66b0dbe9".to_owned(),
                head_ref_oid: "52d2da140191a9d47becf6a446310be803c007fc".to_owned(),
                review_decision: None,
            },
            threads,
            reviews: Vec::new(),
            comments: Vec::new(),
            complete: true,
        }
    }

    /// Plan §4.2, comparison report R3: an open thread's item carries its replies with the author's kind (the pull
    /// request's author is the owner), bot replies without markup and cut, people's whole (the fingerprint sees
    /// every edit; the input cuts them), the latest ones kept.
    #[test]
    fn open_threads_carry_their_replies() {
        let finding = comment("thepastaclaw", "User", "**🔴 Blocking: Bound the retries**");
        let mut replies = vec![
            finding.clone(),
            comment(
                "alice",
                "User",
                "  Per maintainer decision, deferred to #12.  ",
            ),
            comment(
                "thepastaclaw",
                "User",
                "<!-- thepastaclaw-thread-answer v1 sha=9f3c reply=1 finding=7b44 -->\n**Deferred** <sub>x</sub>",
            ),
            comment("bob", "User", &"long ".repeat(200)),
        ];
        let mut pending = comment("carol", "User", "Draft");
        pending.state = Some("PENDING".to_owned());
        replies.push(pending);
        let ids = BTreeMap::from([
            ("open".to_owned(), "T1".to_owned()),
            ("resolved".to_owned(), "T2".to_owned()),
            ("quiet".to_owned(), "T3".to_owned()),
        ]);
        let pr_read = read(vec![
            thread("T1", false, replies),
            thread(
                "T2",
                true,
                vec![finding.clone(), comment("alice", "User", "Fixed.")],
            ),
            thread("T3", false, vec![finding]),
        ]);
        let contexts = of(&pr_read, &BotRule::new(&["thepastaclaw".to_owned()]), &ids);
        assert_eq!(
            contexts.keys().collect::<Vec<_>>(),
            ["open"],
            "resolved threads and threads without replies carry nothing"
        );
        let replies = &contexts["open"].replies;
        assert_eq!(replies.len(), 3, "{replies:?}");
        assert_eq!(replies[0].author_kind, ReplyAuthorKind::Owner);
        assert_eq!(replies[0].body, "Per maintainer decision, deferred to #12.");
        assert_eq!(replies[1].author_kind, ReplyAuthorKind::Bot);
        assert_eq!(replies[1].body, "**Deferred** x");
        assert_eq!(replies[2].author_kind, ReplyAuthorKind::Human);
        assert_eq!(replies[2].body, "long ".repeat(200).trim());
        assert_eq!(
            decision_reply(&contexts["open"]).map(|reply| reply.author.as_str()),
            Some("thepastaclaw"),
            "the latest reply recording a decision"
        );

        let many = (0..MAX_REPLIES + 2)
            .map(|n| comment("bob", "User", &format!("Reply {n}")))
            .collect::<Vec<_>>();
        let read_many = read(vec![thread(
            "T1",
            false,
            [vec![comment("bob", "User", "Finding")], many].concat(),
        )]);
        let kept = &of(&read_many, &BotRule::default(), &ids)["open"];
        assert_eq!(kept.replies.len(), MAX_REPLIES);
        assert_eq!(kept.earlier_replies, 2);
        assert_eq!(kept.replies[0].body, "Reply 2");
        assert_eq!(decision_reply(kept), None);
    }

    /// A bot reply concedes when its first line withdraws or accepts the point; status-like replies do not.
    #[test]
    fn concessions_are_read_from_a_bot_reply_s_first_line() {
        let bot = |body: &str| ThreadReply {
            author: "coderabbitai".to_owned(),
            author_kind: ReplyAuthorKind::Bot,
            body: body.to_owned(),
        };
        for body in [
            "`@alice`, you\u{2019}re right: the bound lives in the caller.",
            "@alice You are correct, this is fine.\nLearnings added.",
            "Thanks for the clarification! The retry is bounded upstream.",
        ] {
            assert!(concedes(&bot(body)), "{body}");
        }
        for body in [
            "**Still applies** (re-reviewed at `81f765f2`).",
            "Confirmed. The writer selects only secp256k1 keys.",
            "Still blocked upstream.\nYou're right that it is tracked.",
        ] {
            assert!(!concedes(&bot(body)), "{body}");
        }
        let mut person = bot("You're right.");
        person.author_kind = ReplyAuthorKind::Owner;
        assert!(!concedes(&person), "only bots concede");
    }

    #[test]
    fn cut_keeps_short_texts_whole() {
        let exact = "x".repeat(MAX_REPLY_CHARS);
        assert_eq!(cut(&exact), exact);
        let longer = "é".repeat(MAX_REPLY_CHARS + 1);
        assert_eq!(cut(&longer).chars().count(), MAX_REPLY_CHARS);
    }
}
