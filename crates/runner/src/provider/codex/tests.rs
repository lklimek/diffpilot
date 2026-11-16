//! Codex provider against a stand-in `codex` script that records its argv, environment and stdin and replays
//! a recorded JSONL stream (`fixtures/pr-482/codex/`). Real Codex is never run here.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
env > "$PWD/child-env.txt"
for arg in "$@"; do printf '%s\n' "$arg"; done > "$PWD/child-args.txt"
cat > "$PWD/child-stdin.txt"
if [ -f "$PWD/rotate" ]; then cat "$PWD/rotate" > "$CODEX_HOME/auth.json"; fi
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
        session_dir: dir.path().join("sessions"),
        prompt: format!("Job: plan. {PROMPT_CANARY}"),
        output_schema: json!({"type": "object", "additionalProperties": false, "properties": {}, "required": []}),
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
            }),
        },
        skills: vec!["severity".to_owned()],
    };
    let provider = CodexProvider::new(CodexSettings {
        binary: fake_codex().to_path_buf(),
        master_auth: master_auth.clone(),
        egress_proxy: Some(PROXY.to_owned()),
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
        outcome.usage.input_tokens, 48210,
        "a new thread's usage is its cumulative total"
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
    assert!(!config.contains("mcp_servers"), "no MCP servers in stage 2");
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

fn kept_copy(h: &Harness) -> PathBuf {
    h.master_auth
        .with_file_name(format!("auth.json.rotated-{}", h.call.job_id))
}

#[tokio::test]
async fn rotated_credential_is_kept_and_the_credential_blocked() {
    let h = harness("plan.jsonl", 0);
    fs::write(h.call.job_dir.join("rotate"), rotated_auth()).unwrap();
    run(&h).await.0.expect("success");
    assert_eq!(fs::read_to_string(kept_copy(&h)).unwrap(), rotated_auth());
    assert!(
        !credential_usable(&h.master_auth),
        "a rotation blocks the credential"
    );
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
async fn rotated_credential_survives_a_dropped_call() {
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
    assert_eq!(fs::read_to_string(kept_copy(&h)).unwrap(), rotated_auth());
    assert!(!credential_usable(&h.master_auth));
}

#[tokio::test]
async fn blocked_credential_refuses_the_next_call_without_starting_codex() {
    let h = harness("plan.jsonl", 0);
    fs::write(h.call.job_dir.join("rotate"), rotated_auth()).unwrap();
    run(&h).await.0.expect("success");
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
async fn refresh_failure_blocks_the_credential() {
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
    assert!(!credential_usable(&h.master_auth));
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

#[test]
fn toml_string_escapes_quotes_backslashes_and_controls() {
    assert_eq!(toml_string("a\"b\\c\nd"), r#""a\"b\\c\u000Ad""#);
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
    assert!(running.lock().unwrap().contains_key(&job));
    assert!(!group.leader_exited());
    group.kill_and_disarm();
    assert!(
        group.pid.is_none(),
        "disarmed: a later drop signals nothing"
    );
    assert!(running.lock().unwrap().is_empty());
    let status = child.wait().await.expect("reap");
    assert!(!status.success(), "the group was killed");
    drop(group);
}
