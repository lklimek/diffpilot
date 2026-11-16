//! The Codex CLI provider: `codex exec --json --output-schema` in the job directory with a per-job
//! `CODEX_HOME` and a constructed environment (S1a, S1b, S6 env contract, threat model T2/T5).
//!
//! Stage 2 implements `run` only. Deferred to WP3-RUN: `resume` and end-of-job compaction (ADR-004), SIGTERM
//! cancel with grace, usage-limit `resets_at` from the rollout, MCP servers in the per-job config (T3c, T6),
//! skills, credential compare-and-swap write-back and its recovery (T13), the two-uid supervisor (T1).

mod stream;

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

use chrono::Utc;
use contracts::ids::JobId;
use contracts::job::event::JobEvent;
use contracts::job::result::Usage;
use contracts::job::spec::{ModelChoice, ReasoningEffort};
use futures::future::BoxFuture;
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};

pub(crate) use stream::{CodexStream, Exit, usage_delta};

use crate::provider::{LlmProvider, ProviderCall, ProviderError, ProviderEvent, ProviderOutcome};
use crate::scrub::{SUMMARY_MAX_CHARS, Scrubber, clip};

/// `PATH` of the agent environment; never the runner's own.
pub const AGENT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
/// `NO_PROXY` of the agent environment: loopback only, never the MemCan host (T2).
pub const AGENT_NO_PROXY: &str = "localhost,127.0.0.1";
/// File names inside the per-job `CODEX_HOME`.
const AUTH_FILE: &str = "auth.json";
const CONFIG_FILE: &str = "config.toml";
const SCHEMA_FILE: &str = "output-schema.json";
/// Longest JSONL line kept; a longer one is skipped (an oversized final answer then fails as not JSON).
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
/// Bytes of Codex's stderr kept for the debug log.
const STDERR_TAIL_BYTES: usize = 4096;
/// How long stdout may stay open after Codex exited (a leftover child holding the pipe).
const DRAIN_AFTER_EXIT: Duration = Duration::from_secs(2);

/// Where the Codex provider finds its binary, credential and egress proxy.
#[derive(Clone, Debug)]
pub struct CodexSettings {
    /// The `codex` binary; a bare name is resolved through [`AGENT_PATH`].
    pub binary: PathBuf,
    /// The master `auth.json` of the runner's credential (runner store; copied into each job).
    pub master_auth: PathBuf,
    /// Explicit egress proxy URL for the agent (`http://egress-proxy:3128`), if any.
    pub egress_proxy: Option<String>,
}

/// Runs agent calls with the Codex CLI.
#[derive(Debug)]
pub struct CodexProvider {
    settings: CodexSettings,
}

impl CodexProvider {
    /// A provider with `settings`.
    pub fn new(settings: CodexSettings) -> Self {
        Self { settings }
    }

    async fn run_call(
        &self,
        call: &ProviderCall,
        events: &mpsc::Sender<ProviderEvent>,
    ) -> Result<ProviderOutcome, ProviderError> {
        let credential = self.prepare(call).await?;
        let scrubber = Scrubber::new(credential.tokens.iter().map(String::as_str));
        let result = self.exec(call, &scrubber, events).await;
        self.keep_rotated_credential(call, &credential.original)
            .await;
        let finished = result.map_err(|error| scrub_error(error, &scrubber))?;
        let output = scrubber.value(finished.output);
        // Execute may have stopped listening (job fenced); the outcome still carries the output.
        let _ = events
            .send(ProviderEvent::OutputReady(output.clone()))
            .await;
        Ok(ProviderOutcome {
            output,
            session_id: finished.thread_id,
            // A new thread starts at zero, so its cumulative total is this call's usage.
            usage: usage_delta(Usage::default(), finished.cumulative_usage),
            skills_commit: None,
        })
    }

