//! Scripted provider for tests and development (feature `fake-provider`, never in release builds): replays
//! the recorded streams of the fixture PR (`fixtures/pr-482/codex/`) through an offline fixture decoder.
//!
//! The recording follows the self-contained job kind and, for `analyse_flow`, the first flow
//! id token `f<n>` of the prompt's `Job: analyse_flow f<n>.` line. `ask` has no recording; `judge_work` answers
//! every item of the prompt's data block neutrally (asked parts 0, eligibility as computed); `verify_comments`
//! answers every item of its data block with a neutral assessment and, in a question pass, the question.
//!
//! A script (JSON file named by [`SCRIPT_ENV`], read before every call; no file: plain replay) overrides calls
//! for the full-stack tests: the first rule matching the job kind, the flow and a prompt substring applies, at
//! most `times` times. Example: `{"rules": [{"id": "pause", "job": "analyse_flow", "flow": 2, "times": 1,
//! "action": {"usage_limit": {"resets_in_secs": 5}}}]}`; actions: `replay` (the recording, `step_delay_ms` per
//! line), `usage_limit`, `fail` (`message`) and `answer` (`output`: a JSON object for JSON jobs,
//! a tagged-text string for file jobs).

use contracts::agent::JobKind;
use contracts::agent::judge_work::{JudgeWorkInput, part_values};
use contracts::agent::verify_comments::VerifyCommentsInput;
use contracts::api::dashboard::ScoreComponent;
use contracts::ids::JobId;
use contracts::job::event::JobEvent;
use contracts::job::result::Usage;
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};

use contracts::time::Timestamp;
use jiff::SignedDuration;

