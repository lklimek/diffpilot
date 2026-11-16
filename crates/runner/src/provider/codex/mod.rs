//! The Codex CLI provider: `codex exec --json --output-schema` in the job directory with a per-job
//! `CODEX_HOME` and a constructed environment (S1a, S1b, S6 env contract, threat model T2/T5).
//!
//! Codex runs as the leader of its own process group; the group is killed when the call ends, is cancelled
//! (SIGTERM) or dropped, so agent commands left in the background do not outlive the attempt (a process that
//! starts its own session escapes this; the uid-wide sweep is WP3-RUN, T1d). Stage 2 implements `run` only.
//! Deferred to WP3-RUN: `resume` and end-of-job compaction (ADR-004), usage-limit `resets_at` from the rollout,
//! MCP servers in the per-job config (T3c, T6), skills, credential compare-and-swap write-back (T13; until
//! then [`credential`] keeps a rotated copy and blocks the credential), the two-uid supervisor (T1).

mod credential;
mod stream;

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};

use chrono::Utc;
use contracts::ids::JobId;
use contracts::job::event::JobEvent;
use contracts::job::result::Usage;
use contracts::job::spec::{ModelChoice, ReasoningEffort};
use futures::future::BoxFuture;
use rustix::process::{Pid, Signal, WaitId, WaitIdOptions, kill_process_group, waitid};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};

use credential::JobCredential;
pub use credential::credential_usable;
pub(crate) use stream::{CodexStream, Exit, usage_delta};

use crate::provider::{LlmProvider, ProviderCall, ProviderError, ProviderEvent, ProviderOutcome};
use crate::scrub::Scrubber;

/// `PATH` of the agent environment; never the runner's own.
pub const AGENT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
/// `NO_PROXY` of the agent environment: loopback only, never the MemCan host (T2).
pub const AGENT_NO_PROXY: &str = "localhost,127.0.0.1";
/// File names inside the per-job `CODEX_HOME`.
const CONFIG_FILE: &str = "config.toml";
const SCHEMA_FILE: &str = "output-schema.json";
/// Longest JSONL line kept; a longer one is skipped (an oversized final answer then fails as not JSON).
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
/// Bytes of Codex's stderr kept for the debug log.
const STDERR_TAIL_BYTES: usize = 4096;
/// How often the loop checks whether Codex exited.
const EXIT_POLL: Duration = Duration::from_millis(50);
/// How long stdout may stay open after Codex exited and its group was killed.
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

/// Process groups of running calls, by job.
type Running = Arc<Mutex<HashMap<JobId, Pid>>>;

/// Runs agent calls with the Codex CLI.
#[derive(Debug)]
pub struct CodexProvider {
    settings: CodexSettings,
    running: Running,
}

impl CodexProvider {
    /// A provider with `settings`.
    pub fn new(settings: CodexSettings) -> Self {
        Self {
            settings,
            running: Running::default(),
        }
    }

    async fn run_call(
        &self,
        call: &ProviderCall,
        events: &mpsc::Sender<ProviderEvent>,
    ) -> Result<ProviderOutcome, ProviderError> {
        let known = Scrubber::new(
            call.access
                .github
                .iter()
                .map(|token| token.token.expose())
                .chain(
                    call.access
                        .memcan
                        .iter()
                        .map(|memcan| memcan.token.expose()),
                ),
        );
        // Dropped after the call (also when the call is dropped): keeps a rotated credential.
        let mut credential = JobCredential::install(
            &self.settings.master_auth,
            &call.codex_home,
            call.job_id,
            known,
        )?;
        self.prepare(call).await?;
        let result = self.exec(call, &mut credential, events).await;
        credential.settle(matches!(result, Err(ProviderError::CredentialUnusable)));
        let scrubber = credential.scrubber();
        let finished = result.map_err(|error| scrub_error(error, scrubber))?;
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

    /// Writes the lean config and output schema into the per-job `CODEX_HOME` and creates the agent's home and
    /// temp directories inside the job directory.
    async fn prepare(&self, call: &ProviderCall) -> Result<(), ProviderError> {
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
        Ok(())
    }

    /// The `codex exec` command: fixed flags and paths only (no secret, no prompt in argv), cwd = job dir
    /// above the checkout (T5), environment built from scratch (T2), its own process group.
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
            .process_group(0)
            .kill_on_drop(true);
        command
    }

