//! Offline fixture JSONL decoding; no process execution or provider authentication.
use crate::provider::ProviderError;
use contracts::job::{event::ActivityKind, result::Usage};
use serde_json::Value;

#[derive(Default)]
pub(super) struct Recording {
    thread: Option<String>,
    output: Option<Value>,
    usage: Option<Usage>,
}

pub(super) struct Finished {
    pub output: Value,
    pub thread_id: Option<String>,
    pub cumulative_usage: Usage,
}

impl Recording {
    pub fn line(&mut self, line: &str) -> Option<(ActivityKind, String)> {
        let row: Value = serde_json::from_str(line).ok()?;
        match row["type"].as_str()? {
            "thread.started" => self.thread = row["thread_id"].as_str().map(str::to_owned),
            "turn.completed" => {
                self.usage = Some(Usage {
                    input_tokens: row["usage"]["input_tokens"].as_u64().unwrap_or(0),
                    cached_input_tokens: row["usage"]["cached_input_tokens"].as_u64().unwrap_or(0),
                    output_tokens: row["usage"]["output_tokens"].as_u64().unwrap_or(0),
                    requests: None,
                })
            }
            "item.completed" => {
                let item = &row["item"];
                let text = |key: &str| item[key].as_str().unwrap_or("").to_owned();
                match item["type"].as_str()? {
                    "agent_message" => {
                        let message = text("text");
                        self.output = serde_json::from_str(&message).ok();
                        let summary = if message.trim_start().starts_with(['{', '[']) {
                            "structured answer".into()
                        } else {
                            message
                        };
                        return Some((ActivityKind::AgentMessage, summary));
                    }
                    "reasoning" => return Some((ActivityKind::Reasoning, text("text"))),
                    "command_execution" => {
                        return Some((
                            ActivityKind::Command,
                            format!("$ {} (exit {})", text("command"), item["exit_code"]),
                        ));
                    }
                    "mcp_tool_call" => {
                        return Some((
                            ActivityKind::McpToolCall,
                            format!("{}.{}", text("server"), text("tool")),
                        ));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        None
    }
    pub fn cumulative_usage(&self) -> Option<Usage> {
        self.usage
    }
    pub fn finish(self) -> Result<Finished, ProviderError> {
        Ok(Finished {
            output: self.output.ok_or(ProviderError::OutputNotJson)?,
            thread_id: self.thread,
            cumulative_usage: self.usage.ok_or_else(|| ProviderError::Failed {
                message: "incomplete fixture recording".into(),
            })?,
        })
    }
}

pub(super) fn usage_delta(before: Usage, after: Usage) -> Usage {
    Usage {
        input_tokens: after.input_tokens.saturating_sub(before.input_tokens),
        cached_input_tokens: after
            .cached_input_tokens
            .saturating_sub(before.cached_input_tokens),
        output_tokens: after.output_tokens.saturating_sub(before.output_tokens),
        requests: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_message_reports_liveness_before_the_final_answer() {
        let mut recording = Recording::default();
        let line = r#"{"type":"item.completed","item":{"type":"agent_message","text":"Reading the next flow."}}"#;
        assert_eq!(
            recording.line(line),
            Some((ActivityKind::AgentMessage, "Reading the next flow.".into()))
        );
    }

    #[test]
    fn structured_answer_activity_does_not_expose_the_output_payload() {
        let mut recording = Recording::default();
        let line =
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"{\"answer\":42}"}}"#;
        assert_eq!(
            recording.line(line),
            Some((ActivityKind::AgentMessage, "structured answer".into()))
        );
    }
}
