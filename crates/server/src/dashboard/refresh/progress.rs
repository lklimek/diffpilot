//! Scan progress of a live refresh: the steps of plan §A.4 and plain log lines, written to the refresh before
//! each `refresh` event is published.

use contracts::api::dashboard::{ScanStep, ScanStepKey, StepState};
use contracts::ids::RefreshId;
use contracts::time::Timestamp;
use db::repo::dashboard::RefreshProgress;
use jiff::tz::TimeZone;

use super::Scope;
use crate::state::AppState;

/// The scan steps with their share of the scan's percent (the ranking is not part of the scan).
const STEPS: [(ScanStepKey, u32); 7] = [
    (ScanStepKey::SearchPrs, 20),
    (ScanStepKey::Issues, 15),
    (ScanStepKey::PrDetails, 30),
    (ScanStepKey::Context, 15),
    (ScanStepKey::History, 15),
    (ScanStepKey::Todos, 5),
    (ScanStepKey::Ranking, 0),
];

/// The refresh ended (cancelled) while its scan ran: the scan stops.
#[derive(Debug, thiserror::Error)]
#[error("the refresh ended while its scan ran")]
pub(super) struct Ended;

/// Progress of one refresh's scan.
pub(super) struct Progress {
    state: AppState,
    scope: Scope,
    id: RefreshId,
    zone: TimeZone,
    steps: Vec<ScanStep>,
    log: Vec<String>,
}

impl Progress {
    pub(super) fn new(state: AppState, scope: Scope, id: RefreshId, zone: TimeZone) -> Self {
        Self {
            state,
            scope,
            id,
            zone,
            steps: STEPS
                .iter()
                .map(|(key, _)| ScanStep {
                    key: *key,
                    state: StepState::Waiting,
                    done: None,
                    total: None,
                    detail: None,
                })
                .collect(),
            log: Vec::new(),
        }
    }

    fn percent(&self) -> u32 {
        let done: u32 = self
            .steps
            .iter()
            .zip(STEPS)
            .map(|(step, (_, weight))| match step.state {
                StepState::Done | StepState::Skipped | StepState::Failed => weight,
                StepState::Running => match (step.done, step.total) {
                    (Some(done), Some(total)) if total > 0 => weight * done.min(total) / total,
                    _ => 0,
                },
                StepState::Waiting => 0,
            })
            .sum();
        done.min(100)
    }

    /// A log line, prefixed with the local time.
    pub(super) fn log(&mut self, line: impl AsRef<str>) {
        let at = self.state.dashboard.now().to_zoned(self.zone.clone());
        self.log
            .push(format!("{} {}", at.strftime("%H:%M:%S"), line.as_ref()));
    }

    /// Sets a step (a step that is not running loses its detail) and records the progress; [`Ended`] once the
    /// refresh is no longer scanning.
    pub(super) async fn step(
        &mut self,
        key: ScanStepKey,
        state: StepState,
        done: Option<u32>,
        total: Option<u32>,
    ) -> Result<(), Ended> {
        if let Some(step) = self.steps.iter_mut().find(|s| s.key == key) {
            step.state = state;
            step.done = done;
            step.total = total;
            if state != StepState::Running {
                step.detail = None;
            }
        }
        self.record().await
    }

    /// Sets the detail of a step (the item it works on); recorded with the next [`Self::step`].
    pub(super) fn detail(&mut self, key: ScanStepKey, detail: String) {
        if let Some(step) = self.steps.iter_mut().find(|s| s.key == key) {
            step.detail = Some(detail);
        }
    }

    /// Writes the progress and log, then publishes the refresh's status. [`Ended`] when the refresh ended
    /// meanwhile; another failed write is logged (the next write carries the progress).
    pub(super) async fn record(&self) -> Result<(), Ended> {
        let progress = RefreshProgress {
            steps: self.steps.clone(),
            percent: self.percent(),
            left_provisional_keys: Vec::new(),
            bulk_rejudge_keys: Vec::new(),
        };
        let written = self
            .state
            .db
            .dashboard()
            .record_progress(
                &self.scope.tenant_id,
                &self.scope.user_id,
                self.id,
                &progress,
                &self.log,
            )
            .await;
        match written {
            Ok(()) => super::publish(&self.state, &self.scope, self.id).await,
            Err(db::error::DbError::Conflict) => return Err(Ended),
            Err(error) => {
                tracing::debug!(%error, refresh = %self.id, "dashboard scan progress not recorded");
            }
        }
        Ok(())
    }

    /// The server state.
    pub(super) fn state(&self) -> &AppState {
        &self.state
    }

    /// The current time.
    pub(super) fn now(&self) -> Timestamp {
        self.state.dashboard.now()
    }

    /// Marks the step that was running as failed (the scan ended early).
    pub(super) async fn fail_running(&mut self, detail: &str) {
        for step in &mut self.steps {
            if step.state == StepState::Running {
                step.state = StepState::Failed;
                step.detail = Some(detail.to_owned());
            }
        }
        // An ended refresh keeps its stored progress.
        let _ = self.record().await;
    }
}
