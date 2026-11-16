//! Parser of the `codex exec --json` event stream (S1a finding 3): normalised activities while it runs, the
//! final structured output or a typed error at the end.

use contracts::job::event::ActivityKind;
use contracts::job::result::Usage;
use serde::Deserialize;
use serde_json::Value;

use crate::provider::ProviderError;
use crate::scrub::clip;

/// Exit code of a process ended by SIGTERM (128 + 15).
const EXIT_SIGTERM: i32 = 143;
/// Signal number of SIGTERM.
const SIGTERM: i32 = 15;
/// Longest provider message kept in an error.
const MESSAGE_MAX_CHARS: usize = 300;
/// `turn.failed` texts of a reached usage limit (S1b finding 10, ADR-003); Codex uses a typographic apostrophe.
const USAGE_LIMIT_PREFIXES: [&str; 3] = [
    "You’ve hit your usage limit",
    "You've hit your usage limit",
    "Your workspace is out of credits",
];
/// Error codes of a credential that can no longer be refreshed (S1a finding 2, ADR-003).
const REFRESH_FAILURES: [&str; 3] = [
    "refresh_token_reused",
    "refresh_token_expired",
    "refresh_token_invalidated",
];

/// How the Codex process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Exit {
    /// Exited with this code.
    Code(i32),
    /// Killed by this signal.
    Signal(i32),
}

impl Exit {
    fn terminated(self) -> bool {
        matches!(self, Self::Code(EXIT_SIGTERM) | Self::Signal(SIGTERM))
    }
}

/// A successful turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Finished {
    /// The final structured output.
    pub(crate) output: Value,
    /// The Codex thread id.
    pub(crate) thread_id: Option<String>,
    /// Cumulative usage of the thread after the turn (S1a finding 3).
    pub(crate) cumulative_usage: Usage,
}

/// State accumulated over the lines of one `codex exec --json` run.
#[derive(Debug, Default)]
pub(crate) struct CodexStream {
    thread_id: Option<String>,
    last_message: Option<String>,
    cumulative_usage: Option<Usage>,
    completed: bool,
    failure: Option<String>,
    last_error: Option<String>,
    unparsed_lines: usize,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Line {
    #[serde(rename = "thread.started")]
    ThreadStarted { thread_id: String },
    #[serde(rename = "turn.completed")]
    TurnCompleted { usage: RawUsage },
    #[serde(rename = "turn.failed")]
    TurnFailed { error: RawError },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "item.completed")]
    ItemCompleted { item: Item },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct RawError {
    message: String,
}

#[derive(Deserialize)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Item {
    AgentMessage {
        text: String,
    },
    Reasoning {
        text: String,
    },
    CommandExecution {
        command: String,
        exit_code: Option<i64>,
    },
    McpToolCall {
        server: String,
        tool: String,
        status: Option<String>,
    },
    #[serde(other)]
    Other,
}

impl CodexStream {
    /// Takes one JSONL line; returns the activity it represents, with an unscrubbed and unclipped summary.
    /// Only completed items are activities; command output, MCP results and file contents never are.
    pub(crate) fn line(&mut self, line: &str) -> Option<(ActivityKind, String)> {
        if line.trim().is_empty() {
            return None;
        }
        let Ok(parsed) = serde_json::from_str::<Line>(line) else {
            self.unparsed_lines += 1;
            return None;
        };
        match parsed {
            Line::ThreadStarted { thread_id } => self.thread_id = Some(thread_id),
            Line::TurnCompleted { usage } => {
                self.completed = true;
                self.cumulative_usage = Some(Usage {
                    input_tokens: usage.input_tokens,
                    cached_input_tokens: usage.cached_input_tokens,
                    output_tokens: usage.output_tokens,
                });
            }
            Line::TurnFailed { error } => self.failure = Some(error.message),
            Line::Error { message } => self.last_error = Some(message),
            Line::ItemCompleted { item } => return self.item(item),
            Line::Other => {}
        }
        None
    }

    fn item(&mut self, item: Item) -> Option<(ActivityKind, String)> {
        match item {
            Item::AgentMessage { text } => {
                let start = text.trim_start();
                let summary = if start.starts_with('{') || start.starts_with('[') {
                    "structured answer".to_owned()
                } else {
                    text.clone()
                };
                self.last_message = Some(text);
                Some((ActivityKind::AgentMessage, summary))
            }
            Item::Reasoning { text } => Some((ActivityKind::Reasoning, text)),
            Item::CommandExecution { command, exit_code } => {
                let command = unwrap_shell(&command);
                let summary = match exit_code {
                    Some(code) => format!("$ {command} (exit {code})"),
                    None => format!("$ {command}"),
                };
                Some((ActivityKind::Command, summary))
            }
            Item::McpToolCall {
                server,
                tool,
                status,
                ..
            } => {
                let status = status.unwrap_or_else(|| "unknown".to_owned());
                Some((
                    ActivityKind::McpToolCall,
                    format!("{server}.{tool} ({status})"),
                ))
            }
            Item::Other => None,
        }
    }

    /// Lines that were not JSON events (logged by the caller).
    pub(crate) fn unparsed_lines(&self) -> usize {
        self.unparsed_lines
    }

