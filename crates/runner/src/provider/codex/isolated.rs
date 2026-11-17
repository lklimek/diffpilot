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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError};

use rustix::process::Pid;
use tokio::io::BufReader;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, timeout};

use super::credential::{JobCredential, UNKNOWN_SUFFIX};
use super::{
    AGENT_PATH, CodexProvider, CodexStream, DRAIN_AFTER_EXIT, EXIT_POLL, Exit, Invocation, Lines,
    SKILLS_ROOT, agent_home, agent_tmp, forward, replace_private, session, stderr_tail,
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
    /// Set when an agent directory could be neither removed nor quarantined: no further job may run.
    poisoned: Arc<AtomicBool>,
}

/// An agent directory could be neither emptied nor taken away from the agent.
#[derive(Debug, thiserror::Error)]
#[error("the agent directory {} could be neither removed nor quarantined", dir.display())]
pub struct LeftoverError {
    dir: PathBuf,
    #[source]
    source: SupervisorError,
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

fn io_error_of(error: LeftoverError) -> ProviderError {
    ProviderError::Io(io::Error::other(error))
}

impl Isolation {
    /// Isolation through `supervisor`, agent `CODEX_HOME`s below `codex_homes`.
    pub fn new(supervisor: Arc<Supervisor>, codex_homes: PathBuf) -> Self {
        Self {
            supervisor,
            codex_homes,
            poisoned: Arc::default(),
        }
    }

    /// False once an agent directory was left reachable by the agent uid: the runner then takes no job
    /// (reported as an unusable credential until a restart recovers or the owner intervenes).
    pub fn healthy(&self) -> bool {
        !self.poisoned.load(Ordering::SeqCst)
    }

    /// The agent's `CODEX_HOME` for the staging home `staging`.
    pub(super) fn codex_home(&self, staging: &Path) -> PathBuf {
        self.codex_homes
            .join(staging.file_name().unwrap_or_default())
    }

