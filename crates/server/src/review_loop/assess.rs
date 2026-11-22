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

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, PoisonError};

use contracts::agent::verify_comments::{
    EarlierDecision, InputError, ItemQuestion, ThreadEntry, VerifyCommentsInput, VerifyItem,
};
use contracts::agent::{self as agent, JobKind, StrictSchemaError};
use contracts::api::pr::PrSummary;
use contracts::api::questions::AskAnchor;
use contracts::api::review_loop::{CommentItemKind, ItemState};
use contracts::common::LlmLanguage;
use contracts::ids::{CommitSha, JobId, PassId};
use contracts::job::access::AgentAccess;
use contracts::job::spec::{
    CredentialKind, JobLimits, JobPriority, JobSpec, JobTarget, LlmCredentialRef,
    PullRequestCommentsTarget, RepoCheckout,
};
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::jobs::JobStage;
use db::repo::questions::QuestionRecord;
use db::repo::review_loop::{
    ItemRecord, NewPass, PassCause, PassRecord, PassStart, SentItem, StoredAssessment,
};
use futures::StreamExt;
use jiff::SignedDuration;
use sha2::{Digest, Sha256};

pub use ask::{ItemAskError, ask_about_item, list_questions};

use crate::github::mirror::{self, MirrorError};
use crate::github::oauth::endpoint;
use crate::jobs::executor::{ExecutorError, JobHandle, JobRequest, JobUpdate};
use crate::model::{ModelContext, select_model};
use crate::pipeline::CREDENTIAL_ID;
use crate::state::AppState;

/// Items one `verify_comments` job assesses at most (plan §4.2).
pub const MAX_ITEMS_PER_PASS: usize = agent::verify_comments::MAX_ITEMS;
/// Least time between the end of a pass and an automatic (Watch) pass of the same pull request (plan §2.8).
// TODO(S13-3): calibrate the spacing against real Codex pass runtimes once the spike is approved.
pub const PASS_SPACING: SignedDuration = SignedDuration::from_mins(10);
/// Passes charged to one user per 24 hours before automatic (Watch) passes stop (plan §2.8).
// TODO(S13-3): calibrate against real Codex runs (tokens and runtime per pass) once the spike is approved.
pub const DAILY_PASSES_PER_USER: u32 = 30;
/// Version of `prompts/verify_comments.md` with the output schema, stored with every assessment.
pub const VERIFY_PROMPT_VERSION: u32 = 1;
/// The window of the daily cap.
const DAY: SignedDuration = SignedDuration::from_hours(24);

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
}

/// What [`request_pass`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PassQueued {
    /// A pass was queued with its job.
    Queued,
    /// A pass of the pull request is queued or running already.
    AlreadyRunning,
    /// Nothing to assess, no review-loop read yet, or spacing or the daily cap holds an automatic pass back.
    Skipped,
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
/// and stops at [`DAILY_PASSES_PER_USER`]; a question pass answers the oldest question no pass took yet.
pub async fn request_pass(
    state: &AppState,
    pr: i64,
    reason: PassReason,
) -> Result<PassQueued, AssessError> {
    request(state, pr, reason, false).await
}

