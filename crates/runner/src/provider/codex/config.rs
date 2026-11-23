//! The generated per-job `config.toml` (S1a lean config, ADR-002, ADR-005, S5, S6, threat model T2, T3c, T5,
//! T6): sandbox off (T1–T4 compensate) except for answer-only jobs, hosted web search off (it would bypass the
//! egress proxy), no project docs, hooks, plugins, analytics, sub-agents or bundled skills; the project is never
//! trusted. The job turn gets the MCP servers its access allows: the official
//! `github-mcp-server` read-only with the job's installation token in that server's environment only, and
//! MemCan with its bearer in a static header (never in Codex's environment).

use std::path::Path;

use contracts::job::access::{AgentAccess, InstallationToken, McpHttpEndpoint};
use contracts::job::spec::{ModelChoice, ReasoningEffort};

/// Tools of `github-mcp-server` the agent is offered (S5): reads of pull requests, issues, code and commits.
pub(crate) const GITHUB_TOOLS: [&str; 10] = [
    "pull_request_read",
    "issue_read",
    "list_issues",
    "list_pull_requests",
    "search_issues",
    "search_pull_requests",
    "search_code",
    "get_file_contents",
    "list_commits",
    "get_commit",
];
/// MemCan search tools: all an Ask job's model is offered (ADR-004).
pub(crate) const MEMCAN_READ_TOOLS: [&str; 4] = [
    "search",
    "search_memories",
    "search_code",
    "search_standards",
];
/// MemCan tools offered to analytic jobs: search and `add_memory` (R6a; defence in depth only, AR-1).
pub(crate) const MEMCAN_TOOLS: [&str; 5] = [
    "search",
    "search_memories",
    "search_code",
    "search_standards",
    "add_memory",
];
/// Variables Codex forwards from its own environment to `github-mcp-server` (S6 finding 10).
const GITHUB_PROXY_VARS: [&str; 4] = ["HTTPS_PROXY", "https_proxy", "NO_PROXY", "no_proxy"];

/// Context size at which Codex compacts a session by itself (ADR-004).
pub(crate) const AUTO_COMPACT_TOKEN_LIMIT: u32 = 400_000;
/// Limit of the first turn of an Ask working session forked from the analytic one: Codex compacts the fork before
/// that turn (pre-turn compaction, S1b finding 3) and the turn continues on the compacted context; above the
/// compacted size plus one turn, so the turn does not compact again (ADR-004 addendum 2026-11-18). Unproven on
/// the deployed Codex with a ~300k-token fork: measure compaction time and per-request input on the real stack.
pub(crate) const FORK_COMPACT_TOKEN_LIMIT: u32 =
    contracts::runtime::FIRST_ASK_COMPACTION_TOKENS as u32;

/// Settings of an answer-only job (dashboard ranking), checked against the pinned Codex 0.171: `shell_tool`
/// off removes `exec_command` and `write_stdin` (its `unified_exec` opt-out is ignored but kept should a later
/// version honour it), `view_image` off; with a read-only sandbox for what remains (`apply_patch`, which no key
/// removes). The code-mode `exec`
/// tool stays: a V8 isolate without network or file system. A tool item still fails the job (`CodexStream`).
const ANSWER_ONLY_FEATURES: &str = "shell_tool = false\nunified_exec = false\nview_image = false\n";