mod host;
mod recording;
use crate::provider::{LlmProvider, ProviderCall, ProviderError, ProviderEvent, ProviderOutcome};
use recording::{Recording, usage_delta};

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
        let kind = call.job_spec.kind;
        let mut scripted_output = None;
        let replay = || {
            if call.job_spec.assessment.as_ref().is_some_and(|run| {
                run.method == contracts::assessment::AssessmentMethod::ClaudiusHost
            }) {
                let data = call
                    .job_spec
                    .pr_data
                    .as_ref()
                    .ok_or_else(|| ProviderError::Failed {
                        message: "host fixture lacks frozen PR data".into(),
                    })?;
                Ok(answer_stream(&host::report(data)))
            } else {
                stream(kind, &call.job_spec.prompt)
            }
        };
        let (jsonl, step_delay) = match self.scripted(kind, &call.job_spec.prompt)? {
            None => (replay()?, self.step_delay),
            Some((id, action)) => {
                tracing::info!(job = %call.job_spec.job_id, rule = %id, "fake provider script rule applied");
                match action {
                    Action::Replay { step_delay_ms } => {
                        (replay()?, Duration::from_millis(step_delay_ms))
                    }
                    Action::UsageLimit { resets_in_secs } => {
                        return Err(ProviderError::UsageLimit {
                            resets_at: resets_in_secs
                                .map(|secs| Timestamp::now() + SignedDuration::from_secs(secs)),
                        });
                    }
                    Action::Fail { message } => return Err(ProviderError::Failed { message }),
                    Action::Answer { output } => {
                        scripted_output = Some(output.clone());
                        (answer_stream(&output), Duration::ZERO)
                    }
                }
            }
        };
        let cancelled = Arc::new(Notify::new());
        let _registration = Registration::new(self, call.job_spec.job_id, Arc::clone(&cancelled));
        let mut stream = Recording::default();
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
                    at: Timestamp::now(),
                    activity,
                    summary,
                };
                let _ = events.send(ProviderEvent::Activity(event)).await;
            }
        }
        let mut finished = stream.finish()?;
        if call.job_spec.flow_file.is_some() {
            let text = if let Some(output) = scripted_output {
                output
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| output.to_string())
            } else {
                let files = [
                    include_str!("../../../../fixtures/pr-482/agent/flow-1.md"),
                    include_str!("../../../../fixtures/pr-482/agent/flow-2.md"),
                    include_str!("../../../../fixtures/pr-482/agent/flow-3.md"),
                    include_str!("../../../../fixtures/pr-482/agent/flow-4.md"),
                    include_str!("../../../../fixtures/pr-482/agent/flow-5.md"),
                ];
                let n = flow_number(&call.job_spec.prompt).unwrap_or(1) as usize;
                files
                    .get(n.saturating_sub(1))
                    .ok_or_else(|| ProviderError::Failed {
                        message: "unknown fixture flow".into(),
                    })?
                    .to_string()
            };
            finished.output = Value::String(text);
        }
        let _ = events
            .send(ProviderEvent::OutputReady(finished.output.clone()))
            .await;
        Ok(ProviderOutcome {
            runtime_session: (!matches!(kind, JobKind::JudgeWork | JobKind::VerifyComments)).then(
                || {
                    let resumed = call
                        .job_spec
                        .working_session
                        .as_ref()
                        .and_then(|w| w.thread_id.as_ref())
                        .is_some()
                        || call.job_spec.session.is_some();
                    contracts::job::result::SessionReport {
                        thread_id: finished
                            .thread_id
                            .clone()
                            .unwrap_or_else(|| "fake-thread".into()),
                        outcome: if resumed {
                            contracts::job::result::SessionOutcome::Resumed
                        } else {
                            contracts::job::result::SessionOutcome::Started
                        },
                        last_context: None,
                        preflight: call
                            .job_spec
                            .working_session
                            .as_ref()
                            .and_then(|w| w.preflight.clone())
                            .map(Box::new),
                    }
                },
            ),
            session_turns: None,
            output: finished.output,
            session_id: (!matches!(kind, JobKind::JudgeWork | JobKind::VerifyComments))
                .then_some(finished.thread_id)
                .flatten(),
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

/// The Codex stream replayed for a call of `kind`: its recording, or for `judge_work` a neutral answer to the
/// prompt's items.
fn stream(kind: JobKind, prompt: &str) -> Result<String, ProviderError> {
    match kind {
        JobKind::JudgeWork => Ok(answer_stream(&judge_answer(prompt)?)),
        JobKind::VerifyComments => Ok(answer_stream(&verify_answer(prompt)?)),
        _ => recorded(kind, prompt).map(str::to_owned),
    }
}

/// A valid neutral answer to every item of the prompt's judge input: each asked part 0 (defect likelihood and
/// impact 0), eligibility kept as computed when asked, no merges, no alarm.
fn judge_answer(prompt: &str) -> Result<Value, ProviderError> {
    let input = JudgeWorkInput::from_prompt(prompt).ok_or_else(|| ProviderError::Failed {
        message: "the fake provider found no judge input in the prompt".to_owned(),
    })?;
    let basis = "fake judgement for tests";
    let items: Vec<Value> = input
        .items
        .iter()
        .map(|item| {
            let defect = item
                .asks
                .contains(&ScoreComponent::DefectSeverity)
                .then(|| json!({"likelihood": 0, "impact": 0, "basis": basis}));
            let parts: Vec<Value> = item
                .asks
                .iter()
                .filter(|component| part_values(**component).is_some())
                .map(|component| json!({"component": component, "points": 0, "basis": basis}))
                .collect();
            let eligibility = item
                .asks_eligibility
                .then(|| json!({"eligible": item.eligibility.eligible, "reason": basis}));
            json!({
                "key": item.key, "defect": defect, "parts": parts, "eligibility": eligibility,
                "also_keys": [], "alarm": null,
                "why": "Fake judgement for tests.", "next": "No action: fake judgement."
            })
        })
        .collect();
    Ok(json!({ "items": items }))
}

/// A valid neutral assessment of every item of the prompt's verify input: partly valid, in scope, no gates,
/// recommended for a fix here; a question pass also answers the question.
fn verify_answer(prompt: &str) -> Result<Value, ProviderError> {
    let input = VerifyCommentsInput::from_prompt(prompt).ok_or_else(|| ProviderError::Failed {
        message: "the fake provider found no verify input in the prompt".to_owned(),
    })?;
    let items: Vec<Value> = input
        .items
        .iter()
        .map(|item| {
            let code = item.location.as_ref().and_then(|location| {
                location.lines.map(|lines| {
                    json!({"path": location.path, "enclosing": lines,
                        "description": "Fake description of the enclosing code for tests."})
                })
            });
            json!({
                "item": item.id, "validity": "partly_valid", "evidence": [], "pre_existing": false,
                "category": "other", "necessity": "in", "necessity_reason": "Fake assessment for tests.",
                "better_pr": null, "gates": [], "elsewhere": [], "fix_sketch": "Fake fix sketch.", "fix_size": "XS",
                "reply_drafts": ["Fixed per maintainer decision."],
                "recommendation": {"kind": "fix_here", "reason": "Fake recommendation for tests.",
                    "reason_class": "fake", "target_pr": null, "target_url": null, "title": null},
                "code": code
            })
        })
        .collect();
    let answer = input
        .question
        .map(|_| "Fake answer to the question for tests.");
    Ok(json!({"purpose": "Fake purpose for tests.", "items": items, "answer": answer}))
}

/// The recording for a call of `kind`; the flow comes from the prompt.
fn recorded(kind: JobKind, prompt: &str) -> Result<&'static str, ProviderError> {
    let name = match kind {
        JobKind::Plan => "plan".to_owned(),
        JobKind::PrContext => "pr-context".to_owned(),
        JobKind::AnalyseFlow => format!("analyse-flow-{}", flow_number(prompt).unwrap_or(1)),
        JobKind::Ask | JobKind::JudgeWork | JobKind::VerifyComments => {
            return Err(ProviderError::Failed {
                message:
                    "the fake provider has no recording for ask, judge_work or verify_comments jobs"
                        .to_owned(),
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
            let mut stream = Recording::default();
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
            job_spec: Box::new(contracts::job::spec::JobSpec {
                prompt: prompt.into(),
                ..spec
            }),
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
        let before = Timestamp::now();
        match run(&provider, JobKind::AnalyseFlow, "Job: analyse_flow f2.").await {
            Err(ProviderError::UsageLimit {
                resets_at: Some(at),
            }) => assert!(
                at >= before + SignedDuration::from_secs(5)
                    && at <= Timestamp::now() + SignedDuration::from_secs(5)
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
        let id = job.job_spec.job_id;
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
            requests: None,
        };
        assert_eq!(cumulative_before(plan), Usage::default());
        let after_plan_compaction = Usage {
            input_tokens: 109_232,
            cached_input_tokens: 78080,
            output_tokens: 2884,
            requests: None,
        };
        let pr_context = Usage {
            input_tokens: 180_776,
            cached_input_tokens: 130_304,
            output_tokens: 4414,
            requests: None,
        };
        assert_eq!(cumulative_before(pr_context), after_plan_compaction);
    }
}

#[cfg(test)]
mod file_tests {
    use super::*;
    #[tokio::test]
    async fn scripted_file_is_written_and_collected() {
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("script.json");
        let text = "# flow f1\n";
        std::fs::write(&script,json!({"rules":[{"id":"file","job":"analyse_flow","action":{"answer":{"output":text}}}]}).to_string()).unwrap();
        let mut spec = crate::test_support::spec(JobKind::AnalyseFlow, root.path());
        spec.prompt = "Job: analyse_flow f1.".into();
        spec.flow_file = Some(contracts::job::spec::FlowFile {
            context: json!({}),
            previous: None,
        });
        let call = ProviderCall {
            job_spec: Box::new(spec),
        };
        let (send, mut receive) = mpsc::channel(32);
        let provider = FakeProvider::default().with_script(script);
        let outcome = provider.run(&call, send).await.unwrap();
        assert_eq!(outcome.output, json!(text));
        assert!(
            std::iter::from_fn(|| receive.try_recv().ok())
                .any(|e| e == ProviderEvent::OutputReady(json!(text)))
        );
    }
}