    /// Cumulative thread usage reported by `turn.completed`, if the turn completed.
    #[cfg(feature = "fake-provider")]
    pub(crate) fn cumulative_usage(&self) -> Option<Usage> {
        self.cumulative_usage
    }

    /// The outcome of the run once the process exited with `exit`.
    pub(crate) fn finish(self, exit: Exit) -> Result<Finished, ProviderError> {
        if let Some(message) = self.failure {
            return Err(classify_failure(&message));
        }
        if self.completed {
            if exit != Exit::Code(0) {
                return Err(ProviderError::Failed {
                    message: format!("Codex finished the turn but exited with {}", describe(exit)),
                });
            }
            let text = self.last_message.ok_or(ProviderError::OutputNotJson)?;
            let output =
                serde_json::from_str::<Value>(&text).map_err(|_| ProviderError::OutputNotJson)?;
            return Ok(Finished {
                output,
                thread_id: self.thread_id,
                cumulative_usage: self.cumulative_usage.unwrap_or_default(),
            });
        }
        if exit.terminated() {
            return Err(ProviderError::Cancelled);
        }
        if let Some(message) = self.last_error {
            return Err(classify_failure(&message));
        }
        Err(ProviderError::Failed {
            message: format!(
                "Codex stopped without finishing the turn ({})",
                describe(exit)
            ),
        })
    }
}

/// Usage of one call: the difference of the thread's cumulative totals before and after it (ADR-003).
pub(crate) fn usage_delta(before: Usage, after: Usage) -> Usage {
    Usage {
        input_tokens: after.input_tokens.saturating_sub(before.input_tokens),
        cached_input_tokens: after
            .cached_input_tokens
            .saturating_sub(before.cached_input_tokens),
        output_tokens: after.output_tokens.saturating_sub(before.output_tokens),
    }
}

fn describe(exit: Exit) -> String {
    match exit {
        Exit::Code(code) => format!("exit code {code}"),
        Exit::Signal(signal) => format!("signal {signal}"),
    }
}

fn classify_failure(message: &str) -> ProviderError {
    if USAGE_LIMIT_PREFIXES
        .iter()
        .any(|prefix| message.trim_start().starts_with(prefix))
    {
        // The text has no machine-readable reset time; WP3 takes it from the rollout (ADR-003).
        return ProviderError::UsageLimit { resets_at: None };
    }
    if REFRESH_FAILURES.iter().any(|code| message.contains(code)) {
        return ProviderError::CredentialUnusable;
    }
    let inner = api_error_message(message);
    if message.contains("invalid_json_schema") {
        return ProviderError::SchemaRejected {
            message: clip(&inner, MESSAGE_MAX_CHARS),
        };
    }
    ProviderError::Failed {
        message: clip(&inner, MESSAGE_MAX_CHARS),
    }
}