/// [`request_pass`]; `continuation` (the rest of a detection after a pass) skips the spacing, never the cap.
async fn request(
    state: &AppState,
    pr: i64,
    reason: PassReason,
    continuation: bool,
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
        PassReason::Watch => match charged_watcher(state, pr, now, continuation).await? {
            Some(user) => (Some(user), None),
            None => return Ok(PassQueued::Skipped),
        },
        PassReason::Opened | PassReason::NewComments => (None, None),
    };
    let items = db.review_loop().items(pr).await?;
    let asked = question
        .as_ref()
        .and_then(|question| match question.question.anchor {
            AskAnchor::CommentItem { item } => Some(item),
            _ => None,
        });
    let head = &record.pr.head_sha;
    let mut code = CodeReader::new(state, &record.pr);
    let mut sent = Vec::new();
    let mut sent_items = Vec::new();
    for item in &items {
        let wanted = match asked {
            Some(asked) => item.id == asked,
            None => assessable(item),
        };
        if !wanted {
            continue;
        }
        let fingerprint = fingerprint(item, head, code.lines(item).await.as_deref());
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
    let pass = NewPass {
        pull_request_id: pr,
        user_id: user.clone(),
        cause: match reason {
            PassReason::Opened => PassCause::Opened,
            PassReason::NewComments => PassCause::Read,
            PassReason::Watch => PassCause::Watch,
            PassReason::Ask => PassCause::Question,
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
    let language = match &user {
        Some(user) => db.users().get(user).await?.language,
        None => LlmLanguage::En,
    };
    let input = VerifyCommentsInput {
        pull_request: format!(
            "{}/{}#{}",
            record.pr.repo.owner, record.pr.repo.name, record.pr.number
        ),
        title: record.pr.title.clone(),
        pr_context: pr_state.purpose.clone(),
        items: sent_items.iter().map(|item| verify_item(item)).collect(),
        earlier_decisions: earlier_decisions(state, pr, &sent_items).await?,
        question: match &question {
            Some(question) => Some(item_question(state, question, &sent_items).await?),
            None => None,
        },
    };
    let submitted = submit(state, &record.pr, pr, id, &input, language).await;
    let handle = match submitted {
        Ok(handle) => handle,
        Err(error) => {
            db.review_loop()
                .finish_pass(id, false, Timestamp::now())
                .await?;
            return Err(error);
        }
    };
    db.review_loop().pass_running(id, handle.job_id).await?;
    if let Some(question) = &question {
        db.questions()
            .set_job(question.question.id, handle.job_id)
            .await?;
    }
    tracing::info!(pr, pass = %id, job = %handle.job_id, ?reason, items = input.items.len(),
        "comment assessment pass queued");
    follow(state, id);
    Ok(PassQueued::Queued)
}

/// The first watcher of `pr` under the daily cap, unless the spacing holds the pass back (not for a continuation).
async fn charged_watcher(
    state: &AppState,
    pr: i64,
    now: Timestamp,
    continuation: bool,
) -> Result<Option<String>, AssessError> {
    let watches = state.db.review_loop().live_watches().await?;
    let mut watchers: Vec<_> = watches
        .iter()
        .filter(|watch| watch.pull_request_id == pr)
        .collect();
    watchers.sort_by_key(|watch| watch.since);
    for watch in watchers {
        let (passes, last_end) = state
            .db
            .review_loop()
            .pass_usage(&watch.user_id, now - DAY, pr)
            .await?;
        if !continuation && last_end.is_some_and(|end| end + PASS_SPACING > now) {
            tracing::debug!(pr, "automatic assessment pass held back by the spacing");
            return Ok(None);
        }
        if passes < DAILY_PASSES_PER_USER {
            return Ok(Some(watch.user_id.clone()));
        }
    }
    tracing::info!(
        pr,
        "automatic assessment pass not started: no watcher is under the daily limit"
    );
    Ok(None)
}

/// Items a pass assesses: comments of open, undecided items (not PR-level questions, which the server asks).
fn assessable(item: &ItemRecord) -> bool {
    item.kind != CommentItemKind::PrQuestion
        && matches!(
            item.state,
            ItemState::Assessing
                | ItemState::Undecided
                | ItemState::AskedBack
                | ItemState::Reopened
        )
}

/// SHA-256 hex of the prompt version, the normalised comment text and the code at its lines; code that cannot be
/// read counts as changed at every new head.
fn fingerprint(item: &ItemRecord, head: &CommitSha, code: Option<&str>) -> String {
    let mut hash = Sha256::new();
    hash.update(format!("{VERIFY_PROMPT_VERSION}\n{}\n", item.body_hash));
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

/// An item as the agent reads it: `C<no>` and its comment, never its id or GitHub facts.
pub(crate) fn verify_item(item: &ItemRecord) -> VerifyItem {
    VerifyItem {
        id: format!("C{}", item.no),
        kind: item.kind,
        author: item.author.clone(),
        author_kind: item.author_kind,
        location: item.location.clone(),
        title: item.title.clone(),
        body: item.body.clone(),
        bot_severity: item.bot_severity.clone(),
    }
}

/// The current decision of every other item of the pull request (an item's newest decision is current).
async fn earlier_decisions(
    state: &AppState,
    pr: i64,
    sent: &[&ItemRecord],
) -> Result<Vec<EarlierDecision>, AssessError> {
    let items = state.db.review_loop().items(pr).await?;
    let decisions = state.db.review_loop().pr_decisions(pr).await?;
    let mut current = Vec::new();
    for item in items
        .iter()
        .filter(|item| sent.iter().all(|sent| sent.id != item.id))
    {
        if let Some(last) = decisions.iter().rev().find(|stored| stored.item == item.id) {
            current.push(EarlierDecision {
                item: format!("C{}", item.no),
                decision: last.decision.kind,
                reason: last.decision.reason.clone(),
            });
        }
    }
    Ok(current)
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
        model: select_model(
            JobKind::VerifyComments,
            &ModelContext::default(),
            state.db.settings().llm_selection().await?.as_ref(),
        ),
        limits: JobLimits::default(),
        credential: LlmCredentialRef {
            id: CREDENTIAL_ID.to_owned(),
            kind: CredentialKind::CodexChatgpt,
        },
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
fn follow(state: &AppState, pass: PassId) {
    if !FOLLOWING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(pass)
    {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = run(&state, pass).await {
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

/// Follows a pass's job to its result and ingests it; then requests the next pass for what is left.
async fn run(state: &AppState, pass: PassId) -> Result<(), AssessError> {
    let record = state.db.review_loop().pass(pass).await?;
    let job = match record.job_id {
        Some(job) => job,
        // The server stopped between queueing the pass and recording its job: adopt a submitted job.
        None => match state.db.jobs().current_for_pass(pass, 0).await? {
            Some(job) => {
                state.db.review_loop().pass_running(pass, job.id).await?;
                if let Some(question) = record.question {
                    state.db.questions().set_job(question, job.id).await?;
                }
                job.id
            }
            None => {
                tracing::warn!(%pass, "comment assessment pass without a job ends failed; the next request starts again");
                state
                    .db
                    .review_loop()
                    .finish_pass(pass, false, Timestamp::now())
                    .await?;
                next(state, &record, false).await;
                return Ok(());
            }
        },
    };
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
    let rest = match record.cause {
        PassCause::Watch => Some(PassReason::Watch),
        PassCause::Opened | PassCause::AssessAgain => Some(PassReason::Opened),
        PassCause::Read => Some(PassReason::NewComments),
        PassCause::Question => None,
    }
    .filter(|_| complete && record.sent.len() == MAX_ITEMS_PER_PASS);
    let outcome = match request(state, pr, PassReason::Ask, true).await {
        Ok(PassQueued::Skipped) => match rest {
            Some(reason) => request(state, pr, reason, true).await,
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
        follow(state, pass.id);
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
