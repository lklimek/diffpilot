//! Numeric run observations; pre-start estimates live only in memory, never in stored documents.

use crate::auth::Session;
use crate::state::AppState;
use contracts::api::analyses::{AnalysisEstimate, AnalysisSummary};
use contracts::api::pr::PrSummary;
use contracts::ids::AnalysisId;
use db::error::DbError;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

type ResolveKey = (String, String, String, String, u32, String);

#[derive(Clone, Copy, Debug, Default)]
struct RunFacts {
    changed_lines: Option<u64>,
    changed_files: Option<u64>,
    agent_files: Option<u64>,
    plan_batches: Option<u64>,
    initial_estimate_seconds: Option<u32>,
}

#[derive(Debug, Default)]
struct Observations {
    resolved: HashMap<ResolveKey, Option<AnalysisEstimate>>,
    runs: HashMap<AnalysisId, RunFacts>,
}

/// Small process-local observations; missing after restart is unknown, never reconstructed as the original ETA.
#[derive(Clone, Debug, Default)]
pub struct Statistics(Arc<Mutex<Observations>>);

fn key(session: &Session, pr: &PrSummary) -> ResolveKey {
    (
        session.tenant_id.clone(),
        session.user_id.clone(),
        pr.repo.owner.clone(),
        pr.repo.name.clone(),
        pr.number,
        pr.head_sha.0.clone(),
    )
}

impl Statistics {
    /// Releases observations when a driver exits, including cancellation during preparation.
    pub fn forget(&self, analysis: AnalysisId) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .runs
            .remove(&analysis);
    }
    /// Keeps the first pre-start estimate actually returned to this reader for this PR head.
    pub fn resolved(&self, session: &Session, pr: &PrSummary, estimate: Option<AnalysisEstimate>) {
        let mut data = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if data.resolved.len() >= 128 {
            data.resolved.clear();
        }
        data.resolved.entry(key(session, pr)).or_insert(estimate);
    }

    /// Associates the reader's pre-start estimate with the newly created analysis.
    pub fn begin(
        &self,
        analysis: AnalysisId,
        session: &Session,
        pr: &PrSummary,
        size: Option<(u64, u32)>,
    ) {
        let mut data = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let estimate = data.resolved.remove(&key(session, pr)).flatten();
        data.runs.insert(
            analysis,
            RunFacts {
                changed_lines: size.map(|s| s.0),
                changed_files: size.map(|s| u64::from(s.1)),
                initial_estimate_seconds: estimate.map(|e| e.remaining_seconds),
                ..RunFacts::default()
            },
        );
    }

    /// Counts from the actual diff and fitted plan, once the driver has them (also after restart).
    pub fn planned(
        &self,
        analysis: AnalysisId,
        lines: u64,
        files: u64,
        agent_files: u64,
        batches: u64,
    ) {
        let mut data = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let facts = data.runs.entry(analysis).or_default();
        facts.changed_lines = Some(lines);
        facts.changed_files = Some(files);
        facts.agent_files = Some(agent_files);
        facts.plan_batches = Some(batches);
    }
}

/// Called by the guarded live-to-terminal transition, before cancellation deletes documents, and when the
/// driver ends for lack of admitted flow work (the run so far).
pub async fn log_end(state: &AppState, summary: &AnalysisSummary) -> Result<(), DbError> {
    let jobs = state.db.jobs().statistics(summary.id).await?;
    let facts = state
        .statistics
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .runs
        .remove(&summary.id)
        .unwrap_or_default();
    let seconds = summary
        .finished_at
        .map(|at| (at - summary.created_at).num_seconds().max(0));
    let usage = summary.usage;
    tracing::info!(analysis = %summary.id,
        changed_lines = facts.changed_lines, changed_files = facts.changed_files,
        agent_files = facts.agent_files, plan_batches = facts.plan_batches,
        flows = summary.progress.as_ref().and_then(|p| p.flows.total),
        repair_runs = jobs.repair_runs, jobs = jobs.jobs,
        plan_seconds = jobs.plan_seconds, pr_context_seconds = jobs.pr_context_seconds,
        flow_seconds = jobs.flow_seconds, ask_seconds = jobs.ask_seconds, total_seconds = seconds,
        input_tokens = usage.map(|u| u.input_tokens), cached_input_tokens = usage.map(|u| u.cached_input_tokens),
        output_tokens = usage.map(|u| u.output_tokens), reasoning_output_tokens = tracing::field::Empty,
        usage_jobs = usage.map(|u| u.jobs), initial_estimate_seconds = facts.initial_estimate_seconds,
        "analysis statistics");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::FromRequestParts;
    use contracts::api::analyses::EstimateBasis;

    #[tokio::test]
    async fn initial_estimate_is_the_first_shown_and_belongs_to_its_reader_and_head() {
        let state = crate::state::tests::test_state().await;
        let id = db::repo::analyses::testing::seed_analysis(&state.db)
            .await
            .unwrap();
        let pr = state.db.analyses().summary(id).await.unwrap().pr;
        let cookie = crate::test_support::session_cookie(&state, "default").await;
        let (mut parts, _) = axum::http::Request::builder()
            .header("cookie", cookie)
            .body(())
            .unwrap()
            .into_parts();
        let mut session = Session::from_request_parts(&mut parts, &state)
            .await
            .unwrap();
        let first = AnalysisEstimate {
            remaining_seconds: 1200,
            basis: EstimateBasis::LinesOnly,
            rough: true,
        };
        state.statistics.resolved(&session, &pr, Some(first));
        state.statistics.resolved(
            &session,
            &pr,
            Some(AnalysisEstimate {
                remaining_seconds: 1800,
                ..first
            }),
        );
        session.user_id = "another-reader".into();
        let other = AnalysisId::new_random();
        state.statistics.begin(other, &session, &pr, None);
        assert_eq!(
            state.statistics.0.lock().unwrap().runs[&other].initial_estimate_seconds,
            None
        );
        session.user_id = "u1".into();
        let mut moved = pr.clone();
        moved.head_sha.0 = "changed-head".into();
        state.statistics.begin(other, &session, &moved, None);
        assert_eq!(
            state.statistics.0.lock().unwrap().runs[&other].initial_estimate_seconds,
            None
        );
        state.statistics.begin(id, &session, &pr, Some((3000, 30)));
        assert_eq!(
            state.statistics.0.lock().unwrap().runs[&id].initial_estimate_seconds,
            Some(1200)
        );
        state.statistics.planned(id, 3100, 31, 20, 3);
        let facts = state.statistics.0.lock().unwrap().runs[&id];
        assert_eq!(facts.initial_estimate_seconds, Some(1200));
        assert_eq!(facts.plan_batches, Some(3));
        state.statistics.forget(id);
        assert!(!state.statistics.0.lock().unwrap().runs.contains_key(&id));
    }
}
