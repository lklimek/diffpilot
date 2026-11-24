//! Host-owned review data and KP9 source roles, shared by both assessment methods.

use super::{clean, markers};
use crate::github::review_loop::PrRead;
use contracts::agent::pr_data::{ItemIds, Manifest, PrData, ReviewScope};
use contracts::api::review_loop::{CommentSource, CommentSourceKind, SourceRole};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(super) fn hash(text: impl AsRef<[u8]>) -> String {
    Sha256::digest(text)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn login(author: Option<&crate::github::review_loop::Author>) -> &str {
    author.map_or("ghost", |a| a.login.as_str())
}

fn role(author: &str, pr_author: &str, body: &str, known_bots: &[String]) -> SourceRole {
    let text = body.trim();
    if text.is_empty()
        || text.starts_with('/')
        || (text
            .split_whitespace()
            .next()
            .and_then(|word| word.strip_prefix('@'))
            .is_some_and(|who| {
                who.ends_with("[bot]") || known_bots.iter().any(|b| b.eq_ignore_ascii_case(who))
            })
            && !text.contains('\n'))
        || markers::gate(text).is_some()
        || markers::is_thread_answer(text)
    {
        SourceRole::Ignored
    } else if author.eq_ignore_ascii_case(pr_author) {
        SourceRole::Context
    } else {
        SourceRole::Source
    }
}

pub(super) fn build(
    read: &PrRead,
    repo: &str,
    known_bots: &[String],
    maintainers: Vec<String>,
) -> (PrData, Vec<CommentSource>) {
    let pr_author = login(read.pr.author.as_ref());
    let bot = |who: Option<&crate::github::review_loop::Author>| {
        who.is_some_and(|who| {
            crate::dashboard::score::is_bot(&who.login, who.typename == "Bot", known_bots)
        })
    };
    let cleaned = |who, text: &str| {
        if bot(who) {
            clean::bot_body(text)
        } else {
            text.trim().to_owned()
        }
    };
    let mut data = PrData {
        manifest: Manifest {
            repo: repo.into(),
            pr: read
                .pr
                .url
                .rsplit('/')
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0),
            head: read.pr.head_ref_oid.clone(),
            base_ref: read.pr.base_ref_name.clone(),
            base_sha: read.pr.base_ref_oid.clone(),
            title: read.pr.title.clone(),
            body: clean::description(&read.pr.body).unwrap_or_default(),
            pr_author: pr_author.into(),
            maintainers,
            bots: known_bots.to_vec(),
            delegates: vec![],
        },
        threads: json!({"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[]}}}}}),
        review_comments: vec![],
        reviews: vec![],
        comments: vec![],
        conventions: BTreeMap::new(),
        ids: ItemIds {
            next_id: 1,
            items: vec![],
        },
        scope: ReviewScope {
            threads: vec![],
            sources: vec![],
        },
    };
    let mut nodes = Vec::new();
    for thread in &read.threads {
        let comments: Vec<Value> = thread.comments.nodes.iter().filter(|c| c.state.as_deref() != Some("PENDING"))
            .map(|c| json!({"databaseId":c.database_id,"author":{"login":login(c.author.as_ref())},"body":cleaned(c.author.as_ref(), &c.body),"url":c.url,"path":thread.path,"line":thread.line,"createdAt":c.created_at})).collect();
        if comments.is_empty() {
            continue;
        }
        for c in &thread.comments.nodes {
            if c.state.as_deref() == Some("PENDING") {
                continue;
            }
            let review = c
                .pull_request_review
                .as_ref()
                .and_then(|r| read.reviews.iter().find(|review| review.id == r.id));
            data.review_comments.push(json!({"id":c.database_id,"user":{"login":login(c.author.as_ref())},"body":cleaned(c.author.as_ref(), &c.body),"html_url":c.url,"path":thread.path,"line":thread.line,"created_at":c.created_at,"commit_id":c.commit.as_ref().or(c.original_commit.as_ref()).map(|c|&c.oid),"pull_request_review_id":review.and_then(|r|r.database_id),"in_reply_to_id":thread.comments.nodes.first().filter(|first|first.database_id!=c.database_id).and_then(|first|first.database_id)}));
        }
        nodes.push(json!({"id":thread.id,"isResolved":thread.is_resolved,"isOutdated":thread.is_outdated,"path":thread.path,"line":thread.line,"startLine":thread.start_line,"comments":{"nodes":comments}}));
        if !thread.is_resolved {
            data.scope.threads.push(thread.id.clone());
        }
    }
    data.threads["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"] = json!(nodes);
    let mut sources = Vec::new();
    for review in &read.reviews {
        if review.submitted_at.is_none() || review.state == "PENDING" {
            continue;
        }
        let who = login(review.author.as_ref());
        let body = cleaned(review.author.as_ref(), &review.body);
        let mut source_role = role(who, pr_author, &body, known_bots);
        if source_role != SourceRole::Ignored
            && bot(review.author.as_ref())
            && (review.state == "DISMISSED"
                || read.reviews.iter().any(|newer| {
                    login(newer.author.as_ref()).eq_ignore_ascii_case(who)
                        && !newer.body.trim().is_empty()
                        && newer.state != "DISMISSED"
                        && newer.submitted_at > review.submitted_at
                }))
        {
            source_role = SourceRole::Context;
        }
        if source_role == SourceRole::Ignored {
            continue;
        }
        let value = json!({"id":review.database_id.map_or_else(|| json!(review.id), |id| json!(id)),"node_id":review.id,"user":{"login":who},"body":body,"state":review.state,"html_url":review.url,"commit_id":review.commit.as_ref().map(|c|&c.oid),"submitted_at":review.submitted_at});
        if source_role == SourceRole::Source {
            data.scope.sources.push(review.url.clone());
        }
        sources.push(CommentSource {
            url: review.url.clone(),
            kind: CommentSourceKind::ReviewBody,
            author: who.into(),
            body_hash: hash(&body),
            body,
            role: source_role,
            read_at: None,
        });
        data.reviews.push(value);
    }
    for comment in &read.comments {
        let who = login(comment.author.as_ref());
        let body = cleaned(comment.author.as_ref(), &comment.body);
        let source_role = role(who, pr_author, &body, known_bots);
        if source_role == SourceRole::Ignored {
            continue;
        }
        data.comments.push(json!({"id":comment.database_id,"node_id":comment.id,"author":{"login":who},"body":body,"url":comment.url,"createdAt":comment.created_at}));
        if source_role == SourceRole::Source {
            data.scope.sources.push(comment.url.clone());
        }
        sources.push(CommentSource {
            url: comment.url.clone(),
            kind: CommentSourceKind::PrComment,
            author: who.into(),
            body_hash: hash(&body),
            body,
            role: source_role,
            read_at: None,
        });
    }
    for author in read
        .reviews
        .iter()
        .filter_map(|r| r.author.as_ref())
        .chain(read.comments.iter().filter_map(|c| c.author.as_ref()))
        .chain(
            read.threads
                .iter()
                .flat_map(|t| t.comments.nodes.iter().filter_map(|c| c.author.as_ref())),
        )
    {
        if bot(Some(author))
            && !data
                .manifest
                .bots
                .iter()
                .any(|b| b.eq_ignore_ascii_case(&author.login))
        {
            data.manifest.bots.push(author.login.clone());
        }
    }
    (data, sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::review_loop::{PrFacts, PrRead};
    use serde_json::json;

    #[test]
    fn handin_only_author_is_context_and_commands_are_omitted() {
        let facts: PrFacts = serde_json::from_value(json!({"title":"Retry", "body":"Bound retries", "url":"https://github.com/o/r/pull/12", "state":"OPEN", "isDraft":false,"author":{"login":"alice"},"baseRefName":"main","baseRefOid":"base","headRefOid":"head","reviewDecision":null})).unwrap();
        let mut read = PrRead {
            repo_github_id: 1,
            private: false,
            pr: facts,
            threads: vec![],
            reviews: vec![],
            comments: vec![],
            complete: true,
        };
        for (i, who, body) in [
            (1, "alice", "Keep author context"),
            (2, "owner", "Release the lock before waiting"),
            (
                3,
                "bot",
                "<details>\n<summary>Nitpick comments</summary>\nBound the retry loop\n</details>",
            ),
            (4, "owner", "/review"),
            (5, "owner", "@bot review"),
        ] {
            read.comments.push(serde_json::from_value(json!({"id":format!("IC{i}"),"databaseId":i,"author":{"login":who},"body":body,"createdAt":"2026-11-23T00:00:00Z","lastEditedAt":null,"url":format!("https://github.com/o/r/pull/12#issuecomment-{i}")})).unwrap());
        }
        let (data, sources) = build(&read, "o/r", &["bot".into()], vec!["owner".into()]);
        assert_eq!(data.comments.len(), 3);
        assert_eq!(data.scope.sources.len(), 2);
        assert_eq!(
            sources[0].role,
            contracts::api::review_loop::SourceRole::Context
        );
        assert_eq!(
            sources[1].role,
            contracts::api::review_loop::SourceRole::Source
        );
        assert!(
            data.comments[2]["body"]
                .as_str()
                .unwrap()
                .contains("Bound the retry loop")
        );
        assert_eq!(data.manifest.maintainers, vec!["owner"]);
        assert!(data.manifest.delegates.is_empty());
    }
    #[test]
    fn handin_reviews_use_rest_ids_for_inline_review_links() {
        let read = PrRead {
            repo_github_id: 1, private: false, complete: true,
            pr: serde_json::from_value(json!({"title":"Review", "body":"", "url":"https://github.com/o/r/pull/12", "state":"OPEN", "isDraft":false,"author":{"login":"alice"},"baseRefName":"main","baseRefOid":"base","headRefOid":"head","reviewDecision":null})).unwrap(),
            threads: vec![], comments: vec![],
            reviews: vec![serde_json::from_value(json!({"id":"PRR_node", "databaseId":42,"author":{"login":"reviewer"},"state":"COMMENTED","submittedAt":"2026-11-23T00:00:00Z","lastEditedAt":null,"commit":{"oid":"head"},"body":"Bound retry attempts", "url":"https://github.com/o/r/pull/12#pullrequestreview-42"})).unwrap()],
        };
        let (data, _) = build(&read, "o/r", &[], vec![]);
        assert_eq!(data.reviews[0]["id"], 42);
        assert_eq!(data.reviews[0]["node_id"], "PRR_node");
        assert_eq!(data.reviews[0]["commit_id"], "head");
        assert_eq!(data.reviews[0]["submitted_at"], "2026-11-23T00:00:00Z");
    }
}