    /// Writes the per-job `CODEX_HOME` (credential copy, lean config, output schema) and the agent's home and
    /// temp directories inside the job directory.
    async fn prepare(&self, call: &ProviderCall) -> Result<CredentialCopy, ProviderError> {
        let original = tokio::fs::read(&self.settings.master_auth)
            .await
            .map_err(|error| {
                tracing::error!(
                    path = %self.settings.master_auth.display(),
                    %error,
                    "cannot read the master Codex credential; sign in with `codex login --device-auth` in the runner container"
                );
                ProviderError::CredentialUnusable
            })?;
        let tokens = credential_tokens(&original);
        write_private(&call.codex_home.join(AUTH_FILE), &original).await?;
        write_private(
            &call.codex_home.join(CONFIG_FILE),
            config_toml(&call.model).as_bytes(),
        )
        .await?;
        let schema = serde_json::to_vec(&call.output_schema).map_err(io::Error::other)?;
        write_private(&call.codex_home.join(SCHEMA_FILE), &schema).await?;
        for dir in [agent_home(&call.job_dir), agent_tmp(&call.job_dir)] {
            create_private_dir(&dir).await?;
        }
        Ok(CredentialCopy { original, tokens })
    }

    /// The `codex exec` command: fixed flags and paths only (no secret, no prompt in argv), cwd = job dir
    /// above the checkout (T5), environment built from scratch (T2).
    fn command(&self, call: &ProviderCall) -> Command {
        let mut command = Command::new(&self.settings.binary);
        command
            .args([
                "exec",
                "--json",
                "--skip-git-repo-check",
                "--ignore-rules",
                "--output-schema",
            ])
            .arg(call.codex_home.join(SCHEMA_FILE))
            .arg("-")
            .current_dir(&call.job_dir)
            .env_clear()
            .envs(agent_env(call, self.settings.egress_proxy.as_deref()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }

    async fn exec(
        &self,
        call: &ProviderCall,
        scrubber: &Scrubber,
        events: &mpsc::Sender<ProviderEvent>,
    ) -> Result<stream::Finished, ProviderError> {
        let mut child = self.command(call).spawn().map_err(|error| {
            tracing::error!(binary = %self.settings.binary.display(), %error, "cannot start Codex; check SR_RUNNER_CODEX_BIN");
            ProviderError::Io(error)
        })?;
        let stdin = child.stdin.take().map(|mut stdin| {
            let prompt = call.prompt.clone();
            tokio::spawn(async move {
                // A Codex that exits early closes stdin; its exit status tells why.
                let _ = stdin.write_all(prompt.as_bytes()).await;
            })
        });
        let stderr = child
            .stderr
            .take()
            .map(|stderr| tokio::spawn(stderr_tail(stderr)));
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("Codex stdout was not captured"))?;
        let mut lines = Lines::new(BufReader::new(stdout));
        let mut parsed = CodexStream::default();
        let status = read_until_exit(&mut child, &mut lines, &mut parsed, scrubber, events).await?;
        if let Some(stdin) = stdin {
            let _ = stdin.await;
        }
        if let Some(stderr) = stderr
            && let Ok(tail) = stderr.await
            && !tail.is_empty()
        {
            tracing::debug!(job = %call.job_id, stderr = %scrubber.text(&tail), "Codex stderr tail");
        }
        if parsed.unparsed_lines() > 0 || lines.skipped > 0 {
            tracing::warn!(
                job = %call.job_id,
                unparsed = parsed.unparsed_lines(),
                oversized = lines.skipped,
                "Codex printed lines that are not JSON events or exceed the line cap; they were skipped"
            );
        }
        parsed.finish(exit_of(status))
    }

    /// Copy-back is WP3-RUN (T13). Until then a credential rotated during the job is kept next to the master
    /// instead of being deleted with the job, so the owner can restore it.
    async fn keep_rotated_credential(&self, call: &ProviderCall, original: &[u8]) {
        let copy = call.codex_home.join(AUTH_FILE);
        let Ok(current) = tokio::fs::read(&copy).await else {
            return;
        };
        if current == original {
            return;
        }
        let kept = self
            .settings
            .master_auth
            .with_file_name(format!("{AUTH_FILE}.rotated-{}", call.job_id));
        match write_private(&kept, &current).await {
            Ok(()) => tracing::error!(
                job = %call.job_id,
                kept = %kept.display(),
                "Codex refreshed its credential during the job and copy-back is not implemented yet; replace the master auth.json with the kept copy"
            ),
            Err(error) => tracing::error!(
                job = %call.job_id,
                %error,
                "Codex refreshed its credential during the job and the rotated copy could not be kept; sign in again"
            ),
        }
    }
}

impl LlmProvider for CodexProvider {
    fn run<'a>(
        &'a self,
        call: &'a ProviderCall,
        events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        Box::pin(async move { self.run_call(call, &events).await })
    }

