//! Codex provider against a stand-in `codex` script that records its argv, environment and stdin and replays
//! a recorded JSONL stream (`fixtures/pr-482/codex/`). Real Codex is never run here.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use chrono::Utc;
use contracts::common::RepoRef;
use contracts::ids::JobId;
use contracts::job::access::{AgentAccess, InstallationToken, McpHttpEndpoint, Secret};
use contracts::job::spec::{JobLimits, ModelChoice, ReasoningEffort};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::mpsc;

use super::*;

const PROXY: &str = "http://egress-proxy:3128";
const GITHUB_CANARY: &str = "ghs_CANARYinstallationTOKEN0001";
const MEMCAN_CANARY: &str = "memcan-CANARY-bearer-0002";
const PROMPT_CANARY: &str = "PROMPT-CANARY-0003";
const ACCESS_CANARY: &str = "codex-access-CANARY-0004";
const REFRESH_CANARY: &str = "codex-refresh-CANARY-0005";
const RUNNER_SECRET_CANARY: &str = "runner-secret-CANARY-0006";
/// The thread of the recorded fixture session.
const THREAD: &str = "019a8f2c-4820-7000-8000-000000000482";

/// The rollout path of `thread` below `sessions/`.
fn rollout_rel(thread: &str) -> String {
    format!("2026/11/16/rollout-2026-11-16T10-00-00-{thread}.jsonl")
}

