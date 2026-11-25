//! Bare mirrors per clone URL, fetched by pinned commit IDs and mounted read-only into workers.
//! Workers own checkouts; the runner never reads checkout content.
//! Each git step runs in its own process group, killed whole when the step is dropped (job runtime limit).

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

use rustix::process::{Pid, Signal, kill_process_group};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::Instant;

use crate::phases::PhaseLog;

/// `PATH` for git; never inherited.
const GIT_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
/// Proxy variables of the runner process passed to git for the mirror fetch.
const PROXY_VARS: [&str; 7] = [
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "NO_PROXY",
    "no_proxy",
];
/// Ref namespace the mirror keeps fetched commits under, so they are never garbage-collected.
const REF_PREFIX: &str = "refs/diffpilot";

/// A git step failed.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// A commit id is not a full hexadecimal object id.
    #[error("the commit id {0:?} is not a full hexadecimal object id")]
    InvalidCommit(String),
    /// git could not be started or its directory prepared.
    #[error("git could not be run")]
    Io(#[from] io::Error),
    /// git exited with an error.
    #[error("git {step} failed ({status})")]
    Failed {
        /// The failed step.
        step: &'static str,
        /// Exit status.
        status: std::process::ExitStatus,
    },
}

/// The bare mirror of `clone_url` below `mirrors`.
pub fn mirror_path(mirrors: &Path, clone_url: &str) -> PathBuf {
    let digest = Sha256::digest(clone_url.as_bytes());
    let name: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    mirrors.join(format!("{name}.git"))
}

/// Ensures the bare mirror holds every pinned commit, fetching only missing objects.
pub async fn prepare_mirror(
    mirror: &Path,
    clone_url: &str,
    commits: &[&str],
    phases: &PhaseLog,
) -> Result<(), GitError> {
    for commit in commits {
        if !is_object_id(commit) {
            return Err(GitError::InvalidCommit((*commit).to_owned()));
        }
    }
    if commits.is_empty() {
        return Ok(());
    }
    if !mirror.exists() {
        git(
            "init",
            None,
            &[os("init"), os("--quiet"), os("--bare"), mirror.into()],
        )
        .await?;
    }
    let step = Instant::now();
    if has_commits(mirror, commits).await? {
        phases.word("mirror", "hit");
    } else {
        // Packed even when small, so the per-job copy stays a few large files; never repacked (no automatic
        // gc or maintenance, detached or not), so no pack disappears while `copy_objects` copies it.
        let mut args = Vec::new();
        for setting in [
            "fetch.unpackLimit=1",
            "gc.auto=0",
            "maintenance.auto=false",
            "gc.autoDetach=false",
            "maintenance.autoDetach=false",
        ] {
            args.extend([os("-c"), os(setting)]);
        }
        args.extend([
            os("fetch"),
            os("--quiet"),
            os("--no-tags"),
            os("--no-recurse-submodules"),
        ]);
        args.extend([os("--"), os(clone_url)]);
        args.extend(
            commits
                .iter()
                .map(|c| os(&format!("+{c}:{REF_PREFIX}/{c}"))),
        );
        git("mirror fetch", Some(mirror), &args).await?;
        phases.word("mirror", "fetched");
    }
    phases.since("mirror", step);
    Ok(())
}

async fn has_commits(mirror: &Path, commits: &[&str]) -> Result<bool, GitError> {
    for commit in commits {
        let mut command = command(Some(mirror));
        command.args(["cat-file", "-e", &format!("{commit}^{{commit}}")]);
        let (status, _) = run(command).await?;
        if !status.success() {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn git(step: &'static str, dir: Option<&Path>, args: &[OsString]) -> Result<(), GitError> {
    let mut command = command(dir);
    command.args(args);
    let (status, stderr) = run(command).await?;
    if status.success() {
        return Ok(());
    }
    tracing::debug!(
        step,
        stderr = %crate::scrub::clip(&String::from_utf8_lossy(&stderr), 500),
        "git step failed"
    );
    Err(GitError::Failed { step, status })
}

/// Runs `command` to its exit and returns its status and stderr. Dropped early, it kills the whole process
/// group (helpers such as `git-remote-https` included) while the leader is not yet reaped.
async fn run(mut command: Command) -> io::Result<(ExitStatus, Vec<u8>)> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut group = KillGroupOnDrop(
        child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(Pid::from_raw),
    );
    let mut stderr = Vec::new();
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_end(&mut stderr).await?;
    }
    let status = child.wait().await?;
    // Reaped: the id may be reused from here on.
    group.0 = None;
    Ok((status, stderr))
}

/// SIGKILL to a git step's process group on drop, unless disarmed after its leader was reaped.
struct KillGroupOnDrop(Option<Pid>);

impl Drop for KillGroupOnDrop {
    fn drop(&mut self) {
        if let Some(pid) = self.0
            && let Err(error) = kill_process_group(pid, Signal::KILL)
            && error != rustix::io::Errno::SRCH
        {
            tracing::warn!(%error, "cannot kill a git step's process group");
        }
    }
}

/// git with a constructed environment: no user or system config (no filters, hooks paths or LFS from the
/// host), no prompts, only the runner's proxy variables.
fn command(dir: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    command
        .env_clear()
        .env("PATH", GIT_PATH)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LANG", "C")
        .process_group(0)
        .kill_on_drop(true);
    for key in PROXY_VARS {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    command
}

fn os(value: &str) -> OsString {
    OsString::from(value)
}

/// A full SHA-1 or SHA-256 object id in hexadecimal.
fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_ids_are_full_hex() {
        assert!(is_object_id(&"a".repeat(40)));
        assert!(is_object_id(&"0".repeat(64)));
        assert!(!is_object_id("--upload-pack=x"));
        assert!(!is_object_id(&"g".repeat(40)));
        assert!(!is_object_id("abc123"));
    }

    #[test]
    fn mirror_path_is_stable_per_url() {
        let root = Path::new("/m");
        assert_eq!(
            mirror_path(root, "file:///a"),
            mirror_path(root, "file:///a")
        );
        assert_ne!(
            mirror_path(root, "file:///a"),
            mirror_path(root, "file:///b")
        );
        assert_eq!(mirror_path(root, "x").parent(), Some(root));
    }

    #[tokio::test]
    async fn mirror_fetches_the_pinned_commits_without_a_checkout() {
        let root = tempfile::tempdir().unwrap();
        let url = format!("file://{}", crate::test_support::shared_repo().display());
        let mirror = mirror_path(root.path(), &url);
        let commits = crate::test_support::commits();
        let wanted = [&*commits.head, &*commits.base];
        prepare_mirror(&mirror, &url, &wanted, &PhaseLog::default())
            .await
            .unwrap();
        assert!(has_commits(&mirror, &wanted).await.unwrap());
        assert!(!mirror.join(".git").exists());
        assert!(!mirror.join("src").exists());
        prepare_mirror(&mirror, &url, &wanted, &PhaseLog::default())
            .await
            .unwrap();
    }
}
