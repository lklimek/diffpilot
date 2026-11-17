//! Bounded, read-only reads of files at pinned commits (`git cat-file`): one reader for the server's line views
//! and for verification. Every read has a per-file size limit, a total budget and a deadline that kills git;
//! paths are looked up only in a commit's tree, never through the filesystem.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use contracts::ids::CommitSha;

/// Largest file read for display or verification (the lines endpoint answers `not_rendered` above it).
pub const MAX_BLOB_BYTES: u64 = 8 * 1024 * 1024;
/// Longest one read may take, git included.
pub const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest `cat-file` answer header read; also bounds the path length.
const HEADER_LIMIT: u64 = 4096;
/// Longest path looked up (its `missing` answer must fit in a header).
const MAX_PATH_BYTES: usize = 3900;
/// Most `git` error output kept.
const MAX_STDERR_BYTES: u64 = 64 * 1024;
/// Interval between checks whether git has exited.
const WAIT_POLL: Duration = Duration::from_millis(10);

/// A repository-relative path that names a tree entry: no leading or trailing `/`, no empty, `.` or `..`
/// component, no control characters, at most a few kilobytes.
pub fn is_tree_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH_BYTES
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Bounds of one [`read_blobs`] call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Largest single file read.
    pub max_blob_bytes: u64,
    /// Most bytes over all requests; a file named twice counts twice.
    pub total_bytes: u64,
    /// Deadline of the whole call.
    pub timeout: Duration,
}

/// The answer to one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blob {
    /// Not a file of that commit's tree (missing, a directory or submodule, or not a tree path).
    Missing,
    /// Larger than [`Limits::max_blob_bytes`]; not read.
    TooLarge {
        /// Size in bytes.
        size: u64,
    },
    /// Would exceed [`Limits::total_bytes`]; not read.
    OverBudget {
        /// Size in bytes.
        size: u64,
    },
    /// The file's bytes (shared by requests that name the same object).
    Content(Arc<[u8]>),
}

/// Reading failed.
#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    /// `git` could not be started or its pipes failed.
    #[error("could not run git")]
    Io(#[from] std::io::Error),
    /// `git` failed or answered unexpectedly.
    #[error("git cat-file failed: {message}")]
    Git {
        /// Plain git error output.
        message: String,
    },
    /// The read did not finish before the deadline; git was killed.
    #[error("git cat-file did not finish within {seconds} seconds")]
    Timeout {
        /// The limit in seconds.
        seconds: u64,
    },
}

/// The files `path` at `commit` for each request, in request order, read from the repository at `repo`.
pub fn read_blobs(
    repo: &Path,
    requests: &[(&CommitSha, &str)],
    limits: Limits,
) -> Result<Vec<Blob>, BlobError> {
    read_blobs_with(OsStr::new("git"), repo, requests, limits)
}

/// An object header: `<oid> <type> <size>`.
struct Header {
    oid: String,
    kind: String,
    size: u64,
}

fn read_blobs_with(
    git: &OsStr,
    repo: &Path,
    requests: &[(&CommitSha, &str)],
    limits: Limits,
) -> Result<Vec<Blob>, BlobError> {
    let deadline = Instant::now() + limits.timeout;
    let mut answers = vec![Blob::Missing; requests.len()];
    let named: Vec<(usize, String)> = requests
        .iter()
        .enumerate()
        .filter(|(_, (_, path))| is_tree_path(path))
        .map(|(index, (commit, path))| (index, format!("{}:{path}\n", commit.0)))
        .collect();
    if named.is_empty() {
        return Ok(answers);
    }
    // Sizes first, so nothing over the limits is ever read.
    let names: String = named.iter().map(|(_, name)| name.as_str()).collect();
    let headers: Vec<Option<Header>> =
        cat_file(git, repo, "--batch-check", names, deadline, limits, |out| {
            named.iter().map(|_| read_header(out)).collect()
        })?;
    let mut budget = limits.total_bytes;
    let mut objects: Vec<String> = Vec::new();
    let mut slot_of: HashMap<String, usize> = HashMap::new();
    let mut chosen: Vec<(usize, usize)> = Vec::new();
    for ((index, _), header) in named.iter().zip(headers) {
        let Some(Header { oid, kind, size }) = header else {
            continue;
        };
        if kind != "blob" {
            continue;
        }
        if size > limits.max_blob_bytes {
            answers[*index] = Blob::TooLarge { size };
            continue;
        }
        if size > budget {
            answers[*index] = Blob::OverBudget { size };
            continue;
        }
        budget -= size;
        let slot = *slot_of.entry(oid.clone()).or_insert_with(|| {
            objects.push(oid);
            objects.len() - 1
        });
        chosen.push((*index, slot));
    }
    if objects.is_empty() {
        return Ok(answers);
    }
    let input: String = objects.iter().map(|oid| format!("{oid}\n")).collect();
    let contents: Vec<Arc<[u8]>> =
        cat_file(git, repo, "--batch", input, deadline, limits, |out| {
            objects.iter().map(|oid| read_content(out, oid)).collect()
        })?;
    for (index, slot) in chosen {
        if let Some(content) = contents.get(slot) {
            answers[index] = Blob::Content(Arc::clone(content));
        }
    }
    Ok(answers)
}

