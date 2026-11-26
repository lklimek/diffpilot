//! LLM judgement of scanned items (plan §B, ADR-017): the ranking of a stored snapshot as `judge_work` jobs through
//! the job executor, reuse of stored judgements by the judged-input fingerprint and prompt version, the caps, and
//! the judgements the scorer reads. A failed or invalid judgement only leaves items provisional: it never fails the
//! refresh, hides an item or reaches GitHub.

mod input;
mod prompt;
mod store;
#[cfg(test)]
mod tests;

use contracts::agent::JobKind;
use contracts::agent::judge_work::{
    InputError, JudgeItem, JudgeWorkInput, JudgeWorkOutput, MAX_ITEMS,
};
use contracts::agent::strict::StrictSchemaError;
use contracts::api::dashboard::{
    ItemKey, ItemTarget, RankingLimit, RankingState, RefreshState, RefreshStatus, ScanStep,
    ScanStepKey, StepState,
};
use contracts::common::LlmLanguage;
use contracts::ids::{JobId, RefreshId};
use contracts::job::access::AgentAccess;
use contracts::job::result::{JobOutcome, JobResult};
use contracts::job::spec::{JobLimits, JobPriority, JobSpec, JobTarget};
use contracts::job::state::JobState;
use db::error::DbError;
use db::repo::Db;
use db::repo::dashboard::{
    JUDGEMENT_RETENTION_DAYS, JudgeCounts, JudgeSend, RankingUpdate, RankingWrite, RefreshProgress,
    RefreshRecord, SendCaps, SendReservation, StoredJudgement,
};
use db::repo::jobs::{JobStage, MAX_REPAIR_RUNS};
use futures::StreamExt;
use futures::future::BoxFuture;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::time::MissedTickBehavior;

use contracts::time::Timestamp;
use jiff::SignedDuration;
use jiff::tz::TimeZone;

use self::input::Candidate;
pub use self::input::JUDGE_INPUT_VERSION;
use self::store::Body;
use super::refresh::rank::{ItemJudgement, RankMode, RankRun, Ranker};
use super::refresh::{self, Scope};
use super::score::{Freshness, ScoreRules};
use super::snapshot::{self, Snapshot};
use crate::jobs::executor::{ExecutorError, JobHandle, JobRequest, JobUpdate};
use crate::model::{ModelContext, select_model};
use crate::state::AppState;

/// Version of the judge prompt and output schema; a mismatch makes a stored judgement stale.
pub const JUDGE_PROMPT_VERSION: u32 = 7;
/// Items sent to the judge per refresh at most, across its attempts and retries (plan §B.4).
pub const REFRESH_CAP: u32 = 48;
/// Items sent to the judge per user and local day at most, retries included (plan §B.4).
pub const DAILY_CAP: u32 = 120;
/// Further runs of a batch whose job failed or whose answer was rejected (as the pipeline's repair runs).
const RETRIES: u32 = MAX_REPAIR_RUNS;
/// How often a waiting judge job is looked at: credential pause, a running analysis job, the claim.
const TICK: Duration = Duration::from_secs(1);
/// A judge job still queued after this long, with the credential usable and no analysis job holding it, ends the
/// ranking: no runner is picking jobs up.
const UNCLAIMED_LIMIT: Duration = Duration::from_secs(10 * 60);
/// Display ids named in a failure message, at most.
const NAMED_ITEMS: usize = 5;
/// Job updates buffered between their reader task and the ranking.
const UPDATES_BUFFERED: usize = 16;

/// The ranking step: judges what the scorer asks about with `judge_work` jobs.
#[derive(Clone, Copy, Debug, Default)]
pub struct JudgeRanker;

impl Ranker for JudgeRanker {
    fn rank(&self, state: AppState, run: RankRun) -> BoxFuture<'static, ()> {
        Box::pin(rank(state, run))
    }

    fn judgements<'a>(
        &'a self,
        state: &'a AppState,
        scope: &'a Scope,
        snapshot: &'a Snapshot,
    ) -> BoxFuture<'a, Result<HashMap<ItemKey, ItemJudgement>, DbError>> {
        Box::pin(judgements(state, scope, snapshot))
    }
}

