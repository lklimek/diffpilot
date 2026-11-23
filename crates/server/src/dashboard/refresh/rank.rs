//! The ranking seam (plan §B): after a snapshot is stored its refresh is `ranking` and [`Ranker::rank`] runs in
//! the refresh's task (a cancel or a superseding refresh aborts it); reading items asks [`Ranker::judgements`].
//! Production ranks with `dashboard::judge::JudgeRanker`; [`NoopRanker`] leaves every item unjudged (tests).

use std::collections::HashMap;
use std::fmt;

use contracts::api::dashboard::{ItemKey, ItemText, RankingState};
use contracts::ids::RefreshId;
use db::error::DbError;
use db::repo::dashboard::JudgeCounts;
use futures::future::BoxFuture;

use super::Scope;
use crate::dashboard::plugin::Overlay;
use crate::dashboard::score::Judgement;
use crate::dashboard::snapshot::Snapshot;
use crate::state::AppState;

/// One ranking attempt of a stored snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankRun {
    /// The user.
    pub scope: Scope,
    /// The refresh holding the snapshot.
    pub refresh_id: RefreshId,
    /// Its ranking attempt: pass it to `set_ranking`/`finish_ranking`, which answer `Stale` once superseded.
    pub attempt: u32,
}

/// A stored judgement applicable to an item now, with its LLM text.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemJudgement {
    /// Judged parts, current or inherited.
    pub judgement: Judgement,
    /// LLM text shown with the item.
    pub text: Option<ItemText>,
    /// Judged with an older ranking prompt or judged-input version (`DashboardSummary.older_prompt_judgements`).
    pub older_prompt: bool,
}

/// The LLM ranking of snapshots.
pub trait Ranker: Send + Sync + fmt::Debug {
    /// Ranks the snapshot of `run`, ending its ranking with `finish_ranking(run.attempt, …)` and publishing the
    /// refresh's status after each write. Runs in the refresh's task.
    fn rank(&self, state: AppState, run: RankRun) -> BoxFuture<'static, ()>;

    /// Judgements of the snapshot's items to score them with, by item key; `overlay` holds the plug-in statements
    /// the items are ranked with.
    fn judgements<'a>(
        &'a self,
        state: &'a AppState,
        scope: &'a Scope,
        snapshot: &'a Snapshot,
        overlay: &'a Overlay,
    ) -> BoxFuture<'a, Result<HashMap<ItemKey, ItemJudgement>, DbError>>;
}

/// Ranking without the LLM: ends at once as finished with nothing judged, so every item keeps its estimates.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopRanker;

impl Ranker for NoopRanker {
    fn rank(&self, state: AppState, run: RankRun) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let now = state.dashboard.now();
            let ended = state
                .db
                .dashboard()
                .finish_ranking(
                    &run.scope.tenant_id,
                    &run.scope.user_id,
                    run.refresh_id,
                    run.attempt,
                    RankingState::Finished,
                    JudgeCounts::default(),
                    now,
                )
                .await;
            match ended {
                Ok(update) => tracing::debug!(?update, refresh = %run.refresh_id,
                    "dashboard ranking skipped: no LLM ranking step is installed"),
                Err(error) => tracing::warn!(%error, refresh = %run.refresh_id,
                    "dashboard ranking could not be ended; the refresh shows it ranking until Retry or restart"),
            }
            super::publish(&state, &run.scope, run.refresh_id).await;
        })
    }

    fn judgements<'a>(
        &'a self,
        _state: &'a AppState,
        _scope: &'a Scope,
        _snapshot: &'a Snapshot,
        _overlay: &'a Overlay,
    ) -> BoxFuture<'a, Result<HashMap<ItemKey, ItemJudgement>, DbError>> {
        Box::pin(async { Ok(HashMap::new()) })
    }
}
