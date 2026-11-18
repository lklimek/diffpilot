//! The Codex CLI provider: `codex exec --json` (new session) or `codex exec resume` (the
//! pull request's session) in the job directory with a per-job `CODEX_HOME` and a constructed environment (S1a,
//! S1b, S6 env contract, threat model T2/T5), saving every successful useful turn (ADR-004).
//!
//! Codex runs as the leader of its own process group; the group is killed when a turn ends, is cancelled
//! (SIGTERM, SIGKILL after the grace) or dropped, so agent commands left in the background do not outlive it.
//! The credential copy is written back with compare-and-swap after Codex exited ([`credential`], T13); the
//! session's rollout is copied in before a resume and back after the job ([`session`], T8).

mod config;
mod credential;
mod isolated;
mod session;
mod stream;

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};

use chrono::Utc;
use contracts::ids::JobId;
use contracts::job::event::JobEvent;
use contracts::job::result::Usage;
use futures::future::BoxFuture;
use rustix::process::{Pid, Signal, WaitId, WaitIdOptions, kill_process_group, waitid};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, timeout};

pub use credential::CodexCredential;
use credential::JobCredential;
pub use isolated::Isolation;
pub(crate) use stream::{CodexStream, Exit, usage_delta};

use crate::provider::{LlmProvider, ProviderCall, ProviderError, ProviderEvent, ProviderOutcome};
use crate::scrub::Scrubber;
use crate::skills::Skills;

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
/// Skill files relative to the skills root, with their contents.
type SkillFileList = Vec<(PathBuf, Vec<u8>)>;
/// Where Codex discovers user skills, below the agent's `HOME` (S2).
const SKILLS_ROOT: &str = ".agents/skills";
/// Where the Codex provider finds its binary, credential and egress proxy.
#[derive(Clone, Debug)]
pub struct CodexSettings {
    /// The `codex` binary; a bare name is resolved through [`AGENT_PATH`].
    pub binary: PathBuf,
    /// The runner's Codex credential (master in the runner store; copied into each job, written back after).
    pub credential: Arc<CodexCredential>,
    /// Explicit egress proxy URL for the agent (`http://egress-proxy:3128`), if any.
    pub egress_proxy: Option<String>,
    /// How long Codex may take to stop after SIGTERM before its group is killed.
    pub term_grace: Duration,
    /// The `github-mcp-server` binary started by Codex for jobs with an installation token.
    pub github_mcp: PathBuf,
    /// The claudius skills given to jobs that ask for them; `None` gives none.
    pub skills: Option<Arc<Skills>>,
    /// Codex as the agent uid through the supervisor (ADR-013); `None` runs it as the runner's own uid
    /// (development only).
    pub isolation: Option<Isolation>,
}

/// A running call: its current process group and when it was asked to stop.
#[derive(Debug, Default)]
struct RunningCall {
    group: Option<Pid>,
    cancelled_at: Option<Instant>,
}

/// Running calls, by job.
type Running = Arc<Mutex<HashMap<JobId, RunningCall>>>;

/// Runs agent calls with the Codex CLI.
#[derive(Debug)]
pub struct CodexProvider {
    settings: CodexSettings,
    running: Running,
}

/// Which `codex exec` invocation.
#[derive(Clone, Copy, Debug)]
enum Invocation<'a> {
    /// A new session.
    New,
    /// The job turn in the pull request's session.
    Resume(&'a str),
}

impl CodexProvider {
    /// A provider with `settings`.
    pub fn new(settings: CodexSettings) -> Self {
        Self {
            settings,
            running: Running::default(),
        }
    }