/// The per-job configuration; `github_mcp` is the GitHub MCP binary, `no_proxy` the bypass hosts,
/// `compact_limit` the context size at which Codex compacts and `answer_only` drops every tool that reaches
/// the network or runs commands.
pub(crate) fn config_toml(
    model: &ModelChoice,
    access: &AgentAccess,
    github_mcp: &Path,
    no_proxy: &str,
    compact_limit: u32,
    answer_only: bool,
) -> String {
    let effort = match model.reasoning_effort {
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
    };
    let (sandbox, extra_features) = if answer_only {
        ("read-only", ANSWER_ONLY_FEATURES)
    } else {
        ("danger-full-access", "")
    };
    let mut config = format!(
        r#"model = {model}
model_reasoning_effort = "{effort}"
model_auto_compact_token_limit = {compact_limit}
sandbox_mode = "{sandbox}"
web_search = "disabled"
approval_policy = "never"
project_doc_max_bytes = 0

[analytics]
enabled = false

[skills.bundled]
enabled = false

[agents]
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
multi_agent_v2 = false
{extra_features}"#,
        model = toml_string(&model.model),
    );
    if let Some(token) = &access.github {
        config.push_str(&github_server(token, github_mcp));
    }
    if let Some(memcan) = &access.memcan {
        match memcan_url(&memcan.url, no_proxy) {
            Some(url) => config.push_str(&memcan_server(url, memcan)),
            None => tracing::error!(
                "the MemCan URL is not an http(s) URL outside NO_PROXY; the job runs without MemCan; check SR_MEMCAN_URL"
            ),
        }
    }
    config
}

/// `github-mcp-server` read-only with a server-side tool allow-list and the same list in Codex (S5); the token
/// only in its own environment (T2, T3c).
fn github_server(token: &InstallationToken, binary: &Path) -> String {
    let tools = GITHUB_TOOLS.join(",");
    format!(
        r#"
[mcp_servers.github]
command = {command}
args = ["stdio", "--read-only", {tools_arg}]
env = {{ GITHUB_PERSONAL_ACCESS_TOKEN = {token} }}
env_vars = {env_vars}
enabled_tools = {enabled}
startup_timeout_sec = 20
tool_timeout_sec = 60
required = true
"#,
        command = toml_string(&binary.to_string_lossy()),
        tools_arg = toml_string(&format!("--tools={tools}")),
        token = toml_string(token.token.expose()),
        env_vars = toml_array(&GITHUB_PROXY_VARS),
        enabled = toml_array(&GITHUB_TOOLS),
    )
}

/// MemCan over streamable HTTP through the egress proxy, bearer in a static header (T2, T6). Not required:
/// an unreachable MemCan leaves the agent without memories instead of failing the job.
fn memcan_server(url: &str, memcan: &McpHttpEndpoint) -> String {
    format!(
        r#"
[mcp_servers.memcan]
url = {url}
http_headers = {{ Authorization = {bearer} }}
enabled_tools = {enabled}
startup_timeout_sec = 20
tool_timeout_sec = 60
required = false
"#,
        url = toml_string(url),
        bearer = toml_string(&format!("Bearer {}", memcan.token.expose())),
        enabled = if memcan.allow_write {
            toml_array(&MEMCAN_TOOLS)
        } else {
            toml_array(&MEMCAN_READ_TOOLS)
        },
    )
}

/// `url` if it is http(s) and its host is not reached around the proxy (`NO_PROXY` never holds MemCan, T2).
fn memcan_url<'a>(url: &'a str, no_proxy: &str) -> Option<&'a str> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next()?;
    let host = match host_port.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next()?,
        None => host_port.split(':').next()?,
    };
    if host.is_empty()
        || no_proxy
            .split(',')
            .any(|entry| entry.trim().eq_ignore_ascii_case(host))
    {
        return None;
    }
    Some(url)
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

fn toml_array(values: &[&str]) -> String {
    let items: Vec<String> = values.iter().map(|value| toml_string(value)).collect();
    format!("[{}]", items.join(", "))
}

#[cfg(test)]
mod tests {
    use contracts::common::RepoRef;
    use contracts::job::access::Secret;
    use contracts::time::Timestamp;

    use super::*;

    const NO_PROXY: &str = "localhost,127.0.0.1";
    const GITHUB_CANARY: &str = "ghs_CONFIG_CANARY";
    const MEMCAN_CANARY: &str = "memcan-CONFIG-CANARY";