/// Why a ranking stopped before its end.
#[derive(Debug, thiserror::Error)]
enum RankError {
    /// The refresh is no longer ranking in this attempt: late results are dropped.
    #[error("the ranking was superseded, cancelled or restarted")]
    Stale,
    #[error("the dashboard data could not be read or written")]
    Db(#[from] DbError),
    #[error("the judge job could not be submitted or followed")]
    Executor(#[from] ExecutorError),
    #[error("the judge input could not be built")]
    Input(#[from] InputError),
    #[error("the judge prompt could not be rendered")]
    Prompt(#[from] crate::prompts::PromptError),
    #[error("the judge output schema is unavailable")]
    Schema(#[from] StrictSchemaError),
    #[error("the judgement could not be stored")]
    Json(#[from] serde_json::Error),
    #[error("the refresh holds no readable snapshot")]
    NoSnapshot,
    #[error("the LLM credential is not usable")]
    Unusable,
    #[error("the judge job failed without an automatic retry")]
    JobFailed(contracts::job::result::JobFailureKind),
    #[error("a judge job was cancelled outside the ranking")]
    JobCancelled,
    #[error("no runner claimed the judge job in time")]
    Unclaimed,
}

impl RankError {
    /// The plain ranking failure message (`RankingStatus.message`).
    fn message(&self) -> &'static str {
        match self {
            Self::JobFailed(kind) => crate::api::settings::plain_failure(Some(
                db::repo::settings::CallFailure::Failed(*kind),
            )),
            Self::Unusable
            | Self::Db(db::error::DbError::CredentialUnlinked)
            | Self::Executor(ExecutorError::Db(db::error::DbError::CredentialUnlinked)) => {
                "Link your Codex account in Settings"
            }
            Self::JobCancelled => "A ranking job was cancelled",
            Self::Unclaimed => "No runner picked up the ranking job",
            Self::NoSnapshot => "The scan result could not be read",
            Self::Stale
            | Self::Db(_)
            | Self::Executor(_)
            | Self::Input(_)
            | Self::Prompt(_)
            | Self::Schema(_)
            | Self::Json(_) => "The ranking stopped on a server error",
        }
    }
}

/// How a ranking ends.
struct End {
    outcome: RankingState,
    message: Option<String>,
}

/// How a batch ended.
enum BatchEnd {
    /// Every item judged.
    Judged,
    /// These display ids stayed unjudged after the last run.
    Unjudged(Vec<String>),
    /// A further run of these items exceeded a cap (named) and was not sent.
    Capped(Vec<String>, RankingLimit),
}

/// An item queued for judgement.
#[derive(Clone, Debug)]
struct Queued {
    item: JudgeItem,
    fingerprint: String,
    /// No readable judgement is stored.
    new: bool,
    /// Requested before this scan's bulk re-judge.
    requested: bool,
    /// Provisional points within a priority group.
    points: f64,
}

/// Ranks `run`'s snapshot and ends its ranking; a superseded or cancelled ranking (task aborted) cancels its jobs.
async fn rank(state: AppState, run: RankRun) {
    let mut guard = CancelOnDrop {
        state: state.clone(),
        refresh: run.refresh_id,
        attempt: run.attempt,
        armed: true,
    };
    let mut ranking = Ranking {
        state: &state,
        run: &run,
        counts: JudgeCounts::default(),
        live: RankingState::Running,
        zone: TimeZone::UTC,
        language: LlmLanguage::En,
        left_provisional_keys: Vec::new(),
        bulk_rejudge_keys: Vec::new(),
        unclaimed_since: None,
        provisional: None,
        left_provisional_by: None,
    };
    let end = match ranking.run().await {
        Ok(None) => {
            guard.armed = false;
            return;
        }
        Ok(Some(end)) => Some(end),
        Err(RankError::Stale) => None,
        Err(error) => {
            tracing::warn!(%error, refresh = %run.refresh_id,
                "dashboard ranking stopped early; unjudged items keep their provisional scores");
            Some(End {
                outcome: RankingState::Failed,
                message: Some(error.message().to_owned()),
            })
        }
    };
    cancel_jobs(&state, run.refresh_id, Some(run.attempt)).await;
    guard.armed = false;
    let _writer = state.dashboard.readers().writer(&run.scope).await;
    match end {
        Some(end) => ranking.finish(end).await,
        None => tracing::debug!(refresh = %run.refresh_id, attempt = run.attempt,
            "dashboard ranking no longer current; its late results are dropped"),
    }
    refresh::continue_rejudges(&state, &run.scope, run.refresh_id, run.attempt).await;
}

/// One ranking attempt in progress.
struct Ranking<'a> {
    state: &'a AppState,
    run: &'a RankRun,
    counts: JudgeCounts,
    live: RankingState,
    zone: TimeZone,
    /// The user's LLM text language, read when the ranking starts.
    language: LlmLanguage,
    /// Never-judged items left behind by the caps.
    left_provisional_keys: Vec<ItemKey>,
    bulk_rejudge_keys: Vec<ItemKey>,
    /// Since when the followed job waits for a runner (queued, credential usable, no analysis job running).
    unclaimed_since: Option<tokio::time::Instant>,
    /// Which cap left items provisional, as the ranking step's detail shows it while no other detail is given.
    provisional: Option<String>,
    left_provisional_by: Option<RankingLimit>,
}

impl<'a> Ranking<'a> {
    fn scope(&self) -> (&'a str, &'a str) {
        (&self.run.scope.tenant_id, &self.run.scope.user_id)
    }

    async fn run(&mut self) -> Result<Option<End>, RankError> {
        let (tenant, user) = self.scope();
        let dashboard = self.state.db.dashboard();
        retention(&self.state.db, self.state.dashboard.now()).await;
        let record = dashboard.refresh(tenant, user, self.run.refresh_id).await?;
        if record.state != RefreshState::Ranking || record.ranking_attempt != self.run.attempt {
            return Err(RankError::Stale);
        }
        self.counts.judged = record.counts.judged;
        if matches!(self.run.mode, RankMode::Rejudge | RankMode::Pending) {
            self.counts.left_provisional = record.counts.left_provisional;
            self.left_provisional_keys = record.progress.left_provisional_keys;
            self.left_provisional_by = record.progress.left_provisional_by;
        }
        self.zone = ranking_zone(self.state, tenant, user).await?;
        self.language = self.state.db.users().get(user).await?.language;
        let mut first = true;
        let mut batch = 0;
        let mut capped = HashSet::new();
        let per_refresh = matches!(self.run.mode, RankMode::Scan | RankMode::ScanWithRejudge)
            .then_some(REFRESH_CAP);
        let readers = self.state.dashboard.readers();
        loop {
            // Hold the same guard as a re-judge request until we commit the end of an empty drain.
            let writer = readers.writer(&self.run.scope).await;
            let (holder, snapshot) = snapshot::load(self.state, tenant, user)
                .await?
                .ok_or(RankError::NoSnapshot)?;
            if holder.id != self.run.refresh_id
                || holder.state != RefreshState::Ranking
                || holder.ranking_attempt != self.run.attempt
            {
                return Err(RankError::Stale);
            }
            let candidates = input::candidates(&snapshot, self.language);
            let keys: Vec<_> = candidates.iter().map(|c| c.item.key.clone()).collect();
            let stored = dashboard
                .judgements(tenant, user, &keys)
                .await?
                .into_iter()
                .map(|j| (j.item_key.clone(), j))
                .collect();
            let now = self.state.dashboard.now();
            dashboard.mark_seen(tenant, user, &keys, now).await?;
            let bulk = first && self.run.mode == RankMode::ScanWithRejudge;
            let (mut queue, reused) = self.queue(&snapshot, &candidates, &stored, bulk);
            self.counts.reused = reused;
            if self.run.mode == RankMode::Rejudge {
                let targets = readers
                    .rejudge_targets(&self.run.scope, holder.id, holder.ranking_attempt)
                    .unwrap_or_default();
                queue.retain(|q| targets.contains(&q.item.key));
            }
            let pending = &snapshot.pending_event_items;
            if self.run.mode == RankMode::Pending {
                queue
                    .retain(|q| pending.contains(&q.item.key) && !stored.contains_key(&q.item.key));
                let previously_capped = self.left_provisional_keys.len();
                self.left_provisional_keys
                    .retain(|key| !queue.iter().any(|q| &q.item.key == key));
                let reconsidered = count(previously_capped - self.left_provisional_keys.len());
                self.counts.left_provisional =
                    self.counts.left_provisional.saturating_sub(reconsidered);
                if self.counts.left_provisional == 0 {
                    self.left_provisional_by = None;
                }
            }
            if !pending.is_empty()
                && (self.run.mode == RankMode::Pending
                    || (per_refresh.is_some()
                        && !queue.iter().any(|q| q.new && pending.contains(&q.item.key))))
            {
                // A scan leaves unsent event keys for a dedicated attempt with the daily allowance.
                dashboard
                    .clear_event_ranking(tenant, user, holder.id)
                    .await?;
            }
            if !first {
                queue.retain(|q| !capped.contains(&q.item.key));
                if queue.is_empty() {
                    self.finish(End {
                        outcome: RankingState::Finished,
                        message: None,
                    })
                    .await;
                    refresh::continue_rejudges(
                        self.state,
                        &self.run.scope,
                        holder.id,
                        holder.ranking_attempt,
                    )
                    .await;
                    return Ok(None);
                }
            }
            drop(writer);

            if let Some(end) = self
                .rank_queue(queue, per_refresh, &mut batch, &mut capped)
                .await?
            {
                return Ok(Some(end));
            }
            first = false;
        }
    }

    async fn rank_queue(
        &mut self,
        mut queue: Vec<Queued>,
        per_refresh_cap: Option<u32>,
        batch: &mut u32,
        capped: &mut HashSet<ItemKey>,
    ) -> Result<Option<End>, RankError> {
        let (tenant, user) = self.scope();
        let dashboard = self.state.db.dashboard();
        let sent = dashboard
            .sent_totals(
                tenant,
                user,
                self.run.refresh_id,
                day_start(self.state.dashboard.now(), &self.zone),
            )
            .await?;
        let per_refresh = per_refresh_cap.map_or(u32::MAX, |cap| cap.saturating_sub(sent.refresh));
        let per_day = DAILY_CAP.saturating_sub(sent.day);
        let room = usize::try_from(per_refresh.min(per_day)).unwrap_or(usize::MAX);
        // Reserve room for explicit clicks and new items before selecting any bulk requests.
        let required = queue.iter().filter(|q| q.requested || q.new).count();
        let mut bulk_room = room.saturating_sub(required);
        queue.retain(|q| {
            if q.requested || q.new {
                return true;
            }
            if bulk_room == 0 {
                return false;
            }
            bulk_room -= 1;
            self.bulk_rejudge_keys.push(q.item.key.clone());
            true
        });
        let take = queue.len().min(room);
        let left = queue.len() - take;
        if self.run.mode != RankMode::Rejudge {
            self.counts.left_provisional += count(left);
        }
        capped.extend(queue[take..].iter().map(|q| q.item.key.clone()));
        self.left_provisional_keys.extend(
            queue[take..]
                .iter()
                .filter(|q| q.new)
                .map(|q| q.item.key.clone()),
        );
        let reused = self.counts.reused;
        let mut lines = vec![format!(
            "ranking: {take} items to judge, {reused} judgements reused"
        )];
        if left > 0 {
            let cap = if per_day < per_refresh {
                RankingLimit::Daily
            } else {
                RankingLimit::PerRefresh
            };
            let note = format!("{} left provisional: {}", items(left), limit_note(cap));
            self.left_provisional_by = Some(cap);
            lines.push(note.clone());
            self.provisional = Some(note);
        }
        let total = self.counts.judged + count(take);
        self.note(Some(total), None, &lines).await?;
        self.write_state().await?;
        self.publish().await;
        tracing::info!(user_id = %user, refresh = %self.run.refresh_id, attempt = self.run.attempt,
            queued = take, reused, left_provisional = left, "dashboard ranking started");
        let mut not_ranked: Vec<String> = Vec::new();
        let mut stop = None;
        let batches: Vec<&[Queued]> = queue[..take].chunks(MAX_ITEMS).collect();
        for (index, items) in batches.iter().enumerate() {
            *batch += 1;
            let reason = match self.batch(*batch, items, per_refresh_cap).await? {
                BatchEnd::Judged => continue,
                BatchEnd::Unjudged(ids) => {
                    not_ranked = ids;
                    "a batch could not be judged".to_owned()
                }
                BatchEnd::Capped(ids, cap) => {
                    self.left_provisional_by = Some(cap);
                    not_ranked = ids;
                    self.left_provisional_keys.extend(
                        batches[index + 1..]
                            .iter()
                            .flat_map(|b| b.iter())
                            .filter(|q| q.new)
                            .map(|q| q.item.key.clone()),
                    );
                    limit_note(cap)
                }
            };
            stop = Some((
                reason,
                batches[index + 1..].iter().map(|b| b.len()).sum::<usize>(),
            ));
            break;
        }
        if let Some((reason, unsent)) = stop.filter(|(_, unsent)| *unsent > 0) {
            self.counts.left_provisional += count(unsent);
            self.write_state().await?;
            self.note(
                None,
                None,
                &[format!("{} left provisional: {reason}", items(unsent))],
            )
            .await?;
        }
        if not_ranked.is_empty() {
            return Ok(None);
        }
        let named = not_ranked
            .iter()
            .take(NAMED_ITEMS)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let more = not_ranked.len().saturating_sub(NAMED_ITEMS);
        let message = if more > 0 {
            format!(
                "{} items could not be judged ({named} and {more} more)",
                not_ranked.len()
            )
        } else {
            format!("{} items could not be judged ({named})", not_ranked.len())
        };
        self.note(
            None,
            None,
            &[format!("not ranked: {}", not_ranked.join(", "))],
        )
        .await?;
        Ok(Some(End {
            outcome: RankingState::Failed,
            message: Some(message),
        }))
    }

    /// Requests first, followed by never-judged items by provisional score, and the count of fresh answers reused.
    fn queue(
        &self,
        snapshot: &Snapshot,
        candidates: &[Candidate<'_>],
        stored: &HashMap<ItemKey, StoredJudgement>,
        rejudge_outdated: bool,
    ) -> (Vec<Queued>, u32) {
        let rules = ScoreRules::new(&snapshot.own_login, &snapshot.settings);
        let mut queue = Vec::new();
        let mut reused = 0;
        for candidate in candidates.iter().filter(|c| c.asked()) {
            let record = stored.get(&candidate.item.key);
            let body = record.and_then(body_of);
            let requested = record.is_some_and(|r| r.rejudge_requested_at.is_some());
            if let Some(record) =
                record.filter(|r| !requested && body.is_some() && fresh(r, &candidate.fingerprint))
            {
                if record.refresh_id != Some(self.run.refresh_id) {
                    reused += 1;
                }
                continue;
            }
            if body.is_some() && !requested && !rejudge_outdated {
                continue;
            }
            let inherited = record
                .zip(body.as_ref())
                .map(|(record, body)| body.judgement(record.judged_at, Freshness::Inherited));
            let points = candidate
                .score(&rules, inherited.as_ref(), snapshot.scanned_at)
                .map_or(candidate.scored.points, |scored| scored.points);
            let mut item = candidate.item.clone();
            item.previous = body
                .as_ref()
                .map(|body| input::previous(body, self.language));
            queue.push(Queued {
                item,
                fingerprint: candidate.fingerprint.clone(),
                new: body.is_none(),
                requested,
                points,
            });
        }
        queue.sort_by(|a, b| {
            b.requested
                .cmp(&a.requested)
                .then_with(|| a.new.cmp(&b.new))
                .then_with(|| b.points.total_cmp(&a.points))
                .then_with(|| a.item.key.cmp(&b.item.key))
        });
        (queue, reused)
    }

    /// Judges one batch, running it again for the items whose answer failed or was rejected while runs are left
    /// and the caps allow it. Every run's items are counted against the caps before its job is submitted.
    async fn batch(
        &mut self,
        batch: u32,
        items: &[Queued],
        per_refresh: Option<u32>,
    ) -> Result<BatchEnd, RankError> {
        let ids = |pending: &[&Queued]| pending.iter().map(|q| q.item.id.clone()).collect();
        let mut pending: Vec<&Queued> = items.iter().collect();
        for generation in 0..=RETRIES {
            let input = JudgeWorkInput {
                items: pending.iter().map(|q| q.item.clone()).collect(),
            };
            let job = JobId::new_random();
            if let Some(cap) = self.reserve(job, &pending, per_refresh).await? {
                tracing::info!(refresh = %self.run.refresh_id, batch, generation, items = pending.len(), ?cap,
                    "dashboard judge batch not sent: the ranking cap is reached");
                self.left_provisional_keys
                    .extend(pending.iter().filter(|q| q.new).map(|q| q.item.key.clone()));
                return Ok(BatchEnd::Capped(ids(&pending), cap));
            }
            let submitted = self.submit(job, batch, generation, &input).await;
            if !matches!(&submitted, Ok(handle) if handle.job_id == job) {
                self.release(job).await?;
            }
            let handle = submitted?;
            let result = self.follow(&handle).await?;
            pending = self.apply(handle.job_id, &input, pending, result).await?;
            if pending.is_empty() {
                break;
            }
            tracing::info!(refresh = %self.run.refresh_id, batch, generation, items = pending.len(),
                "dashboard judge batch left items unjudged; judging them again while runs are left");
        }
        Ok(if pending.is_empty() {
            BatchEnd::Judged
        } else {
            BatchEnd::Unjudged(ids(&pending))
        })
    }

    /// Counts `items` as sent against both caps; the limit reached when they do not fit (nothing counted).
    async fn reserve(
        &self,
        job_id: JobId,
        items: &[&Queued],
        per_refresh: Option<u32>,
    ) -> Result<Option<RankingLimit>, RankError> {
        let (tenant, user) = self.scope();
        let _writer = self.state.dashboard.readers().writer(&self.run.scope).await;
        let sent_at = self.state.dashboard.now();
        let reserved = self
            .state
            .db
            .dashboard()
            .reserve_sends(
                tenant,
                user,
                self.run.refresh_id,
                self.run.attempt,
                JudgeSend {
                    job_id,
                    items: count(items.len()),
                    sent_at,
                    day_start: day_start(sent_at, &self.zone),
                },
                SendCaps {
                    per_refresh,
                    per_day: DAILY_CAP,
                },
            )
            .await?;
        match reserved {
            SendReservation::Reserved => {
                self.state.dashboard.readers().rejudges_dispatched(
                    &self.run.scope,
                    self.run.refresh_id,
                    self.run.attempt,
                    items.iter().map(|q| &q.item.key),
                );
                Ok(None)
            }
            SendReservation::OverRefreshCap => Ok(Some(RankingLimit::PerRefresh)),
            SendReservation::OverDailyCap => Ok(Some(RankingLimit::Daily)),
            SendReservation::Stale => Err(RankError::Stale),
        }
    }

    async fn release(&self, job: JobId) -> Result<(), RankError> {
        let (tenant, user) = self.scope();
        if self
            .state
            .db
            .dashboard()
            .release_send(tenant, user, job, self.state.dashboard.now())
            .await?
        {
            tracing::info!(user_id = %user, refresh = %self.run.refresh_id, %job,
                "unused dashboard judge reservation released after submission");
        }
        Ok(())
    }

    async fn submit(
        &self,
        job_id: JobId,
        batch: u32,
        generation: u32,
        input: &JudgeWorkInput,
    ) -> Result<JobHandle, RankError> {
        crate::jobs::payer::require_linked(&self.state.db, &self.run.scope.user_id)
            .await
            .map_err(|error| match error {
                crate::jobs::payer::PayerError::Missing => RankError::Unusable,
                crate::jobs::payer::PayerError::Db(error) => RankError::Db(error),
            })?;
        let saved = self.state.db.settings().llm_selection().await?;
        let spec = JobSpec {
            job_id,
            attempt: 0,
            kind: JobKind::JudgeWork,
            target: JobTarget::Dashboard {
                refresh_id: self.run.refresh_id,
            },
            prompt: prompt::render(input, self.language)?,
            output_schema: contracts::agent::output_schema(JobKind::JudgeWork)?,
            flow_file: None,
            assessment: None,
            pr_data: None,
            model: select_model(JobKind::JudgeWork, &ModelContext::default(), saved.as_ref()),
            limits: JobLimits::default(),
            payer: contracts::ids::UserId(self.run.scope.user_id.clone()),
            session: None,
            working_session: None,
            context_pack: None,
            skills: Vec::new(),
            access: AgentAccess::default(),
        };
        let handle = self
            .state
            .executor
            .submit(JobRequest {
                spec,
                priority: JobPriority::Interactive,
                stage: JobStage::Judge {
                    refresh: self.run.refresh_id,
                    batch,
                },
                generation,
                pr_repo: None,
            })
            .await?;
        tracing::debug!(refresh = %self.run.refresh_id, job = %handle.job_id, batch, generation,
            items = input.items.len(), "dashboard judge job submitted");
        Ok(handle)
    }

    /// Follows a judge job to its result, keeping the ranking state in step with it. The job's updates are read
    /// by their own task: a read suspended by a tick could otherwise hold a database connection the tick needs.
    async fn follow(&mut self, handle: &JobHandle) -> Result<JobResult, RankError> {
        let (sender, mut updates) = mpsc::channel(UPDATES_BUFFERED);
        let mut stream = self.state.executor.events(handle);
        let _reader = AbortOnDrop(
            tokio::spawn(async move {
                while let Some(update) = stream.next().await {
                    if sender.send(update).await.is_err() {
                        break;
                    }
                }
            })
            .abort_handle(),
        );
        self.unclaimed_since = None;
        let mut ticks = tokio::time::interval(TICK);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                update = updates.recv() => match update {
                    Some(Ok(JobUpdate::Finished { result, .. })) => return Ok(result),
                    Some(Ok(_)) => self.observe(handle.job_id).await?,
                    Some(Err(error)) => return Err(error.into()),
                    None => return Err(ExecutorError::Closed.into()),
                },
                _ = ticks.tick() => self.observe(handle.job_id).await?,
            }
        }
    }

    /// Reports payer pauses and shared runner-capacity waits without changing container claim eligibility.
    async fn observe(&mut self, job: JobId) -> Result<(), RankError> {
        let db = &self.state.db;
        let record = db.jobs().get(job).await?;
        let live = match record.state {
            JobState::Paused => RankingState::Paused,
            JobState::Queued => {
                let credential = match db.llm_credentials().get(&record.spec.payer.0).await {
                    Ok(credential) => Some(credential),
                    Err(DbError::NotFound) => None,
                    Err(error) => return Err(error.into()),
                };
                if credential
                    .as_ref()
                    .and_then(|c| c.paused_until)
                    .is_some_and(|until| until > Timestamp::now())
                {
                    RankingState::Paused
                } else if credential.is_some_and(|c| c.unusable_since.is_some()) {
                    return Err(RankError::Unusable);
                } else if analysis_running(db).await? {
                    RankingState::WaitingForAnalysis
                } else {
                    RankingState::Running
                }
            }
            JobState::Claimed
            | JobState::Running
            | JobState::Succeeded
            | JobState::Failed
            | JobState::Cancelled => RankingState::Running,
        };
        let unclaimed = record.state == JobState::Queued
            && live == RankingState::Running
            && !db.jobs().judge_busy(&record.spec.payer.0).await?;
        self.unclaimed_since = unclaimed.then(|| {
            self.unclaimed_since
                .unwrap_or_else(tokio::time::Instant::now)
        });
        if self
            .unclaimed_since
            .is_some_and(|since| since.elapsed() >= UNCLAIMED_LIMIT)
        {
            tracing::warn!(refresh = %self.run.refresh_id, %job,
                "dashboard judge job was never claimed; the ranking ends and items stay provisional");
            return Err(RankError::Unclaimed);
        }
        if live != self.live {
            self.live = live;
            self.write_state().await?;
            tracing::debug!(refresh = %self.run.refresh_id, state = ?live, "dashboard ranking state changed");
            self.publish().await;
        }
        Ok(())
    }

    /// Stores the valid answers of a finished job; returns the items to judge again.
    async fn apply<'q>(
        &mut self,
        job: JobId,
        input: &JudgeWorkInput,
        pending: Vec<&'q Queued>,
        result: JobResult,
    ) -> Result<Vec<&'q Queued>, RankError> {
        let output = match result.outcome {
            JobOutcome::Succeeded { output } => output,
            JobOutcome::Failed { kind, .. } => {
                tracing::warn!(refresh = %self.run.refresh_id, %job, ?kind, "dashboard judge job failed");
                return if kind.retryable() {
                    Ok(pending)
                } else {
                    Err(RankError::JobFailed(kind))
                };
            }
            JobOutcome::UsageLimited { .. } => return Ok(pending),
            JobOutcome::Cancelled => return Err(RankError::JobCancelled),
        };
        let answer: JudgeWorkOutput = match serde_json::from_value(output) {
            Ok(answer) => answer,
            Err(error) => {
                tracing::warn!(refresh = %self.run.refresh_id, %job, %error,
                    "dashboard judge answer is not in the judge_work shape; its items are judged again");
                return Ok(pending);
            }
        };
        let validated = answer.validate(input);
        if validated.unknown_answers > 0 {
            tracing::info!(%job, unknown = validated.unknown_answers,
                "dashboard judge answered items outside its batch; those answers are dropped");
        }
        let mut valid = Vec::new();
        let mut again = Vec::new();
        for ((key, outcome), queued) in validated.items.into_iter().zip(pending) {
            match outcome {
                Ok(answer) => valid.push((queued, answer)),
                Err(reason) => {
                    tracing::info!(%job, item = %key, %reason,
                        "dashboard judge answer of an item rejected; the item stays provisional");
                    again.push(queued);
                }
            }
        }
        if valid.is_empty() {
            return Ok(again);
        }
        let model = self.state.db.jobs().get(job).await?.spec.model.model;
        let now = self.state.dashboard.now();
        let judgements = valid
            .into_iter()
            .map(|(queued, answer)| {
                Ok(StoredJudgement {
                    item_key: queued.item.key.clone(),
                    input_fingerprint: queued.fingerprint.clone(),
                    input_version: JUDGE_INPUT_VERSION,
                    prompt_version: JUDGE_PROMPT_VERSION,
                    body: serde_json::to_value(Body::of(answer, &queued.item, self.language))?,
                    refresh_id: Some(self.run.refresh_id),
                    job_id: Some(job),
                    model: model.clone(),
                    judged_at: now,
                    last_seen_at: now,
                    rejudge_requested_at: None,
                })
            })
            .collect::<Result<Vec<_>, serde_json::Error>>()?;
        let mut counts = self.counts;
        counts.judged += count(judgements.len());
        // Counts and judgements in one transaction fenced by the attempt: a late answer is never stored.
        let writer = self.state.dashboard.readers().writer(&self.run.scope).await;
        let before = self.bulk_rejudge_keys.len();
        self.bulk_rejudge_keys
            .retain(|key| !judgements.iter().any(|j| &j.item_key == key));
        let progress = if self.bulk_rejudge_keys.len() < before {
            let (tenant, user) = self.scope();
            let mut record = self
                .state
                .db
                .dashboard()
                .refresh(tenant, user, self.run.refresh_id)
                .await?;
            record
                .progress
                .bulk_rejudge_keys
                .clone_from(&self.bulk_rejudge_keys);
            Some((record.progress, record.log))
        } else {
            None
        };
        self.write(
            RankingState::Running,
            counts,
            &judgements,
            progress
                .as_ref()
                .map(|(progress, log)| (progress, log.as_slice())),
        )
        .await?;
        drop(writer);
        self.counts = counts;
        self.live = RankingState::Running;
        self.publish().await;
        Ok(again)
    }

    /// Records the live state and counts; `Stale` once the refresh is no longer ranking in this attempt.
    async fn write_state(&self) -> Result<(), RankError> {
        self.write(self.live, self.counts, &[], None).await
    }

    /// One fenced ranking write (see `Dashboard::write_ranking`); `Stale` once the refresh is no longer ranking in
    /// this attempt, and then nothing is written.
    async fn write(
        &self,
        state: RankingState,
        counts: JudgeCounts,
        judgements: &[StoredJudgement],
        progress: Option<(&RefreshProgress, &[String])>,
    ) -> Result<(), RankError> {
        let (tenant, user) = self.scope();
        let write = RankingWrite {
            state,
            counts,
            judgements,
            progress,
        };
        let update = self
            .state
            .db
            .dashboard()
            .write_ranking(tenant, user, self.run.refresh_id, self.run.attempt, &write)
            .await;
        match update {
            Ok(RankingUpdate::Applied) => Ok(()),
            Ok(RankingUpdate::Stale) | Err(DbError::Conflict) => Err(RankError::Stale),
            Err(error) => Err(error.into()),
        }
    }

    /// Writes the ranking step's total (when given) and detail (else the provisional note), and appends log lines.
    async fn note(
        &self,
        total: Option<u32>,
        detail: Option<&str>,
        lines: &[String],
    ) -> Result<(), RankError> {
        let (tenant, user) = self.scope();
        let dashboard = self.state.db.dashboard();
        let record = dashboard.refresh(tenant, user, self.run.refresh_id).await?;
        let mut progress = record.progress;
        progress.left_provisional_by = self.left_provisional_by;
        progress
            .left_provisional_keys
            .clone_from(&self.left_provisional_keys);
        progress
            .bulk_rejudge_keys
            .clone_from(&self.bulk_rejudge_keys);
        if !progress.steps.iter().any(|s| s.key == ScanStepKey::Ranking) {
            progress.steps.push(ScanStep {
                key: ScanStepKey::Ranking,
                state: StepState::Running,
                done: None,
                total: None,
                detail: None,
            });
        }
        for step in progress
            .steps
            .iter_mut()
            .filter(|s| s.key == ScanStepKey::Ranking)
        {
            if total.is_some() {
                step.total = total;
            }
            step.detail = detail
                .map(str::to_owned)
                .or_else(|| self.provisional.clone());
        }
        let at = self.state.dashboard.now().to_zoned(self.zone.clone());
        let mut log = record.log;
        log.extend(
            lines
                .iter()
                .map(|line| format!("{} {line}", at.strftime("%H:%M:%S"))),
        );
        self.write(self.live, self.counts, &[], Some((&progress, &log)))
            .await
    }

    async fn publish(&self) {
        refresh::publish(self.state, &self.run.scope, self.run.refresh_id).await;
    }

    /// Ends the ranking with `end`; a late end of a superseded attempt is dropped.
    async fn finish(&self, end: End) {
        if let Some(message) = &end.message
            && let Err(error) = self
                .note(None, Some(message), &[format!("ranking failed: {message}")])
                .await
        {
            tracing::debug!(%error, refresh = %self.run.refresh_id, "dashboard ranking failure text not recorded");
        }
        let (tenant, user) = self.scope();
        let ended = self
            .state
            .db
            .dashboard()
            .finish_ranking(
                tenant,
                user,
                self.run.refresh_id,
                self.run.attempt,
                end.outcome,
                self.counts,
                self.state.dashboard.now(),
            )
            .await;
        match ended {
            Ok(RankingUpdate::Applied) => {
                tracing::info!(user_id = %user, refresh = %self.run.refresh_id,
                outcome = ?end.outcome, judged = self.counts.judged, reused = self.counts.reused,
                left_provisional = self.counts.left_provisional, "dashboard ranking ended")
            }
            Ok(RankingUpdate::Stale) | Err(DbError::Conflict) => {
                tracing::debug!(refresh = %self.run.refresh_id,
                "dashboard ranking ended after it was superseded; its end is dropped")
            }
            Err(error) => tracing::warn!(%error, refresh = %self.run.refresh_id,
                "dashboard ranking end could not be stored; the refresh shows it ranking until Retry or restart"),
        }
        self.publish().await;
    }
}

/// Log text for the cap that leaves items provisional.
fn limit_note(cap: RankingLimit) -> String {
    match cap {
        RankingLimit::Daily => format!("today's ranking limit ({DAILY_CAP})"),
        RankingLimit::PerRefresh => format!("this refresh's ranking limit ({REFRESH_CAP})"),
    }
}

/// `n item(s)`.
fn items(n: usize) -> String {
    if n == 1 {
        "1 item".to_owned()
    } else {
        format!("{n} items")
    }
}

/// The stored judgements that apply to the snapshot's items now: only items the scorer asks about, current when
/// the fingerprint and both versions match, else inherited (provisional).
async fn judgements(
    state: &AppState,
    scope: &Scope,
    snapshot: &Snapshot,
) -> Result<HashMap<ItemKey, ItemJudgement>, DbError> {
    let language = state.db.users().get(&scope.user_id).await?.language;
    let candidates = input::candidates(snapshot, language);
    let asked: HashMap<&ItemKey, &Candidate<'_>> = candidates
        .iter()
        .filter(|c| c.asked())
        .map(|c| (&c.item.key, c))
        .collect();
    let keys: Vec<ItemKey> = asked.keys().map(|key| (*key).clone()).collect();
    let stored = state
        .db
        .dashboard()
        .judgements(&scope.tenant_id, &scope.user_id, &keys)
        .await?;
    let mut applying = HashMap::new();
    for record in stored {
        let (Some(candidate), Some(body)) = (asked.get(&record.item_key), body_of(&record)) else {
            continue;
        };
        let freshness = if fresh(&record, &candidate.fingerprint) {
            Freshness::Current
        } else {
            Freshness::Inherited
        };
        applying.insert(
            record.item_key.clone(),
            ItemJudgement {
                judgement: body.judgement(record.judged_at, freshness),
                text: Some(body.text(record.judged_at, freshness)),
                older_prompt: older(&record),
                rejudge_requested: record.rejudge_requested_at.is_some(),
            },
        );
    }
    Ok(applying)
}

/// Why the user's single-item re-judge request could not be accepted.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RejudgeError {
    #[error("the item's judgement is not outdated")]
    NotOutdated,
    #[error("today's ranking limit has been reached")]
    DailyCap,
    #[error(transparent)]
    Db(#[from] DbError),
}

/// Persists one click under the user's writer guard, or returns false for an already scheduled item.
pub(super) async fn request_rejudge(
    state: &AppState,
    scope: &Scope,
    key: &ItemKey,
) -> Result<bool, RejudgeError> {
    let (tenant, user) = (&scope.tenant_id, &scope.user_id);
    let (holder, snapshot) = snapshot::load(state, tenant, user)
        .await?
        .ok_or(DbError::NotFound)?;
    let same_repo = |a: &contracts::common::RepoRef, b: &contracts::common::RepoRef| {
        a.owner.eq_ignore_ascii_case(&b.owner) && a.name.eq_ignore_ascii_case(&b.name)
    };
    let present = match key.target() {
        ItemTarget::PullRequest { repo, number } => snapshot
            .pull_requests
            .iter()
            .any(|pr| pr.facts.number == number && same_repo(&pr.facts.repo, &repo)),
        ItemTarget::Issue { repo, number } => snapshot
            .issues
            .iter()
            .any(|issue| issue.facts.number == number && same_repo(&issue.facts.repo, &repo)),
        ItemTarget::Todo(id) => snapshot.todos.iter().any(|todo| todo.facts.id == id),
    };
    if !present {
        return Err(DbError::NotFound.into());
    }
    let language = state.db.users().get(user).await?.language;
    let candidate = input::candidates(&snapshot, language)
        .into_iter()
        .find(|candidate| candidate.asked() && candidate.item.key == *key)
        .ok_or(RejudgeError::NotOutdated)?;
    let dashboard = state.db.dashboard();
    let keys = std::slice::from_ref(key);
    let record = dashboard
        .judgements(tenant, user, keys)
        .await?
        .into_iter()
        .find(|record| !fresh(record, &candidate.fingerprint) && body_of(record).is_some())
        .ok_or(RejudgeError::NotOutdated)?;
    if holder.state == RefreshState::Ranking {
        let targets =
            state
                .dashboard
                .readers()
                .rejudge_targets(scope, holder.id, holder.ranking_attempt);
        if holder.progress.bulk_rejudge_keys.contains(key)
            || (record.rejudge_requested_at.is_some()
                && targets.is_none_or(|targets| targets.contains(key)))
        {
            return Ok(false);
        }
    }
    let zone = ranking_zone(state, tenant, user).await?;
    let now = state.dashboard.now();
    if dashboard
        .sent_today(tenant, user, day_start(now, &zone))
        .await?
        >= DAILY_CAP
    {
        return Err(RejudgeError::DailyCap);
    }
    dashboard
        .request_rejudgements(tenant, user, keys, now)
        .await?;
    Ok(true)
}

/// The judgement was made with other versions of the prompt or the judged inputs.
fn older(record: &StoredJudgement) -> bool {
    record.prompt_version != JUDGE_PROMPT_VERSION || record.input_version != JUDGE_INPUT_VERSION
}

/// Same judged inputs and versions: the stored judgement is reused as is.
fn fresh(record: &StoredJudgement, fingerprint: &str) -> bool {
    record.input_fingerprint == fingerprint && !older(record)
}

fn body_of(record: &StoredJudgement) -> Option<Body> {
    serde_json::from_value(record.body.clone())
        .inspect_err(|error| {
            tracing::warn!(%error, item = %record.item_key,
                "stored dashboard judgement is unreadable; the item is judged again");
        })
        .ok()
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The user's configured ranking time zone, or UTC.
pub(crate) async fn ranking_zone(
    state: &AppState,
    tenant: &str,
    user: &str,
) -> Result<TimeZone, DbError> {
    let settings = super::settings::effective(&state.db, tenant, user).await?;
    Ok(settings
        .time_zone
        .as_deref()
        .and_then(super::zone::parse)
        .unwrap_or(TimeZone::UTC))
}

/// Start of the local day of `now` (the daily cap counts from it).
pub(crate) fn day_start(now: Timestamp, zone: &TimeZone) -> Timestamp {
    now.to_zoned(zone.clone())
        .start_of_day()
        .map(|day| Timestamp::from(day.timestamp()))
        .unwrap_or(now - SignedDuration::from_hours(24))
}

/// Counts the never-judged items still in the current snapshot that hit a ranking cap.
pub(crate) async fn capped_count(
    state: &AppState,
    tenant: &str,
    user: &str,
) -> Result<u32, DbError> {
    let Some((holder, snapshot)) = snapshot::load(state, tenant, user).await? else {
        return Ok(0);
    };
    if holder.state == RefreshState::Ranking || holder.progress.left_provisional_keys.is_empty() {
        return Ok(0);
    }
    let capped: HashSet<_> = holder.progress.left_provisional_keys.iter().collect();
    let keys: Vec<_> = input::candidates(&snapshot, LlmLanguage::En)
        .into_iter()
        .filter(|candidate| candidate.asked() && capped.contains(&candidate.item.key))
        .map(|candidate| candidate.item.key)
        .collect();
    let judged = state.db.dashboard().judgements(tenant, user, &keys).await?;
    Ok(count(
        keys.len().saturating_sub(
            judged
                .iter()
                .filter(|record| body_of(record).is_some())
                .count(),
        ),
    ))
}

/// Deletes judgements not seen for [`JUDGEMENT_RETENTION_DAYS`]; a failure only delays it to the next ranking.
async fn retention(db: &Db, now: Timestamp) {
    match db
        .dashboard()
        .delete_unseen_judgements(now - SignedDuration::from_hours(24 * JUDGEMENT_RETENTION_DAYS))
        .await
    {
        Ok(0) => {}
        Ok(deleted) => tracing::info!(deleted, "dashboard judgements of long unseen items deleted"),
        Err(error) => tracing::warn!(%error,
            "old dashboard judgements could not be deleted; the next ranking tries again"),
    }
}

/// An active pipeline turn can occupy the runner's shared turn capacity.
async fn analysis_running(db: &Db) -> Result<bool, DbError> {
    for analysis in db.analyses().unfinished().await? {
        let active = db
            .jobs()
            .list_for_analysis(analysis.id, &[JobState::Claimed, JobState::Running])
            .await?;
        if !active.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Requests cancellation of the refresh's unfinished judge jobs (of `attempt` only, when given), so an ended
/// ranking leaves no job behind. Failures are logged; such a job is never claimed (its refresh no longer ranks).
pub async fn cancel_jobs(state: &AppState, refresh: RefreshId, attempt: Option<u32>) {
    let live = state
        .db
        .jobs()
        .list_for_refresh(
            refresh,
            &[
                JobState::Queued,
                JobState::Claimed,
                JobState::Running,
                JobState::Paused,
            ],
        )
        .await;
    let live = match live {
        Ok(live) => live,
        Err(error) => {
            tracing::warn!(%error, %refresh,
                "judge jobs of an ended dashboard ranking could not be listed; they are never claimed");
            Vec::new()
        }
    };
    for job in live
        .into_iter()
        .filter(|job| attempt.is_none_or(|a| job.round == a))
    {
        match state.executor.cancel(&JobHandle { job_id: job.id }).await {
            Ok(()) => tracing::info!(%refresh, job = %job.id,
                "judge job of an ended dashboard ranking cancelled"),
            Err(error) => tracing::warn!(%error, %refresh, job = %job.id,
                "judge job of an ended dashboard ranking could not be cancelled"),
        }
    }
    match state
        .db
        .dashboard()
        .release_unsubmitted_sends(refresh, attempt, state.dashboard.now())
        .await
    {
        Ok(0) => {}
        Ok(released) => {
            tracing::info!(%refresh, released, "unsubmitted dashboard sends refunded after ranking ended")
        }
        Err(error) => {
            tracing::warn!(%error, %refresh, "unsubmitted dashboard sends could not be refunded after ranking ended")
        }
    }
}

/// Stops a task when dropped (the ranking's job-update reader ends with the ranking).
struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Cancels the attempt's judge jobs when the ranking task is dropped before its end (R3 cancel, supersede).
struct CancelOnDrop {
    state: AppState,
    refresh: RefreshId,
    attempt: u32,
    armed: bool,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(refresh = %self.refresh,
                "judge jobs of a stopped dashboard ranking not cancelled: no runtime; they are never claimed");
            return;
        };
        let (state, refresh, attempt) = (self.state.clone(), self.refresh, self.attempt);
        runtime.spawn(async move { cancel_jobs(&state, refresh, Some(attempt)).await });
    }
}

/// The wire status of a stored refresh with what only the jobs know: when a paused ranking continues.
pub async fn status(state: &AppState, record: &RefreshRecord) -> RefreshStatus {
    let mut status = refresh::status_of(record);
    if let Some(ranking) = status
        .ranking
        .as_mut()
        .filter(|ranking| ranking.state == RankingState::Paused)
    {
        ranking.resumes_at = match state.db.llm_credentials().get(&record.user_id).await {
            Ok(credential) => credential
                .paused_until
                .filter(|until| *until > Timestamp::now()),
            Err(error) => {
                tracing::debug!(%error, "LLM credential pause not read for the ranking status");
                None
            }
        };
    }
    status
}
