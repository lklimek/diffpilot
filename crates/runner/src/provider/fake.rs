//! Scripted provider for tests and development (feature `fake-provider`, never in release builds): replays
//! the recorded Codex streams of the fixture PR (`fixtures/pr-482/codex/`) through the Codex stream parser.
//!
//! The recording is chosen by the call's output schema (job kind) and, for `analyse_flow`, by the first flow
//! id token `f<n>` of the prompt's `Job: analyse_flow f<n>.` line. `ask` has no recording.
//!
//! A script (JSON file named by [`SCRIPT_ENV`], read before every call; no file: plain replay) overrides calls
//! for the full-stack tests: the first rule matching the job kind, the flow and a prompt substring applies, at
//! most `times` times. Example: `{"rules": [{"id": "pause", "job": "analyse_flow", "flow": 2, "times": 1,
//! "action": {"usage_limit": {"resets_in_secs": 5}}}]}`; actions: `replay` (the recording, `step_delay_ms` per
//! line), `usage_limit`, `fail` (`message`) and `answer` (`output`: the final JSON, e.g. an ask answer).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{TimeDelta, Utc};
use contracts::agent::{JobKind, output_schema};
use contracts::ids::JobId;
use contracts::job::event::JobEvent;
use contracts::job::result::Usage;
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
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

/// Environment variable naming the script file of the full-stack tests.
pub const SCRIPT_ENV: &str = "SR_RUNNER_FAKE_SCRIPT";

/// Replays fixture recordings; `cancel` ends a running replay with [`ProviderError::Cancelled`].
#[derive(Debug, Default)]
pub struct FakeProvider {
    step_delay: Duration,
    running: Mutex<HashMap<JobId, Arc<Notify>>>,
    script: Option<PathBuf>,
    /// Applications per script rule id.
    uses: Mutex<HashMap<String, u32>>,
}

/// Overrides of the full-stack tests.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Script {
    rules: Vec<Rule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    id: String,
    job: JobKind,
    flow: Option<u32>,
    prompt_contains: Option<String>,
    /// Unlimited when absent.
    times: Option<u32>,
    action: Action,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Replay { step_delay_ms: u64 },
    UsageLimit { resets_in_secs: Option<i64> },
    Fail { message: String },
    Answer { output: Value },
}

impl FakeProvider {
    /// A provider that waits `step_delay` before each recorded line (zero: replay at once), scripted by the file
    /// [`SCRIPT_ENV`] names, if set.
    pub fn new(step_delay: Duration) -> Self {
        Self {
            step_delay,
            script: std::env::var_os(SCRIPT_ENV).map(PathBuf::from),
            ..Self::default()
        }
    }

    /// The same provider scripted by the file at `path`.
    pub fn with_script(mut self, path: PathBuf) -> Self {
        self.script = Some(path);
        self
    }

    /// The action of the first script rule matching the call that has uses left; counts the use.
    fn scripted(
        &self,
        kind: JobKind,
        prompt: &str,
    ) -> Result<Option<(String, Action)>, ProviderError> {
        let Some(path) = &self.script else {
            return Ok(None);
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(ProviderError::Io(error)),
        };
        let script: Script =
            serde_json::from_str(&text).map_err(|error| ProviderError::Failed {
                message: format!("the fake provider script is not valid: {error}"),
            })?;
        let flow = flow_number(prompt);
        let mut uses = self.uses.lock().unwrap_or_else(PoisonError::into_inner);
        let rule = script.rules.into_iter().find(|rule| {
            rule.job == kind
                && rule.flow.is_none_or(|wanted| Some(wanted) == flow)
                && rule
                    .prompt_contains
                    .as_deref()
                    .is_none_or(|needle| prompt.contains(needle))
                && rule
                    .times
                    .is_none_or(|times| uses.get(&rule.id).copied().unwrap_or(0) < times)
        });
        Ok(rule.map(|rule| {
            *uses.entry(rule.id.clone()).or_default() += 1;
            (rule.id, rule.action)
        }))
    }

