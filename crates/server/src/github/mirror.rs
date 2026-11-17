//! The server's bare mirror of each analysed repository (`<data dir>/mirrors/<owner>/<name>.git`), fetched from
//! the configured clone base: file content for the UI and the diff for the pipeline. Paths and ranges are read
//! only from a pinned commit's tree, never through the filesystem.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use analysis::blob::{self, Blob, BlobError, Limits};
use contracts::common::RepoRef;
use contracts::ids::CommitSha;
use tokio::process::Command;

use crate::config::Config;
use crate::github::oauth::endpoint;
use crate::github::rest::is_valid_repo;

/// Longest a fetch may take (large repositories on a first fetch).
const FETCH_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Longest a local read may take.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
pub use analysis::blob::{MAX_BLOB_BYTES, is_tree_path};
/// Longest wait for another fetch into the same mirror (one fetch plus its local checks).
const LOCK_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// One lock per mirror: fetches into the same bare repository are serialised.
static MIRROR_LOCKS: LazyLock<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Mutex::default);

/// A mirror operation failed.
#[derive(Debug, thiserror::Error)]
pub enum MirrorError {
    /// The repository owner or name is not a valid GitHub name.
    #[error("the repository name is not valid")]
    InvalidRepo,
    /// `git` could not be started or its pipes failed.
    #[error("could not run git")]
    Io(#[from] std::io::Error),
    /// `git` did not finish in time.
    #[error("git {operation} did not finish within {} seconds", .timeout.as_secs())]
    Timeout {
        /// The git operation.
        operation: &'static str,
        /// The limit.
        timeout: Duration,
    },
    /// `git` failed.
    #[error("git {operation} failed: {message}")]
    Git {
        /// The git operation.
        operation: &'static str,
        /// Plain git error output.
        message: String,
    },
    /// A file is larger than [`MAX_BLOB_BYTES`].
    #[error("the file has {size} bytes, more than the display limit")]
    BlobTooLarge {
        /// Its size in bytes.
        size: u64,
    },
    /// A commit is still missing after the fetch.
    #[error("commit {0} is not in the repository")]
    MissingCommit(CommitSha),
    /// The blocking file read did not complete.
    #[error("the file read did not complete")]
    Task(#[source] tokio::task::JoinError),
}

/// The mirror directory of `repo` (lower-cased: GitHub names are case-insensitive).
pub fn mirror_path(config: &Config, repo: &RepoRef) -> Result<PathBuf, MirrorError> {
    if !is_valid_repo(repo) {
        return Err(MirrorError::InvalidRepo);
    }
    Ok(config
        .data_dir
        .join("mirrors")
        .join(repo.owner.to_ascii_lowercase())
        .join(format!("{}.git", repo.name.to_ascii_lowercase())))
}

fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("-C")
        .arg(dir)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    command
}

async fn run(
    mut command: Command,
    operation: &'static str,
    timeout: Duration,
) -> Result<Output, MirrorError> {
    tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| MirrorError::Timeout { operation, timeout })?
        .map_err(MirrorError::Io)
}

async fn has_commit(mirror: &Path, commit: &CommitSha) -> Result<bool, MirrorError> {
    let mut command = git(mirror);
    command
        .args(["cat-file", "-e", "--end-of-options"])
        .arg(format!("{}^{{commit}}", commit.0));
    Ok(run(command, "cat-file", READ_TIMEOUT)
        .await?
        .status
        .success())
}

/// Makes sure the mirror of `repo` holds `commits`, fetching them from the clone base when missing; returns the
/// mirror directory. Fetched commits are kept by refs `refs/smart-review/<sha>`.
pub async fn ensure_commits(
    config: &Config,
    repo: &RepoRef,
    commits: &[&CommitSha],
) -> Result<PathBuf, MirrorError> {
    let mirror = mirror_path(config, repo)?;
    let _guard = lock_mirror(&mirror, LOCK_TIMEOUT).await?;

    if !mirror.join("HEAD").exists() {
        tokio::fs::create_dir_all(&mirror).await?;
        let mut init = git(&mirror);
        init.args(["init", "--bare", "--quiet"]);
        check(run(init, "init", READ_TIMEOUT).await?, "init")?;
    }
    let mut missing = Vec::new();
    for commit in commits {
        if !has_commit(&mirror, commit).await? {
            missing.push(*commit);
        }
    }
    if missing.is_empty() {
        return Ok(mirror);
    }
    let url = endpoint(
        &config.github.clone_url,
        &format!("{}/{}.git", repo.owner, repo.name),
    );
    let mut fetch = git(&mirror);
    fetch
        .args(["fetch", "--no-tags", "--quiet", "--end-of-options"])
        .arg(url.as_str())
        .args(
            missing
                .iter()
                .map(|commit| format!("+{0}:refs/smart-review/{0}", commit.0)),
        );
    tracing::info!(repo = %format!("{}/{}", repo.owner, repo.name), commits = missing.len(), "fetching commits into the mirror");
    check(run(fetch, "fetch", FETCH_TIMEOUT).await?, "fetch")?;
    for commit in missing {
        if !has_commit(&mirror, commit).await? {
            return Err(MirrorError::MissingCommit(commit.clone()));
        }
    }
    Ok(mirror)
}

