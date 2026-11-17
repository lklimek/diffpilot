//! Codex as the agent uid through the supervisor (ADR-013, T1a/b/d, R1). The provider keeps working on a
//! runner-private staging `CODEX_HOME` (`ProviderCall::codex_home`); the agent's own `CODEX_HOME`,
//! `HOME` and `TMPDIR` are handed over empty and filled with `copy-in`. After every Codex run: SIGTERM, grace,
//! `sweep`, then `copy-out` of `auth.json` and this analysis's rollout into the staging home, where the
//! credential (T13) and session (T8b) code validate them as before. At the end the agent's directories,
//! `/dev/mqueue` and System V IPC are emptied and the directories removed. Helpers run only after a sweep or
//! before Codex starts, so no agent process can interfere with them.

use std::ffi::OsString;
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};

use rustix::process::Pid;
use tokio::io::BufReader;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, timeout};

use super::credential::JobCredential;
use super::{
    AGENT_PATH, CONFIG_FILE, CodexProvider, CodexStream, DRAIN_AFTER_EXIT, EXIT_POLL, Exit,
    Invocation, Lines, SKILLS_ROOT, agent_home, agent_tmp, forward, replace_private, session,
    stderr_tail,
};
use crate::provider::{ProviderCall, ProviderError, ProviderEvent};
use crate::supervisor::client::{Supervisor, SupervisorError};

/// The supervisor and where the agents' `CODEX_HOME`s live.
#[derive(Clone, Debug)]
pub struct Isolation {
    /// The supervisor connection.
    pub supervisor: Arc<Supervisor>,
    /// `<state>/codex-homes`: the bounded filesystem holding the per-job agent `CODEX_HOME`s.
    pub codex_homes: PathBuf,
}

const AUTH_FILE: &str = "auth.json";
/// Largest `auth.json` copied back.
const MAX_AUTH_BYTES: usize = 64 * 1024;
/// How long the supervisor may take to report a swept Codex's status.
const REAP_DEADLINE: Duration = Duration::from_secs(10);
/// Time before the runtime cap at which a running Codex is stopped, so the copy-back fits in.
const STOP_MARGIN: Duration = Duration::from_secs(5);

fn io_error(error: SupervisorError) -> ProviderError {
    ProviderError::Io(io::Error::other(error))
}

impl Isolation {
    /// The agent's `CODEX_HOME` for the staging home `staging`.
    pub(super) fn codex_home(&self, staging: &Path) -> PathBuf {
        self.codex_homes
            .join(staging.file_name().unwrap_or_default())
    }

    /// Start-up (ADR-013 step 1): sweep, then for every leftover agent `CODEX_HOME` copy `auth.json` into the
    /// staging home of the same key (where credential recovery finds it), clear and remove it; the same for
    /// the agent directories of leftover job directories. Returns how many directories were removed.
    pub async fn recover(&self, staging: &Path, work: &Path) -> Result<usize, ProviderError> {
        self.supervisor.sweep().await.map_err(io_error)?;
        let mut removed = 0;
        for entry in std::fs::read_dir(&self.codex_homes)? {
            let home = entry?.path();
            let key = home.file_name().unwrap_or_default().to_owned();
            if staging
                .join(format!("{}.cas", key.to_string_lossy()))
                .exists()
            {
                match self
                    .supervisor
                    .copy_out(&home, Path::new(AUTH_FILE), MAX_AUTH_BYTES)
                    .await
                {
                    Ok(bytes) => {
                        let target = staging.join(&key);
                        create_dir(&target)?;
                        replace_private(&target.join(AUTH_FILE), &bytes)?;
                    }
                    Err(error) => {
                        tracing::warn!(dir = %home.display(), %error, "no credential copy to recover from a leftover agent CODEX_HOME")
                    }
                }
            }
            self.remove(&home).await;
            removed += 1;
        }
        if let Ok(entries) = std::fs::read_dir(work) {
            for entry in entries {
                let job = entry?.path();
                for dir in [agent_home(&job), agent_tmp(&job)] {
                    if dir.exists() {
                        self.remove(&dir).await;
                        removed += 1;
                    }
                }
            }
        }
        Ok(removed)
    }

