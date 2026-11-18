//! Scan progress of a live refresh: the steps of plan §A.4 and plain log lines, written to the refresh before
//! each `refresh` event is published.

use chrono_tz::Tz;
use contracts::api::dashboard::{ScanStep, ScanStepKey, StepState};
use contracts::ids::RefreshId;
use db::repo::dashboard::RefreshProgress;

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

/// Progress of one refresh's scan.
pub(super) struct Progress {
    state: AppState,
    scope: Scope,
    id: RefreshId,
    zone: Tz,
    steps: Vec<ScanStep>,
    log: Vec<String>,
}

impl Progress {
    pub(super) fn new(state: AppState, scope: Scope, id: RefreshId, zone: Tz) -> Self {
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
        let at = self.state.dashboard.now().with_timezone(&self.zone);
        self.log
            .push(format!("{} {}", at.format("%H:%M:%S"), line.as_ref()));
    }

    /// Sets a step and records the progress.
    pub(super) async fn step(
        &mut self,
        key: ScanStepKey,
        state: StepState,
        done: Option<u32>,
        total: Option<u32>,
    ) {
        if let Some(step) = self.steps.iter_mut().find(|s| s.key == key) {
            step.state = state;
            step.done = done;
            step.total = total;
        }
        self.record().await;
    }

    /// Writes the progress and log, then publishes the refresh's status. A failed write is logged: the refresh
    /// goes on and its next write carries the progress.
    pub(super) async fn record(&self) {
        let progress = RefreshProgress {
            steps: self.steps.clone(),
            percent: self.percent(),
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
            Err(error) => {
                tracing::debug!(%error, refresh = %self.id, "dashboard scan progress not recorded");
            }
        }
    }

    /// The server state.
    pub(super) fn state(&self) -> &AppState {
        &self.state
    }

    /// The current time.
    pub(super) fn now(&self) -> chrono::DateTime<chrono::Utc> {
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
        self.record().await;
    }
}