/// The stand-in `codex`: writes argv, env and stdin into the job dir, replays `scenario.jsonl` and exits with
/// the code in `scenario-exit`. Written once per process, so no test executes a file another thread still
/// has open for writing (ETXTBSY).
fn fake_codex() -> &'static Path {
    static SCRIPT: OnceLock<PathBuf> = OnceLock::new();
    SCRIPT.get_or_init(|| {
        let dir = TempDir::new().expect("script dir").keep();
        let path = dir.join("codex");
        fs::write(
            &path,
            r#"#!/bin/sh
mode=child
echo call >> "$PWD/invocations.txt"
env > "$PWD/$mode-env.txt"
for arg in "$@"; do printf '%s\n' "$arg"; done > "$PWD/$mode-args.txt"
cat > "$PWD/$mode-stdin.txt"
if [ -f "$PWD/check-skills" ]; then
  cat "$HOME/.agents/skills/severity/SKILL.md" > "$PWD/$mode-skill.txt" || exit 42
  id -u > "$PWD/$mode-uid.txt"
fi
cp "$CODEX_HOME/config.toml" "$PWD/$mode-config.toml" 2>/dev/null
find "$CODEX_HOME/sessions" -type f > "$PWD/$mode-sessions.txt" 2>/dev/null
if [ -f "$PWD/rollout-rel" ]; then
  rollout="$CODEX_HOME/sessions/$(cat "$PWD/rollout-rel")"
  mkdir -p "$(dirname "$rollout")"
  if [ -f "$PWD/$mode-rollout" ]; then cat "$PWD/$mode-rollout" >> "$rollout"; else echo '{"type":"turn"}' >> "$rollout"; fi
fi
if [ -f "$PWD/ignore-term" ]; then trap '' TERM; fi
if [ -f "$PWD/tamper" ]; then printf '\n[mcp_servers.evil]\ncommand = "/bin/evil"\n' >> "$CODEX_HOME/config.toml"; fi
if [ -f "$PWD/rotate" ]; then cat "$PWD/rotate" > "$CODEX_HOME/auth.json"; fi
if [ -f "$PWD/stderr" ]; then cat "$PWD/stderr" >&2; fi
if [ -f "$PWD/chmod-auth" ]; then chmod 000 "$CODEX_HOME/auth.json"; fi
if [ -f "$PWD/lock-home" ]; then mkdir -p "$HOME/locked/deeper"; echo left > "$HOME/locked/deeper/x"; chmod 000 "$HOME/locked/deeper" "$HOME/locked"; fi
if [ -f "$PWD/symlink-auth" ]; then rm -f "$CODEX_HOME/auth.json"; ln -s "$(cat "$PWD/symlink-auth")" "$CODEX_HOME/auth.json"; fi
if [ -f "$PWD/background" ]; then sleep 300 </dev/null >/dev/null 2>&1 & echo $! > "$PWD/background.pid"; fi
cat "$PWD/scenario.jsonl"
if [ -f "$PWD/hang" ]; then sleep 300; fi
exit "$(cat "$PWD/scenario-exit")"
"#,
        )
        .expect("write script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    })
}

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/pr-482")
        .join(rel)
}

struct Harness {
    _dir: TempDir,
    provider: CodexProvider,
    call: ProviderCall,
    master_auth: PathBuf,
}

fn harness(recording: &str, exit_code: i32) -> Harness {
    let dir = TempDir::new().expect("temp dir");
    let job_dir = dir.path().join("job");
    let codex_home = dir.path().join("codex-home");
    fs::create_dir(&job_dir).unwrap();
    fs::create_dir(&codex_home).unwrap();
    fs::create_dir(job_dir.join("repo")).unwrap();
    fs::copy(
        fixture(&format!("codex/{recording}")),
        job_dir.join("scenario.jsonl"),
    )
    .unwrap();
    fs::write(job_dir.join("scenario-exit"), exit_code.to_string()).unwrap();
    fs::write(job_dir.join("rollout-rel"), rollout_rel(THREAD)).unwrap();
    let master_auth = dir.path().join("credentials").join("auth.json");
    fs::create_dir(master_auth.parent().unwrap()).unwrap();
    fs::write(
        &master_auth,
        json!({"auth_mode": "chatgpt", "tokens": {"access_token": ACCESS_CANARY, "refresh_token": REFRESH_CANARY, "account_id": "acct-1"}})
            .to_string(),
    )
    .unwrap();
    let call = ProviderCall {
        job_id: JobId::new_random(),
        job_dir,
        codex_home,
        session_dir: dir.path().join("sessions").join("analysis-a"),
        restore_dir: dir.path().join("sessions").join("analysis-a"),
        prompt: format!("Job: plan. {PROMPT_CANARY}"),
        output_schema: json!({"type": "object", "additionalProperties": false, "properties": {}, "required": []}),
        flow_file: None,
        model: ModelChoice {
            model: "gpt-6.1-sol".to_owned(),
            reasoning_effort: ReasoningEffort::High,
        },
        limits: JobLimits::default(),
        access: AgentAccess {
            github: Some(InstallationToken {
                token: Secret::new(GITHUB_CANARY),
                expires_at: Utc::now(),
                repository: RepoRef {
                    owner: "acme".to_owned(),
                    name: "node-client".to_owned(),
                },
            }),
            memcan: Some(McpHttpEndpoint {
                url: "http://memcan.example:8190/".to_owned(),
                token: Secret::new(MEMCAN_CANARY),
                allow_write: false,
            }),
        },
        skills: Vec::new(),
    };
    let provider = CodexProvider::new(CodexSettings {
        binary: fake_codex().to_path_buf(),
        credential: Arc::new(CodexCredential::new(master_auth.clone())),
        egress_proxy: Some(PROXY.to_owned()),
        term_grace: Duration::from_secs(10),
        github_mcp: PathBuf::from("/opt/github-mcp/github-mcp-server"),
        skills: None,
        isolation: None,
    });
    Harness {
        _dir: dir,
        provider,
        call,
        master_auth,
    }
}

async fn run(h: &Harness) -> (Result<ProviderOutcome, ProviderError>, Vec<ProviderEvent>) {
    let (tx, mut rx) = mpsc::channel(1024);
    let result = h.provider.run(&h.call, tx).await;
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    (result, events)
}

fn child_file(h: &Harness, name: &str) -> String {
    fs::read_to_string(h.call.job_dir.join(name)).expect(name)
}

fn child_env(h: &Harness) -> Vec<(String, String)> {
    child_file(h, "child-env.txt")
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

#[tokio::test]
async fn recorded_success_returns_the_structured_output() {
    let h = harness("plan.jsonl", 0);
    let (result, events) = run(&h).await;
    let outcome = result.expect("success");
    let expected: Value =
        serde_json::from_str(&fs::read_to_string(fixture("agent/plan.json")).unwrap()).unwrap();
    assert_eq!(outcome.output, expected);
    assert_eq!(
        outcome.session_id.as_deref(),
        Some("019a8f2c-4820-7000-8000-000000000482")
    );
    assert_eq!(
        outcome.usage.input_tokens, 48_210,
        "a new thread's usage is its useful turn total"
    );
    assert_eq!(
        events.last(),
        Some(&ProviderEvent::OutputReady(expected)),
        "output is announced after every activity"
    );
    assert!(events.len() > 1);
    assert_eq!(
        child_file(&h, "child-stdin.txt"),
        h.call.prompt,
        "prompt goes through stdin"
    );
}

#[tokio::test]
async fn secrets_never_in_argv() {
    let h = harness("plan.jsonl", 0);
    run(&h).await.0.expect("success");
    let args: Vec<String> = child_file(&h, "child-args.txt")
        .lines()
        .map(str::to_owned)
        .collect();
    let schema = h.call.codex_home.join("output-schema.json");
    assert_eq!(
        args,
        [
            "exec",
            "--json",
            "--skip-git-repo-check",
            "--ignore-rules",
            "--output-schema",
            schema.to_str().unwrap(),
            "-",
        ]
    );
    for canary in [
        GITHUB_CANARY,
        MEMCAN_CANARY,
        PROMPT_CANARY,
        ACCESS_CANARY,
        REFRESH_CANARY,
    ] {
        assert!(!args.iter().any(|a| a.contains(canary)), "{canary} in argv");
    }
}

#[tokio::test]
async fn codex_env_has_no_secret() {
    let h = harness("plan.jsonl", 0);
    run(&h).await.0.expect("success");
    let env = child_env(&h);
    // `sh` itself adds PWD, SHLVL and `_`; everything else must come from the constructed list.
    let names: BTreeSet<&str> = env
        .iter()
        .map(|(k, _)| k.as_str())
        .filter(|k| !["PWD", "SHLVL", "_", "OLDPWD"].contains(k))
        .collect();
    let expected: BTreeSet<&str> = [
        "HOME",
        "PATH",
        "CODEX_HOME",
        "TMPDIR",
        "LANG",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_KEY_0",
        "GIT_CONFIG_VALUE_0",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "NO_PROXY",
        "no_proxy",
    ]
    .into();
    assert_eq!(names, expected);
    for (name, value) in &env {
        for canary in [GITHUB_CANARY, MEMCAN_CANARY, ACCESS_CANARY, REFRESH_CANARY] {
            assert!(!value.contains(canary), "{name} carries a secret");
        }
    }
    let value = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    assert_eq!(value("PATH").as_deref(), Some(AGENT_PATH));
    assert_eq!(value("NO_PROXY").as_deref(), Some(AGENT_NO_PROXY));
    assert!(
        !value("no_proxy").unwrap().contains("memcan"),
        "memcan_not_in_no_proxy"
    );
    assert_eq!(value("https_proxy").as_deref(), Some(PROXY));
    assert_eq!(
        value("HOME").map(PathBuf::from),
        Some(h.call.job_dir.join("home"))
    );
    assert_eq!(
        value("TMPDIR").map(PathBuf::from),
        Some(h.call.job_dir.join("tmp"))
    );
    assert_eq!(
        value("PWD").map(PathBuf::from),
        Some(h.call.job_dir.clone()),
        "Codex runs above the checkout, so repository config is never discovered (T5)"
    );
}

/// Re-runs [`runner_secret_canary_inner`] in a child test process whose environment holds a runner-secret
/// canary, so the "runner process" really has the variable.
#[test]
fn runner_env_canary_is_absent_from_codex_env() {
    let exe = std::env::current_exe().expect("test binary");
    let output = std::process::Command::new(exe)
        .args([
            "provider::codex::tests::runner_secret_canary_inner",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("SR_RUNNER_SECRET", RUNNER_SECRET_CANARY)
        .output()
        .expect("re-run the test binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("runner secret canary checked")
            || stderr.contains("runner secret canary checked"),
        "inner test did not run: {stdout}\n{stderr}"
    );
}

#[tokio::test]
#[ignore = "run by runner_env_canary_is_absent_from_codex_env with the canary in its environment"]
async fn runner_secret_canary_inner() {
    assert_eq!(
        std::env::var("SR_RUNNER_SECRET").as_deref(),
        Ok(RUNNER_SECRET_CANARY),
        "the parent test sets the canary"
    );
    let h = harness("plan.jsonl", 0);
    run(&h).await.0.expect("success");
    let env = child_file(&h, "child-env.txt");
    assert!(!env.contains("SR_RUNNER_SECRET"), "{env}");
    assert!(!env.contains(RUNNER_SECRET_CANARY), "{env}");
    println!("runner secret canary checked");
}

#[tokio::test]
async fn schema_error_exit_1_is_schema_rejected() {
    let h = harness("schema-error.jsonl", 1);
    let (result, events) = run(&h).await;
    assert!(
        matches!(result, Err(ProviderError::SchemaRejected { .. })),
        "{result:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ProviderEvent::OutputReady(_)))
    );
}

#[tokio::test]
async fn cancelled_exit_143_without_terminal_event_is_cancelled() {
    let h = harness("cancelled.jsonl", 143);
    let (result, _) = run(&h).await;
    assert!(
        matches!(result, Err(ProviderError::Cancelled)),
        "{result:?}"
    );
}

#[tokio::test]
async fn usage_limit_text_is_usage_limit() {
    let h = harness("usage-limit.jsonl", 1);
    let (result, _) = run(&h).await;
    assert!(
        matches!(result, Err(ProviderError::UsageLimit { resets_at: None })),
        "{result:?}"
    );
}

#[tokio::test]
async fn per_job_codex_home_has_lean_config_and_private_files() {
    let h = harness("plan.jsonl", 0);
    run(&h).await.0.expect("success");
    let config = fs::read_to_string(h.call.codex_home.join("config.toml")).unwrap();
    for line in [
        "model = \"gpt-6.1-sol\"",
        "model_reasoning_effort = \"high\"",
        "project_doc_max_bytes = 0",
        "hooks = false",
        "multi_agent = false",
        "approval_policy = \"never\"",
    ] {
        assert!(config.lines().any(|l| l == line), "missing {line}");
    }
    assert!(
        !config.contains("trust_level"),
        "the project is never trusted"
    );
    for file in ["config.toml", "auth.json", "output-schema.json"] {
        let mode = fs::metadata(h.call.codex_home.join(file))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{file}");
    }
    let schema: Value = serde_json::from_str(
        &fs::read_to_string(h.call.codex_home.join("output-schema.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(schema, h.call.output_schema);
}

#[tokio::test]
async fn credential_tokens_are_scrubbed_from_events_and_output() {
    let h = harness("plan.jsonl", 0);
    let leaky = format!(
        "{}\n{}\n{}\n",
        json!({"type": "thread.started", "thread_id": "t-1"}),
        json!({"type": "item.completed", "item": {"id": "i1", "type": "command_execution",
            "command": format!("/bin/bash -lc 'echo {ACCESS_CANARY}'"), "aggregated_output": "", "exit_code": 0, "status": "completed"}}),
        json!({"type": "item.completed", "item": {"id": "i2", "type": "agent_message",
            "text": json!({"leak": REFRESH_CANARY}).to_string()}}),
    ) + &json!({"type": "turn.completed", "usage": {"input_tokens": 1, "cached_input_tokens": 0, "output_tokens": 1}})
        .to_string();
    fs::write(h.call.job_dir.join("scenario.jsonl"), leaky).unwrap();
    let (result, events) = run(&h).await;
    let outcome = result.expect("success");
    let all = format!("{events:?} {:?}", outcome.output);
    assert!(!all.contains(ACCESS_CANARY), "{all}");
    assert!(!all.contains(REFRESH_CANARY), "{all}");
    assert_eq!(outcome.output, json!({"leak": "[redacted]"}));
}

const NEW_ACCESS_CANARY: &str = "codex-access-ROTATED-CANARY-0007";
const NEW_REFRESH_CANARY: &str = "codex-refresh-ROTATED-CANARY-0008";

fn rotated_auth() -> String {
    json!({"auth_mode": "chatgpt", "tokens": {"access_token": NEW_ACCESS_CANARY, "refresh_token": NEW_REFRESH_CANARY, "account_id": "acct-1"}})
        .to_string()
}

fn usable(h: &Harness) -> bool {
    h.provider.settings.credential.usable()
}

#[tokio::test]
async fn rotated_credential_is_written_back_to_the_master() {
    let h = harness("plan.jsonl", 0);
    fs::write(h.call.job_dir.join("rotate"), rotated_auth()).unwrap();
    run(&h).await.0.expect("success");
    assert_eq!(fs::read_to_string(&h.master_auth).unwrap(), rotated_auth());
    assert!(usable(&h), "a refresh keeps the credential usable");
}

#[tokio::test]
async fn rotated_tokens_are_scrubbed_from_events_and_output() {
    let h = harness("plan.jsonl", 0);
    fs::write(h.call.job_dir.join("rotate"), rotated_auth()).unwrap();
    let leaky = [
        json!({"type": "thread.started", "thread_id": "t-1"}),
        json!({"type": "item.completed", "item": {"id": "i1", "type": "command_execution",
            "command": format!("cat auth.json # {NEW_ACCESS_CANARY}"), "aggregated_output": "", "exit_code": 0}}),
        json!({"type": "item.completed", "item": {"id": "i2", "type": "agent_message",
            "text": json!({"leak": NEW_REFRESH_CANARY}).to_string()}}),
        json!({"type": "turn.completed", "usage": {"input_tokens": 1, "cached_input_tokens": 0, "output_tokens": 1}}),
    ]
    .map(|line| line.to_string())
    .join("\n");
    fs::write(h.call.job_dir.join("scenario.jsonl"), leaky).unwrap();
    let (result, events) = run(&h).await;
    let outcome = result.expect("success");
    let all = format!("{events:?} {:?}", outcome.output);
    assert!(!all.contains(NEW_ACCESS_CANARY), "{all}");
    assert!(!all.contains(NEW_REFRESH_CANARY), "{all}");
}

#[tokio::test]
async fn secret_straddling_the_summary_limit_is_scrubbed_whole() {
    let h = harness("plan.jsonl", 0);
    let command = format!("echo {}{GITHUB_CANARY}", "x".repeat(185));
    let lines = [
        json!({"type": "item.completed", "item": {"id": "i1", "type": "command_execution", "command": command, "exit_code": 0}}),
        json!({"type": "item.completed", "item": {"id": "i2", "type": "agent_message", "text": "{}"}}),
        json!({"type": "turn.completed", "usage": {"input_tokens": 1, "cached_input_tokens": 0, "output_tokens": 1}}),
    ]
    .map(|line| line.to_string())
    .join("\n");
    fs::write(h.call.job_dir.join("scenario.jsonl"), lines).unwrap();
    let (_, events) = run(&h).await;
    let summary = match &events[0] {
        ProviderEvent::Activity(event) => event.summary.clone(),
        other => panic!("{other:?}"),
    };
    assert!(summary.ends_with("[redacted] (exit 0)"), "{summary}");
    assert!(!summary.contains(&GITHUB_CANARY[..8]), "{summary}");
}

#[tokio::test]
async fn rotated_credential_is_written_back_when_the_call_is_dropped() {
    let h = harness("plan.jsonl", 0);
    fs::write(h.call.job_dir.join("rotate"), rotated_auth()).unwrap();
    fs::write(h.call.job_dir.join("hang"), "").unwrap();
    let (tx, _rx) = mpsc::channel(1024);
    let dropped =
        tokio::time::timeout(Duration::from_millis(500), h.provider.run(&h.call, tx)).await;
    assert!(
        dropped.is_err(),
        "the hanging call is dropped like at the runtime cap"
    );
    assert_eq!(fs::read_to_string(&h.master_auth).unwrap(), rotated_auth());
    assert!(usable(&h));
}

#[tokio::test]
async fn unusable_credential_refuses_the_next_call_without_starting_codex() {
    let h = harness("plan.jsonl", 1);
    fs::write(
        h.call.job_dir.join("scenario.jsonl"),
        json!({"type": "turn.failed", "error": {"message": "refresh failed: refresh_token_reused"}}).to_string(),
    )
    .unwrap();
    let _ = run(&h).await;
    fs::remove_file(h.call.job_dir.join("child-args.txt")).unwrap();
    fs::remove_file(h.call.codex_home.join("auth.json")).unwrap();
    let (result, _) = run(&h).await;
    assert!(
        matches!(result, Err(ProviderError::CredentialUnusable)),
        "{result:?}"
    );
    assert!(
        !h.call.job_dir.join("child-args.txt").exists(),
        "Codex must not start"
    );
}

#[tokio::test]
async fn refresh_reused_marks_credential_unusable() {
    let h = harness("plan.jsonl", 1);
    fs::write(
        h.call.job_dir.join("scenario.jsonl"),
        json!({"type": "turn.failed", "error": {"message": "refresh failed: refresh_token_reused"}}).to_string(),
    )
    .unwrap();
    let (result, _) = run(&h).await;
    assert!(
        matches!(result, Err(ProviderError::CredentialUnusable)),
        "{result:?}"
    );
    assert!(!usable(&h));
}

/// Whether `pid` still runs (a zombie counts as gone).
fn alive(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit(") ")
            .next()
            .is_some_and(|rest| !rest.starts_with('Z'))
    })
}

async fn assert_gone(pid: u32) {
    for _ in 0..100 {
        if !alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("background process {pid} outlived the call");
}

fn background_pid(h: &Harness) -> u32 {
    child_file(h, "background.pid").trim().parse().expect("pid")
}

#[tokio::test]
async fn background_child_is_killed_at_the_end_of_the_call() {
    let h = harness("plan.jsonl", 0);
    fs::write(h.call.job_dir.join("background"), "").unwrap();
    run(&h).await.0.expect("success");
    assert_gone(background_pid(&h)).await;
}

#[tokio::test]
async fn background_child_is_killed_when_the_call_is_dropped() {
    let h = harness("plan.jsonl", 0);
    fs::write(h.call.job_dir.join("background"), "").unwrap();
    fs::write(h.call.job_dir.join("hang"), "").unwrap();
    let (tx, _rx) = mpsc::channel(1024);
    let dropped =
        tokio::time::timeout(Duration::from_millis(500), h.provider.run(&h.call, tx)).await;
    assert!(dropped.is_err());
    assert_gone(background_pid(&h)).await;
}

#[tokio::test]
async fn cancel_terminates_the_process_group() {
    let h = harness("cancelled.jsonl", 0);
    fs::write(h.call.job_dir.join("background"), "").unwrap();
    fs::write(h.call.job_dir.join("hang"), "").unwrap();
    let (tx, _rx) = mpsc::channel(1024);
    let run = h.provider.run(&h.call, tx);
    let cancel = async {
        while !h.call.job_dir.join("background.pid").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        h.provider.cancel(h.call.job_id).await
    };
    let (result, cancelled) =
        tokio::time::timeout(Duration::from_secs(10), async { tokio::join!(run, cancel) })
            .await
            .expect("cancel ends the call");
    cancelled.expect("cancel");
    assert!(
        matches!(result, Err(ProviderError::Cancelled)),
        "{result:?}"
    );
    assert_gone(background_pid(&h)).await;
}

#[tokio::test]
async fn missing_master_credential_is_credential_unusable() {
    let h = harness("plan.jsonl", 0);
    fs::remove_file(&h.master_auth).unwrap();
    let (result, _) = run(&h).await;
    assert!(
        matches!(result, Err(ProviderError::CredentialUnusable)),
        "{result:?}"
    );
}

#[tokio::test]
async fn missing_binary_is_an_io_error() {
    let mut h = harness("plan.jsonl", 0);
    h.provider.settings.binary = PathBuf::from("/nonexistent/codex");
    let (result, _) = run(&h).await;
    assert!(matches!(result, Err(ProviderError::Io(_))), "{result:?}");
}

#[tokio::test]
async fn oversized_line_is_skipped_and_the_next_one_read() {
    let mut input = vec![b'x'; MAX_LINE_BYTES + 1];
    input.extend_from_slice(b"\nnext\nlast");
    let mut lines = Lines::new(input.as_slice());
    assert_eq!(lines.next().await.unwrap().as_deref(), Some("next"));
    assert_eq!(lines.next().await.unwrap().as_deref(), Some("last"));
    assert_eq!(lines.next().await.unwrap(), None);
    assert_eq!(lines.skipped, 1);
}

#[tokio::test]
async fn group_is_disarmed_after_the_call_and_a_late_cancel_is_a_no_op() {
    let h = harness("plan.jsonl", 0);
    run(&h).await.0.expect("success");
    assert!(
        h.provider.running.lock().unwrap().is_empty(),
        "no group id stays registered after the leader is reaped"
    );
    h.provider
        .cancel(h.call.job_id)
        .await
        .expect("cancel after the end does nothing");
}

#[tokio::test]
async fn kill_and_disarm_unregisters_before_the_leader_is_reaped() {
    let running = Running::default();
    let job = JobId::new_random();
    let mut child = tokio::process::Command::new("sleep")
        .arg("30")
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .expect("spawn sleep");
    let mut group = ProcessGroup::register(&running, job, child.id());
    assert!(running.lock().unwrap()[&job].group.is_some());
    assert!(!group.leader_exited());
    group.kill_and_disarm();
    assert!(
        group.pid.is_none(),
        "disarmed: a later drop signals nothing"
    );
    assert!(running.lock().unwrap()[&job].group.is_none());
    let status = child.wait().await.expect("reap");
    assert!(!status.success(), "the group was killed");
    drop(group);
}

#[tokio::test]
async fn unreadable_copy_ends_the_call_without_agent_text() {
    let h = harness("plan.jsonl", 1);
    // The agent rotates the copy, then replaces it with a symlink: its new tokens stay unknown to the scrubber.
    let elsewhere = h.call.job_dir.join("elsewhere.json");
    fs::write(&elsewhere, rotated_auth()).unwrap();
    fs::write(
        h.call.job_dir.join("scenario.jsonl"),
        json!({"type": "turn.failed", "error": {"message": format!("boom {NEW_REFRESH_CANARY}")}})
            .to_string(),
    )
    .unwrap();
    fs::write(
        h.call.job_dir.join("symlink-auth"),
        elsewhere.to_str().unwrap(),
    )
    .unwrap();
    let (result, events) = run(&h).await;
    let all = format!("{result:?} {events:?}");
    assert!(!all.contains(NEW_REFRESH_CANARY), "{all}");
    assert!(
        matches!(result, Err(ProviderError::CredentialUnusable)),
        "an uncertain settle ends the call with the fixed error: {result:?}"
    );
    assert!(!usable(&h));
}

/// A token_count rollout record with the thread's cumulative input tokens and one rate-limit window.
fn token_count_line(input: u64, used_percent: f64, resets_at: i64) -> String {
    json!({"type": "event_msg", "payload": {"type": "token_count",
        "info": {"total_token_usage": {"input_tokens": input, "cached_input_tokens": 0, "output_tokens": 1}},
        "rate_limits": {"primary": {"used_percent": used_percent, "window_minutes": 300, "resets_at": resets_at}}}})
    .to_string()
}

/// Puts a rollout of the fixture thread into the analysis's store, as an earlier job left it.
fn store_session(h: &Harness, cumulative_input: u64) -> PathBuf {
    let path = h.call.session_dir.join(rollout_rel(THREAD));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        format!(
            "{{\"type\":\"session_meta\"}}\n{}\n",
            token_count_line(cumulative_input, 10.0, 0)
        ),
    )
    .unwrap();
    path
}

async fn resume(h: &Harness) -> (Result<ProviderOutcome, ProviderError>, Vec<ProviderEvent>) {
    let (tx, mut rx) = mpsc::channel(1024);
    let result = h.provider.resume(THREAD, &h.call, tx).await;
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    (result, events)
}

fn lines(h: &Harness, name: &str) -> Vec<String> {
    child_file(h, name).lines().map(str::to_owned).collect()
}

#[tokio::test]
async fn resume_restores_the_session_and_runs_exec_resume() {
    let h = harness("pr-context.jsonl", 0);
    store_session(&h, 50_000);
    let outcome = resume(&h).await.0.expect("success");
    let schema = h.call.codex_home.join("output-schema.json");
    assert_eq!(
        lines(&h, "child-args.txt"),
        [
            "exec",
            "resume",
            "--json",
            "--skip-git-repo-check",
            "--ignore-rules",
            "--output-schema",
            schema.to_str().unwrap(),
            THREAD,
            "-",
        ]
    );
    let restored = h.call.codex_home.join("sessions").join(rollout_rel(THREAD));
    assert_eq!(
        lines(&h, "child-sessions.txt"),
        [restored.to_str().unwrap()],
        "only the analysis's rollout is in CODEX_HOME when Codex starts"
    );
    assert_eq!(outcome.session_id.as_deref(), Some(THREAD));
    assert_eq!(
        outcome.usage.input_tokens,
        180_776 - 50_000,
        "usage is the delta of the thread's cumulative totals"
    );
}

#[tokio::test]
async fn lost_session_is_session_unavailable_before_codex_starts() {
    let h = harness("pr-context.jsonl", 0);
    let (result, events) = resume(&h).await;
    assert!(
        matches!(result, Err(ProviderError::SessionUnavailable)),
        "{result:?}"
    );
    assert!(events.is_empty());
    assert!(!h.call.job_dir.join("child-args.txt").exists());
}

#[tokio::test]
async fn session_codex_cannot_resume_is_session_unavailable() {
    let h = harness("pr-context.jsonl", 1);
    store_session(&h, 1);
    fs::write(
        h.call.job_dir.join("scenario.jsonl"),
        json!({"type": "error", "message": "no rollout found for thread id"}).to_string(),
    )
    .unwrap();
    let (result, _) = resume(&h).await;
    assert!(
        matches!(result, Err(ProviderError::SessionUnavailable)),
        "{result:?}"
    );
    assert!(
        h.call.session_dir.join(rollout_rel(THREAD)).exists(),
        "a failed job leaves the stored session as it was"
    );
}

#[tokio::test]
async fn stored_session_is_scrubbed_and_has_no_auth() {
    let h = harness("plan.jsonl", 0);
    fs::write(
        h.call.job_dir.join("child-rollout"),
        format!("{{\"cmd\":\"echo {GITHUB_CANARY} {MEMCAN_CANARY} {REFRESH_CANARY}\"}}\n"),
    )
    .unwrap();
    run(&h).await.0.expect("success");
    let mut stored = Vec::new();
    let mut dirs = vec![h.call.session_dir.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                stored.push(path);
            }
        }
    }
    assert_eq!(stored, [h.call.session_dir.join(rollout_rel(THREAD))]);
    let text = fs::read_to_string(&stored[0]).unwrap();
    for canary in [GITHUB_CANARY, MEMCAN_CANARY, REFRESH_CANARY] {
        assert!(!text.contains(canary), "{text}");
    }
    assert!(text.contains("[redacted]"));
}

#[tokio::test]
async fn usage_limit_carries_resets_at_from_the_rollout() {
    let h = harness("usage-limit.jsonl", 1);
    store_session(&h, 1);
    fs::write(
        h.call.job_dir.join("child-rollout"),
        token_count_line(2, 100.0, 1_794_990_905) + "\n",
    )
    .unwrap();
    let (result, _) = resume(&h).await;
    match result {
        Err(ProviderError::UsageLimit { resets_at }) => {
            assert_eq!(
                resets_at,
                chrono::DateTime::from_timestamp(1_794_990_905, 0)
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(
        child_file(&h, "invocations.txt").lines().count() == 1,
        "no further turn after the limit"
    );
}

#[tokio::test]
async fn cancel_kills_codex_that_ignores_sigterm_after_the_grace() {
    let mut h = harness("cancelled.jsonl", 0);
    h.provider.settings.term_grace = Duration::from_millis(300);
    fs::write(h.call.job_dir.join("ignore-term"), "").unwrap();
    fs::write(h.call.job_dir.join("hang"), "").unwrap();
    let (tx, _rx) = mpsc::channel(1024);
    let run = h.provider.run(&h.call, tx);
    let cancel = async {
        while !h.call.job_dir.join("child-stdin.txt").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        h.provider.cancel(h.call.job_id).await
    };
    let (result, cancelled) =
        tokio::time::timeout(Duration::from_secs(20), async { tokio::join!(run, cancel) })
            .await
            .expect("the grace ends the call");
    cancelled.expect("cancel");
    assert!(
        matches!(result, Err(ProviderError::Cancelled)),
        "{result:?}"
    );
    assert!(child_file(&h, "invocations.txt").lines().count() == 1);
}

/// Unknown event types and item types a newer Codex may print; they must be skipped, never fail the call.
const UNKNOWN_EVENTS: [&str; 5] = [
    r#"{"type":"turn.plan_updated","plan":[{"step":"read","status":"in_progress"}]}"#,
    r#"{"type":"item.completed","item":{"id":"u1","type":"web_search","query":"node-client retry"}}"#,
    r#"{"type":"item.completed","item":{"id":"u2","type":"collab_tool_call","tool":"wait","status":"completed"}}"#,
    r#"{"type":"thread.tokens_updated","usage":{"input_tokens":1}}"#,
    r#"{"type":"item.started","item":{"id":"u3","type":"future_item_kind"}}"#,
];

/// Golden normalised events of a resumed session (`pr-context.jsonl`, recorded as a resume of the fixture
/// thread) with unknown events interleaved. `UPDATE_GOLDEN=1` rewrites the golden file.
#[tokio::test]
async fn golden_resumed_session_with_unknown_events() {
    let h = harness("pr-context.jsonl", 0);
    store_session(&h, 1);
    let recorded = fs::read_to_string(fixture("codex/pr-context.jsonl")).unwrap();
    let mut scenario: Vec<&str> = recorded.lines().collect();
    for (index, unknown) in UNKNOWN_EVENTS.iter().enumerate() {
        scenario.insert(1 + index * 3, unknown);
    }
    fs::write(h.call.job_dir.join("scenario.jsonl"), scenario.join("\n")).unwrap();
    let (result, events) = resume(&h).await;
    let outcome = result.expect("unknown events do not fail the call");
    assert_eq!(outcome.session_id.as_deref(), Some(THREAD));
    let normalised: Vec<Value> = events
        .iter()
        .map(|event| match event {
            ProviderEvent::Activity(event) => {
                json!({"activity": event.activity, "summary": event.summary})
            }
            ProviderEvent::OutputReady(_) => json!({"output_ready": true}),
        })
        .collect();
    let golden = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/provider/codex/golden/resumed-pr-context.events.json");
    let actual = serde_json::to_string_pretty(&normalised).unwrap() + "\n";
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        fs::create_dir_all(golden.parent().unwrap()).unwrap();
        fs::write(&golden, &actual).unwrap();
    }
    assert_eq!(
        actual,
        fs::read_to_string(&golden).expect("golden file; run with UPDATE_GOLDEN=1"),
        "normalised events differ from the golden file"
    );
}

async fn configured_skills(h: &mut Harness) {
    let repo = h._dir.path().join("claudius");
    fs::create_dir_all(repo.join("skills/severity")).unwrap();
    fs::create_dir_all(repo.join("skills/delegate")).unwrap();
    fs::write(
        repo.join("skills/severity/SKILL.md"),
        "---\nname: severity\ndescription: Rate change risk.\n---\nSeverity guidance.\n",
    )
    .unwrap();
    fs::write(repo.join("skills/delegate/SKILL.md"), "delegate skill\n").unwrap();
    for args in [
        &["init", "--quiet", "-b", "main"][..],
        &["add", "-A"],
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--quiet",
            "-m",
            "s",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let skills = Arc::new(Skills::new(
        format!("file://{}", repo.display()),
        "main".to_owned(),
        h._dir.path().join("checkout"),
        vec!["severity".to_owned()],
        Duration::from_secs(3600),
    ));
    skills.refresh_if_due().await;
    h.provider.settings.skills = Some(skills);
    h.call.skills = vec!["severity".to_owned()];
}

#[tokio::test]
async fn job_logs_skill_names_made_available_to_the_agent() {
    let log = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let sink = Arc::clone(&log);
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(move || Captured(Arc::clone(&sink)))
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let mut h = harness("plan.jsonl", 0);
    configured_skills(&mut h).await;
    run(&h).await.0.unwrap();
    let logged = String::from_utf8(log.lock().unwrap().clone()).unwrap();
    let line = logged
        .lines()
        .find(|line| line.contains("job agent skills available"))
        .expect("skill log");
    assert!(
        line.contains("severity") && line.contains(&h.call.job_id.to_string()),
        "{line}"
    );
    assert!(
        !line.contains(&h.call.job_dir.display().to_string()),
        "{line}"
    );
}

#[tokio::test]
async fn allowed_skills_are_copied_into_the_agent_home_with_their_commit() {
    let mut h = harness("plan.jsonl", 0);
    configured_skills(&mut h).await;
    let outcome = run(&h).await.0.expect("success");
    assert_eq!(outcome.skills_commit.as_deref().map(str::len), Some(40));
    let root = h.call.job_dir.join("home/.agents/skills");
    assert_eq!(
        fs::read_to_string(root.join("severity/SKILL.md")).unwrap(),
        "---\nname: severity\ndescription: Rate change risk.\n---\nSeverity guidance.\n"
    );
    assert!(
        !root.join("delegate").exists(),
        "skills outside the allow-list are never given"
    );
}

/// The supervisor binary next to this test binary (built with the package's tests).
fn supervisor_binary() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let binary = exe
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("smart-review-supervisor");
    assert!(
        binary.exists(),
        "build the smart-review-supervisor binary first: {}",
        binary.display()
    );
    binary
}

/// The harness under an in-process unprivileged supervisor (ADR-013 layout below the temp dir).
fn isolated(h: &mut Harness) -> PathBuf {
    isolated_with(h, None)
}

/// [`isolated`] with `clear` refused for directories named `fail_clears` (test-only fault injection).
fn isolated_with(h: &mut Harness, fail_clears: Option<&str>) -> PathBuf {
    let root = h._dir.path().to_path_buf();
    let agent_homes = root.join("codex-homes");
    fs::create_dir(&agent_homes).unwrap();
    let socket = crate::supervisor::serve::in_process(crate::supervisor::serve::ServeConfig {
        runner: crate::supervisor::serve::Ids {
            uid: rustix::process::getuid().as_raw(),
            gid: rustix::process::getgid().as_raw(),
        },
        agent: crate::supervisor::serve::Ids {
            uid: rustix::process::getuid().as_raw(),
            gid: rustix::process::getgid().as_raw(),
        },
        privileged: false,
        state_dir: root.clone(),
        work_dir: root.clone(),
        sweep_deadline: Duration::from_secs(5),
        fail_sweeps: false,
        fail_clears: fail_clears.map(str::to_owned),
        binary: supervisor_binary(),
    })
    .unwrap();
    let supervisor = crate::supervisor::client::Supervisor::connect(
        socket,
        supervisor_binary(),
        Duration::from_secs(30),
    )
    .unwrap();
    h.provider.settings.isolation = Some(Isolation::new(supervisor, agent_homes.clone()));
    agent_homes.join("codex-home")
}

#[tokio::test]
async fn isolated_call_copies_back_and_removes_the_agent_dirs() {
    let mut h = harness("plan.jsonl", 0);
    let agent_home = isolated(&mut h);
    let rotated = json!({"auth_mode": "chatgpt", "tokens": {"access_token": "rotated-access", "refresh_token": "rotated-refresh", "account_id": "acct-1"}});
    fs::write(h.call.job_dir.join("rotate"), rotated.to_string()).unwrap();
    let outcome = run(&h).await.0.expect("success");
    assert_eq!(outcome.session_id.as_deref(), Some(THREAD));
    let env = fs::read_to_string(h.call.job_dir.join("child-env.txt")).unwrap();
    assert!(
        env.lines()
            .any(|line| line == format!("CODEX_HOME={}", agent_home.display()))
    );
    assert!(!env.contains("SR_SUPERVISOR"), "{env}");
    assert!(
        fs::read_to_string(&h.master_auth)
            .unwrap()
            .contains("rotated-refresh"),
        "the rotated credential was copied back and written to the master"
    );
    let stored = fs::read_dir(&h.call.session_dir).unwrap().count();
    assert!(stored > 0, "the rollout was copied back and stored");
    assert!(!agent_home.exists(), "the agent CODEX_HOME is removed");
    assert!(!h.call.job_dir.join("home").exists() && !h.call.job_dir.join("tmp").exists());
    assert_eq!(
        fs::metadata(&h.call.job_dir).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[tokio::test]
async fn isolated_cancel_stops_codex_ignoring_sigterm_with_a_sweep() {
    let mut h = harness("plan.jsonl", 0);
    isolated(&mut h);
    h.provider.settings.term_grace = Duration::from_millis(300);
    fs::write(h.call.job_dir.join("ignore-term"), "").unwrap();
    fs::write(h.call.job_dir.join("hang"), "").unwrap();
    let job = h.call.job_id;
    let started = std::time::Instant::now();
    let (result, ()) = tokio::join!(run(&h), async {
        while !h.call.job_dir.join("child-stdin.txt").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        h.provider.cancel(job).await.unwrap();
    });
    assert!(
        matches!(result.0, Err(ProviderError::Cancelled)),
        "{:?}",
        result.0
    );
    assert!(started.elapsed() < Duration::from_secs(60));
}

const ROTATED_ACCESS: &str = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJyb3RhdGVkIn0.c2lnbmF0dXJlLXJvdGF0ZWQ";
/// Same shape as the master's refresh token (prefix `codex-`), as a rotation by the same issuer is.
const ROTATED_REFRESH: &str = "codex-refresh-ROTATEDtokenABCDEFGHIJ0123";

fn rotate_to_new_tokens(h: &Harness) {
    let rotated = json!({"auth_mode": "chatgpt", "tokens": {"access_token": ROTATED_ACCESS, "refresh_token": ROTATED_REFRESH, "account_id": "acct-1"}});
    fs::write(h.call.job_dir.join("rotate"), rotated.to_string()).unwrap();
}

#[tokio::test]
async fn isolated_rotated_tokens_never_leave_in_events_or_output() {
    let mut h = harness("plan.jsonl", 0);
    isolated(&mut h);
    rotate_to_new_tokens(&h);
    let command = format!("curl -H 'Authorization: Bearer {ROTATED_ACCESS}' -d {ROTATED_REFRESH}");
    let answer = json!({"note": format!("refresh {ROTATED_REFRESH}")}).to_string();
    let lines = [
        json!({"type": "thread.started", "thread_id": THREAD}),
        json!({"type": "turn.started"}),
        json!({"type": "item.completed", "item": {"id": "i1", "type": "command_execution", "command": command, "aggregated_output": "", "exit_code": 0, "status": "completed"}}),
        json!({"type": "item.completed", "item": {"id": "i2", "type": "agent_message", "text": answer}}),
        json!({"type": "turn.completed", "usage": {"input_tokens": 10, "cached_input_tokens": 0, "output_tokens": 5, "reasoning_output_tokens": 0}}),
    ];
    let scenario: String = lines.iter().map(|line| format!("{line}\n")).collect();
    fs::write(h.call.job_dir.join("scenario.jsonl"), scenario).unwrap();
    let (result, events) = run(&h).await;
    let outcome = result.expect("success");
    let seen = format!("{events:?} {:?}", outcome.output);
    assert!(
        !seen.contains(ROTATED_ACCESS) && !seen.contains(ROTATED_REFRESH),
        "{seen}"
    );
    assert!(seen.contains("[redacted]"));
}

#[tokio::test]
async fn isolated_failed_auth_copy_back_marks_the_credential_unusable() {
    let mut h = harness("plan.jsonl", 0);
    isolated(&mut h);
    fs::write(h.call.job_dir.join("symlink-auth"), "/etc/passwd").unwrap();
    let (result, _) = run(&h).await;
    assert!(
        matches!(result, Err(ProviderError::CredentialUnusable)),
        "{result:?}"
    );
    assert!(
        !h.provider.settings.credential.usable(),
        "the credential state is unknown, so it is unusable"
    );
}

#[tokio::test]
async fn isolated_locked_auth_is_unlocked_and_copied_back() {
    let mut h = harness("plan.jsonl", 0);
    isolated(&mut h);
    fs::write(h.call.job_dir.join("chmod-auth"), "").unwrap();
    run(&h).await.0.expect("success");
    assert!(h.provider.settings.credential.usable());
}

#[tokio::test]
async fn isolated_locked_agent_dirs_are_removed_and_the_next_job_runs() {
    let mut h = harness("plan.jsonl", 0);
    isolated(&mut h);
    fs::write(h.call.job_dir.join("lock-home"), "").unwrap();
    run(&h).await.0.expect("success");
    assert!(
        !h.call.job_dir.join("home").exists(),
        "the locked leftovers are gone"
    );
    assert!(h.provider.settings.isolation.as_ref().unwrap().healthy());
    fs::remove_file(h.call.job_dir.join("lock-home")).unwrap();
    fs::remove_file(h.call.job_dir.join("child-args.txt")).unwrap();
    run(&h).await.0.expect("the next job runs");
    assert!(
        h.call.job_dir.join("child-args.txt").exists(),
        "Codex ran for the next job"
    );
}

#[tokio::test]
async fn isolated_runtime_cap_stops_codex_and_still_copies_back() {
    let mut h = harness("plan.jsonl", 0);
    let agent_home = isolated(&mut h);
    h.provider.settings.term_grace = Duration::from_millis(300);
    h.call.limits.max_runtime_secs = 6;
    rotate_to_new_tokens(&h);
    fs::write(h.call.job_dir.join("ignore-term"), "").unwrap();
    fs::write(h.call.job_dir.join("hang"), "").unwrap();
    let started = std::time::Instant::now();
    let (result, _) = run(&h).await;
    assert!(matches!(result, Err(ProviderError::Timeout)), "{result:?}");
    assert!(
        started.elapsed() < Duration::from_secs(6),
        "stopped before the cap"
    );
    assert!(
        fs::read_to_string(&h.master_auth)
            .unwrap()
            .contains(ROTATED_REFRESH),
        "the rotated credential was copied back at the cap"
    );
    assert!(!agent_home.exists());
}

/// One Codex run whose command line carries the rotated tokens glued to a preceding character.
fn glued_token_scenario(h: &Harness) {
    let command = format!("echo x{ROTATED_ACCESS} x{ROTATED_REFRESH}");
    let lines = [
        json!({"type": "thread.started", "thread_id": THREAD}),
        json!({"type": "turn.started"}),
        json!({"type": "item.completed", "item": {"id": "i1", "type": "command_execution", "command": command, "aggregated_output": "", "exit_code": 0, "status": "completed"}}),
        json!({"type": "item.completed", "item": {"id": "i2", "type": "agent_message", "text": "{}"}}),
        json!({"type": "turn.completed", "usage": {"input_tokens": 10, "cached_input_tokens": 0, "output_tokens": 5, "reasoning_output_tokens": 0}}),
    ];
    let scenario: String = lines.iter().map(|line| format!("{line}\n")).collect();
    fs::write(h.call.job_dir.join("scenario.jsonl"), scenario).unwrap();
}

#[tokio::test]
async fn isolated_glued_rotated_tokens_never_leave_in_events() {
    let mut h = harness("plan.jsonl", 0);
    isolated(&mut h);
    rotate_to_new_tokens(&h);
    glued_token_scenario(&h);
    let (result, events) = run(&h).await;
    result.expect("success");
    let seen = format!("{events:?}");
    assert!(
        !seen.contains(ROTATED_ACCESS) && !seen.contains(ROTATED_REFRESH),
        "{seen}"
    );
    assert!(seen.contains("x[redacted]"), "{seen}");
}

#[tokio::test]
async fn isolated_glued_rotated_tokens_never_leave_in_stderr() {
    let log = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let sink = Arc::clone(&log);
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .with_writer(move || Captured(Arc::clone(&sink)))
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let mut h = harness("plan.jsonl", 0);
    isolated(&mut h);
    rotate_to_new_tokens(&h);
    glued_token_scenario(&h);
    fs::write(
        h.call.job_dir.join("stderr"),
        format!("refresh failed for x{ROTATED_REFRESH} with x{ROTATED_ACCESS}\n"),
    )
    .unwrap();
    run(&h).await.0.expect("success");
    let logged = String::from_utf8(log.lock().unwrap().clone()).unwrap();
    assert!(logged.contains("Codex stderr tail"), "{logged}");
    assert!(
        !logged.contains(ROTATED_ACCESS) && !logged.contains(ROTATED_REFRESH),
        "{logged}"
    );
}

fn mode_and_owner(path: &Path) -> (u32, u32) {
    use std::os::unix::fs::MetadataExt as _;
    let meta = fs::symlink_metadata(path).unwrap();
    (meta.mode() & 0o7777, meta.uid())
}

#[tokio::test]
async fn isolated_uncleared_agent_dirs_are_quarantined_0700_by_the_supervisor() {
    let mut h = harness("plan.jsonl", 0);
    let agent_home = isolated_with(&mut h, Some("codex-home"));
    run(&h).await.0.expect("success");
    let me = rustix::process::getuid().as_raw();
    assert_eq!(
        mode_and_owner(&agent_home),
        (0o700, me),
        "quarantined, not removed"
    );
    assert!(h.provider.settings.isolation.as_ref().unwrap().healthy());
}

#[tokio::test]
async fn quarantine_sets_the_mode_in_the_supervisor_before_replying() {
    let mut h = harness("plan.jsonl", 0);
    let agent_home = isolated(&mut h);
    fs::create_dir(&agent_home).unwrap();
    fs::write(agent_home.join("left"), "x").unwrap();
    fs::set_permissions(&agent_home, fs::Permissions::from_mode(0o755)).unwrap();
    let isolation = h.provider.settings.isolation.as_ref().unwrap();
    let reply = isolation
        .supervisor
        .request_for_tests(crate::supervisor::protocol::Op::Quarantine {
            path: agent_home.to_string_lossy().into_owned(),
        })
        .await;
    assert!(reply.is_ok(), "{reply:?}");
    assert_eq!(
        mode_and_owner(&agent_home).0,
        0o700,
        "set by the supervisor itself"
    );
}

#[tokio::test]
async fn recovery_quarantines_a_reachable_leftover_and_verifies_it() {
    let mut h = harness("plan.jsonl", 0);
    let agent_home = isolated_with(&mut h, Some("stale-1"));
    // A crash left a directory the agent can still enter (mode 0755, content inside).
    let stale = agent_home.with_file_name("stale-1");
    fs::create_dir(&stale).unwrap();
    fs::write(stale.join("left"), "x").unwrap();
    fs::set_permissions(&stale, fs::Permissions::from_mode(0o755)).unwrap();
    let staging = h._dir.path().join("staging");
    fs::create_dir(&staging).unwrap();
    let isolation = h.provider.settings.isolation.as_ref().unwrap();
    isolation
        .recover(&staging, &h._dir.path().join("work"))
        .await
        .expect("recovered");
    let me = rustix::process::getuid().as_raw();
    assert_eq!(mode_and_owner(&stale), (0o700, me));
}

#[tokio::test]
async fn recovery_stops_when_a_leftover_cannot_be_quarantined() {
    let mut h = harness("plan.jsonl", 0);
    let agent_home = isolated(&mut h);
    std::os::unix::fs::symlink("/etc", agent_home.with_file_name("linked-1")).unwrap();
    let staging = h._dir.path().join("staging");
    fs::create_dir(&staging).unwrap();
    let isolation = h.provider.settings.isolation.as_ref().unwrap();
    let result = isolation
        .recover(&staging, &h._dir.path().join("work"))
        .await;
    assert!(result.is_err(), "start-up must stop: {result:?}");
}

/// A crash left `<staging>/<key>.cas` and the old staged copy; the agent-side `auth.json` is a link.
fn crashed_job_with_rejected_auth(h: &Harness, agent_homes: &Path) -> PathBuf {
    use sha2::Digest as _;
    let staging = h._dir.path().join("staging");
    let key = "job-1";
    fs::create_dir_all(staging.join(key)).unwrap();
    let master = fs::read(&h.master_auth).unwrap();
    fs::write(staging.join(key).join("auth.json"), &master).unwrap();
    let digest: String = sha2::Sha256::digest(&master)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    fs::write(staging.join(format!("{key}.cas")), digest).unwrap();
    let agent = agent_homes.join(key);
    fs::create_dir(&agent).unwrap();
    std::os::unix::fs::symlink(&h.master_auth, agent.join("auth.json")).unwrap();
    staging
}

#[tokio::test]
async fn recovery_with_a_rejected_agent_auth_marks_the_credential_unusable() {
    let mut h = harness("plan.jsonl", 0);
    let agent_home = isolated(&mut h);
    let staging = crashed_job_with_rejected_auth(&h, agent_home.parent().unwrap());
    let isolation = h.provider.settings.isolation.as_ref().unwrap();
    isolation
        .recover(&staging, &h._dir.path().join("work"))
        .await
        .expect("recovered");
    h.provider.settings.credential.recover(&staging).unwrap();
    assert!(
        !h.provider.settings.credential.usable(),
        "the old staged copy proves nothing after a crash"
    );
}

#[tokio::test]
async fn recovery_with_a_rejected_agent_auth_keeps_a_newer_login() {
    let mut h = harness("plan.jsonl", 0);
    let agent_home = isolated(&mut h);
    let staging = crashed_job_with_rejected_auth(&h, agent_home.parent().unwrap());
    // The owner signed in again after the crash.
    let newer = json!({"auth_mode": "chatgpt", "tokens": {"access_token": "new-access", "refresh_token": "new-refresh", "account_id": "acct-1"}});
    fs::write(&h.master_auth, newer.to_string()).unwrap();
    let isolation = h.provider.settings.isolation.as_ref().unwrap();
    isolation
        .recover(&staging, &h._dir.path().join("work"))
        .await
        .expect("recovered");
    h.provider.settings.credential.recover(&staging).unwrap();
    assert!(h.provider.settings.credential.usable());
    assert_eq!(
        fs::read_to_string(&h.master_auth).unwrap(),
        newer.to_string()
    );
}

/// A log writer appending to a shared buffer.
struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn required_skills_fail_closed_before_starting_codex() {
    let mut h = harness("plan.jsonl", 0);
    h.call.skills = vec!["severity".to_owned()];
    assert!(matches!(
        h.provider.prepare(&h.call).await,
        Err(ProviderError::Configuration(_))
    ));
    assert!(!h.call.job_dir.join("child-args.txt").exists());
}

#[tokio::test]
async fn isolated_agent_can_read_required_skills_from_its_discovery_home() {
    // TODO: repeat with distinct production UIDs; the local sandbox only permits the unprivileged supervisor.
    let mut h = harness("plan.jsonl", 0);
    configured_skills(&mut h).await;
    isolated(&mut h);
    fs::write(h.call.job_dir.join("check-skills"), "").unwrap();
    let outcome = run(&h).await.0.expect("agent reads skill");
    assert!(outcome.skills_commit.is_some());
    assert!(
        fs::read_to_string(h.call.job_dir.join("child-skill.txt"))
            .unwrap()
            .contains("name: severity")
    );
    assert_eq!(
        fs::read_to_string(h.call.job_dir.join("child-uid.txt"))
            .unwrap()
            .trim(),
        rustix::process::getuid().as_raw().to_string()
    );
}

#[tokio::test]
async fn missing_or_disallowed_required_manifest_is_a_configuration_failure() {
    let mut h = harness("plan.jsonl", 0);
    configured_skills(&mut h).await;
    for required in ["delegate", "missing"] {
        h.call.skills = vec![required.to_owned()];
        assert!(matches!(
            h.provider.prepare(&h.call).await,
            Err(ProviderError::Configuration(_))
        ));
    }
    h.call.skills = vec!["severity".to_owned()];
    fs::remove_file(h._dir.path().join("checkout/skills/severity/SKILL.md")).unwrap();
    assert!(matches!(
        h.provider.prepare(&h.call).await,
        Err(ProviderError::Configuration(_))
    ));
}

#[tokio::test]
async fn useful_turn_is_archived_without_an_extra_turn() {
    for resumed in [false, true] {
        let mut h = harness("plan.jsonl", 0);
        h.call.limits.max_runtime_secs = 10;
        if resumed {
            store_session(&h, 1);
        }
        let outcome = if resumed {
            resume(&h).await.0
        } else {
            run(&h).await.0
        }
        .unwrap();
        assert_eq!(outcome.session_id.as_deref(), Some(THREAD));
        assert!(h.call.session_dir.join(rollout_rel(THREAD)).exists());
        assert!(child_file(&h, "invocations.txt").lines().count() == 1);
        assert!(
            child_file(&h, "child-config.toml").contains("model_auto_compact_token_limit = 400000")
        );
    }
}

#[tokio::test]
async fn agent_plain_git_accepts_the_runner_owned_checkout() {
    let h = harness("plan.jsonl", 0);
    let repo = h.call.job_dir.join("repo");
    let commits = crate::test_support::commits();
    crate::git::checkout(
        &h._dir.path().join("mirror.git"),
        &format!("file://{}", crate::test_support::shared_repo().display()),
        &[&commits.head, &commits.base],
        &repo,
    )
    .await
    .unwrap();
    for args in [
        vec!["diff", &commits.base, &commits.head, "--", "src/config.rs"],
        vec!["log", "-1"],
        vec!["show", &commits.base],
        vec!["blame", "HEAD", "--", "src/config.rs"],
    ] {
        let output = std::process::Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .env_clear()
            .envs(agent_env(&h.call, &h.call.codex_home, None))
            .env("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[tokio::test]
async fn private_archive_fork_restores_analytic_but_never_changes_it() {
    let mut h = harness("pr-context.jsonl", 0);
    let analytic_file = store_session(&h, 50_000);
    let original = fs::read(&analytic_file).unwrap();
    h.call.session_dir = h.call.session_dir.parent().unwrap().join("users/user-a");
    let outcome = resume(&h).await.0.expect("fork");
    assert_eq!(outcome.session_id.as_deref(), Some(THREAD));
    assert_eq!(fs::read(&analytic_file).unwrap(), original);
    assert!(h.call.session_dir.join(rollout_rel(THREAD)).exists());
    assert_eq!(outcome.usage.input_tokens, 180_776 - 50_000);
}

#[tokio::test]
async fn flow_file_is_collected_without_a_schema_and_plain_final_message_is_allowed() {
    let mut h = harness("plan.jsonl", 0);
    h.call.flow_file = Some(contracts::job::spec::FlowFile {
        context: json!({"version":1}),
        previous: None,
    });
    super::super::output_file::prepare(&h.call.job_dir, h.call.flow_file.as_ref().unwrap())
        .unwrap();
    let document = "# flow f1\n";
    fs::write(h.call.job_dir.join("out/flow.md"), document).unwrap();
    let scenario = fs::read_to_string(h.call.job_dir.join("scenario.jsonl")).unwrap();
    let scenario = scenario
        .lines()
        .map(|line| {
            let mut value: Value = serde_json::from_str(line).unwrap();
            if value["item"]["type"] == "agent_message" {
                value["item"]["text"] = json!("Flow file ready.");
            }
            value.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(h.call.job_dir.join("scenario.jsonl"), scenario).unwrap();
    let (result, events) = run(&h).await;
    let result = result.unwrap();
    assert_eq!(result.output, json!(document));
    assert!(
        result.session_id.is_some(),
        "saving the file must still archive the useful session"
    );
    assert_eq!(
        events.last(),
        Some(&ProviderEvent::OutputReady(json!(document)))
    );
    assert!(!child_file(&h, "child-args.txt").contains("--output-schema"));
    assert!(!h.call.codex_home.join("output-schema.json").exists());
}

#[tokio::test]
async fn missing_flow_file_is_a_failed_attempt_with_a_repair_instruction() {
    let mut h = harness("plan.jsonl", 0);
    h.call.flow_file = Some(contracts::job::spec::FlowFile {
        context: json!({}),
        previous: None,
    });
    super::super::output_file::prepare(&h.call.job_dir, h.call.flow_file.as_ref().unwrap())
        .unwrap();
    let (result, events) = run(&h).await;
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("out/flow.md is missing")
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ProviderEvent::OutputReady(_)))
    );
}

#[tokio::test]
async fn required_skill_read_is_instructed_only_when_starting_a_session() {
    let mut h = harness("plan.jsonl", 0);
    configured_skills(&mut h).await;
    run(&h).await.0.expect("first session");
    let instruction = "Read the required skills once for this new session:";
    let first = child_file(&h, "child-stdin.txt");
    assert!(first.contains(instruction), "{first}");
    assert!(first.contains("$HOME/.agents/skills/severity/SKILL.md"));
    let mut resumed_job = harness("plan.jsonl", 0);
    configured_skills(&mut resumed_job).await;
    resumed_job.call.restore_dir = h.call.session_dir.clone();
    let (tx, _rx) = mpsc::channel(1024);
    resumed_job
        .provider
        .resume(THREAD, &resumed_job.call, tx)
        .await
        .expect("resume");
    let resumed = child_file(&resumed_job, "child-stdin.txt");
    assert!(!resumed.contains(instruction), "{resumed}");
    // A context-pack fallback starts a new session through the same run entry point.
    let mut fallback = harness("plan.jsonl", 0);
    configured_skills(&mut fallback).await;
    fallback.call.prompt = format!("Context pack: accepted plan.\n{}", fallback.call.prompt);
    run(&fallback).await.0.expect("new session from pack");
    assert!(child_file(&fallback, "child-stdin.txt").contains(instruction));
}
