//! Token-scoped submissions and exact links to their persisted destinations.

use contracts::api::review_loop::{
    AgentCommentReceipt, AgentCommentTarget, AuthorKind, CommentItemKind, ItemGithubFacts,
    ReadFailure, SubmitAgentComment,
};
use contracts::common::PrRef;
use contracts::ids::CommitSha;
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::review_loop::{AgentSubmission, NewAgentComment, NewAgentSubmission, ObservedItem};
use url::Url;

use super::{
    Loaded, LoopError, PrHandle, ReadOutcome, failure_of, find, handin, markers, notify,
    read_if_due,
};
use crate::auth::{TokenPrincipal, user_access_token, user_token_rejected};
use crate::github::graphql::{GraphqlError, GraphqlErrorType};
use crate::github::review_loop::{self as reads, ReadError};
use crate::github::threads::github_web_url;
use crate::state::AppState;

const MAX_BODY_CHARS: usize = 6000;

/// Resolves the same GitHub destination from a review read and from a deep link.
pub(super) fn item_no(loaded: &Loaded, conversation: &str, url: Option<&str>) -> Option<u32> {
    loaded.items.iter().find_map(|item| {
        if item.gone_at.is_some() || item.from_api {
            return None;
        }
        let matches = match item.kind {
            CommentItemKind::Thread => loaded.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot
                    .thread_ids
                    .get(&item.stable_key)
                    .is_some_and(|id| id == conversation)
            }),
            CommentItemKind::PrComment | CommentItemKind::ReviewBody => {
                (conversation.starts_with("issue:") || conversation.starts_with("review:"))
                    && url.is_some_and(|url| item.url.as_deref() == Some(url))
            }
            CommentItemKind::PrQuestion => false,
        };
        matches.then_some(item.no)
    })
}

/// Admits the token's PR and stores one comment or supplemental annotation.
pub(crate) async fn submit(
    state: &AppState,
    agent: &TokenPrincipal,
    pr: &PrRef,
    mut request: SubmitAgentComment,
) -> Result<(AgentCommentReceipt, bool), LoopError> {
    request.body = request.body.trim().to_owned();
    if request.body.is_empty() || request.body.chars().count() > MAX_BODY_CHARS {
        return Err(LoopError::BadSubmission);
    }
    let selector = match &request.target {
        AgentCommentTarget::Standalone => None,
        AgentCommentTarget::Github { comment } => Some(Selector::parse(pr, comment)?),
    };
    let fingerprint = handin::hash(serde_json::to_vec(&request).map_err(DbError::from)?);
    let rl = state.db.review_loop();
    let input_for = |pr| NewAgentSubmission {
        tenant: &agent.tenant_id,
        pr,
        token_id: &agent.token_id,
        key: request.submission_key,
        fingerprint: &fingerprint,
        agent: &agent.token_name,
        now: Timestamp::now(),
    };
    match find(state, &agent.tenant_id, pr).await {
        Ok(stored) => {
            if let Some(previous) = rl.agent_submission(&input_for(stored.id)).await? {
                return Ok((
                    receipt(state, stored.id, previous, &fingerprint).await?,
                    false,
                ));
            }
        }
        Err(LoopError::NotFound) => {}
        Err(error) => return Err(error),
    }
    let opened = read_if_due(state, &agent.tenant_id, &agent.user_id, pr, false).await?;
    if let ReadOutcome::Failed(failure) = opened.outcome {
        return Err(LoopError::Unreadable(failure));
    }
    let handle = opened.handle;
    let head = rl
        .pr_state(handle.id)
        .await?
        .and_then(|row| row.github_snapshot)
        .and_then(|snapshot| snapshot["head"].as_str().map(str::to_owned))
        .map(CommitSha)
        .ok_or(LoopError::Unreadable(ReadFailure::Unavailable))?;
    let input = input_for(handle.id);
    if let Some(previous) = rl.agent_submission(&input).await? {
        return Ok((
            receipt(state, handle.id, previous, &fingerprint).await?,
            false,
        ));
    }
    let (stored, created) = if let Some(selector) = selector {
        let target = github_target(state, agent, &handle, &selector).await?;
        rl.submit_agent_comment(
            &input,
            NewAgentComment::Annotation {
                conversation: &target.conversation,
                target_url: &target.url,
                body: &request.body,
            },
        )
        .await?
    } else {
        let observed = ObservedItem {
            kind: CommentItemKind::PrComment,
            stable_key: String::new(),
            author: agent.token_name.clone(),
            author_kind: AuthorKind::Bot,
            status_comment: false,
            url: None,
            location: None,
            title: markers::title_of(&request.body),
            body_hash: markers::body_hash(&request.body),
            body: request.body,
            head,
            round: None,
            bot_severity: None,
            github: ItemGithubFacts {
                resolved: None,
                resolved_by: None,
                outdated: None,
                bot_status: None,
                edited_after_decision: false,
                replies: 0,
                marker: None,
            },
            outside: None,
        };
        rl.submit_agent_comment(&input, NewAgentComment::Standalone(Box::new(observed)))
            .await?
    };
    Ok((
        receipt(state, handle.id, stored, &fingerprint).await?,
        created,
    ))
}

