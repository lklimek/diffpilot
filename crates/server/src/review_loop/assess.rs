//! Assessment passes over a pull request's review-comment items (ADR-023, plan §2.2, §2.8, §4.2): one live
//! `verify_comments` pass per pull request, unchanged items reuse their assessment (same normalised comment text and
//! the same code at its lines), at most [`MAX_ITEMS_PER_PASS`] items per job, spacing and a daily cap for automatic
//! (Watch) passes. A follower per pass ingests the job's output into `comment_assessments`, also after a server
//! restart ([`resume`]). Ask about an item without an analysis of the head is answered by a question pass
//! ([`ask_about_item`]).

mod ask;
mod ingest;
mod prompt;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, PoisonError};

use contracts::agent::pr_context::PrContextOutput;
use contracts::agent::verify_comments::{
    EarlierDecision, InputError, ItemQuestion, ReplyAuthorKind, ThreadEntry, ThreadReply,
    VerifyCommentsInput, VerifyItem,
};
use contracts::agent::{self as agent, JobKind, StrictSchemaError};
use contracts::api::pr::PrSummary;
use contracts::api::questions::AskAnchor;
use contracts::api::review_loop::{AuthorKind, CommentItemKind, ItemState, Written};
use contracts::common::LlmLanguage;
use contracts::ids::{CommentItemId, CommitSha, JobId, PassId, QuestionId};
use contracts::job::access::AgentAccess;
use contracts::job::spec::{
    JobLimits, JobPriority, JobSpec, JobTarget, PullRequestCommentsTarget, RepoCheckout,
};
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::documents::DocumentKind;
use db::repo::jobs::JobStage;
use db::repo::questions::QuestionRecord;
use db::repo::review_loop::{
    ItemRecord, NewPass, PassCause, PassRecord, PassStart, SentItem, StoredAssessment,
};
use futures::StreamExt;
use jiff::SignedDuration;
use sha2::{Digest, Sha256};

pub use ask::{ItemAskError, answer_landed, ask_about_item, list_questions};
pub(crate) use ingest::guard_posted;

use super::clean;
use super::context::{self, ItemContext};

use crate::github::mirror::{self, MirrorError};
use crate::github::oauth::endpoint;
use crate::jobs::executor::{ExecutorError, JobHandle, JobRequest, JobUpdate};
use crate::model::{ModelContext, select_model};
use crate::prompts;
use crate::state::AppState;

/// Items one `verify_comments` job assesses at most (plan §4.2).
pub const MAX_ITEMS_PER_PASS: usize = agent::verify_comments::MAX_ITEMS;
/// Least time between the end of a pass and an automatic (Watch) pass of the same pull request (plan §2.8); about
/// twice a full pass at high effort (spike S13-3).
pub const PASS_SPACING: SignedDuration = SignedDuration::from_mins(10);
/// Passes charged to one user per 24 hours before automatic (Watch) passes stop (plan §2.8); the owner's busiest
/// recorded day needed about 55 automatic passes, plus on-demand ones (spike S13-3).
pub const DAILY_PASSES_PER_USER: u32 = 80;
/// Version of `prompts/verify_comments.md` with the output schema, stored with every assessment.
pub const VERIFY_PROMPT_VERSION: u32 = 4;
/// The window of the daily cap.
const DAY: SignedDuration = SignedDuration::from_hours(24);

/// First wait before a failed link of a pass to its submitted job is tried again; it doubles each time.
const LINK_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(200);

/// How often a pass's follower tries to link the pass to its job.
const LINK_ATTEMPTS: u32 = 6;

/// Why a pass is requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PassReason {
    /// Comments or the PR page opened with unassessed items (on demand).
    Opened,
    /// A read found new or changed comments or a moved head (on demand).
    NewComments,
    /// A watched pull request's detection (automatic: spacing and the daily cap apply).
    Watch,
    /// An owner question about an item without an analysis of the head (no spacing).
    Ask,
    /// "Assess again" on one item (on demand): it is sent even when its stored assessment is current.
    AssessAgain(CommentItemId),
}

/// What [`request_pass`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PassQueued {
    /// A pass was queued with its job.
    Queued,
    /// A pass of the pull request is queued or running already.
    AlreadyRunning,
    /// An automatic pass is held back: the previous pass ended less than [`PASS_SPACING`] ago.
    Spaced,
    /// An automatic pass is held back: every watcher reached [`DAILY_PASSES_PER_USER`].
    DailyLimit,
    /// Nothing to assess, no review-loop read yet, or no watcher for an automatic pass.
    Skipped,
}