    /// Prepare (ADR-013 step 2): hands the agent its three directories (job directory 0755, checkout
    /// read-only) and copies the staging home and the job's skills in.
    pub(super) async fn enter(
        &self,
        call: &ProviderCall,
        skills: &[(PathBuf, Vec<u8>)],
    ) -> Result<(), ProviderError> {
        let home = self.codex_home(&call.codex_home);
        if home.exists() {
            // An earlier run of this key left it.
            self.remove(&home).await;
        }
        create_dir(&home)?;
        std::fs::set_permissions(&call.job_dir, std::fs::Permissions::from_mode(0o755))?;
        for dir in [&home, &agent_home(&call.job_dir), &agent_tmp(&call.job_dir)] {
            self.supervisor.handover(dir).await.map_err(io_error)?;
        }
        for rel in staging_files(&call.codex_home)? {
            let contents = std::fs::read(call.codex_home.join(&rel))?;
            self.supervisor
                .copy_in(&home, &rel, &contents)
                .await
                .map_err(io_error)?;
        }
        let skills_root = agent_home(&call.job_dir).join(SKILLS_ROOT);
        for (rel, contents) in skills {
            let target = skills_root.join(rel);
            let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
                continue;
            };
            let relative = parent
                .strip_prefix(agent_home(&call.job_dir))
                .unwrap_or(parent)
                .join(name);
            self.supervisor
                .copy_in(&agent_home(&call.job_dir), &relative, contents)
                .await
                .map_err(io_error)?;
        }
        Ok(())
    }

    /// Copies the regenerated `config.toml` in (before the compaction turn, T1d).
    pub(super) async fn push_config(&self, call: &ProviderCall) -> Result<(), ProviderError> {
        let contents = std::fs::read(call.codex_home.join(CONFIG_FILE))?;
        self.supervisor
            .copy_in(
                &self.codex_home(&call.codex_home),
                Path::new(CONFIG_FILE),
                &contents,
            )
            .await
            .map_err(io_error)
    }

    /// Copy-back after a sweep (ADR-013 step 5): `auth.json` and the rollout of `thread` into the staging home.
    /// A missing or unsafe `auth.json` leaves the staged copy as it was.
    pub(super) async fn pull(
        &self,
        call: &ProviderCall,
        thread: Option<&str>,
    ) -> Result<(), ProviderError> {
        let home = self.codex_home(&call.codex_home);
        match self
            .supervisor
            .copy_out(&home, Path::new(AUTH_FILE), MAX_AUTH_BYTES)
            .await
        {
            Ok(bytes) => replace_private(&call.codex_home.join(AUTH_FILE), &bytes)?,
            Err(error) => {
                tracing::warn!(job = %call.job_id, %error, "the agent's auth.json could not be copied back");
            }
        }
        let Some(thread) = thread.filter(|thread| session::valid_thread_id(thread)) else {
            return Ok(());
        };
        let suffix = format!("-{thread}.jsonl");
        for entry in self.supervisor.list(&home).await.map_err(io_error)? {
            let parts: Vec<&str> = entry.path.split('/').collect();
            let rollout = matches!(parts.as_slice(), [sessions, y, m, d, name]
                if *sessions == session::SESSIONS_DIR
                    && [y, m, d].iter().all(|part| part.bytes().all(|b| b.is_ascii_digit()))
                    && name.starts_with("rollout-")
                    && name.ends_with(&suffix));
            if !rollout || !entry.file || entry.size > session::MAX_ROLLOUT_BYTES {
                continue;
            }
            let max = usize::try_from(session::MAX_ROLLOUT_BYTES).unwrap_or(usize::MAX);
            let bytes = self
                .supervisor
                .copy_out(&home, Path::new(&entry.path), max)
                .await
                .map_err(io_error)?;
            let target = call.codex_home.join(&entry.path);
            if let Some(parent) = target.parent() {
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)?;
            }
            replace_private(&target, &bytes)?;
        }
        Ok(())
    }

    /// The end of a call (ADR-013 step 5): sweep, empty and remove the agent's directories, `/dev/mqueue` and
    /// System V IPC. Best effort: failures are logged (a later start-up recovers).
    pub(super) async fn leave(&self, call: &ProviderCall) {
        if let Err(error) = self.supervisor.sweep().await {
            tracing::error!(job = %call.job_id, %error, "the final sweep of a call failed");
            return;
        }
        for dir in [
            self.codex_home(&call.codex_home),
            agent_home(&call.job_dir),
            agent_tmp(&call.job_dir),
        ] {
            self.remove(&dir).await;
        }
        if Path::new("/dev/mqueue").is_dir()
            && let Err(error) = self.supervisor.clear(Path::new("/dev/mqueue")).await
        {
            tracing::warn!(%error, "cannot empty /dev/mqueue");
        }
        if Path::new("/usr/bin/ipcrm").exists()
            && let Err(error) = self
                .supervisor
                .run_agent_tool(&["/usr/bin/ipcrm", "--all"])
                .await
        {
            tracing::warn!(%error, "cannot remove the agent's System V IPC objects");
        }
    }

    /// `clear` as the agent, then `rmdir` as the runner (the parent is the runner's).
    async fn remove(&self, dir: &Path) {
        if let Err(error) = self.supervisor.clear(dir).await {
            tracing::warn!(dir = %dir.display(), %error, "cannot empty an agent directory");
        }
        match std::fs::remove_dir(dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(dir = %dir.display(), %error, "cannot remove an agent directory")
            }
        }
    }
}