/// `error.message` of an API error body embedded as JSON text, else the text itself.
fn api_error_message(message: &str) -> String {
    serde_json::from_str::<Value>(message)
        .ok()
        .and_then(|body| {
            body.pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| message.to_owned())
}

/// `cmd` of Codex's `/bin/bash -lc '<cmd>'` wrapper, else the command unchanged.
fn unwrap_shell(command: &str) -> &str {
    command
        .strip_prefix("/bin/bash -lc '")
        .and_then(|rest| rest.strip_suffix('\''))
        .unwrap_or(command)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::*;

    fn codex_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482/codex")
    }

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482")
    }

    fn parse(file: &str) -> (CodexStream, Vec<(ActivityKind, String)>) {
        let text = fs::read_to_string(codex_dir().join(file)).expect("fixture");
        let mut stream = CodexStream::default();
        let activities = text.lines().filter_map(|line| stream.line(line)).collect();
        (stream, activities)
    }

    #[derive(Deserialize)]
    struct ManifestEntry {
        file: String,
        thread_id: String,
        exit_code: i32,
        outcome: String,
        output: Option<String>,
    }

    /// Every recorded stream of the manifest ends as the manifest says.
    #[test]
    fn recorded_streams_end_as_the_manifest_says() {
        let manifest: Vec<ManifestEntry> =
            serde_json::from_str(&fs::read_to_string(codex_dir().join("manifest.json")).unwrap())
                .unwrap();
        assert!(manifest.len() >= 12, "manifest lists every recording");
        for entry in manifest {
            let (stream, _) = parse(&entry.file);
            assert_eq!(stream.unparsed_lines(), 0, "{}", entry.file);
            let result = stream.finish(Exit::Code(entry.exit_code));
            match entry.outcome.as_str() {
                // Compaction turns answer a trivial prompt in plain text; they never carry the job output.
                "succeeded" if entry.output.is_none() => assert!(
                    matches!(result, Err(ProviderError::OutputNotJson)),
                    "{}: {result:?}",
                    entry.file
                ),
                "succeeded" => {
                    let finished = result.unwrap_or_else(|e| panic!("{}: {e}", entry.file));
                    assert_eq!(
                        finished.thread_id.as_deref(),
                        Some(entry.thread_id.as_str())
                    );
                    assert!(finished.cumulative_usage.input_tokens > 0, "{}", entry.file);
                    if let Some(rel) = entry.output {
                        let expected: Value = serde_json::from_str(
                            &fs::read_to_string(fixture_root().join(&rel)).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(finished.output, expected, "{}", entry.file);
                    }
                }
                "failed" => assert!(
                    matches!(result, Err(ProviderError::SchemaRejected { .. })),
                    "{}: {result:?}",
                    entry.file
                ),
                "cancelled" => assert!(
                    matches!(result, Err(ProviderError::Cancelled)),
                    "{}: {result:?}",
                    entry.file
                ),
                "usage_limited" => assert!(
                    matches!(result, Err(ProviderError::UsageLimit { resets_at: None })),
                    "{}: {result:?}",
                    entry.file
                ),
                other => panic!("unknown outcome {other}"),
            }
        }
    }

    #[test]
    fn schema_error_carries_the_plain_api_message() {
        let (stream, _) = parse("schema-error.jsonl");
        match stream.finish(Exit::Code(1)) {
            Err(ProviderError::SchemaRejected { message }) => {
                assert!(
                    message.starts_with("Invalid schema for response_format"),
                    "{message}"
                );
                assert!(!message.contains("\"status\""), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn cancelled_by_signal_without_terminal_event() {
        let (stream, _) = parse("cancelled.jsonl");
        assert!(matches!(
            stream.finish(Exit::Signal(SIGTERM)),
            Err(ProviderError::Cancelled)
        ));
    }

    #[test]
    fn stream_cut_without_sigterm_is_a_failure() {
        let (stream, _) = parse("cancelled.jsonl");
        assert!(matches!(
            stream.finish(Exit::Code(1)),
            Err(ProviderError::Failed { .. })
        ));
    }

    #[test]
    fn completed_turn_with_non_json_answer_is_output_not_json() {
        let mut stream = CodexStream::default();
        for line in [
            r#"{"type":"item.completed","item":{"id":"i","type":"agent_message","text":"Done."}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":1,"cached_input_tokens":0,"output_tokens":1}}"#,
        ] {
            stream.line(line);
        }
        assert!(matches!(
            stream.finish(Exit::Code(0)),
            Err(ProviderError::OutputNotJson)
        ));
    }

    #[test]
    fn refresh_failure_marks_the_credential_unusable() {
        let mut stream = CodexStream::default();
        stream.line(r#"{"type":"turn.failed","error":{"message":"token refresh failed: refresh_token_reused"}}"#);
        assert!(matches!(
            stream.finish(Exit::Code(1)),
            Err(ProviderError::CredentialUnusable)
        ));
    }

    #[test]
    fn activities_carry_command_line_and_exit_code_not_output() {
        let (_, activities) = parse("plan.jsonl");
        let commands: Vec<&String> = activities
            .iter()
            .filter(|(kind, _)| *kind == ActivityKind::Command)
            .map(|(_, summary)| summary)
            .collect();
        assert_eq!(commands[0], "$ git diff --stat HEAD~1 HEAD (exit 0)");
        let all = activities
            .iter()
            .map(|(_, s)| s.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !all.contains("13 files changed"),
            "command output leaked: {all}"
        );
        assert!(
            !all.contains("impl RpcClient"),
            "file contents leaked: {all}"
        );
    }

    #[test]
    fn mcp_calls_are_activities_without_results() {
        let (_, activities) = parse("pr-context.jsonl");
        let calls: Vec<&String> = activities
            .iter()
            .filter(|(kind, _)| *kind == ActivityKind::McpToolCall)
            .map(|(_, summary)| summary)
            .collect();
        assert!(!calls.is_empty());
        assert!(calls.iter().all(|c| c.starts_with("github.")), "{calls:?}");
    }

    #[test]
    fn final_answer_is_not_repeated_in_a_summary() {
        let (_, activities) = parse("plan.jsonl");
        let (kind, summary) = activities.last().expect("activities");
        assert_eq!(*kind, ActivityKind::AgentMessage);
        assert_eq!(summary, "structured answer");
    }

    #[test]
    fn usage_delta_subtracts_cumulative_totals() {
        let before = Usage {
            input_tokens: 100,
            cached_input_tokens: 40,
            output_tokens: 10,
        };
        let after = Usage {
            input_tokens: 250,
            cached_input_tokens: 90,
            output_tokens: 13,
        };
        assert_eq!(
            usage_delta(before, after),
            Usage {
                input_tokens: 150,
                cached_input_tokens: 50,
                output_tokens: 3
            }
        );
        assert_eq!(usage_delta(after, before), Usage::default());
    }

    #[test]
    fn unknown_events_and_items_are_ignored() {
        let mut stream = CodexStream::default();
        assert!(stream.line(r#"{"type":"turn.started"}"#).is_none());
        assert!(
            stream
                .line(
                    &json!({"type":"item.completed","item":{"type":"todo_list","items":[]}})
                        .to_string()
                )
                .is_none()
        );
        assert!(stream.line("not json").is_none());
        assert_eq!(stream.unparsed_lines(), 1);
    }
}
