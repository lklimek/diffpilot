//! Scripted provider for tests and development (feature `fake-provider`, never in release builds): replays
//! the recorded Codex streams of the fixture PR (`fixtures/pr-482/codex/`) through the Codex stream parser.
//!
//! The recording is chosen by the call's output schema (job kind) and, for `analyse_flow`, by the first flow
//! id token `f<n>` in the prompt (the prompts start with "Job: analyse_flow f<n>."). `ask` has no recording.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::Utc;
use contracts::agent::{JobKind, output_schema};
use contracts::ids::JobId;
use contracts::job::event::JobEvent;
use contracts::job::result::Usage;
use futures::future::BoxFuture;
use tokio::sync::{Notify, mpsc};

use crate::provider::codex::{CodexStream, Exit, usage_delta};
use crate::provider::{LlmProvider, ProviderCall, ProviderError, ProviderEvent, ProviderOutcome};

/// Every recording of the fixture session, for the cumulative usage before a job.
const SESSION: [(&str, &str); 9] = [
    (
        "plan",
        include_str!("../../../../fixtures/pr-482/codex/plan.jsonl"),
    ),
    (
        "compaction-after-plan",
        include_str!("../../../../fixtures/pr-482/codex/compaction-after-plan.jsonl"),
    ),
    (
        "pr-context",
        include_str!("../../../../fixtures/pr-482/codex/pr-context.jsonl"),
    ),
    (
        "analyse-flow-1",
        include_str!("../../../../fixtures/pr-482/codex/analyse-flow-1.jsonl"),
    ),
    (
        "compaction-after-flow-1",
        include_str!("../../../../fixtures/pr-482/codex/compaction-after-flow-1.jsonl"),
    ),
    (
        "analyse-flow-2",
        include_str!("../../../../fixtures/pr-482/codex/analyse-flow-2.jsonl"),
    ),
    (
        "analyse-flow-3",
        include_str!("../../../../fixtures/pr-482/codex/analyse-flow-3.jsonl"),
    ),
    (
        "analyse-flow-4",
        include_str!("../../../../fixtures/pr-482/codex/analyse-flow-4.jsonl"),
    ),
    (
        "analyse-flow-5",
        include_str!("../../../../fixtures/pr-482/codex/analyse-flow-5.jsonl"),
    ),
];

/// Replays fixture recordings; `cancel` ends a running replay with [`ProviderError::Cancelled`].
#[derive(Debug, Default)]
pub struct FakeProvider {
    step_delay: Duration,
    running: Mutex<HashMap<JobId, Arc<Notify>>>,
}

impl FakeProvider {
    /// A provider that waits `step_delay` before each recorded line (zero: replay at once).
    pub fn new(step_delay: Duration) -> Self {
        Self {
            step_delay,
            running: Mutex::new(HashMap::new()),
        }
    }

    async fn replay(
        &self,
        call: &ProviderCall,
        events: &mpsc::Sender<ProviderEvent>,
    ) -> Result<ProviderOutcome, ProviderError> {
        let name = recording_name(call)?;
        let jsonl = recording(&name).ok_or_else(|| ProviderError::Failed {
            message: format!("the fake provider has no recording {name}"),
        })?;
        let cancelled = Arc::new(Notify::new());
        let _registration = Registration::new(self, call.job_id, Arc::clone(&cancelled));
        let mut stream = CodexStream::default();
        for line in jsonl.lines() {
            if !self.step_delay.is_zero() {
                tokio::select! {
                    () = tokio::time::sleep(self.step_delay) => {}
                    () = cancelled.notified() => return Err(ProviderError::Cancelled),
                }
            }
            if let Some((activity, summary)) = stream.line(line) {
                let event = JobEvent {
                    seq: 0,
                    at: Utc::now(),
                    activity,
                    summary,
                };
                let _ = events.send(ProviderEvent::Activity(event)).await;
            }
        }
        let finished = stream.finish(Exit::Code(0))?;
        let _ = events
            .send(ProviderEvent::OutputReady(finished.output.clone()))
            .await;
        Ok(ProviderOutcome {
            output: finished.output,
            session_id: finished.thread_id,
            usage: usage_delta(
                cumulative_before(finished.cumulative_usage),
                finished.cumulative_usage,
            ),
            skills_commit: None,
        })
    }
}