impl CodexProvider {
    /// Runs one Codex invocation as the agent (ADR-013 steps 3–5): spawn through the supervisor, read events
    /// until it exits; on cancel or the runtime cap SIGTERM, then after the grace a sweep. Always ends with a
    /// sweep and the copy-back. Every wait is bounded: the runtime cap, then the grace, then the sweep.
    #[expect(
        clippy::too_many_arguments,
        reason = "the dev path's arguments plus the isolation and the cap"
    )]
    pub(super) async fn exec_isolated(
        &self,
        isolation: &Isolation,
        call: &ProviderCall,
        invocation: Invocation<'_>,
        prompt: &str,
        credential: &mut JobCredential<'_>,
        events: Option<&mpsc::Sender<ProviderEvent>>,
        deadline: Instant,
    ) -> Result<(CodexStream, Exit), ProviderError> {
        let supervisor = &isolation.supervisor;
        let mut argv = vec![absolute_binary(&self.settings.binary)?];
        argv.extend(self.codex_args(call, invocation).into_iter().map(lossy));
        let env = super::agent_env(
            call,
            &isolation.codex_home(&call.codex_home),
            self.settings.egress_proxy.as_deref(),
        )
        .into_iter()
        .map(|(key, value)| format!("{key}={}", value.to_string_lossy()))
        .collect();
        let process = supervisor
            .spawn(argv, env, &call.job_dir)
            .await
            .map_err(io_error)?;
        let pid = process.pid;
        self.set_group(call, Pid::from_raw(pid));
        let mut stdin = process.stdin;
        let prompt = prompt.to_owned();
        let writer = tokio::spawn(async move {
            // A Codex that exits early closes stdin; its exit status tells why.
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stdin, prompt.as_bytes()).await;
        });
        let stderr = tokio::spawn(stderr_tail(process.stderr));
        let mut lines = Lines::new(BufReader::new(process.stdout));
        let mut parsed = CodexStream::default();
        let stop_at = deadline
            .checked_sub(self.settings.term_grace + STOP_MARGIN)
            .unwrap_or_else(Instant::now);
        let mut timed_out = false;
        let mut stopping_since: Option<Instant> = None;
        let mut swept = false;
        let outcome = async {
            let wait = supervisor.wait(pid);
            tokio::pin!(wait);
            let mut tick = tokio::time::interval(EXIT_POLL);
            let mut open = true;
            loop {
                tokio::select! {
                    line = lines.next(), if open => match line? {
                        Some(line) => forward(&mut parsed, &line, credential, events).await?,
                        None => open = false,
                    },
                    status = &mut wait => return status.map_err(io_error),
                    _ = tick.tick() => {
                        if !timed_out && Instant::now() >= stop_at {
                            timed_out = true;
                            tracing::warn!(job = %call.job_id, "the runtime cap is near; stopping Codex");
                            stopping_since = Some(Instant::now());
                            supervisor.signal_now(pid, 15);
                        }
                        let since = stopping_since.or_else(|| self.cancelled_at(call));
                        if !swept && since.is_some_and(|at| at.elapsed() >= self.settings.term_grace) {
                            tracing::warn!(job = %call.job_id, "Codex did not stop within the grace after SIGTERM; sweeping");
                            swept = true;
                            supervisor.sweep().await.map_err(io_error)?;
                        }
                    }
                }
            }
        }
        .await;
        // Codex has exited (or failed): nothing of the agent may outlive the run.
        let sweep = supervisor.sweep().await;
        self.set_group(call, None);
        writer.abort();
        let drain = async {
            while let Some(line) = lines.next().await? {
                forward(&mut parsed, &line, credential, events).await?;
            }
            Ok::<(), ProviderError>(())
        };
        if timeout(DRAIN_AFTER_EXIT, drain).await.is_err() {
            tracing::warn!(job = %call.job_id, "Codex output stayed open after the sweep");
        }
        let status = match outcome {
            Ok(status) => status,
            Err(error) => {
                // Collect the swept Codex's status so the supervisor forgets it.
                let _ = timeout(REAP_DEADLINE, supervisor.wait(pid)).await;
                return Err(error);
            }
        };
        sweep.map_err(io_error)?;
        if let Ok(Ok(tail)) = timeout(DRAIN_AFTER_EXIT, stderr).await
            && !tail.is_empty()
            && credential.watch()
        {
            tracing::debug!(job = %call.job_id, stderr = %credential.scrubber().text(&tail), "Codex stderr tail");
        }
        let thread = parsed.thread_id().map(str::to_owned).or(match invocation {
            Invocation::Resume(thread) | Invocation::Compaction(thread) => Some(thread.to_owned()),
            Invocation::New => None,
        });
        isolation.pull(call, thread.as_deref()).await?;
        if timed_out {
            return Err(ProviderError::Timeout);
        }
        Ok((parsed, exit_of_code(status)))
    }

    fn set_group(&self, call: &ProviderCall, pid: Option<Pid>) {
        if let Some(running) = self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&call.job_id)
        {
            running.group = pid;
        }
    }

    fn cancelled_at(&self, call: &ProviderCall) -> Option<Instant> {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&call.job_id)
            .and_then(|running| running.cancelled_at)
    }
}

