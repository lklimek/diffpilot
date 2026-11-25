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
    ItemKey, RankingState, RefreshState, RefreshStatus, ScanStep, ScanStepKey, StepState,
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
    JUDGEMENT_RETENTION_DAYS, JudgeCounts, RankingUpdate, RankingWrite, RefreshProgress,
    RefreshRecord, SendCaps, SendReservation, StoredJudgement,
};
use db::repo::jobs::{JobStage, MAX_REPAIR_RUNS};
use futures::StreamExt;
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::time::MissedTickBehavior;

use contracts::time::Timestamp;
use jiff::SignedDuration;
use jiff::civil::Time;
use jiff::tz::TimeZone;

use self::input::Candidate;
pub use self::input::JUDGE_INPUT_VERSION;
use self::store::Body;
use super::refresh::rank::{ItemJudgement, RankRun, Ranker};
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
/// Both caps.
const CAPS: SendCaps = SendCaps {
    per_refresh: REFRESH_CAP,
    per_day: DAILY_CAP,
};
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
    Capped(Vec<String>, String),
}

/// An item queued for judgement.
#[derive(Clone, Debug)]
struct Queued {
    item: JudgeItem,
    fingerprint: String,
    /// Never judged before (new items go first).
    new: bool,
    /// Provisional points (changed items in descending order).
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
        day: Timestamp::UNIX_EPOCH,
        unclaimed_since: None,
        provisional: None,
    };
    let end = match ranking.run().await {
        Ok(end) => Some(end),
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
    if end
        .as_ref()
        .is_none_or(|end| end.outcome != RankingState::Finished)
    {
        cancel_jobs(&state, run.refresh_id, Some(run.attempt)).await;
    }
    guard.armed = false;
    match end {
        Some(end) => ranking.finish(end).await,
        None => tracing::debug!(refresh = %run.refresh_id, attempt = run.attempt,
            "dashboard ranking no longer current; its late results are dropped"),
    }
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
    /// Start of the user's local day the ranking began in (the daily cap counts from it).
    day: Timestamp,
    /// Since when the followed job waits for a runner (queued, credential usable, no analysis job running).
    unclaimed_since: Option<tokio::time::Instant>,
    /// Which cap left items provisional, as the ranking step's detail shows it while no other detail is given.
    provisional: Option<String>,
}

impl<'a> Ranking<'a> {
    fn scope(&self) -> (&'a str, &'a str) {
        (&self.run.scope.tenant_id, &self.run.scope.user_id)
    }