    /// One agent call: the job turn (new or resumed session), the ready output and session copy-back.
    async fn call(
        &self,
        resume: Option<&str>,
        call: &ProviderCall,
        events: &mpsc::Sender<ProviderEvent>,
    ) -> Result<ProviderOutcome, ProviderError> {
        let result = match &self.settings.isolation {
            Some(isolation) if !isolation.healthy() => {
                tracing::error!(job = %call.job_id, "an earlier agent directory stays reachable by the agent; the job is refused");
                return Err(ProviderError::Io(io::Error::other(
                    "process isolation is compromised by a leftover agent directory",
                )));
            }
            Some(_) => self.call_steps(resume, call, events).await,
            None => {
                tracing::warn!(job = %call.job_id, "process isolation off (development): Codex runs as the runner's uid");
                self.call_steps(resume, call, events).await
            }
        };
        if let Some(isolation) = &self.settings.isolation {
            isolation.leave(call).await;
        }
        result
    }

    async fn call_steps(
        &self,
        resume: Option<&str>,
        call: &ProviderCall,
        events: &mpsc::Sender<ProviderEvent>,
    ) -> Result<ProviderOutcome, ProviderError> {
        let deadline =
            Instant::now() + Duration::from_secs(u64::from(call.limits.max_runtime_secs));
        let _registration = Registration::new(&self.running, call.job_id);
        if self.cancel_requested(call.job_id) {
            return Err(ProviderError::Cancelled);
        }
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
        // Dropped after the call (also when the call is dropped): writes a refreshed credential back.
        let mut credential = JobCredential::install(
            &self.settings.credential,
            &call.codex_home,
            call.job_id,
            known,
        )?;
        let before = match resume {
            Some(thread) => session::restore(&call.restore_dir, thread, &call.codex_home)?,
            None => Usage::default(),
        };
        let (skills_commit, skill_files) = self.prepare(call).await?;
        if let Some(isolation) = &self.settings.isolation {
            isolation.enter(call, &skill_files).await?;
        }
        tracing::info!(job = %call.job_id, skills = ?call.skills, "job agent skills available");
        let invocation = resume.map_or(Invocation::New, Invocation::Resume);
        let prompt = if resume.is_none() && !call.skills.is_empty() {
            let files = call
                .skills
                .iter()
                .map(|name| format!("$HOME/{SKILLS_ROOT}/{name}/SKILL.md"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "Read the required skills once for this new session: {files}. Apply them throughout this session.\n\n{}",
                call.prompt
            )
        } else {
            call.prompt.clone()
        };
        let (stream, exit) = self
            .exec(
                call,
                invocation,
                &prompt,
                &mut credential,
                Some(events),
                deadline,
            )
            .await?;
        let started = stream.thread_id().is_some();
        let thread = stream.thread_id().or(resume).map(str::to_owned);
        let result = if call.flow_file.is_some() {
            match stream.finish_file(exit) {
                Ok(mut finished) => {
                    let text = match &self.settings.isolation {
                        Some(isolation) => isolation.collect_flow(call).await,
                        None => super::output_file::collect(&call.job_dir),
                    };
                    text.map(|text| {
                        finished.output = serde_json::Value::String(text);
                        finished
                    })
                }
                Err(error) => Err(error),
            }
        } else {
            stream.finish(exit)
        };
        let result = match result {
            _ if self.cancel_requested(call.job_id) => Err(ProviderError::Cancelled),
            Err(ProviderError::UsageLimit { .. }) => Err(ProviderError::UsageLimit {
                resets_at: thread
                    .as_deref()
                    .and_then(|thread| session::usage_limit_resets_at(&call.codex_home, thread)),
            }),
            Err(
                ProviderError::Failed { .. }
                | ProviderError::SchemaRejected { .. }
                | ProviderError::OutputNotJson,
            ) if resume.is_some() && !started => {
                tracing::warn!(job = %call.job_id, "Codex could not resume the pull request's session");
                Err(ProviderError::SessionUnavailable)
            }
            other => other,
        };
        let finished = match result {
            Ok(finished) => finished,
            Err(error) => {
                let refresh_failed = matches!(error, ProviderError::CredentialUnusable);
                if !credential.settle(refresh_failed) {
                    // Rotated tokens may be missing from the scrubber: no agent text leaves.
                    return Err(ProviderError::CredentialUnusable);
                }
                return Err(scrub_error(error, credential.scrubber()));
            }
        };
        if !credential.rescan() {
            credential.settle(false);
            return Err(ProviderError::CredentialUnusable);
        }
        let output = credential.scrubber().value(finished.output);
        // Execute may have stopped listening (job fenced); the outcome still carries the output.
        let _ = events
            .send(ProviderEvent::OutputReady(output.clone()))
            .await;
        let certain = credential.settle(false);
        let mut session_turns = None;
        let session_id = match thread
            .as_ref()
            .filter(|thread| session::valid_thread_id(thread))
        {
            Some(thread) if certain => {
                match session::save(
                    &call.codex_home,
                    thread,
                    &call.session_dir,
                    credential.scrubber(),
                ) {
                    Ok(turns) => {
                        session_turns = turns;
                        Some(thread.clone())
                    }
                    Err(error) => {
                        tracing::warn!(job = %call.job_id, %error, "the agent session could not be stored; the next job starts a new one");
                        None
                    }
                }
            }
            _ => None,
        };
        if session_id.is_none() {
            session::forget(&call.session_dir);
        }
        Ok(ProviderOutcome {
            output,
            session_id,
            session_turns,
            usage: usage_delta(before, finished.cumulative_usage),
            skills_commit,
        })
    }

    fn cancel_requested(&self, job: JobId) -> bool {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&job)
            .is_some_and(|running| running.cancelled_at.is_some())
    }