/// Who an automatic pass is charged to, or why none runs.
enum Charge {
    /// The first watcher under the daily cap.
    To(String),
    /// Held back by [`PASS_SPACING`].
    Spaced,
    /// Every watcher reached the daily cap.
    DailyLimit,
    /// Nobody watches the pull request.
    NoWatcher,
}

/// A pass could not be requested or ingested.
#[derive(Debug, thiserror::Error)]
pub enum AssessError {
    /// The database failed.
    #[error("the review-loop state could not be read or stored")]
    Db(#[from] DbError),
    /// The job executor failed.
    #[error("the verify_comments job could not be submitted or followed")]
    Executor(#[from] ExecutorError),
    /// The job input could not be built.
    #[error("the verify_comments input could not be built")]
    Input(#[from] InputError),
    /// The shared rules of the prompt could not be rendered.
    #[error("the verify_comments prompt could not be rendered")]
    Prompt(#[from] prompts::PromptError),
    /// The output schema could not be generated.
    #[error("the verify_comments output schema could not be generated")]
    Schema(#[from] StrictSchemaError),
    /// The job's update stream ended without a result.
    #[error("the verify_comments job ended without a result")]
    NoResult,
}

/// Passes with a follower task in this process.
static FOLLOWING: LazyLock<Mutex<HashSet<PassId>>> = LazyLock::new(Mutex::default);

/// Requests an assessment pass of pull request `pr` (`pull_requests.id`): at most one live pass per pull request;
/// unchanged items reuse their stored assessment; an automatic pass keeps [`PASS_SPACING`] after the previous pass
/// and stops at [`DAILY_PASSES_PER_USER`]; a question pass answers the oldest question no pass took yet; "Assess
/// again" sends its one item.
pub async fn request_pass(
    state: &AppState,
    pr: i64,
    reason: PassReason,
) -> Result<PassQueued, AssessError> {
    request(state, pr, reason, false, None).await
}

/// [`request_pass`] of an on-demand pass caused by `user`, who is charged with it: on-demand passes count toward
/// the daily cap that stops automatic passes (plan §2.8) and still run when it is reached.
pub async fn request_pass_for(
    state: &AppState,
    pr: i64,
    reason: PassReason,
    user: &str,
) -> Result<PassQueued, AssessError> {
    request(state, pr, reason, false, Some(user)).await
}

/// [`request_pass`]; `continuation` (the rest of a detection after a pass) skips the spacing, never the cap; `by`
/// is the user charged with an on-demand pass.
async fn request(
    state: &AppState,
    pr: i64,
    reason: PassReason,
    continuation: bool,
    by: Option<&str>,
) -> Result<PassQueued, AssessError> {
    let db = &state.db;
    let Some(pr_state) = db.review_loop().pr_state(pr).await? else {
        tracing::debug!(
            pr,
            "no assessment pass: the pull request was never read for comments"
        );
        return Ok(PassQueued::Skipped);
    };
    // Cheap check first; `start_pass` decides under its transaction.
    if db
        .review_loop()
        .live_passes()
        .await?
        .iter()
        .any(|pass| pass.pull_request_id == pr)
    {
        return Ok(PassQueued::AlreadyRunning);
    }
    let record = db.pull_requests().get(pr).await?;
    let now = Timestamp::now();
    let (user, question) = match reason {
        PassReason::Ask => match db.questions().next_unsent_for_pr(pr, now).await? {
            Some(question) => (Some(question.user_id.clone()), Some(question)),
            None => return Ok(PassQueued::Skipped),
        },
        // Decided before any code is read: the watch task holds its detection lock meanwhile. Watchers hear of
        // questions on-demand passes assessed when those passes end (`watch::pass_ended`).
        PassReason::Watch => match charged_watcher(state, pr, now, continuation).await? {
            Charge::To(user) => (Some(user), None),
            Charge::Spaced => return Ok(PassQueued::Spaced),
            Charge::DailyLimit => return Ok(PassQueued::DailyLimit),
            Charge::NoWatcher => return Ok(PassQueued::Skipped),
        },
        PassReason::Opened | PassReason::NewComments | PassReason::AssessAgain(_) => {
            (by.map(str::to_owned), None)
        }
    };
    let items = db.review_loop().items(pr).await?;
    // The one item a question or "Assess again" pass sends, whatever its stored assessment.
    let asked = match reason {
        PassReason::AssessAgain(item) => Some(item),
        _ => question
            .as_ref()
            .and_then(|question| match question.question.anchor {
                AskAnchor::CommentItem { item } => Some(item),
                _ => None,
            }),
    };
    let head = &record.pr.head_sha;
    let snapshot = pr_state
        .github_snapshot
        .clone()
        .and_then(|value| serde_json::from_value::<super::observe::Snapshot>(value).ok());
    let description = snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.description.as_deref())
        .and_then(clean::description);
    let contexts = snapshot
        .as_ref()
        .map(|snapshot| snapshot.item_context.clone())
        .unwrap_or_default();
    let written = snapshot
        .as_ref()
        .map(|snapshot| snapshot.written.clone())
        .unwrap_or_default();
    let purpose = purpose_hash(&record.pr.title, description.as_deref());
    let mut code = CodeReader::new(state, &record.pr);
    let mut sent = Vec::new();
    let mut sent_items = Vec::new();
    for item in &items {
        let wanted = match asked {
            Some(asked) => item.id == asked && item.kind != CommentItemKind::PrQuestion,
            None => assessable(item),
        };
        if !wanted {
            continue;
        }
        let fingerprint = fingerprint(
            item,
            head,
            code.lines(item).await.as_deref(),
            &purpose,
            contexts.get(&item.stable_key),
        );
        if asked.is_none()
            && let Some(stored) = db.review_loop().latest_assessment(item.id).await?
            && stored.fingerprint == fingerprint
        {
            reuse(state, &record.tenant_id, item, stored, head).await?;
            continue;
        }
        if sent.len() < MAX_ITEMS_PER_PASS {
            sent.push(SentItem {
                item: item.id,
                no: item.no,
                fingerprint,
            });
            sent_items.push(item);
        }
    }
    if sent.is_empty() {
        if let Some(question) = &question {
            ask::fail(state, question.question.id, ask::ITEM_GONE).await?;
        }
        return Ok(PassQueued::Skipped);
    }
    // Everything fallible is read before the pass is stored: a live pass without a job would hold every later
    // request back as already running.
    let language = match (&user, reason) {
        (Some(user), PassReason::Ask | PassReason::Watch) => db.users().get(user).await?.language,
        _ => LlmLanguage::En,
    };
    let analysis = analysis_inputs(state, &record.tenant_id, pr, &record.pr, language).await?;
    let input = VerifyCommentsInput {
        pull_request: format!(
            "{}/{}#{}",
            record.pr.repo.owner, record.pr.repo.name, record.pr.number
        ),
        title: record.pr.title.clone(),
        description: description
            .map(|text| text.chars().take(prompts::MAX_DESCRIPTION_CHARS).collect()),
        pr_context: analysis.pr_context,
        project_profile: analysis.profile,
        items: sent_items
            .iter()
            .map(|item| {
                let key = &item.stable_key;
                with_context(verify_item(item), contexts.get(key), written.get(key))
            })
            .collect(),
        earlier_decisions: earlier_decisions(state, pr, &sent_items, &contexts).await?,
        question: match &question {
            Some(question) => Some(item_question(state, question, &sent_items).await?),
            None => None,
        },
    };
    let pass = NewPass {
        pull_request_id: pr,
        user_id: user.clone(),
        cause: match reason {
            PassReason::Opened => PassCause::Opened,
            PassReason::NewComments => PassCause::Read,
            PassReason::Watch => PassCause::Watch,
            PassReason::Ask => PassCause::Question,
            PassReason::AssessAgain(_) => PassCause::AssessAgain,
        },
        head: head.clone(),
        sent,
        question: question.as_ref().map(|question| question.question.id),
    };
    let id = match db
        .review_loop()
        .start_pass(&record.tenant_id, &pass, now)
        .await?
    {
        PassStart::Started(id) => id,
        PassStart::Busy(_) => return Ok(PassQueued::AlreadyRunning),
    };
    let handle = match submit(state, &record.pr, pr, id, &input, language).await {
        Ok(handle) => handle,
        Err(error) => {
            // No job exists, so nothing follows this pass: it ends here.
            db.review_loop()
                .finish_pass(id, false, Timestamp::now())
                .await?;
            return Err(error);
        }
    };
    // A failed link is retried by the pass's follower, which knows the job.
    if let Err(error) = link(state, id, pass.question, handle.job_id).await {
        tracing::debug!(pr, pass = %id, job = %handle.job_id, ?error,
            "the submitted assessment job is not linked to its pass yet; the pass's follower links it");
    }
    tracing::info!(pr, pass = %id, job = %handle.job_id, ?reason, items = input.items.len(),
        "comment assessment pass queued");
    follow(state, id, Some(handle.job_id));
    Ok(PassQueued::Queued)
}

/// The first watcher of `pr` under the daily cap, unless the spacing holds the pass back (not for a continuation).
async fn charged_watcher(
    state: &AppState,
    pr: i64,
    now: Timestamp,
    continuation: bool,
) -> Result<Charge, AssessError> {
    let watches = state.db.review_loop().live_watches().await?;
    let mut watchers: Vec<_> = watches
        .iter()
        .filter(|watch| watch.pull_request_id == pr)
        .collect();
    if watchers.is_empty() {
        return Ok(Charge::NoWatcher);
    }
    watchers.sort_by_key(|watch| watch.since);
    for watch in watchers {
        let (passes, last_end) = state
            .db
            .review_loop()
            .pass_usage(&watch.user_id, now - DAY, pr)
            .await?;
        if !continuation && last_end.is_some_and(|end| end + PASS_SPACING > now) {
            tracing::debug!(pr, "automatic assessment pass held back by the spacing");
            return Ok(Charge::Spaced);
        }
        if passes < DAILY_PASSES_PER_USER {
            return Ok(Charge::To(watch.user_id.clone()));
        }
    }
    tracing::info!(
        pr,
        "automatic assessment pass not started: no watcher is under the daily limit"
    );
    Ok(Charge::DailyLimit)
}

/// Items a pass assesses: comments of open, undecided items the read still produces (not PR-level questions, which
/// the server asks).
fn assessable(item: &ItemRecord) -> bool {
    item.kind != CommentItemKind::PrQuestion
        && item.gone_at.is_none()
        && matches!(
            item.state,
            ItemState::Assessing
                | ItemState::Undecided
                | ItemState::AskedBack
                | ItemState::Reopened
        )
}

/// SHA-256 hex of the PR title and description without bot blocks (the purpose the author states).
fn purpose_hash(title: &str, description: Option<&str>) -> String {
    let mut hash = Sha256::new();
    hash.update(format!("{title}\n{}", description.unwrap_or_default()));
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// SHA-256 hex of the PR purpose (`purpose_hash`), the normalised comment text, the bot's status class, the whole
/// replies of people in its thread, which bots conceded in it ([`context::concedes`]) and the code at its lines; code
/// that cannot be read counts as changed at every new head. Bot replies' text (re-reviews each round) and how many
/// replies were left out are not part of it. The prompt version is not either: a new prompt alone re-assesses nothing
/// (owner decision 2026-11-23), and the claudius skills commit is provenance only (Q28).
fn fingerprint(
    item: &ItemRecord,
    head: &CommitSha,
    code: Option<&str>,
    purpose: &str,
    context: Option<&ItemContext>,
) -> String {
    let mut hash = Sha256::new();
    hash.update(format!("fingerprint 2\n{purpose}\n{}\n", item.body_hash));
    let replies = context.iter().flat_map(|context| {
        context
            .fingerprint_replies
            .as_ref()
            .unwrap_or(&context.replies)
    });
    let people: Vec<&ThreadReply> = replies
        .clone()
        .filter(|reply| reply.author_kind != ReplyAuthorKind::Bot)
        .collect();
    let conceded: BTreeSet<String> = replies
        .filter(|reply| context::concedes(reply))
        .map(|reply| reply.author.to_ascii_lowercase())
        .collect();
    hash.update(
        serde_json::to_vec(&(item.github.bot_status, people, conceded)).unwrap_or_default(),
    );
    hash.update("\n");
    hash_lines(&mut hash, item, head, code);
    hex(hash)
}

/// Hashes the item's path, lines and the code at them.
fn hash_lines(hash: &mut Sha256, item: &ItemRecord, head: &CommitSha, code: Option<&str>) {
    if let Some(location) = &item.location {
        hash.update(format!("{}\n", location.path));
        if let Some(lines) = location.lines {
            hash.update(format!("{}-{}\n", lines.start_line, lines.end_line));
            match code {
                Some(code) => hash.update(code),
                None => hash.update(format!("unread at {}", head.0)),
            }
        }
    }
}

/// The lowercase hex of the hash.
fn hex(hash: Sha256) -> String {
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Stores the unchanged assessment again at `head`, marked as reused since the head it was made at.
async fn reuse(
    state: &AppState,
    tenant: &str,
    item: &ItemRecord,
    stored: StoredAssessment,
    head: &CommitSha,
) -> Result<(), AssessError> {
    if stored.assessment.head == *head {
        return Ok(());
    }
    let mut assessment = stored.assessment;
    assessment.reused_since = Some(
        assessment
            .reused_since
            .take()
            .unwrap_or_else(|| assessment.head.clone()),
    );
    assessment.head = head.clone();
    state
        .db
        .review_loop()
        .add_assessment(tenant, item.id, &assessment, &stored.fingerprint)
        .await?;
    tracing::debug!(item = %item.id, "unchanged comment item reuses its assessment at the new head");
    Ok(())
}

/// Reads the code at items' lines from the server's mirror at the head; one fetch per pass, failures read as unread.
struct CodeReader<'a> {
    state: &'a AppState,
    pr: &'a PrSummary,
    mirror: Option<Option<PathBuf>>,
}

impl<'a> CodeReader<'a> {
    fn new(state: &'a AppState, pr: &'a PrSummary) -> Self {
        Self {
            state,
            pr,
            mirror: None,
        }
    }

    /// The text of the item's lines at the head; `None` without lines or when it cannot be read.
    async fn lines(&mut self, item: &ItemRecord) -> Option<String> {
        let location = item.location.as_ref()?;
        let lines = location.lines?;
        if self.mirror.is_none() {
            let fetched =
                mirror::ensure_commits(&self.state.config, &self.pr.repo, &[&self.pr.head_sha])
                    .await;
            if let Err(error) = &fetched {
                tracing::debug!(%error, "comment code not readable from the mirror; items count as changed");
            }
            self.mirror = Some(fetched.ok());
        }
        let mirror = self.mirror.clone().flatten()?;
        let bytes = match mirror::read_blob(&mirror, &self.pr.head_sha, &location.path).await {
            Ok(bytes) => bytes?,
            Err(MirrorError::BlobTooLarge { .. }) => return None,
            Err(error) => {
                tracing::debug!(%error, "a commented file could not be read from the mirror");
                return None;
            }
        };
        let text = String::from_utf8_lossy(&bytes);
        let start = usize::try_from(lines.start_line).ok()?.saturating_sub(1);
        let count = usize::try_from(lines.end_line - lines.start_line + 1).ok()?;
        Some(
            text.lines()
                .skip(start)
                .take(count)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

/// An item as the agent reads it: `C<no>` and its comment (a bot's without markup), never its id or GitHub facts
/// other than the bot's own status.
pub(crate) fn verify_item(item: &ItemRecord) -> VerifyItem {
    VerifyItem {
        id: format!("C{}", item.no),
        kind: item.kind,
        author: item.author.clone(),
        author_kind: item.author_kind,
        location: item.location.clone(),
        commit: None,
        title: item.title.clone(),
        body: match item.author_kind {
            AuthorKind::Bot => clean::bot_body(&item.body),
            _ => item.body.clone(),
        },
        bot_severity: item.bot_severity.clone(),
        bot_status: item.github.bot_status,
        replies: Vec::new(),
        earlier_replies: 0,
    }
}

/// `item` with the commit it was written on (the queue's `written` fact) and the replies of its thread from the last
/// read.
fn with_context(
    mut item: VerifyItem,
    context: Option<&ItemContext>,
    written: Option<&Written>,
) -> VerifyItem {
    item.commit = written
        .and_then(|written| written.commit.as_ref())
        .map(|commit| commit.0.clone());
    if let Some(context) = context {
        item.replies = context
            .replies
            .iter()
            .map(|reply| ThreadReply {
                body: context::cut(&reply.body),
                ..reply.clone()
            })
            .collect();
        item.earlier_replies = context.earlier_replies;
    }
    item
}

/// The standing decision (not superseded, not undone) of every other item of the pull request, then for each sent
/// item the latest reply in its thread that records a maintainer's decision.
async fn earlier_decisions(
    state: &AppState,
    pr: i64,
    sent: &[&ItemRecord],
    contexts: &BTreeMap<String, ItemContext>,
) -> Result<Vec<EarlierDecision>, AssessError> {
    let items = state.db.review_loop().items(pr).await?;
    let decisions = state.db.review_loop().pr_decisions(pr).await?;
    let standing = super::standing(&decisions);
    let mut current = Vec::new();
    for item in items
        .iter()
        .filter(|item| sent.iter().all(|sent| sent.id != item.id))
    {
        if let Some(stored) = standing.get(&item.id.to_string()) {
            current.push(EarlierDecision {
                item: format!("C{}", item.no),
                decision: Some(stored.decision.kind),
                reason: stored.decision.reason.clone(),
            });
        }
    }
    for item in sent {
        if let Some(reply) = contexts
            .get(&item.stable_key)
            .and_then(context::decision_reply)
        {
            current.push(EarlierDecision {
                item: format!("C{}", item.no),
                decision: None,
                reason: Some(context::cut(&reply.body)),
            });
        }
    }
    Ok(current)
}

/// The PR context of the PR's latest analysis and the project profile a pass's input carries.
#[derive(Default)]
struct AnalysisInputs {
    /// The analysis's PR context, as one text.
    pr_context: Option<String>,
    /// The project profile file at the PR's base.
    profile: Option<String>,
}

/// The project profile file at the pull request's current base (`summary`), read from the server's mirror, and the
/// PR context of the PR's latest analysis in the tenant (any head; `language` first). Only when the mirror cannot
/// be read does the profile come from that analysis's stored prepass, and only if it read the same base.
async fn analysis_inputs(
    state: &AppState,
    tenant: &str,
    pr: i64,
    summary: &PrSummary,
    language: LlmLanguage,
) -> Result<AnalysisInputs, AssessError> {
    let base = match base_profile(state, summary).await {
        Ok(profile) => Some(profile),
        Err(error) => {
            tracing::debug!(%error, "project profile not readable from the mirror; an analysis's is used");
            None
        }
    };
    let Some(analysis) = state
        .db
        .analyses()
        .latest_for_pr(pr, language)
        .await?
        .filter(|analysis| analysis.tenant_id == tenant)
    else {
        return Ok(AnalysisInputs {
            pr_context: None,
            profile: base
                .flatten()
                .map(|text| text.chars().take(prompts::MAX_PROFILE_CHARS).collect()),
        });
    };
    let context: Option<PrContextOutput> = state
        .db
        .documents()
        .get(analysis.id, DocumentKind::PrContext, "")
        .await?;
    let profile = match base {
        Some(profile) => profile,
        // Another base's profile is not the current one.
        None if analysis.base_sha != summary.base_sha => None,
        None => crate::pipeline::prepass::stored(state, analysis.id)
            .await?
            .and_then(|prepass| prepass.profile_file),
    };
    Ok(AnalysisInputs {
        pr_context: context.map(|context| {
            format!(
                "Problem: {} Why needed: {} What it gives: {}",
                context.problem, context.why_needed, context.what_it_gives
            )
        }),
        profile: profile.map(|text| text.chars().take(prompts::MAX_PROFILE_CHARS).collect()),
    })
}

/// The project profile file at the pull request's base, from the server's mirror; `None` when it is absent or
/// empty there.
async fn base_profile(state: &AppState, pr: &PrSummary) -> Result<Option<String>, MirrorError> {
    let mirror = mirror::ensure_commits(&state.config, &pr.repo, &[&pr.base_sha]).await?;
    let bytes = mirror::read_blob(&mirror, &pr.base_sha, analysis::tech::PROFILE_PATH).await?;
    Ok(bytes
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .filter(|text| !text.trim().is_empty()))
}

/// The owner's question with the earlier questions of its thread.
async fn item_question(
    state: &AppState,
    question: &QuestionRecord,
    sent: &[&ItemRecord],
) -> Result<ItemQuestion, AssessError> {
    let thread = state
        .db
        .questions()
        .thread_before(question.question.id, Timestamp::now())
        .await?
        .into_iter()
        .map(|earlier| ThreadEntry {
            question: earlier.question.text,
            answer: earlier.question.answer.map(|answer| answer.answer),
        })
        .collect();
    Ok(ItemQuestion {
        item: sent
            .first()
            .map(|item| format!("C{}", item.no))
            .unwrap_or_default(),
        text: question.question.text.clone(),
        thread,
    })
}

/// Submits the pass's `verify_comments` job (idempotent per pass).
async fn submit(
    state: &AppState,
    pr: &PrSummary,
    pull_request_id: i64,
    pass: PassId,
    input: &VerifyCommentsInput,
    language: LlmLanguage,
) -> Result<JobHandle, AssessError> {
    let spec = JobSpec {
        job_id: JobId::new_random(),
        attempt: 0,
        kind: JobKind::VerifyComments,
        target: JobTarget::PullRequestComments(PullRequestCommentsTarget {
            pull_request_id,
            repo: RepoCheckout {
                clone_url: endpoint(
                    &state.config.github.clone_url,
                    &format!("{}/{}.git", pr.repo.owner, pr.repo.name),
                )
                .to_string(),
                base_sha: pr.base_sha.clone(),
                head_sha: pr.head_sha.clone(),
            },
            pass_id: pass,
        }),
        prompt: prompt::render(input, pr, language)?,
        output_schema: agent::output_schema(JobKind::VerifyComments)?,
        flow_file: None,
        assessment: None,
        pr_data: None,
        model: select_model(
            JobKind::VerifyComments,
            &ModelContext::default(),
            state.db.settings().llm_selection().await?.as_ref(),
        ),
        limits: JobLimits::default(),
        payer: contracts::ids::UserId(
            state
                .db
                .review_loop()
                .pass(pass)
                .await?
                .user_id
                .ok_or(DbError::NotFound)?,
        ),
        session: None,
        working_session: None,
        context_pack: None,
        skills: Vec::new(),
        access: AgentAccess::default(),
    };
    Ok(state
        .executor
        .submit(JobRequest {
            spec,
            priority: JobPriority::Interactive,
            stage: JobStage::Verify(pass),
            generation: 0,
            pr_repo: Some(pr.repo.clone()),
        })
        .await?)
}

/// Starts the follower of `pass` unless one runs already.
fn follow(state: &AppState, pass: PassId, known: Option<JobId>) {
    if !FOLLOWING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(pass)
    {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = run(&state, pass, known).await {
            tracing::error!(%pass, error = ?error, "the comment assessment pass could not be followed");
            if let Err(error) = end_failed(&state, pass).await {
                tracing::error!(%pass, error = ?error, "could not end the failed comment assessment pass");
            }
        }
        FOLLOWING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&pass);
    });
}

/// Follows a pass's job to its result and ingests it; then requests the next pass for what is left. A pass not yet
/// linked to its job (`known` from the submit, else the latest job of the pass in any state, after a restart) is
/// linked first; the job is followed even while the link keeps failing, so its result is never abandoned.
async fn run(state: &AppState, pass: PassId, known: Option<JobId>) -> Result<(), AssessError> {
    let record = state.db.review_loop().pass(pass).await?;
    let job = match record.job_id.or(known) {
        Some(job) => Some(job),
        None => state
            .db
            .jobs()
            .latest_for_pass(pass, 0)
            .await?
            .map(|job| job.id),
    };
    let Some(job) = job else {
        tracing::warn!(%pass, "comment assessment pass without a job ends failed; the next request starts again");
        state
            .db
            .review_loop()
            .finish_pass(pass, false, Timestamp::now())
            .await?;
        unanswerable(state, &record).await?;
        next(state, &record, false).await;
        return Ok(());
    };
    if record.job_id.is_none() {
        link_with_retries(state, &record, job).await;
    }
    let mut updates = state.executor.events(&JobHandle { job_id: job });
    while let Some(update) = updates.next().await {
        match update? {
            JobUpdate::Event(_) | JobUpdate::Requeued { .. } => {
                ask::answering(state, &record).await?;
            }
            JobUpdate::Paused { resumes_at } => {
                tracing::info!(%pass, ?resumes_at, "comment assessment pass waits for the LLM usage limit");
                ask::pending(state, &record).await?;
            }
            JobUpdate::OutputReady(_) => {}
            JobUpdate::Finished { result, .. } => {
                let complete = ingest::result(state, &record, result).await?;
                next(state, &record, complete).await;
                return Ok(());
            }
        }
    }
    Err(AssessError::NoResult)
}

/// Links the pass's question and then the pass to `job` (the question first: while the pass has no job, the link
/// is made again).
async fn link(
    state: &AppState,
    pass: PassId,
    question: Option<QuestionId>,
    job: JobId,
) -> Result<(), DbError> {
    if let Some(question) = question {
        state.db.questions().set_job(question, job).await?;
    }
    state.db.review_loop().pass_running(pass, job).await
}

/// [`link`] retried with a doubling delay from [`LINK_RETRY_DELAY`], [`LINK_ATTEMPTS`] times in all.
async fn link_with_retries(state: &AppState, record: &PassRecord, job: JobId) {
    let mut delay = LINK_RETRY_DELAY;
    for attempt in 1..=LINK_ATTEMPTS {
        match link(state, record.id, record.question, job).await {
            Ok(()) => return,
            Err(error) if attempt == LINK_ATTEMPTS => {
                tracing::warn!(pass = %record.id, %job, ?error,
                    "the assessment job could not be linked to its pass; it is followed unlinked and linked again after a restart");
            }
            Err(_) => {
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
        }
    }
}

/// A pass's question whose pass has no job: one linked to a job that no longer exists can never be taken again, so
/// it fails; an unlinked one is taken by the next question pass.
async fn unanswerable(state: &AppState, record: &PassRecord) -> Result<(), AssessError> {
    let Some(question) = record.question else {
        return Ok(());
    };
    let linked = state
        .db
        .questions()
        .get(question, Timestamp::now())
        .await?
        .job_id
        .is_some();
    if linked {
        ask::fail_pass_question(state, record, ask::SERVER_ERROR).await?;
    }
    Ok(())
}

/// Ends a pass that could not be followed and fails its question.
async fn end_failed(state: &AppState, pass: PassId) -> Result<(), AssessError> {
    let record = state.db.review_loop().pass(pass).await?;
    match state
        .db
        .review_loop()
        .finish_pass(pass, false, Timestamp::now())
        .await
    {
        Ok(()) | Err(DbError::Conflict) => {}
        Err(error) => return Err(error.into()),
    }
    ask::fail_pass_question(state, &record, ask::SERVER_ERROR).await
}

/// After a pass: the next question pass, else, when `complete` (every sent item assessed) and the pass was full,
/// the rest of the same detection. A pass with an unassessed item ends the chain, so failing items are never sent
/// again and again; they wait for the next trigger.
async fn next(state: &AppState, record: &PassRecord, complete: bool) {
    let pr = record.pull_request_id;
    if let Err(error) = super::watch::pass_ended(state, record).await {
        tracing::warn!(pr, error = ?error, "the end of an automatic assessment pass could not be recorded on its watches");
    }
    let rest = match record.cause {
        PassCause::Watch => Some(PassReason::Watch),
        PassCause::Opened | PassCause::AssessAgain => Some(PassReason::Opened),
        PassCause::Read => Some(PassReason::NewComments),
        PassCause::Question => None,
    }
    .filter(|_| complete && record.sent.len() == MAX_ITEMS_PER_PASS);
    let outcome = match request(state, pr, PassReason::Ask, true, None).await {
        Ok(PassQueued::Skipped) => match rest {
            Some(reason) => request(state, pr, reason, true, record.user_id.as_deref()).await,
            None => Ok(PassQueued::Skipped),
        },
        other => other,
    };
    match outcome {
        Ok(queued) => tracing::debug!(pr, ?queued, "next comment assessment pass requested"),
        Err(error) => {
            tracing::warn!(pr, error = ?error, "the next comment assessment pass could not be requested")
        }
    }
}

/// Follows every live pass again and requests a question pass for each pull request with an unsent question
/// (server start).
pub async fn resume(state: &AppState) -> Result<usize, DbError> {
    let live = state.db.review_loop().live_passes().await?;
    for pass in &live {
        follow(state, pass.id, None);
    }
    for pr in state.db.questions().pull_requests_with_unsent().await? {
        if let Err(error) = request_pass(state, pr, PassReason::Ask).await {
            tracing::warn!(pr, error = ?error, "a question pass could not be requested at start");
        }
    }
    if !live.is_empty() {
        tracing::info!(
            passes = live.len(),
            "following the live comment assessment passes again"
        );
    }
    Ok(live.len())
}