/// One `--batch-check` answer; `None` for `<name> missing` or `ambiguous`.
fn read_header(out: &mut BufReader<ChildStdout>) -> Result<Option<Header>, BlobError> {
    let mut line = String::new();
    out.by_ref().take(HEADER_LIMIT).read_line(&mut line)?;
    let Some(line) = line.strip_suffix('\n') else {
        return Err(BlobError::Git {
            message: "git cat-file gave a truncated answer".to_owned(),
        });
    };
    if line.ends_with(" missing") || line.ends_with(" ambiguous") {
        return Ok(None);
    }
    let mut fields = line.split(' ');
    let (Some(oid), Some(kind), Some(size), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Ok(None);
    };
    let Ok(size) = size.parse::<u64>() else {
        return Ok(None);
    };
    if oid.is_empty() || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(None);
    }
    Ok(Some(Header {
        oid: oid.to_owned(),
        kind: kind.to_owned(),
        size,
    }))
}

/// One `--batch` answer for `oid`, whose size the caller checked.
fn read_content(out: &mut BufReader<ChildStdout>, oid: &str) -> Result<Arc<[u8]>, BlobError> {
    let Some(header) = read_header(out)?.filter(|header| header.oid == oid) else {
        return Err(BlobError::Git {
            message: format!("git cat-file did not return object {oid}"),
        });
    };
    let mut content = Vec::with_capacity(usize::try_from(header.size).unwrap_or_default());
    out.by_ref().take(header.size).read_to_end(&mut content)?;
    let mut newline = [0u8; 1];
    out.read_exact(&mut newline)?;
    if u64::try_from(content.len()).ok() != Some(header.size) {
        return Err(BlobError::Git {
            message: format!("git cat-file returned a short object {oid}"),
        });
    }
    Ok(content.into())
}

