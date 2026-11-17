//! The generated per-job `config.toml` (S1a lean config, ADR-002, threat model T3c, T5, T6): sandbox off (T1–T4
//! compensate), no project docs, hooks, plugins, analytics, sub-agents or bundled skills; the project is never
//! trusted.

use contracts::job::spec::{ModelChoice, ReasoningEffort};

/// What the configuration is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    /// The job's model turn.
    Job,
    /// The compaction turn after it.
    Compaction,
}

/// The per-job configuration for `purpose`.
pub(crate) fn config_toml(model: &ModelChoice, _purpose: Purpose) -> String {
    let effort = match model.reasoning_effort {
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
    };
    format!(
        r#"model = {model}
model_reasoning_effort = "{effort}"
sandbox_mode = "danger-full-access"
approval_policy = "never"
project_doc_max_bytes = 0

[analytics]
enabled = false

[skills.bundled]
enabled = false

[features]
plugins = false
apps = false
remote_plugin = false
plugin_discovery_prefetch = false
image_generation = false
browser_use = false
computer_use = false
hooks = false
tool_suggest = false
goals = false
daemon_auto_start = false
multi_agent = false
"#,
        model = toml_string(&model.model),
    )
}

/// A TOML basic string.
pub(crate) fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ch if ch.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(ch))),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}