    fn model() -> ModelChoice {
        ModelChoice {
            model: "gpt-6.1-sol".to_owned(),
            reasoning_effort: ReasoningEffort::High,
        }
    }

    fn access(memcan_url: &str) -> AgentAccess {
        access_with(memcan_url, true)
    }

    fn access_with(memcan_url: &str, allow_write: bool) -> AgentAccess {
        AgentAccess {
            github: Some(InstallationToken {
                token: Secret::new(GITHUB_CANARY),
                expires_at: Timestamp::now(),
                repository: RepoRef {
                    owner: "acme".to_owned(),
                    name: "node-client".to_owned(),
                },
            }),
            memcan: Some(McpHttpEndpoint {
                url: memcan_url.to_owned(),
                token: Secret::new(MEMCAN_CANARY),
                allow_write,
            }),
        }
    }

    fn job_config(access: &AgentAccess) -> String {
        config_toml(
            &model(),
            access,
            Path::new("/opt/github-mcp/github-mcp-server"),
            NO_PROXY,
            AUTO_COMPACT_TOKEN_LIMIT,
            false,
        )
    }

    #[test]
    fn the_compaction_limit_is_the_given_one() {
        let access = AgentAccess::default();
        assert!(job_config(&access).contains("model_auto_compact_token_limit = 400000\n"));
        let fork = config_toml(
            &model(),
            &access,
            Path::new("/x"),
            NO_PROXY,
            FORK_COMPACT_TOKEN_LIMIT,
            false,
        );
        assert!(fork.contains("model_auto_compact_token_limit = 80000\n"));
    }

    /// The MCP part of the config: everything from the first `[mcp_servers.` table.
    fn mcp_part(config: &str) -> &str {
        config
            .find("\n[mcp_servers.")
            .map_or("", |start| &config[start..])
    }

    #[test]
    fn generated_config_has_only_github_read_only_and_memcan() {
        let config = job_config(&access("http://agentic.lan:8190/mcp"));
        let expected = r#"
[mcp_servers.github]
command = "/opt/github-mcp/github-mcp-server"
args = ["stdio", "--read-only", "--tools=pull_request_read,issue_read,list_issues,list_pull_requests,search_issues,search_pull_requests,search_code,get_file_contents,list_commits,get_commit"]
env = { GITHUB_PERSONAL_ACCESS_TOKEN = "ghs_CONFIG_CANARY" }
env_vars = ["HTTPS_PROXY", "https_proxy", "NO_PROXY", "no_proxy"]
enabled_tools = ["pull_request_read", "issue_read", "list_issues", "list_pull_requests", "search_issues", "search_pull_requests", "search_code", "get_file_contents", "list_commits", "get_commit"]
startup_timeout_sec = 20
tool_timeout_sec = 60
required = true

[mcp_servers.memcan]
url = "http://agentic.lan:8190/mcp"
http_headers = { Authorization = "Bearer memcan-CONFIG-CANARY" }
enabled_tools = ["search", "search_memories", "search_code", "search_standards", "add_memory"]
startup_timeout_sec = 20
tool_timeout_sec = 60
required = false
"#;
        assert_eq!(mcp_part(&config), expected);
        assert_eq!(config.matches("[mcp_servers.").count(), 2);
    }

    #[test]
    fn read_only_memcan_access_offers_no_write_tool() {
        let config = job_config(&access_with("http://agentic.lan:8190/mcp", false));
        let memcan = &mcp_part(&config)[mcp_part(&config).find("[mcp_servers.memcan]").unwrap()..];
        assert!(
            memcan.contains(
                r#"enabled_tools = ["search", "search_memories", "search_code", "search_standards"]"#
            ),
            "{memcan}"
        );
        assert!(!memcan.contains("add_memory"), "{memcan}");
    }

