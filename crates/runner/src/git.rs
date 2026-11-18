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
/// created), then checks the first commit out into the new repository `repo`, which holds a copy of the mirror's
/// objects and a ref per commit.
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
        // Packed even when small, so the per-job copy stays a few large files.
        let mut args = vec![
            os("-c"),
            os("fetch.unpackLimit=1"),
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
    // A plain copy of the mirror's object files instead of a fetch, which re-indexes every object. The copy
    // belongs to the job (the agent never reads the private mirror); the refs below verify each pinned commit.
    let (source, target) = (mirror.join("objects"), repo.join(".git").join("objects"));
    tokio::task::spawn_blocking(move || copy_objects(&source, &target))
        .await
        .map_err(io::Error::other)??;
    for commit in commits {
        git(
            "pin commit",
            Some(repo),
            &[
                os("update-ref"),
                os(&format!("{REF_PREFIX}/{commit}")),
                os(&format!("{commit}^{{commit}}")),
            ],
        )
        .await?;
    }
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

/// Makes `checkout` (a plain repository, created when absent) hold `reference` of `url` at its tip, checked
/// out detached; returns the commit.
pub async fn sync_ref(checkout: &Path, url: &str, reference: &str) -> Result<String, GitError> {
    if reference.starts_with('-')
        || reference.is_empty()
        || !reference
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._/-".contains(&byte))
    {
        return Err(GitError::InvalidCommit(reference.to_owned()));
    }
    if !checkout.join(".git").exists() {
        git("init", None, &[os("init"), os("--quiet"), checkout.into()]).await?;
    }
    git(
        "ref fetch",
        Some(checkout),
        &[
            os("fetch"),
            os("--quiet"),
            os("--no-tags"),
            os("--no-recurse-submodules"),
            os("--depth=1"),
            os("--"),
            os(url),
            os(reference),
        ],
    )
    .await?;
    git(
        "ref checkout",
        Some(checkout),
        &[
            os("-c"),
            os("advice.detachedHead=false"),
            os("checkout"),
            os("--quiet"),
            os("--force"),
            os("--detach"),
            os("FETCH_HEAD"),
        ],
    )
    .await?;
    let mut command = command(Some(checkout));
    command.args(["rev-parse", "HEAD"]);
    let output = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await?;
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || !is_object_id(&commit) {
        return Err(GitError::Failed {
            step: "rev-parse",
            status: output.status,
        });
    }
    // A ref given as a commit id is a pin: the checkout must be exactly that commit.
    if is_object_id(reference) && !commit.eq_ignore_ascii_case(reference) {
        return Err(GitError::InvalidCommit(reference.to_owned()));
    }
    Ok(commit)
}

/// Copies the packs and loose objects of the object directory `source` into `target`: regular files only,
/// never a link, an alternates file or a temporary file of an interrupted fetch.
fn copy_objects(source: &Path, target: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let is_loose_dir = name.len() == 2 && name.bytes().all(|byte| byte.is_ascii_hexdigit());
        if !(entry.file_type()?.is_dir() && (is_loose_dir || name == "pack")) {
            continue;
        }
        let dir = target.join(&*name);
        std::fs::create_dir_all(&dir)?;
        for file in std::fs::read_dir(entry.path())? {
            let file = file?;
            let file_name = file.file_name();
            if file.file_type()?.is_file() && !file_name.to_string_lossy().starts_with("tmp_") {
                std::fs::copy(file.path(), dir.join(&file_name))?;
            }
        }
    }
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

    fn git_out(repo: &Path, args: &[&str]) -> Option<String> {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    fn fixture_url() -> String {
        format!("file://{}", crate::test_support::shared_repo().display())
    }

    #[tokio::test]
    async fn checkout_is_self_contained_and_pins_the_commits() {
        let dir = tempfile::TempDir::new().unwrap();
        let (mirror, repo) = (dir.path().join("m.git"), dir.path().join("repo"));
        let commits = crate::test_support::commits();
        let pinned = [commits.head.as_str(), commits.base.as_str()];
        checkout(&mirror, &fixture_url(), &pinned, &repo)
            .await
            .unwrap();
        // An interrupted mirror fetch leaves temporary files; a later checkout never copies them.
        std::fs::write(mirror.join("objects/pack/tmp_pack_x"), b"partial").unwrap();
        let second = dir.path().join("second");
        checkout(&mirror, &fixture_url(), &pinned, &second)
            .await
            .unwrap();
        assert!(!second.join(".git/objects/pack/tmp_pack_x").exists());
        // The agent never reads the private mirror: each checkout holds its own objects.
        std::fs::remove_dir_all(&mirror).unwrap();
        for repo in [&repo, &second] {
            assert!(!repo.join(".git/objects/info/alternates").exists());
            assert!(git_out(repo, &["log", "--oneline", &commits.base]).is_some());
            assert!(git_out(repo, &["diff", &commits.base, &commits.head]).is_some());
            for commit in pinned {
                let pinned_ref = format!("{REF_PREFIX}/{commit}");
                assert_eq!(
                    git_out(repo, &["rev-parse", "--verify", &pinned_ref]).as_deref(),
                    Some(commit)
                );
            }
            assert_eq!(
                git_out(repo, &["rev-parse", "HEAD"]).as_deref(),
                Some(commits.head.as_str())
            );
        }
    }

    #[tokio::test]
    async fn checkout_copies_loose_mirror_objects() {
        let dir = tempfile::TempDir::new().unwrap();
        let (mirror, repo) = (dir.path().join("m.git"), dir.path().join("repo"));
        let commits = crate::test_support::commits();
        checkout(&mirror, &fixture_url(), &[&commits.head], &repo)
            .await
            .unwrap();
        // A commit stored as a loose object in the mirror, as a small fetch leaves it.
        let tree = format!("{}^{{tree}}", commits.head);
        let loose = git_out(&mirror, &["commit-tree", &tree, "-m", "loose"]).unwrap();
        let pinned_ref = format!("{REF_PREFIX}/{loose}");
        assert!(git_out(&mirror, &["update-ref", &pinned_ref, &loose]).is_some());
        let second = dir.path().join("second");
        checkout(&mirror, &fixture_url(), &[&loose, &commits.head], &second)
            .await
            .unwrap();
        std::fs::remove_dir_all(&mirror).unwrap();
        assert_eq!(
            git_out(&second, &["rev-parse", "HEAD"]).as_deref(),
            Some(loose.as_str())
        );
    }

    #[tokio::test]
    async fn checkout_refuses_a_pinned_id_that_is_not_a_commit() {
        let dir = tempfile::TempDir::new().unwrap();
        let (mirror, repo) = (dir.path().join("m.git"), dir.path().join("repo"));
        let commits = crate::test_support::commits();
        checkout(&mirror, &fixture_url(), &[&commits.head], &repo)
            .await
            .unwrap();
        let tree = git_out(
            &mirror,
            &["rev-parse", &format!("{}^{{tree}}", commits.head)],
        )
        .unwrap();
        // The mirror answers `has_commits` for the head only; the tree id is then fetched and must fail.
        let result = checkout(
            &mirror,
            &fixture_url(),
            &[&commits.head, &tree],
            &dir.path().join("second"),
        )
        .await;
        assert!(matches!(result, Err(GitError::Failed { .. })), "{result:?}");
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