    async fn exec(
        &self,
        call: &ProviderCall,
        credential: &mut JobCredential,
        events: &mpsc::Sender<ProviderEvent>,
    ) -> Result<stream::Finished, ProviderError> {
        let mut child = self.command(call).spawn().map_err(|error| {
            tracing::error!(binary = %self.settings.binary.display(), %error, "cannot start Codex; check SR_RUNNER_CODEX_BIN");
            ProviderError::Io(error)
        })?;
        let mut group = ProcessGroup::register(&self.running, call.job_id, child.id());
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
        let status = read_until_exit(
            &mut child,
            &mut group,
            &mut lines,
            &mut parsed,
            credential,
            events,
        )
        .await?;
        drop(group);
        if let Some(stdin) = stdin {
            let _ = stdin.await;
        }
        if let Some(stderr) = stderr
            && let Ok(tail) = stderr.await
            && !tail.is_empty()
            && credential.watch()
        {
            tracing::debug!(job = %call.job_id, stderr = %credential.scrubber().text(&tail), "Codex stderr tail");
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

    /// SIGTERM to the call's process group; Codex then exits with 143 and the call ends as cancelled.
    fn cancel(&self, job: JobId) -> BoxFuture<'_, Result<(), ProviderError>> {
        // Signalled under the lock that unregisters the group before its leader is reaped, so the id is ours.
        let running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        let result = match running.get(&job) {
            Some(pid) => {
                signal_group(*pid, Signal::TERM).map_err(|error| ProviderError::Io(error.into()))
            }
            None => Ok(()),
        };
        drop(running);
        Box::pin(async move { result })
    }
}

/// The process group of a running call, registered for `cancel`. Signalled only while its leader is not
/// reaped (a zombie leader keeps the id reserved): [`ProcessGroup::kill_and_disarm`] before reaping, SIGKILL on
/// drop while still armed.
struct ProcessGroup {
    running: Running,
    job: JobId,
    pid: Option<Pid>,
}

impl ProcessGroup {
    fn register(running: &Running, job: JobId, pid: Option<u32>) -> Self {
        let pid = pid
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(Pid::from_raw);
        if let Some(pid) = pid {
            running
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(job, pid);
        }
        Self {
            running: Arc::clone(running),
            job,
            pid,
        }
    }

    /// Whether the leader exited; it stays unreaped (`WNOWAIT`), so the group id is still ours.
    fn leader_exited(&self) -> bool {
        let Some(pid) = self.pid else {
            return true;
        };
        let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
        // Only this provider reaps the leader (`Child::wait`), so an error cannot mean "reaped by someone else".
        !matches!(waitid(WaitId::Pid(pid), options), Ok(None))
    }

    /// SIGKILL to every process left in the group, then unregister; call before reaping the leader.
    fn kill_and_disarm(&mut self) {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(pid) = self.pid.take()
            && let Err(error) = signal_group(pid, Signal::KILL)
        {
            tracing::warn!(job = %self.job, %error, "cannot kill the agent's process group");
        }
        running.remove(&self.job);
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill_and_disarm();
    }
}

/// Signals a process group; an empty group is not an error.
fn signal_group(pid: Pid, signal: Signal) -> Result<(), rustix::io::Errno> {
    match kill_process_group(pid, signal) {
        Err(rustix::io::Errno::SRCH) => Ok(()),
        other => other,
    }
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

/// Reads events until Codex exits; kills what is left of its group while the exited leader is not yet reaped
/// (so the group id cannot have been reused), then reaps it and drains stdout briefly.
async fn read_until_exit<R: AsyncBufRead + Unpin>(
    child: &mut Child,
    group: &mut ProcessGroup,
    lines: &mut Lines<R>,
    parsed: &mut CodexStream,
    credential: &mut JobCredential,
    events: &mpsc::Sender<ProviderEvent>,
) -> Result<ExitStatus, ProviderError> {
    let mut exit_poll = tokio::time::interval(EXIT_POLL);
    let mut open = true;
    loop {
        tokio::select! {
            line = lines.next(), if open => match line? {
                Some(line) => forward(parsed, &line, credential, events).await?,
                None => open = false,
            },
            _ = exit_poll.tick() => if group.leader_exited() {
                break;
            },
        }
    }
    group.kill_and_disarm();
    let status = child.wait().await?;
    let drain = async {
        while let Some(line) = lines.next().await? {
            forward(parsed, &line, credential, events).await?;
        }
        Ok::<(), ProviderError>(())
    };
    match timeout(DRAIN_AFTER_EXIT, drain).await {
        Ok(result) => result?,
        Err(_) => {
            tracing::warn!(
                "Codex exited but its output stayed open; a process outside its group holds it"
            )
        }
    }
    Ok(status)
}

/// Forwards the activity of `line` with every secret the provider knows scrubbed, the tokens of a credential
/// rotated meanwhile included. Not clipped: `execute` clips after scrubbing the secrets only it knows. Fails
/// when the credential copy was rewritten more often than a refresh does.
async fn forward(
    parsed: &mut CodexStream,
    line: &str,
    credential: &mut JobCredential,
    events: &mpsc::Sender<ProviderEvent>,
) -> Result<(), ProviderError> {
    if let Some((activity, summary)) = parsed.line(line) {
        if !credential.watch() {
            return Err(ProviderError::CredentialUnusable);
        }
        let event = JobEvent {
            seq: 0,
            at: Utc::now(),
            activity,
            summary: credential.scrubber().text(&summary),
        };
        // A closed receiver means execute stopped listening; Codex keeps running until it is stopped.
        let _ = events.send(ProviderEvent::Activity(event)).await;
    }
    Ok(())
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