    #[test]
    fn generated_config_lean() {
        let config = job_config(&AgentAccess::default());
        for line in [
            "model = \"gpt-6.1-sol\"",
            "model_reasoning_effort = \"high\"",
            "sandbox_mode = \"danger-full-access\"",
            "approval_policy = \"never\"",
            "project_doc_max_bytes = 0",
            "hooks = false",
            "plugins = false",
        ] {
            assert!(config.lines().any(|l| l == line), "missing {line}");
        }
        assert!(config.contains("[skills.bundled]\nenabled = false"));
        assert!(config.contains("[agents]\nenabled = false"));
        assert!(
            !config.contains("trust_level"),
            "the project is never trusted"
        );
        assert!(!config.contains("[projects"));
    }

    #[test]
    fn answer_only_config_offers_no_command_web_or_image_tool() {
        let config = config_toml(
            &model(),
            &AgentAccess::default(),
            Path::new("/x"),
            NO_PROXY,
            AUTO_COMPACT_TOKEN_LIMIT,
            true,
        );
        for line in [
            "sandbox_mode = \"read-only\"",
            "web_search = \"disabled\"",
            "approval_policy = \"never\"",
            "shell_tool = false",
            "unified_exec = false",
            "view_image = false",
        ] {
            assert!(
                config.lines().any(|l| l == line),
                "missing {line}:\n{config}"
            );
        }
        assert!(!config.contains("danger-full-access"), "{config}");
        assert!(!config.contains("mcp_servers"), "{config}");
        let features = config.find("\n[features]\n").expect("a features table");
        let web_search = config.find("\nweb_search").expect("web_search");
        assert!(
            web_search < config.find("\n[").unwrap_or(0),
            "web_search is a top-level key:\n{config}"
        );
        let after_features = &config[features..];
        let next_table = after_features[1..]
            .find("\n[")
            .map_or(after_features.len(), |i| i + 1);
        for key in [
            "shell_tool = false",
            "unified_exec = false",
            "view_image = false",
        ] {
            assert!(
                after_features[..next_table].lines().any(|l| l == key),
                "{key} in [features]:\n{config}"
            );
        }
    }

    #[test]
    fn analysis_config_keeps_its_tools() {
        let config = job_config(&AgentAccess::default());
        for absent in ["shell_tool", "unified_exec", "view_image", "read-only"] {
            assert!(!config.contains(absent), "{absent}:\n{config}");
        }
    }

    /// Hosted web search runs at the provider, around the egress proxy and its log: off for every job kind.
    #[test]
    fn every_job_kind_config_disables_hosted_web_search() {
        use contracts::agent::JobKind;
        for kind in JobKind::ALL {
            let answer_only = kind == JobKind::JudgeWork;
            let access = match kind {
                JobKind::JudgeWork => AgentAccess::default(),
                JobKind::VerifyComments => AgentAccess {
                    memcan: None,
                    ..access("http://agentic.lan:8190/mcp")
                },
                JobKind::Ask => access_with("http://agentic.lan:8190/mcp", false),
                JobKind::Plan | JobKind::PrContext | JobKind::AnalyseFlow => {
                    access("http://agentic.lan:8190/mcp")
                }
            };
            for limit in [AUTO_COMPACT_TOKEN_LIMIT, FORK_COMPACT_TOKEN_LIMIT] {
                let config = config_toml(
                    &model(),
                    &access,
                    Path::new("/x"),
                    NO_PROXY,
                    limit,
                    answer_only,
                );
                let top_level = &config[..config.find("\n[").unwrap_or(config.len())];
                assert_eq!(
                    top_level
                        .lines()
                        .filter(|l| l.starts_with("web_search"))
                        .collect::<Vec<_>>(),
                    ["web_search = \"disabled\""],
                    "{kind:?}:\n{config}"
                );
            }
        }
    }