    /// Writes the config and output schema into the per-job `CODEX_HOME`, creates the agent's home and temp
    /// directories inside the job directory and copies the job's skills to `$HOME/.agents/skills` (S2; when
    /// isolated, the files are returned for `copy-in` instead). Returns the claudius commit of the skills given.
    async fn prepare(
        &self,
        call: &ProviderCall,
    ) -> Result<(Option<String>, SkillFileList), ProviderError> {
        self.write_config(call)?;
        if call.flow_file.is_none() {
            let schema = serde_json::to_vec(&call.output_schema).map_err(io::Error::other)?;
            replace_private(&call.codex_home.join(SCHEMA_FILE), &schema)?;
        }
        for dir in [agent_home(&call.job_dir), agent_tmp(&call.job_dir)] {
            match create_private_dir(&dir).await {
                Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
                    return Err(error.into());
                }
                _ => {}
            }
        }
        if call.skills.is_empty() {
            return Ok((None, Vec::new()));
        }
        let skills = self
            .settings
            .skills
            .as_ref()
            .ok_or(crate::skills::SkillError::Unavailable)?;
        let given = skills.required_files(&call.skills)?;
        if self.settings.isolation.is_some() {
            return Ok((Some(given.commit), given.files));
        }
        let root = agent_home(&call.job_dir).join(SKILLS_ROOT);
        for (rel, contents) in &given.files {
            let path = root.join(rel);
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path.parent().unwrap_or(&root))?;
            replace_private(&path, contents)?;
        }
        Ok((Some(given.commit), Vec::new()))
    }

    /// (Re)generates `config.toml`, replacing whatever the agent may have left there.
    fn write_config(&self, call: &ProviderCall) -> io::Result<()> {
        let config = config::config_toml(
            &call.model,
            &call.access,
            &self.settings.github_mcp,
            AGENT_NO_PROXY,
        );
        replace_private(&call.codex_home.join(CONFIG_FILE), config.as_bytes())
    }

    /// The `codex exec` command: fixed flags and paths only (no secret, no prompt in argv), cwd = job dir
    /// above the checkout (T5), environment built from scratch (T2), its own process group.
    fn command(&self, call: &ProviderCall, invocation: Invocation<'_>) -> Command {
        let mut command = Command::new(&self.settings.binary);
        command
            .args(self.codex_args(call, invocation))
            .current_dir(&call.job_dir)
            .env_clear()
            .envs(agent_env(
                call,
                &call.codex_home,
                self.settings.egress_proxy.as_deref(),
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        command
    }

    /// The arguments after the binary: fixed flags and paths only.
    fn codex_args(&self, call: &ProviderCall, invocation: Invocation<'_>) -> Vec<OsString> {
        let codex_home = match &self.settings.isolation {
            Some(isolation) => isolation.codex_home(&call.codex_home),
            None => call.codex_home.clone(),
        };
        let mut args: Vec<OsString> = vec!["exec".into()];
        if let Invocation::Resume(_) = invocation {
            args.push("resume".into());
        }
        args.extend(["--json", "--skip-git-repo-check", "--ignore-rules"].map(OsString::from));
        if call.flow_file.is_none() {
            args.push("--output-schema".into());
            args.push(codex_home.join(SCHEMA_FILE).into());
        }
        if let Invocation::Resume(thread) = invocation {
            args.push(thread.into());
        }
        args.push("-".into());
        args
    }

    /// Runs one Codex invocation to its exit; `events` gets its activities.
    async fn exec(
        &self,
        call: &ProviderCall,
        invocation: Invocation<'_>,
        prompt: &str,
        credential: &mut JobCredential<'_>,
        events: Option<&mpsc::Sender<ProviderEvent>>,
        deadline: Instant,
    ) -> Result<(CodexStream, Exit), ProviderError> {
        if let Some(isolation) = &self.settings.isolation {
            return self
                .exec_isolated(
                    isolation, call, invocation, prompt, credential, events, deadline,
                )
                .await;
        }
        let mut child = self.command(call, invocation).spawn().map_err(|error| {
            tracing::error!(binary = %self.settings.binary.display(), %error, "cannot start Codex; check SR_RUNNER_CODEX_BIN");
            ProviderError::Io(error)
        })?;
        let mut group = ProcessGroup::register(&self.running, call.job_id, child.id());
        let stdin = child.stdin.take().map(|mut stdin| {
            let prompt = prompt.to_owned();
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
            self.settings.term_grace,
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
            tracing::debug!(job = %call.job_id, stderr = %credential.scrubber().text_and_token_shapes(&tail), "Codex stderr tail");
        }
        if parsed.unparsed_lines() > 0 || lines.skipped > 0 {
            tracing::warn!(
                job = %call.job_id,
                unparsed = parsed.unparsed_lines(),
                oversized = lines.skipped,
                "Codex printed lines that are not JSON events or exceed the line cap; they were skipped"
            );
        }
        Ok((parsed, exit_of(status)))
    }
}

impl LlmProvider for CodexProvider {
    fn run<'a>(
        &'a self,
        call: &'a ProviderCall,
        events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        Box::pin(async move { self.call(None, call, &events).await })
    }

    fn resume<'a>(
        &'a self,
        session_id: &'a str,
        call: &'a ProviderCall,
        events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        Box::pin(async move { self.call(Some(session_id), call, &events).await })
    }

    /// SIGTERM to the call's process group; Codex then exits with 143 and the call ends as cancelled. A Codex
    /// still running after the grace is killed; no new turn starts after a cancel.
    fn cancel(&self, job: JobId) -> BoxFuture<'_, Result<(), ProviderError>> {
        // Signalled under the lock that unregisters the group before its leader is reaped, so the id is ours.
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        let result = match running.get_mut(&job) {
            Some(call) => {
                call.cancelled_at.get_or_insert_with(Instant::now);
                match (call.group, &self.settings.isolation) {
                    (Some(pid), Some(isolation)) => {
                        isolation
                            .supervisor
                            .signal_now(pid.as_raw_nonzero().get(), 15);
                        Ok(())
                    }
                    (Some(pid), None) => signal_group(pid, Signal::TERM)
                        .map_err(|error| ProviderError::Io(error.into())),
                    (None, _) => Ok(()),
                }
            }
            // Under the supervisor a dropped attempt is cancelled before its call may have started.
            None if self.settings.isolation.is_some() => {
                running.insert(
                    job,
                    RunningCall {
                        group: None,
                        cancelled_at: Some(Instant::now()),
                    },
                );
                Ok(())
            }
            None => Ok(()),
        };
        drop(running);
        Box::pin(async move { result })
    }
}