async fn receipt(
    state: &AppState,
    pr_id: i64,
    submission: AgentSubmission,
    fingerprint: &str,
) -> Result<AgentCommentReceipt, LoopError> {
    if submission.fingerprint != fingerprint {
        return Err(LoopError::SubmissionConflict);
    }
    let summary = state.db.pull_requests().get(pr_id).await?.pr;
    let pr = PrRef {
        repo: summary.repo,
        number: summary.number,
    };
    let mut url = state
        .config
        .public_url
        .join(&notify::comments_path(&pr))
        .map_err(|_| DbError::Corrupt {
            column: "public_url",
        })?;
    if let Some(no) = submission.item_no {
        url.query_pairs_mut().append_pair("item", &format!("C{no}"));
    }
    if let Some(conversation) = submission.conversation {
        url.query_pairs_mut().append_pair("thread", &conversation);
        url.set_fragment(Some(&format!("agent-comment-{}", submission.id)));
    }
    Ok(AgentCommentReceipt {
        submission_id: submission.id,
        item_no: submission.item_no,
        url: url.into(),
    })
}

enum Selector<'a> {
    Id(&'a str),
    Fragment(String),
}

impl<'a> Selector<'a> {
    fn parse(pr: &PrRef, raw: &'a str) -> Result<Self, LoopError> {
        if raw.is_empty() || raw.len() > 1024 || !raw.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(LoopError::BadSubmission);
        }
        if !raw.contains("://") {
            return Ok(Self::Id(raw));
        }
        let url = github_web_url(raw)
            .and_then(|canonical| Url::parse(&canonical).ok())
            .ok_or(LoopError::BadSubmission)?;
        let mut segments = url.path_segments().ok_or(LoopError::BadSubmission)?;
        if !segments
            .next()
            .is_some_and(|s| s.eq_ignore_ascii_case(&pr.repo.owner))
            || !segments
                .next()
                .is_some_and(|s| s.eq_ignore_ascii_case(&pr.repo.name))
            || segments.next() != Some("pull")
            || segments.next().and_then(|s| s.parse().ok()) != Some(pr.number)
            || segments.next().is_some()
        {
            return Err(LoopError::NotFound);
        }
        let fragment = url.fragment().ok_or(LoopError::BadSubmission)?;
        let valid = ["discussion_r", "issuecomment-", "pullrequestreview-"]
            .iter()
            .filter_map(|prefix| fragment.strip_prefix(prefix))
            .any(|n| n.parse::<u64>().is_ok_and(|id| id > 0));
        if !valid || url.query().is_some() {
            return Err(LoopError::BadSubmission);
        }
        Ok(Self::Fragment(fragment.to_owned()))
    }

    fn matches(&self, id: Option<&str>, url: &str) -> bool {
        match self {
            Self::Id(wanted) => id.is_some_and(|id| id == *wanted),
            Self::Fragment(wanted) => Url::parse(url)
                .ok()
                .and_then(|url| url.fragment().map(|f| f == wanted))
                .unwrap_or(false),
        }
    }
}

struct Target {
    conversation: String,
    url: String,
}

impl Target {
    fn new(conversation: String, url: &str) -> Result<Self, LoopError> {
        Ok(Self {
            conversation,
            url: github_web_url(url).ok_or(LoopError::NotFound)?,
        })
    }
}

async fn github_target(
    state: &AppState,
    agent: &TokenPrincipal,
    handle: &PrHandle,
    selector: &Selector<'_>,
) -> Result<Target, LoopError> {
    let token = user_access_token(state, &agent.user_id).await?;
    // The locator is data only. GitHub's configured endpoint, OAuth reader, and PR connection establish scope.
    let read = match reads::read(&state.config.github, &token, &handle.pr, None).await {
        Ok(read) => read,
        Err(ReadError::NotFound) => return Err(LoopError::NotFound),
        Err(ReadError::Graphql(error))
            if matches!(
                error.kind(),
                Some(GraphqlErrorType::NotFound | GraphqlErrorType::Forbidden)
            ) =>
        {
            return Err(LoopError::NotFound);
        }
        Err(error) => {
            if matches!(error, ReadError::Graphql(GraphqlError::Unauthorized)) {
                user_token_rejected(state, &agent.user_id, &token).await;
            }
            return Err(LoopError::Unreadable(failure_of(&error)));
        }
    };
    if read.private {
        return Err(LoopError::PrivateRepository);
    }
    for thread in &read.threads {
        if let Some(comment) = thread.comments.nodes.iter().find(|comment| {
            comment.state.as_deref() != Some("PENDING")
                && selector.matches(comment.id.as_deref(), &comment.url)
        }) {
            return Target::new(thread.id.clone(), &comment.url);
        }
    }
    if let Some(comment) = read
        .comments
        .iter()
        .find(|comment| selector.matches(Some(&comment.id), &comment.url))
    {
        return Target::new(format!("issue:{}", comment.id), &comment.url);
    }
    if let Some(review) = read.reviews.iter().find(|review| {
        review.submitted_at.is_some()
            && review.state != "PENDING"
            && selector.matches(Some(&review.id), &review.url)
    }) {
        return Target::new(format!("review:{}", review.id), &review.url);
    }
    Err(LoopError::NotFound)
}