/// The supervisor's status: an exit code, or 128 + signal.
fn exit_of_code(status: i32) -> Exit {
    match status {
        129..=192 => Exit::Signal(status - 128),
        code => Exit::Code(code),
    }
}

/// The Codex binary as an absolute path (the supervisor runs only absolute programs); a bare name is looked
/// up in [`AGENT_PATH`].
fn absolute_binary(binary: &Path) -> Result<String, ProviderError> {
    if binary.is_absolute() {
        return Ok(binary.to_string_lossy().into_owned());
    }
    AGENT_PATH
        .split(':')
        .map(|dir| Path::new(dir).join(binary))
        .find(|candidate| candidate.is_file())
        .map(|found| found.to_string_lossy().into_owned())
        .ok_or_else(|| {
            tracing::error!(binary = %binary.display(), "cannot find Codex; check SR_RUNNER_CODEX_BIN");
            ProviderError::Io(io::Error::from(io::ErrorKind::NotFound))
        })
}

fn lossy(value: OsString) -> String {
    value.to_string_lossy().into_owned()
}

fn create_dir(dir: &Path) -> io::Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => Err(error),
        _ => Ok(()),
    }
}

/// The regular files below the staging home, relative to it (links and temp files skipped).
fn staging_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    fn walk(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
        for entry in std::fs::read_dir(root.join(rel))? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = rel.join(entry.file_name());
            if kind.is_dir() {
                walk(root, &path, out)?;
            } else if kind.is_file() && path.extension().is_none_or(|ext| ext != "tmp") {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, Path::new(""), &mut out)?;
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supervisor_status_maps_signals() {
        assert!(matches!(exit_of_code(143), Exit::Signal(15)));
        assert!(matches!(exit_of_code(0), Exit::Code(0)));
        assert!(matches!(exit_of_code(1), Exit::Code(1)));
    }

    #[test]
    fn staging_files_are_regular_files_only() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("sessions/2026/11/17")).unwrap();
        std::fs::write(dir.path().join("sessions/2026/11/17/rollout-x.jsonl"), "x").unwrap();
        std::fs::write(dir.path().join("config.toml"), "x").unwrap();
        std::fs::write(dir.path().join("config.toml.tmp"), "x").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", dir.path().join("link")).unwrap();
        assert_eq!(
            staging_files(dir.path()).unwrap(),
            [
                PathBuf::from("config.toml"),
                PathBuf::from("sessions/2026/11/17/rollout-x.jsonl")
            ]
        );
    }
}