/// Takes the lock of `mirror`, waiting at most `wait`.
async fn lock_mirror(
    mirror: &Path,
    wait: Duration,
) -> Result<tokio::sync::OwnedMutexGuard<()>, MirrorError> {
    let lock = {
        let mut locks = MIRROR_LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(locks.entry(mirror.to_path_buf()).or_default())
    };
    tokio::time::timeout(wait, lock.lock_owned())
        .await
        .map_err(|_| MirrorError::Timeout {
            operation: "lock",
            timeout: wait,
        })
}

fn check(output: Output, operation: &'static str) -> Result<Output, MirrorError> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(MirrorError::Git {
            operation,
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

/// The blob at `path` in the tree of `commit`; `None` when the path is not a file there. Blobs over
/// [`MAX_BLOB_BYTES`] are refused before any content is read (the shared bounded reader, `analysis::blob`).
pub async fn read_blob(
    mirror: &Path,
    commit: &CommitSha,
    path: &str,
) -> Result<Option<Vec<u8>>, MirrorError> {
    if !is_tree_path(path) {
        return Ok(None);
    }
    let (mirror, commit, path) = (mirror.to_path_buf(), commit.clone(), path.to_owned());
    let limits = Limits {
        max_blob_bytes: MAX_BLOB_BYTES,
        total_bytes: MAX_BLOB_BYTES,
        timeout: READ_TIMEOUT,
    };
    let answers =
        tokio::task::spawn_blocking(move || blob::read_blobs(&mirror, &[(&commit, &path)], limits))
            .await
            .map_err(MirrorError::Task)?
            .map_err(|error| match error {
                BlobError::Io(error) => MirrorError::Io(error),
                BlobError::Git { message } => MirrorError::Git {
                    operation: "cat-file",
                    message,
                },
                BlobError::Timeout { .. } => MirrorError::Timeout {
                    operation: "cat-file",
                    timeout: READ_TIMEOUT,
                },
            })?;
    match answers.into_iter().next() {
        Some(Blob::Content(bytes)) => Ok(Some(bytes.to_vec())),
        Some(Blob::TooLarge { size } | Blob::OverBudget { size }) => {
            Err(MirrorError::BlobTooLarge { size })
        }
        Some(Blob::Missing) | None => Ok(None),
    }
}

/// The merge base of `base` and `head`: the commit a pull request's changes are compared with (GitHub's
/// three-dot comparison).
pub async fn merge_base(
    mirror: &Path,
    base: &CommitSha,
    head: &CommitSha,
) -> Result<CommitSha, MirrorError> {
    let mut command = git(mirror);
    command
        .args(["merge-base", "--end-of-options"])
        .arg(&base.0)
        .arg(&head.0);
    let output = check(
        run(command, "merge-base", READ_TIMEOUT).await?,
        "merge-base",
    )?;
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if sha.is_empty() {
        return Err(MirrorError::Git {
            operation: "merge-base",
            message: "the commits share no history".to_owned(),
        });
    }
    Ok(CommitSha(sha))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn waiting_for_a_busy_mirror_is_bounded() {
        let dir = tempfile::tempdir().expect("temp dir");
        let held = lock_mirror(dir.path(), Duration::from_secs(1))
            .await
            .expect("first lock");
        let waited = lock_mirror(dir.path(), Duration::from_millis(50)).await;
        assert!(
            matches!(
                waited,
                Err(MirrorError::Timeout {
                    operation: "lock",
                    ..
                })
            ),
            "{waited:?}"
        );
        drop(held);
        assert!(
            lock_mirror(dir.path(), Duration::from_secs(1))
                .await
                .is_ok()
        );
    }

    #[test]
    fn tree_paths_reject_traversal_and_absolute_paths() {
        for path in ["src/lib.rs", "README.md", ".github/x.yml", "a b/c:d.rs"] {
            assert!(is_tree_path(path), "{path}");
        }
        for path in [
            "",
            "/etc/passwd",
            "../secret",
            "src/../../x",
            "src/./lib.rs",
            "src//lib.rs",
            "src/",
            "src/lib.rs\n",
            "src/\0x",
        ] {
            assert!(!is_tree_path(path), "{path:?}");
        }
    }
}