    async fn run(&mut self) -> Result<End, RankError> {
        let (tenant, user) = self.scope();
        let dashboard = self.state.db.dashboard();
        let now = self.state.dashboard.now();
        retention(&self.state.db, now).await;
        let record = dashboard.refresh(tenant, user, self.run.refresh_id).await?;
        if record.state != RefreshState::Ranking || record.ranking_attempt != self.run.attempt {
            return Err(RankError::Stale);
        }
        self.counts.judged = record.counts.judged;
        let (holder, snapshot) = snapshot::load(self.state, tenant, user)
            .await?
            .ok_or(RankError::NoSnapshot)?;
        if holder.id != self.run.refresh_id {
            return Err(RankError::Stale);
        }
        self.zone = zone(&snapshot);
        self.language = self.state.db.users().get(user).await?.language;
        let candidates = input::candidates(&snapshot, self.language);
        let keys: Vec<ItemKey> = candidates.iter().map(|c| c.item.key.clone()).collect();
        let stored: HashMap<ItemKey, StoredJudgement> = dashboard
            .judgements(tenant, user, &keys)
            .await?
            .into_iter()
            .map(|j| (j.item_key.clone(), j))
            .collect();
        dashboard.mark_seen(tenant, user, &keys, now).await?;
        let (queue, reused) = self.queue(&snapshot, &candidates, &stored);
        self.day = day_start(now, &self.zone);
        let sent = dashboard
            .sent_totals(tenant, user, self.run.refresh_id, self.day)
            .await?;
        let per_refresh = REFRESH_CAP.saturating_sub(sent.refresh);
        let per_day = DAILY_CAP.saturating_sub(sent.day);
        let room = usize::try_from(per_refresh.min(per_day)).unwrap_or(usize::MAX);
        let take = queue.len().min(room);
        let left = queue.len() - take;
        self.counts.reused = reused;
        self.counts.left_provisional = count(left);
        let mut lines = vec![format!(
            "ranking: {take} items to judge, {reused} judgements reused"
        )];
        if left > 0 {
            let note = format!(
                "{} left provisional: {}",
                items(left),
                limit(per_refresh, per_day)
            );
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
            let reason = match self.batch(count(index + 1), items).await? {
                BatchEnd::Judged => continue,
                BatchEnd::Unjudged(ids) => {
                    not_ranked = ids;
                    "a batch could not be judged".to_owned()
                }
                BatchEnd::Capped(ids, limit) => {
                    not_ranked = ids;
                    limit
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
            return Ok(End {
                outcome: RankingState::Finished,
                message: None,
            });
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
        Ok(End {
            outcome: RankingState::Failed,
            message: Some(message),
        })
    }

    /// Items to judge in queue order (new first, then changed by provisional score), and the count of stored
    /// judgements reused (not counting those an earlier attempt of this refresh made: they are in `judged`).
    fn queue(
        &self,
        snapshot: &Snapshot,
        candidates: &[Candidate<'_>],
        stored: &HashMap<ItemKey, StoredJudgement>,
    ) -> (Vec<Queued>, u32) {
        let rules = ScoreRules::new(&snapshot.own_login, &snapshot.settings);
        let mut queue = Vec::new();
        let mut reused = 0;
        for candidate in candidates.iter().filter(|c| c.asked()) {
            let record = stored.get(&candidate.item.key);
            let body = record.and_then(body_of);
            if let Some(record) =
                record.filter(|r| body.is_some() && fresh(r, &candidate.fingerprint))
            {
                if record.refresh_id != Some(self.run.refresh_id) {
                    reused += 1;
                }
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
                points,
            });
        }
        queue.sort_by(|a, b| {
            b.new
                .cmp(&a.new)
                .then_with(|| b.points.total_cmp(&a.points))
                .then_with(|| a.item.key.cmp(&b.item.key))
        });
        (queue, reused)
    }

    /// Judges one batch, running it again for the items whose answer failed or was rejected while runs are left
    /// and the caps allow it. Every run's items are counted against the caps before its job is submitted.
    async fn batch(&mut self, batch: u32, items: &[Queued]) -> Result<BatchEnd, RankError> {
        let ids = |pending: &[&Queued]| pending.iter().map(|q| q.item.id.clone()).collect();
        let mut pending: Vec<&Queued> = items.iter().collect();
        for generation in 0..=RETRIES {
            let input = JudgeWorkInput {
                items: pending.iter().map(|q| q.item.clone()).collect(),
            };
            if let Some(limit) = self.reserve(count(pending.len())).await? {
                tracing::info!(refresh = %self.run.refresh_id, batch, generation, items = pending.len(), %limit,
                    "dashboard judge batch not sent: the ranking cap is reached");
                return Ok(BatchEnd::Capped(ids(&pending), limit));
            }
            let handle = self.submit(batch, generation, &input).await?;
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
    async fn reserve(&self, items: u32) -> Result<Option<String>, RankError> {
        let (tenant, user) = self.scope();
        let reserved = self
            .state
            .db
            .dashboard()
            .reserve_sends(
                tenant,
                user,
                self.run.refresh_id,
                self.run.attempt,
                items,
                CAPS,
                self.day,
            )
            .await?;
        match reserved {
            SendReservation::Reserved => Ok(None),
            SendReservation::OverRefreshCap => Ok(Some(limit(0, 1))),
            SendReservation::OverDailyCap => Ok(Some(limit(1, 0))),
            SendReservation::Stale => Err(RankError::Stale),
        }
    }

    async fn submit(
        &self,
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
            job_id: JobId::new_random(),
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
                })
            })
            .collect::<Result<Vec<_>, serde_json::Error>>()?;
        let mut counts = self.counts;
        counts.judged += count(judgements.len());
        // Counts and judgements in one transaction fenced by the attempt: a late answer is never stored.
        self.write(RankingState::Running, counts, &judgements, None)
            .await?;
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

/// The cap that leaves items provisional, as the dashboard names it: the daily one when less of it is left.
fn limit(per_refresh: u32, per_day: u32) -> String {
    if per_day < per_refresh {
        format!("today's ranking limit ({DAILY_CAP})")
    } else {
        format!("this refresh's ranking limit ({REFRESH_CAP})")
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
            },
        );
    }
    Ok(applying)
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

/// The user's time zone from the snapshot's settings; UTC while none is stored.
fn zone(snapshot: &Snapshot) -> TimeZone {
    snapshot
        .settings
        .time_zone
        .as_deref()
        .and_then(super::zone::parse)
        .unwrap_or(TimeZone::UTC)
}

/// Start of the local day of `now` (the daily cap counts from it).
fn day_start(now: Timestamp, zone: &TimeZone) -> Timestamp {
    let midnight = now
        .to_zoned(zone.clone())
        .date()
        .to_datetime(Time::midnight());
    super::zone::earliest(midnight, zone).unwrap_or(now - SignedDuration::from_hours(24))
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
            return;
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