    fn resume<'a>(
        &'a self,
        _session_id: &'a str,
        _call: &'a ProviderCall,
        _events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        // Session resume arrives with WP3-RUN; the caller starts a new session with the context pack.
        Box::pin(async { Err(ProviderError::SessionUnavailable) })
    }

    fn cancel(&self, _job: JobId) -> BoxFuture<'_, Result<(), ProviderError>> {
        // SIGTERM with grace arrives with WP3-RUN; until then the job loop stops the call by dropping it
        // (`kill_on_drop` sends SIGKILL).
        Box::pin(async {
            Err(ProviderError::Failed {
                message: "cancelling a running Codex call is not supported yet".to_owned(),
            })
        })
    }
}

struct CredentialCopy {
    original: Vec<u8>,
    tokens: Vec<String>,
}

/// Token values of an `auth.json` (`tokens.*`, `OPENAI_API_KEY`), for scrubbing.
fn credential_tokens(auth: &[u8]) -> Vec<String> {
    let Ok(json) = serde_json::from_slice::<Value>(auth) else {
        return Vec::new();
    };
    let mut tokens = Vec::new();
    if let Some(Value::Object(map)) = json.get("tokens") {
        tokens.extend(map.values().filter_map(Value::as_str).map(str::to_owned));
    }
    if let Some(key) = json.get("OPENAI_API_KEY").and_then(Value::as_str) {
        tokens.push(key.to_owned());
    }
    tokens
}

fn scrub_error(error: ProviderError, scrubber: &Scrubber) -> ProviderError {
    match error {
        ProviderError::Failed { message } => ProviderError::Failed {
            message: scrubber.text(&message),
        },
        ProviderError::SchemaRejected { message } => ProviderError::SchemaRejected {
            message: scrubber.text(&message),
        },
        other => other,
    }
}

/// The agent's environment (threat model T2): home, temp dir, `CODEX_HOME`, locale, a fixed `PATH` and the
/// proxy variables; nothing inherited from the runner, no secret.
pub(crate) fn agent_env(
    call: &ProviderCall,
    egress_proxy: Option<&str>,
) -> Vec<(&'static str, OsString)> {
    let mut env: Vec<(&'static str, OsString)> = vec![
        ("HOME", agent_home(&call.job_dir).into_os_string()),
        ("PATH", AGENT_PATH.into()),
        ("CODEX_HOME", call.codex_home.clone().into_os_string()),
        ("TMPDIR", agent_tmp(&call.job_dir).into_os_string()),
        ("LANG", "C.UTF-8".into()),
    ];
    if let Some(proxy) = egress_proxy {
        for key in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
            env.push((key, proxy.into()));
        }
        for key in ["NO_PROXY", "no_proxy"] {
            env.push((key, AGENT_NO_PROXY.into()));
        }
    }
    env
}

fn agent_home(job_dir: &Path) -> PathBuf {
    job_dir.join("home")
}

fn agent_tmp(job_dir: &Path) -> PathBuf {
    job_dir.join("tmp")
}

