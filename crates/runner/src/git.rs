//! Repository access for a job: a bare mirror per clone URL in the runner's cache, fetched by commit id, and a
//! fresh checkout of the head commit in the job directory. The runner never reads checkout content.
//! Each git step runs in its own process group, killed whole when the step is dropped (job runtime limit).

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

use rustix::process::{Pid, Signal, kill_process_group};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

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
const REF_PREFIX: &str = "refs/smart-review";

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

/// Makes sure `mirror` holds `commits` (fetching them from `clone_url` when missing; an absent mirror is
/// created), then checks the first commit out into the new repository `repo`, with every commit fetched.
pub async fn checkout(
    mirror: &Path,
    clone_url: &str,
    commits: &[&str],
    repo: &Path,
) -> Result<(), GitError> {
    for commit in commits {
        if !is_object_id(commit) {
            return Err(GitError::InvalidCommit((*commit).to_owned()));
        }
    }
    let Some(head) = commits.first() else {
        return Ok(());
    };
    if !mirror.exists() {
        git(
            "init",
            None,
            &[os("init"), os("--quiet"), os("--bare"), mirror.into()],
        )
        .await?;
    }
    if !has_commits(mirror, commits).await? {
        let mut args = vec![
            os("fetch"),
            os("--quiet"),
            os("--no-tags"),
            os("--no-recurse-submodules"),
        ];
        args.extend([os("--"), os(clone_url)]);
        args.extend(
            commits
                .iter()
                .map(|c| os(&format!("+{c}:{REF_PREFIX}/{c}"))),
        );
        git("mirror fetch", Some(mirror), &args).await?;
    }
    git("init", None, &[os("init"), os("--quiet"), repo.into()]).await?;
    let mut args = vec![
        os("fetch"),
        os("--quiet"),
        os("--no-tags"),
        os("--no-recurse-submodules"),
    ];
    args.extend([os("--"), mirror.into()]);
    args.extend(
        commits
            .iter()
            .map(|c| os(&format!("{REF_PREFIX}/{c}:{REF_PREFIX}/{c}"))),
    );
    git("checkout fetch", Some(repo), &args).await?;
    git(
        "checkout",
        Some(repo),
        &[
            os("-c"),
            os("advice.detachedHead=false"),
            os("checkout"),
            os("--quiet"),
            os("--detach"),
            os(head),
        ],
    )
    .await
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
    async fn rejects_option_like_commit_ids() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = checkout(
            &dir.path().join("m.git"),
            "file:///nowhere",
            &["--upload-pack=touch /tmp/x"],
            &dir.path().join("repo"),
        )
        .await;
        assert!(matches!(result, Err(GitError::InvalidCommit(_))));
        assert!(!dir.path().join("m.git").exists());
    }
}