    /// Sub-agents off (ADR-002): `[agents] enabled = false` also wins over a model whose catalog entry selects
    /// sub-agents, unless `features.multi_agent_v2` is on, so that is pinned off; the deprecated
    /// `features.multi_agent` does neither, and Codex 0.168+ warns about it on every job.
    #[test]
    fn every_job_config_disables_sub_agents() {
        for answer_only in [false, true] {
            let config = config_toml(
                &model(),
                &access("http://agentic.lan:8190/mcp"),
                Path::new("/x"),
                NO_PROXY,
                AUTO_COMPACT_TOKEN_LIMIT,
                answer_only,
            );
            assert!(config.contains("\n[agents]\nenabled = false\n"), "{config}");
            let features = &config[config.find("\n[features]\n").expect("a features table")..];
            let features = &features[..features[1..].find("\n[").map_or(features.len(), |i| i + 1)];
            assert!(
                features.lines().any(|l| l == "multi_agent_v2 = false"),
                "{config}"
            );
            assert!(
                !config.lines().any(|l| l.starts_with("multi_agent =")),
                "{config}"
            );
        }
    }

    #[test]
    fn uninstalled_repo_job_has_no_github_access() {
        let mut access = access("http://agentic.lan:8190/mcp");
        access.github = None;
        let config = job_config(&access);
        assert!(!config.contains("[mcp_servers.github]"));
        assert!(!config.contains("github-mcp-server"));
        assert!(config.contains("[mcp_servers.memcan]"));
    }

    #[test]
    fn no_access_means_no_mcp_server() {
        assert!(!job_config(&AgentAccess::default()).contains("mcp_servers"));
    }

    /// The generated configs as golden files `golden/config-*.toml` (fake credentials), the inputs of the offline
    /// checks of a Codex upgrade (runbook "Codex CLI upgrade"). `UPDATE_GOLDEN=1` rewrites them.
    #[test]
    fn generated_configs_match_the_golden_files() {
        let github = Path::new("/usr/local/bin/github-mcp-server");
        for (name, access, answer_only) in [
            (
                "analysis-mcp",
                access("http://memcan.invalid:8190/mcp"),
                false,
            ),
            ("analysis", AgentAccess::default(), false),
            ("answer-only", AgentAccess::default(), true),
        ] {
            let config = config_toml(
                &model(),
                &access,
                github,
                NO_PROXY,
                AUTO_COMPACT_TOKEN_LIMIT,
                answer_only,
            );
            let golden = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("src/provider/codex/golden/config-{name}.toml"));
            if std::env::var_os("UPDATE_GOLDEN").is_some() {
                std::fs::write(&golden, &config).unwrap();
            }
            assert_eq!(
                config,
                std::fs::read_to_string(&golden).expect("golden file; run with UPDATE_GOLDEN=1"),
                "{name} differs from its golden file (UPDATE_GOLDEN=1)"
            );
        }
    }

    #[test]
    fn memcan_not_in_no_proxy() {
        for url in [
            "http://localhost:8190/mcp",
            "http://127.0.0.1:8190/mcp",
            "http://user@LOCALHOST/mcp",
            "file:///etc/passwd",
            "http:///mcp",
        ] {
            let config = job_config(&access(url));
            assert!(!config.contains("[mcp_servers.memcan]"), "{url}");
            assert!(!config.contains(MEMCAN_CANARY), "{url}");
        }
        assert!(job_config(&access("https://[::1]:8190/mcp")).contains("[mcp_servers.memcan]"));
    }

    #[test]
    fn values_are_toml_escaped() {
        let mut access = access("http://agentic.lan:8190/mcp\"\n[mcp_servers.evil]");
        access.github = None;
        let config = job_config(&access);
        assert!(!config.contains("\n[mcp_servers.evil]"), "{config}");
    }

    #[test]
    fn toml_string_escapes_quotes_backslashes_and_controls() {
        assert_eq!(toml_string("a\"b\\c\nd"), r#""a\"b\\c\u000Ad""#);
    }
}