/// The lean per-job config (S1a, ADR-002): sandbox off (T1–T4 compensate), no project docs, no hooks, no
/// plugins or analytics, no sub-agents, no bundled skills.
pub(crate) fn config_toml(model: &ModelChoice) -> String {
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
fn toml_string(value: &str) -> String {
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

async fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .await?;
    file.write_all(contents).await?;
    file.flush().await
}

async fn create_private_dir(path: &Path) -> io::Result<()> {
    tokio::fs::DirBuilder::new().mode(0o700).create(path).await
}

/// Reads events until Codex exits, then drains what is left on stdout for a short while.
async fn read_until_exit<R: AsyncBufRead + Unpin>(
    child: &mut Child,
    lines: &mut Lines<R>,
    parsed: &mut CodexStream,
    scrubber: &Scrubber,
    events: &mpsc::Sender<ProviderEvent>,
) -> Result<ExitStatus, ProviderError> {
    let status = loop {
        tokio::select! {
            line = lines.next() => match line? {
                Some(line) => forward(parsed, &line, scrubber, events).await,
                None => break child.wait().await?,
            },
            status = child.wait() => break status?,
        }
    };
    let drain = async {
        while let Some(line) = lines.next().await? {
            forward(parsed, &line, scrubber, events).await;
        }
        Ok::<(), io::Error>(())
    };
    match timeout(DRAIN_AFTER_EXIT, drain).await {
        Ok(result) => result?,
        Err(_) => {
            tracing::warn!("Codex exited but its output stayed open; a leftover process holds it")
        }
    }
    Ok(status)
}

async fn forward(
    parsed: &mut CodexStream,
    line: &str,
    scrubber: &Scrubber,
    events: &mpsc::Sender<ProviderEvent>,
) {
    if let Some((activity, summary)) = parsed.line(line) {
        let event = JobEvent {
            seq: 0,
            at: Utc::now(),
            activity,
            summary: clip(&scrubber.text(&summary), SUMMARY_MAX_CHARS),
        };
        // A closed receiver means execute stopped listening; Codex keeps running until it is stopped.
        let _ = events.send(ProviderEvent::Activity(event)).await;
    }
}

fn exit_of(status: ExitStatus) -> Exit {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => Exit::Code(code),
        (None, Some(signal)) => Exit::Signal(signal),
        (None, None) => Exit::Code(-1),
    }
}

async fn stderr_tail(mut stderr: tokio::process::ChildStderr) -> String {
    let mut tail: Vec<u8> = Vec::new();
    let mut chunk = [0_u8; 4096];
    while let Ok(read) = stderr.read(&mut chunk).await {
        if read == 0 {
            break;
        }
        tail.extend_from_slice(&chunk[..read]);
        if tail.len() > STDERR_TAIL_BYTES {
            tail.drain(..tail.len() - STDERR_TAIL_BYTES);
        }
    }
    String::from_utf8_lossy(&tail).into_owned()
}

/// Newline-separated lines with a length cap; cancellation-safe (all state lives in the struct).
struct Lines<R> {
    reader: R,
    line: Vec<u8>,
    oversized: bool,
    skipped: usize,
}

impl<R: AsyncBufRead + Unpin> Lines<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            line: Vec::new(),
            oversized: false,
            skipped: 0,
        }
    }

    /// The next line within the cap (oversized lines are skipped and counted), or `None` at end of input.
    async fn next(&mut self) -> io::Result<Option<String>> {
        loop {
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                return Ok(self.take_line());
            }
            let (end, used, complete) = match available.iter().position(|byte| *byte == b'\n') {
                Some(index) => (index, index + 1, true),
                None => (available.len(), available.len(), false),
            };
            if !self.oversized {
                if self.line.len() + end > MAX_LINE_BYTES {
                    self.oversized = true;
                    self.line = Vec::new();
                } else {
                    self.line.extend_from_slice(&available[..end]);
                }
            }
            self.reader.consume(used);
            if complete && let Some(line) = self.take_line() {
                return Ok(Some(line));
            }
        }
    }

    /// The buffered line, or `None` (counting a skipped oversized line).
    fn take_line(&mut self) -> Option<String> {
        if std::mem::take(&mut self.oversized) {
            self.skipped += 1;
            return None;
        }
        if self.line.is_empty() {
            return None;
        }
        let line = String::from_utf8_lossy(&self.line).into_owned();
        self.line.clear();
        Some(line)
    }
}

#[cfg(test)]
mod tests;