/// Registers a call as running for `cancel`; unregisters it when the call ends or is dropped.
struct Registration {
    running: Running,
    job: JobId,
}

impl Registration {
    fn new(running: &Running, job: JobId) -> Self {
        running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(job)
            .or_default();
        Self {
            running: Arc::clone(running),
            job,
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.job);
    }
}

/// The process group of a running Codex invocation, registered for `cancel`. Signalled only while its leader
/// is not reaped (a zombie leader keeps the id reserved): [`ProcessGroup::kill_and_disarm`] before reaping,
/// SIGKILL on drop while still armed.
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
                .entry(job)
                .or_default()
                .group = Some(pid);
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

    /// Whether a cancel was requested longer than `grace` ago.
    fn grace_over(&self, grace: Duration) -> bool {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&self.job)
            .and_then(|call| call.cancelled_at)
            .is_some_and(|at| at.elapsed() >= grace)
    }

    /// SIGKILL to every process left in the group, then unregister; call before reaping the leader.
    fn kill_and_disarm(&mut self) {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(pid) = self.pid.take()
            && let Err(error) = signal_group(pid, Signal::KILL)
        {
            tracing::warn!(job = %self.job, %error, "cannot kill the agent's process group");
        }
        if let Some(call) = running.get_mut(&self.job) {
            call.group = None;
        }
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
    codex_home: &Path,
    egress_proxy: Option<&str>,
) -> Vec<(&'static str, OsString)> {
    let mut env: Vec<(&'static str, OsString)> = vec![
        ("HOME", agent_home(&call.job_dir).into_os_string()),
        ("PATH", AGENT_PATH.into()),
        ("CODEX_HOME", codex_home.as_os_str().to_owned()),
        ("TMPDIR", agent_tmp(&call.job_dir).into_os_string()),
        ("LANG", "C.UTF-8".into()),
        // The read-only checkout belongs to the runner, not the agent uid. Trust only this job's repo.
        ("GIT_CONFIG_COUNT", "1".into()),
        ("GIT_CONFIG_KEY_0", "safe.directory".into()),
        (
            "GIT_CONFIG_VALUE_0",
            call.job_dir.join("repo").into_os_string(),
        ),
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

/// Replaces `path` with a new private file: an agent-planted symlink at `path` is replaced, never followed.
fn replace_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    match std::fs::remove_file(&temp) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    std::fs::rename(&temp, path)
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
    credential: &mut JobCredential<'_>,
    events: Option<&mpsc::Sender<ProviderEvent>>,
    term_grace: Duration,
) -> Result<ExitStatus, ProviderError> {
    let mut exit_poll = tokio::time::interval(EXIT_POLL);
    let mut open = true;
    loop {
        tokio::select! {
            line = lines.next(), if open => match line? {
                Some(line) => forward(parsed, &line, credential, events).await?,
                None => open = false,
            },
            _ = exit_poll.tick() => {
                if group.leader_exited() {
                    break;
                }
                if group.grace_over(term_grace) {
                    tracing::warn!("Codex did not stop within the grace after SIGTERM; killing its process group");
                    group.kill_and_disarm();
                }
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
    credential: &mut JobCredential<'_>,
    events: Option<&mpsc::Sender<ProviderEvent>>,
) -> Result<(), ProviderError> {
    if let Some((activity, summary)) = parsed.line(line) {
        if !credential.watch() {
            return Err(ProviderError::CredentialUnusable);
        }
        let event = JobEvent {
            seq: 0,
            at: Utc::now(),
            activity,
            summary: credential.scrubber().text_and_token_shapes(&summary),
        };
        // A closed receiver means execute stopped listening; Codex keeps running until it is stopped.
        if let Some(events) = events {
            let _ = events.send(ProviderEvent::Activity(event)).await;
        }
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

async fn stderr_tail(mut stderr: impl tokio::io::AsyncRead + Unpin) -> String {
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