impl LlmProvider for FakeProvider {
    fn run<'a>(
        &'a self,
        call: &'a ProviderCall,
        events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        Box::pin(async move { self.replay(call, &events).await })
    }

    fn resume<'a>(
        &'a self,
        _session_id: &'a str,
        call: &'a ProviderCall,
        events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        Box::pin(async move { self.replay(call, &events).await })
    }

    fn cancel(&self, job: JobId) -> BoxFuture<'_, Result<(), ProviderError>> {
        let running = self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&job)
            .cloned();
        if let Some(cancelled) = running {
            // notify_one keeps a permit, so a cancel between two waits is not lost.
            cancelled.notify_one();
        }
        Box::pin(async { Ok(()) })
    }
}

/// Removes the job from the running set when the replay ends or is dropped.
struct Registration<'a> {
    provider: &'a FakeProvider,
    job: JobId,
}

impl<'a> Registration<'a> {
    fn new(provider: &'a FakeProvider, job: JobId, cancelled: Arc<Notify>) -> Self {
        provider
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(job, cancelled);
        Self { provider, job }
    }
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.provider
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.job);
    }
}

fn recording(name: &str) -> Option<&'static str> {
    SESSION
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, jsonl)| *jsonl)
}

/// The recording for the call: job kind from the output schema, flow from the prompt.
fn recording_name(call: &ProviderCall) -> Result<String, ProviderError> {
    let kind = JobKind::ALL
        .into_iter()
        .find(|kind| output_schema(*kind).is_ok_and(|schema| schema == call.output_schema))
        .ok_or_else(|| ProviderError::Failed {
            message: "the fake provider does not know this output schema".to_owned(),
        })?;
    match kind {
        JobKind::Plan => Ok("plan".to_owned()),
        JobKind::PrContext => Ok("pr-context".to_owned()),
        JobKind::AnalyseFlow => Ok(format!(
            "analyse-flow-{}",
            flow_number(&call.prompt).unwrap_or(1)
        )),
        JobKind::Ask => Err(ProviderError::Failed {
            message: "the fake provider has no recording for ask jobs".to_owned(),
        }),
    }
}

/// The number of the first `f<n>` token in `prompt`.
fn flow_number(prompt: &str) -> Option<u32> {
    prompt
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .find_map(|token| token.strip_prefix('f')?.parse().ok())
}

/// The session's cumulative usage before the recording that ended at `after`: the largest earlier total.
fn cumulative_before(after: Usage) -> Usage {
    SESSION
        .iter()
        .filter_map(|(_, jsonl)| {
            let mut stream = CodexStream::default();
            for line in jsonl.lines() {
                stream.line(line);
            }
            stream.cumulative_usage()
        })
        .filter(|usage| usage.input_tokens < after.input_tokens)
        .max_by_key(|usage| usage.input_tokens)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_number_is_the_first_flow_token() {
        assert_eq!(
            flow_number("Job: analyse_flow f3. Plan: f1, f2, f3"),
            Some(3)
        );
        assert_eq!(flow_number("Job: plan"), None);
        assert_eq!(flow_number("of fx f12"), Some(12));
    }

    #[test]
    fn first_job_usage_is_its_total_and_later_jobs_the_difference() {
        let plan = Usage {
            input_tokens: 48210,
            cached_input_tokens: 30976,
            output_tokens: 1904,
        };
        assert_eq!(cumulative_before(plan), Usage::default());
        let after_plan_compaction = Usage {
            input_tokens: 109_232,
            cached_input_tokens: 78080,
            output_tokens: 2884,
        };
        let pr_context = Usage {
            input_tokens: 180_776,
            cached_input_tokens: 130_304,
            output_tokens: 4414,
        };
        assert_eq!(cumulative_before(pr_context), after_plan_compaction);
    }
}
