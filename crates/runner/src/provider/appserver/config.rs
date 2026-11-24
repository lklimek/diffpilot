/// Fixed MemCan tools for private Ask threads.
const MEMCAN_READ_TOOLS: [&str; 4] = [
    "search",
    "search_memories",
    "search_code",
    "search_standards",
];
/// Analytic threads may also record durable findings.
const MEMCAN_TOOLS: [&str; 5] = [
    "search",
    "search_memories",
    "search_code",
    "search_standards",
    "add_memory",
];
use super::*;
use contracts::job::spec::{ModelChoice, WorkingSession};
use serde_json::json;

pub(super) fn thread_params(
    kind: ThreadKind,
    model: &ModelChoice,
    window: u64,
    memcan: Option<&contracts::job::access::McpHttpEndpoint>,
) -> Value {
    let judge = kind == ThreadKind::Judge;
    let mut config = json!({
        "model_auto_compact_token_limit": 400_000.min(window.saturating_mul(9) / 10),
        "web_search": "disabled", "project_doc_max_bytes": 0,
        "skills.include_instructions": false, "features.hooks": false,
        "features.multi_agent_v2": false, "agents.enabled": false,
        "mcp_servers": {},
        "model_context_window": window,
        "projects./work.trust_level": "untrusted",
        "projects./work/repo.trust_level": "untrusted"
    });
    if let Some(endpoint) = memcan {
        config["mcp_servers"] = json!({"memcan": {"enabled":kind.memcan_read(),"url":endpoint.url,"http_headers":{"Authorization":format!("Bearer {}",endpoint.token.expose())}}});
    }
    if let Some(endpoint) = memcan {
        config["mcp_servers"]["memcan"]["enabled_tools"] =
            if kind.memcan_write() && endpoint.allow_write {
                json!(MEMCAN_TOOLS)
            } else {
                json!(MEMCAN_READ_TOOLS)
            };
    }
    if judge {
        config["features.shell_tool"] = json!(false);
        config["features.unified_exec"] = json!(false);
        config["features.view_image"] = json!(false);
    }
    json!({"model": model.model, "modelProvider": "diffpilot", "config": config,
        "cwd": if judge { "/tmp" } else { "/work" }, "approvalPolicy": "never",
        "sandbox": if judge { "read-only" } else { "danger-full-access" }})
}

pub(super) fn working_kind(
    working: &contracts::job::spec::WorkingSession,
) -> Result<ThreadKind, ProviderError> {
    match working.kind {
        Some(kind @ (ThreadKind::Ask | ThreadKind::Verify)) => Ok(kind),
        None if working.thread_id.is_none() => Ok(ThreadKind::Ask),
        _ => Err(fault(
            JobFailureKind::Configuration,
            "thread/config",
            "unknown working thread kind",
        )),
    }
}

pub(super) fn needs_compaction(
    working: &WorkingSession,
    seed: SessionSeed,
) -> Result<bool, ProviderError> {
    if let Some(preflight) = &working.preflight {
        if preflight.generation != working.analytic_generation || preflight.seed != seed {
            return Err(fault(
                JobFailureKind::Configuration,
                "thread/config",
                "preflight seed or generation mismatch",
            ));
        }
        if matches!(
            preflight.state,
            PreflightState::Done | PreflightState::NotNeeded
        ) {
            return Ok(false);
        }
        if seed == SessionSeed::Fork {
            return Ok(true);
        }
    }
    if seed == SessionSeed::ContextPack {
        return Ok(false);
    }
    let context = working
        .parent_context
        .as_ref()
        .filter(|c| c.generation == working.analytic_generation)
        .ok_or_else(|| {
            fault(
                JobFailureKind::Configuration,
                "thread/config",
                "missing or stale parent context",
            )
        })?;
    Ok(context.tokens > 80_000)
}

pub(super) fn model_window(model: &str) -> Result<u64, ProviderError> {
    match model {
        "gpt-6.2-sol" | "gpt-6.1-sol" | "gpt-6.1-terra" => Ok(500_000),
        "gpt-6.2-astra" | "gpt-6-sol" | "gpt-5.6-sol" | "gpt-6-astra" => Ok(272_000),
        _ => Err(fault(
            JobFailureKind::ModelUnavailable,
            "thread/config",
            "unknown model context window",
        )),
    }
}