    async fn replay(
        &self,
        call: &ProviderCall,
        events: &mpsc::Sender<ProviderEvent>,
    ) -> Result<ProviderOutcome, ProviderError> {
        let kind = job_kind(call)?;
        let (jsonl, step_delay) = match self.scripted(kind, &call.prompt)? {
            None => (recorded(kind, &call.prompt)?.to_owned(), self.step_delay),
            Some((id, action)) => {
                tracing::info!(job = %call.job_id, rule = %id, "fake provider script rule applied");
                match action {
                    Action::Replay { step_delay_ms } => (
                        recorded(kind, &call.prompt)?.to_owned(),
                        Duration::from_millis(step_delay_ms),
                    ),
                    Action::UsageLimit { resets_in_secs } => {
                        return Err(ProviderError::UsageLimit {
                            resets_at: resets_in_secs
                                .map(|secs| Utc::now() + TimeDelta::seconds(secs)),
                        });
                    }
                    Action::Fail { message } => return Err(ProviderError::Failed { message }),
                    Action::Answer { output } => (answer_stream(&output), Duration::ZERO),
                }
            }
        };
        let cancelled = Arc::new(Notify::new());
        let _registration = Registration::new(self, call.job_id, Arc::clone(&cancelled));
        let mut stream = CodexStream::default();
        for line in jsonl.lines() {
            if !step_delay.is_zero() {
                tokio::select! {
                    () = tokio::time::sleep(step_delay) => {}
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

/// The call's job kind, from its output schema.
fn job_kind(call: &ProviderCall) -> Result<JobKind, ProviderError> {
    JobKind::ALL
        .into_iter()
        .find(|kind| output_schema(*kind).is_ok_and(|schema| schema == call.output_schema))
        .ok_or_else(|| ProviderError::Failed {
            message: "the fake provider does not know this output schema".to_owned(),
        })
}

/// The recording for a call of `kind`; the flow comes from the prompt.
fn recorded(kind: JobKind, prompt: &str) -> Result<&'static str, ProviderError> {
    let name = match kind {
        JobKind::Plan => "plan".to_owned(),
        JobKind::PrContext => "pr-context".to_owned(),
        JobKind::AnalyseFlow => format!("analyse-flow-{}", flow_number(prompt).unwrap_or(1)),
        JobKind::Ask => {
            return Err(ProviderError::Failed {
                message: "the fake provider has no recording for ask jobs".to_owned(),
            });
        }
    };
    recording(&name).ok_or_else(|| ProviderError::Failed {
        message: format!("the fake provider has no recording {name}"),
    })
}

/// A Codex stream of the fixture session whose final message is `output`; its cumulative usage lies above every
/// recording's, so the call's usage is a positive difference.
fn answer_stream(output: &Value) -> String {
    [
        json!({"type": "thread.started", "thread_id": "019a8f2c-4820-7000-8000-000000000482"}),
        json!({"type": "turn.started"}),
        json!({"type": "item.completed", "item": {"id": "item_0", "type": "agent_message",
            "text": output.to_string()}}),
        json!({"type": "turn.completed", "usage": {"input_tokens": 2_000_000,
            "cached_input_tokens": 1_500_000, "cache_write_input_tokens": 0, "output_tokens": 50_000,
            "reasoning_output_tokens": 0}}),
    ]
    .map(|line| line.to_string())
    .join("\n")
}

/// The number of the first `f<n>` token of the prompt's `Job:` line (a context pack before it may name other
/// flows).
fn flow_number(prompt: &str) -> Option<u32> {
    prompt
        .lines()
        .find(|line| line.starts_with("Job: "))?
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
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;
    use crate::test_support::fixture;

    fn call(kind: JobKind, prompt: &str) -> ProviderCall {
        let spec = crate::test_support::spec(kind, Path::new("/nonexistent"));
        ProviderCall {
            job_id: spec.job_id,
            job_dir: PathBuf::new(),
            codex_home: PathBuf::new(),
            session_dir: PathBuf::new(),
            prompt: prompt.to_owned(),
            output_schema: spec.output_schema,
            model: spec.model,
            limits: spec.limits,
            access: spec.access,
            skills: Vec::new(),
        }
    }

    fn scripted(dir: &TempDir, script: &Value) -> FakeProvider {
        let path = dir.path().join("script.json");
        std::fs::write(&path, script.to_string()).unwrap();
        FakeProvider::new(Duration::ZERO).with_script(path)
    }

    async fn run(
        provider: &FakeProvider,
        kind: JobKind,
        prompt: &str,
    ) -> Result<Value, ProviderError> {
        let (events, _received) = mpsc::channel(256);
        provider
            .run(&call(kind, prompt), events)
            .await
            .map(|outcome| outcome.output)
    }

    fn agent_output(file: &str) -> Value {
        serde_json::from_str(&std::fs::read_to_string(fixture(&format!("agent/{file}"))).unwrap())
            .unwrap()
    }

    #[tokio::test]
    async fn script_usage_limit_applies_its_times_then_the_recording_replays() {
        let dir = TempDir::new().unwrap();
        let provider = scripted(
            &dir,
            &json!({"rules": [{"id": "pause", "job": "analyse_flow", "flow": 2, "times": 1,
                "action": {"usage_limit": {"resets_in_secs": 5}}}]}),
        );
        let before = Utc::now();
        match run(&provider, JobKind::AnalyseFlow, "Job: analyse_flow f2.").await {
            Err(ProviderError::UsageLimit {
                resets_at: Some(at),
            }) => assert!(
                at >= before + TimeDelta::seconds(5) && at <= Utc::now() + TimeDelta::seconds(5)
            ),
            other => panic!("expected a usage limit with a reset time, got {other:?}"),
        }
        assert_eq!(
            run(&provider, JobKind::AnalyseFlow, "Job: analyse_flow f1.")
                .await
                .unwrap(),
            agent_output("flow-1.json"),
            "other flows replay"
        );
        assert_eq!(
            run(&provider, JobKind::AnalyseFlow, "Job: analyse_flow f2.")
                .await
                .unwrap(),
            agent_output("flow-2.json"),
            "the rule is used up"
        );
    }

    #[tokio::test]
    async fn script_answers_and_fails_ask_jobs_by_prompt_substring() {
        let dir = TempDir::new().unwrap();
        let answer = json!({"answer": "It reads line 99.", "evidence": []});
        let provider = scripted(
            &dir,
            &json!({"rules": [
                {"id": "ok", "job": "ask", "prompt_contains": "[answer]", "action": {"answer": {"output": answer}}},
                {"id": "no", "job": "ask", "prompt_contains": "[fail]", "action": {"fail": {"message": "scripted"}}}
            ]}),
        );
        let (events, mut received) = mpsc::channel(256);
        let outcome = provider
            .run(&call(JobKind::Ask, "Job: ask.\nWhy? [answer]"), events)
            .await
            .unwrap();
        assert_eq!(outcome.output, answer);
        assert_eq!(
            outcome.session_id.as_deref(),
            Some("019a8f2c-4820-7000-8000-000000000482")
        );
        assert!(outcome.usage.input_tokens > 0);
        let mut last = None;
        while let Ok(event) = received.try_recv() {
            last = Some(event);
        }
        assert_eq!(last, Some(ProviderEvent::OutputReady(answer)));
        assert!(matches!(
            run(&provider, JobKind::Ask, "Job: ask.\n[fail]").await,
            Err(ProviderError::Failed { message }) if message == "scripted"
        ));
        assert!(
            run(&provider, JobKind::Ask, "Job: ask.").await.is_err(),
            "no rule, no recording"
        );
    }

    #[tokio::test]
    async fn missing_script_replays_and_an_invalid_script_fails_the_call() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("script.json");
        let provider = FakeProvider::new(Duration::ZERO).with_script(path.clone());
        assert_eq!(
            run(&provider, JobKind::Plan, "Job: plan.").await.unwrap(),
            agent_output("plan.json")
        );
        std::fs::write(&path, r#"{"rules": [{"id": "x"}]}"#).unwrap();
        assert!(matches!(
            run(&provider, JobKind::Plan, "Job: plan.").await,
            Err(ProviderError::Failed { message }) if message.starts_with("the fake provider script is not valid")
        ));
    }

    #[tokio::test]
    async fn scripted_slow_replay_is_cancellable() {
        let dir = TempDir::new().unwrap();
        let provider = Arc::new(scripted(
            &dir,
            &json!({"rules": [{"id": "slow", "job": "plan", "action": {"replay": {"step_delay_ms": 200}}}]}),
        ));
        let job = call(JobKind::Plan, "Job: plan.");
        let id = job.job_id;
        let running = tokio::spawn({
            let provider = Arc::clone(&provider);
            async move {
                let (events, _received) = mpsc::channel(256);
                provider.run(&job, events).await
            }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        provider.cancel(id).await.unwrap();
        assert!(matches!(
            running.await.unwrap(),
            Err(ProviderError::Cancelled)
        ));
    }

    #[test]
    fn flow_number_is_the_first_flow_token() {
        assert_eq!(
            flow_number("Job: analyse_flow f3. Plan: f1, f2, f3"),
            Some(3)
        );
        assert_eq!(flow_number("Job: plan"), None);
        assert_eq!(flow_number("Job: of fx f12"), Some(12));
        assert_eq!(
            flow_number("Context pack\n{\"flow_id\":\"f1\"}\n\nJob: repair analyse_flow f4.\nf2"),
            Some(4),
            "the context pack before the Job line names other flows"
        );
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