    /// Start-up (ADR-013 step 1): sweep, then for every leftover agent `CODEX_HOME` copy `auth.json` into the
    /// staging home of the same key (where credential recovery finds it), clear and remove it; the same for
    /// the agent directories of leftover job directories. A copy that cannot be brought back leaves the
    /// credential state unknown (`<key>.unknown`, see credential recovery). Whatever cannot be removed is
    /// quarantined and verified (runner-owned, 0700), or start-up stops. Returns how many directories were
    /// handled.
    pub async fn recover(&self, staging: &Path, work: &Path) -> Result<usize, ProviderError> {
        self.supervisor.sweep().await.map_err(io_error)?;
        let mut removed = 0;
        for entry in std::fs::read_dir(&self.codex_homes)? {
            let home = entry?.path();
            if crate::codex_homes::is_lost_found(&home) {
                continue;
            }
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
                        tracing::error!(dir = %home.display(), %error, "the credential copy of a crashed job cannot be brought back; its state is unknown");
                        replace_private(
                            &staging.join(format!("{}{UNKNOWN_SUFFIX}", key.to_string_lossy())),
                            b"",
                        )?;
                    }
                }
            }
            self.remove(&home).await.map_err(io_error_of)?;
            removed += 1;
        }
        if let Ok(entries) = std::fs::read_dir(work) {
            for entry in entries {
                let job = entry?.path();
                for dir in [agent_home(&job), agent_tmp(&job), job.join("out")] {
                    if std::fs::symlink_metadata(&dir).is_ok() {
                        self.remove(&dir).await.map_err(io_error_of)?;
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
            self.remove(&home).await.map_err(io_error_of)?;
        }
        create_dir(&home)?;
        std::fs::set_permissions(&call.job_dir, std::fs::Permissions::from_mode(0o755))?;
        for dir in [&home, &agent_home(&call.job_dir), &agent_tmp(&call.job_dir)] {
            self.supervisor.handover(dir).await.map_err(io_error)?;
        }
        if let Some(spec) = &call.flow_file {
            let out = call.job_dir.join("out");
            if spec.previous.is_some() {
                std::fs::remove_file(out.join("flow.md"))?;
            }
            self.supervisor.handover(&out).await.map_err(io_error)?;
            if let Some(previous) = &spec.previous {
                self.supervisor
                    .copy_in(&out, Path::new("flow.md"), previous.as_bytes())
                    .await
                    .map_err(io_error)?;
            }
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

    /// Copy-back after a sweep (ADR-013 step 5): `auth.json` and the rollout of `thread` into the staging home.
    /// An `auth.json` that cannot be copied back (missing, link, oversized, not a single regular file) leaves
    /// the credential state unknown: Codex may have rotated the single-use refresh token, so the old staged
    /// copy proves nothing and the call fails as [`ProviderError::CredentialUnusable`] (a new login is needed).
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
                tracing::error!(job = %call.job_id, %error, "the agent's auth.json could not be copied back; the Codex login is treated as unusable until the owner signs in again");
                return Err(ProviderError::CredentialUnusable);
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

    /// Called only after exec's sweep; the helper rejects links, mounts and oversized files.
    pub(super) async fn collect_flow(&self, call: &ProviderCall) -> Result<String, ProviderError> {
        let bytes = self
            .supervisor
            .copy_out(
                &call.job_dir.join("out"),
                Path::new("flow.md"),
                crate::provider::output_file::MAX_BYTES,
            )
            .await
            .map_err(|_| crate::provider::output_file::refused())?;
        String::from_utf8(bytes).map_err(|_| crate::provider::output_file::refused())
    }

    /// The end of a call (ADR-013 step 5): sweep, empty and remove the agent's directories, `/dev/mqueue` and
    /// System V IPC. A directory that can be neither removed nor quarantined poisons the isolation (no next
    /// job); the IPC clean-up is best effort.
    pub(super) async fn leave(&self, call: &ProviderCall) {
        if let Err(error) = self.supervisor.sweep().await {
            tracing::error!(job = %call.job_id, %error, "the final sweep of a call failed; no further job is taken");
            self.poisoned.store(true, Ordering::SeqCst);
            return;
        }
        for dir in [
            self.codex_home(&call.codex_home),
            agent_home(&call.job_dir),
            agent_tmp(&call.job_dir),
            call.job_dir.join("out"),
        ] {
            if let Err(error) = self.remove(&dir).await {
                tracing::error!(job = %call.job_id, %error, "an agent directory stays reachable by the agent; no further job is taken");
                self.poisoned.store(true, Ordering::SeqCst);
            }
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

    /// `clear` as the agent (which first restores its own access to locked entries), then `rmdir` as the
    /// runner (the parent is the runner's); absent is fine. Anything left is quarantined (fail closed): the
    /// supervisor gives it to the runner, mode 0700, so the next job's agent cannot reach it.
    async fn remove(&self, dir: &Path) -> Result<(), LeftoverError> {
        if dir.symlink_metadata().is_err() {
            return Ok(());
        }
        if let Err(error) = self.supervisor.clear(dir).await {
            tracing::warn!(dir = %dir.display(), %error, "cannot empty an agent directory");
        }
        match std::fs::remove_dir(dir) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                tracing::warn!(dir = %dir.display(), %error, "cannot remove an agent directory; quarantining it")
            }
        }
        self.supervisor
            .quarantine(dir)
            .await
            .map_err(|source| LeftoverError {
                dir: dir.to_path_buf(),
                source,
            })?;
        tracing::error!(dir = %dir.display(), "an agent directory could not be emptied and is quarantined; inspect and remove it by hand");
        Ok(())
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
        // Only a panic drops this run before its own sweep; the guard still sweeps then.
        let mut guard = SweepOnDrop(Some(Arc::clone(supervisor)));
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
        let mut swept_at: Option<Instant> = None;
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
                        if swept_at.is_none() && since.is_some_and(|at| at.elapsed() >= self.settings.term_grace) {
                            tracing::warn!(job = %call.job_id, "Codex did not stop within the grace after SIGTERM; sweeping");
                            swept_at = Some(Instant::now());
                            supervisor.sweep().await.map_err(io_error)?;
                        }
                        if swept_at.is_some_and(|at| at.elapsed() >= REAP_DEADLINE) {
                            return Err(ProviderError::Io(io::Error::other(
                                "the supervisor did not report the swept Codex's exit",
                            )));
                        }
                    }
                }
            }
        }
        .await;
        // Codex has exited (or failed): nothing of the agent may outlive the run.
        let sweep = supervisor.sweep().await;
        guard.0 = None;
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
            tracing::debug!(job = %call.job_id, stderr = %credential.scrubber().text_and_token_shapes(&tail), "Codex stderr tail");
        }
        let thread = parsed.thread_id().map(str::to_owned).or(match invocation {
            Invocation::Resume(thread) => Some(thread.to_owned()),
            Invocation::New => None,
        });
        if let Err(error) = isolation.pull(call, thread.as_deref()).await {
            if matches!(error, ProviderError::CredentialUnusable) {
                credential.unknown("the agent's auth.json could not be copied back");
            }
            return Err(error);
        }
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

/// Sweeps the agent processes from a task when a supervised run is dropped before its own sweep.
struct SweepOnDrop(Option<Arc<Supervisor>>);

impl Drop for SweepOnDrop {
    fn drop(&mut self) {
        if let Some(supervisor) = self.0.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move {
                if let Err(error) = supervisor.sweep().await {
                    tracing::error!(%error, "the sweep after a dropped Codex run failed");
                }
            });
        }
    }
}

/// The supervisor's status: an exit code, or the negated signal number.
fn exit_of_code(status: i32) -> Exit {
    if status < 0 {
        Exit::Signal(-status)
    } else {
        Exit::Code(status)
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
        assert!(matches!(exit_of_code(-15), Exit::Signal(15)));
        assert!(matches!(exit_of_code(143), Exit::Code(143)));
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