/// Runs `git cat-file <mode>` with `input` on stdin and `read` on stdout; git is killed at `deadline`.
fn cat_file<T>(
    git: &OsStr,
    repo: &Path,
    mode: &str,
    input: String,
    deadline: Instant,
    limits: Limits,
    read: impl FnOnce(&mut BufReader<ChildStdout>) -> Result<T, BlobError>,
) -> Result<T, BlobError> {
    let mut child = Command::new(git)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", mode])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let child = Arc::new(Mutex::new(child));
    let kill = |child: &Arc<Mutex<std::process::Child>>| {
        let _ = child.lock().unwrap_or_else(PoisonError::into_inner).kill();
    };
    let (finished, wait_for_end) = mpsc::channel::<()>();
    let watchdog = {
        let child = Arc::clone(&child);
        let remaining = deadline.saturating_duration_since(Instant::now());
        thread::spawn(move || {
            let expired = matches!(
                wait_for_end.recv_timeout(remaining),
                Err(mpsc::RecvTimeoutError::Timeout)
            );
            if expired {
                kill(&child);
            }
            expired
        })
    };
    // git answers while reading, so stdin is written by its own thread.
    let writer = stdin.map(|mut stdin| {
        thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        })
    });
    let stderr_reader = stderr.map(|pipe| {
        thread::spawn(move || {
            let mut text = Vec::new();
            let _ = pipe.take(MAX_STDERR_BYTES).read_to_end(&mut text);
            text
        })
    });
    let result = match stdout {
        Some(stdout) => read(&mut BufReader::new(stdout)),
        None => Err(BlobError::Git {
            message: "git cat-file has no output".to_owned(),
        }),
    };
    if result.is_err() {
        kill(&child);
    }
    // Poll: the watchdog needs the lock to kill a git that stops answering.
    let status = loop {
        let exited = child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_wait();
        match exited {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(WAIT_POLL),
            Err(error) => break Err(error),
        }
    };
    let _ = finished.send(());
    let timed_out = watchdog.join().unwrap_or(false);
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    let stderr = stderr_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    if timed_out {
        return Err(BlobError::Timeout {
            seconds: limits.timeout.as_secs(),
        });
    }
    let status = status?;
    let failed = || BlobError::Git {
        message: String::from_utf8_lossy(&stderr).trim().to_owned(),
    };
    if !status.success() && !stderr.is_empty() {
        return Err(failed());
    }
    let value = result?;
    if !status.success() {
        return Err(failed());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.test")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.test")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git");
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// A repository with `files` committed; returns the commit.
    fn repo(dir: &Path, files: &[(&str, &[u8])]) -> CommitSha {
        git(dir, &["init", "-q", "-b", "main"]);
        for (path, content) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, content).expect("write");
        }
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "c"]);
        CommitSha(git(dir, &["rev-parse", "HEAD"]))
    }

    fn limits(max_blob_bytes: u64, total_bytes: u64) -> Limits {
        Limits {
            max_blob_bytes,
            total_bytes,
            timeout: Duration::from_secs(30),
        }
    }

    #[test]
    fn reads_files_and_answers_missing_for_everything_else() {
        let dir = tempfile::tempdir().expect("temp dir");
        let commit = repo(dir.path(), &[("src/a.rs", b"a\nb\n"), ("b c.txt", b"x")]);
        let answers = read_blobs(
            dir.path(),
            &[
                (&commit, "src/a.rs"),
                (&commit, "src"),
                (&commit, "missing.rs"),
                (&commit, "../a.rs"),
                (&commit, "b c.txt"),
                (&commit, "x missing"),
            ],
            limits(1024, 1024),
        )
        .expect("read");
        assert_eq!(
            answers,
            [
                Blob::Content(Arc::from(&b"a\nb\n"[..])),
                Blob::Missing,
                Blob::Missing,
                Blob::Missing,
                Blob::Content(Arc::from(&b"x"[..])),
                Blob::Missing,
            ]
        );
    }

    #[test]
    fn huge_blob_is_refused_before_reading() {
        let dir = tempfile::tempdir().expect("temp dir");
        let big = vec![b'\n'; 4096];
        let commit = repo(dir.path(), &[("big.txt", &big)]);
        let answers =
            read_blobs(dir.path(), &[(&commit, "big.txt")], limits(1024, 1 << 20)).expect("read");
        assert_eq!(answers, [Blob::TooLarge { size: 4096 }]);
    }

    #[test]
    fn many_paths_to_one_large_blob_share_one_read_and_the_budget() {
        let dir = tempfile::tempdir().expect("temp dir");
        let content = vec![b'\n'; 1000];
        let names: Vec<String> = (0..6).map(|n| format!("alias{n}.txt")).collect();
        let files: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &content[..])).collect();
        let commit = repo(dir.path(), &files);
        let requests: Vec<(&CommitSha, &str)> =
            names.iter().map(|n| (&commit, n.as_str())).collect();
        let answers = read_blobs(dir.path(), &requests, limits(1000, 3000)).expect("read");
        let read: Vec<&Arc<[u8]>> = answers
            .iter()
            .filter_map(|answer| match answer {
                Blob::Content(bytes) => Some(bytes),
                _ => None,
            })
            .collect();
        assert_eq!(read.len(), 3, "{answers:?}");
        assert!(
            read.iter().all(|bytes| Arc::ptr_eq(bytes, read[0])),
            "one object, read once"
        );
        assert_eq!(answers[3..], vec![Blob::OverBudget { size: 1000 }; 3][..]);
    }

    #[test]
    fn hanging_git_is_killed_at_the_deadline() {
        let dir = tempfile::tempdir().expect("temp dir");
        let stub = dir.path().join("git");
        std::fs::write(&stub, "#!/bin/sh\nexec sleep 30\n").expect("stub");
        let made_executable = Command::new("chmod")
            .arg("+x")
            .arg(&stub)
            .status()
            .expect("chmod");
        assert!(made_executable.success());
        let commit = CommitSha("0".repeat(40));
        let started = Instant::now();
        let limits = Limits {
            timeout: Duration::from_millis(300),
            ..limits(1024, 1024)
        };
        let result = read_blobs_with(stub.as_os_str(), dir.path(), &[(&commit, "a")], limits);
        assert!(
            matches!(result, Err(BlobError::Timeout { .. })),
            "{result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn broken_repository_is_an_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let commit = CommitSha("0".repeat(40));
        let result = read_blobs(dir.path(), &[(&commit, "a")], limits(1024, 1024));
        assert!(matches!(result, Err(BlobError::Git { .. })), "{result:?}");
    }

    #[test]
    fn tree_paths_reject_traversal_absolute_and_overlong_paths() {
        for path in [
            "src/lib.rs",
            "README.md",
            ".github/x.yml",
            "a b/c:d.rs",
            "dir/..x",
        ] {
            assert!(is_tree_path(path), "{path}");
        }
        let long = "a".repeat(MAX_PATH_BYTES + 1);
        for path in [
            "",
            "/etc/passwd",
            "../secret",
            "src/../../x",
            "src/./lib.rs",
            "src//lib.rs",
            "src/",
            "a\nb",
            "a\0b",
            long.as_str(),
        ] {
            assert!(!is_tree_path(path), "{path:?}");
        }
    }
}
