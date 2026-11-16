//! Hunks of a pull request from `git diff -U0 -M` between base and head.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::Duration;

use contracts::common::LineSide;
use contracts::ids::{CommitSha, HunkId};

/// Lines of one side of a hunk, `git diff -U0` style: `len == 0` means "after line `start`".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineSpan {
    /// First line (1-based), or the line after which lines were added or removed.
    pub start: u32,
    /// Number of lines.
    pub len: u32,
}

/// How a file changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileStatus {
    /// New file.
    Added,
    /// Changed in place.
    Modified,
    /// Removed.
    Deleted,
    /// Renamed (possibly with changes).
    Renamed,
}

/// The diff of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    /// Path at the head commit (at the base commit for deleted files).
    pub path: String,
    /// Path at the base commit of a renamed file.
    pub old_path: Option<String>,
    /// How the file changed.
    pub status: FileStatus,
    /// Binary file: no hunks.
    pub binary: bool,
    /// Hunks in file order.
    pub hunks: Vec<Hunk>,
}

/// One hunk, without context lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// `h1`, `h2`, … in path order then file order; stable for one base/head pair.
    pub id: HunkId,
    /// Base side.
    pub base: LineSpan,
    /// Head side.
    pub head: LineSpan,
    /// Added head lines.
    pub added: Vec<u32>,
    /// Removed base lines.
    pub removed: Vec<u32>,
}

/// The diff could not be computed.
#[derive(Debug, thiserror::Error)]
pub enum DiffError {
    /// `git` failed.
    #[error("git diff failed: {message}")]
    Git {
        /// Plain git error output.
        message: String,
    },
    /// Running `git` failed.
    #[error("could not run git")]
    Io(#[from] std::io::Error),
    /// Two hunks share an id.
    #[error("hunk id {id} appears twice in the diff")]
    DuplicateHunk {
        /// The id.
        id: HunkId,
    },
    /// `git diff` ran longer than allowed and was killed.
    #[error("git diff did not finish within {seconds} seconds")]
    Timeout {
        /// The limit in seconds.
        seconds: u64,
    },
    /// Declared but not implemented yet.
    #[error("the analysis operation {operation} is not implemented yet")]
    NotImplemented {
        /// The operation.
        operation: &'static str,
    },
}

impl FileDiff {
    /// Path on `side`; `None` on the head side of a deleted file and the base side of an added file.
    pub fn path_on(&self, side: LineSide) -> Option<&str> {
        match (side, self.status) {
            (LineSide::Head, FileStatus::Deleted) | (LineSide::Base, FileStatus::Added) => None,
            (LineSide::Head, _) => Some(&self.path),
            (LineSide::Base, _) => Some(self.old_path.as_deref().unwrap_or(&self.path)),
        }
    }
}

/// A hunk with the file it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HunkAt<'a> {
    /// The file.
    pub file: &'a FileDiff,
    /// The hunk.
    pub hunk: &'a Hunk,
}

impl HunkAt<'_> {
    /// The file's path on `side` (see [`FileDiff::path_on`]).
    pub fn path(&self, side: LineSide) -> Option<&str> {
        self.file.path_on(side)
    }
}

/// Lookup of hunks by id and of removed base lines, over the diff of one pull request.
#[derive(Clone, Debug)]
pub struct DiffIndex<'a> {
    files: &'a [FileDiff],
    hunks: HashMap<&'a HunkId, HunkAt<'a>>,
}

impl<'a> DiffIndex<'a> {
    /// Indexes `files`; hunk ids must be unique.
    pub fn new(files: &'a [FileDiff]) -> Result<Self, DiffError> {
        let mut hunks = HashMap::new();
        for file in files {
            for hunk in &file.hunks {
                if hunks.insert(&hunk.id, HunkAt { file, hunk }).is_some() {
                    return Err(DiffError::DuplicateHunk {
                        id: hunk.id.clone(),
                    });
                }
            }
        }
        Ok(Self { files, hunks })
    }

    /// The hunk with this id.
    pub fn hunk(&self, id: &HunkId) -> Option<HunkAt<'a>> {
        self.hunks.get(id).copied()
    }

    /// The files of the diff.
    pub fn files(&self) -> &'a [FileDiff] {
        self.files
    }

    /// Head line after which the removed `base_line` of `base_path` is shown (0 = before line 1); `None` if the
    /// line was not removed or the file no longer exists at head.
    pub fn deletion_marker(&self, base_path: &str, base_line: u32) -> Option<u32> {
        let file = self
            .files
            .iter()
            .find(|file| file.path_on(LineSide::Base) == Some(base_path))?;
        file.path_on(LineSide::Head)?;
        let hunk = file
            .hunks
            .iter()
            .find(|hunk| hunk.removed.contains(&base_line))?;
        Some(if hunk.head.len == 0 {
            hunk.head.start
        } else {
            hunk.head.start.saturating_sub(1)
        })
    }
}

/// Most `git diff` output read (a pull request's diff is attacker-sized input).
pub const MAX_DIFF_BYTES: u64 = 64 * 1024 * 1024;
/// Longest `git diff` may run before it is killed.
pub const DIFF_TIMEOUT: Duration = Duration::from_secs(120);
/// Interval between checks whether git has exited.
const WAIT_POLL: Duration = Duration::from_millis(10);
/// Most `git` error output kept.
const MAX_STDERR_BYTES: u64 = 64 * 1024;

/// The changed files between `base` and `head` in the repository at `repo` (read-only); fails when the diff
/// output exceeds [`MAX_DIFF_BYTES`] or git runs longer than [`DIFF_TIMEOUT`].
pub fn diff(repo: &Path, base: &CommitSha, head: &CommitSha) -> Result<Vec<FileDiff>, DiffError> {
    diff_with(OsStr::new("git"), repo, base, head, DIFF_TIMEOUT)
}

fn diff_with(
    git: &OsStr,
    repo: &Path,
    base: &CommitSha,
    head: &CommitSha,
    timeout: Duration,
) -> Result<Vec<FileDiff>, DiffError> {
    // Fixed options and no user or system config: hunks (and their ids) depend only on the two commits.
    let mut child = Command::new(git)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "core.quotePath=false",
            "diff",
            "-U0",
            "-M",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--diff-algorithm=myers",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--end-of-options",
        ])
        .arg(&base.0)
        .arg(&head.0)
        .arg("--")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let child = Arc::new(Mutex::new(child));
    // The watchdog kills git at the deadline; the reads below then end at EOF.
    let (finished, deadline) = mpsc::channel::<()>();
    let watchdog = {
        let child = Arc::clone(&child);
        thread::spawn(move || {
            let expired = matches!(
                deadline.recv_timeout(timeout),
                Err(mpsc::RecvTimeoutError::Timeout)
            );
            if expired {
                let _ = child.lock().unwrap_or_else(PoisonError::into_inner).kill();
            }
            expired
        })
    };
    let stderr_reader = thread::spawn(move || {
        let mut stderr = Vec::new();
        if let Some(pipe) = stderr_pipe {
            let _ = pipe.take(MAX_STDERR_BYTES).read_to_end(&mut stderr);
        }
        stderr
    });
    let mut stdout = Vec::new();
    let read = match stdout_pipe {
        Some(pipe) => pipe
            .take(MAX_DIFF_BYTES + 1)
            .read_to_end(&mut stdout)
            .map(|_| ()),
        None => Ok(()),
    };
    let too_large = stdout.len() > usize::try_from(MAX_DIFF_BYTES).unwrap_or(usize::MAX);
    if too_large || read.is_err() {
        // git may block on a full pipe; it is no longer needed.
        let _ = child.lock().unwrap_or_else(PoisonError::into_inner).kill();
    }
    // Poll instead of a blocking wait: the watchdog needs the lock to kill a git that closed stdout but runs on.
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
    let stderr = stderr_reader.join().unwrap_or_default();
    if timed_out {
        return Err(DiffError::Timeout {
            seconds: timeout.as_secs(),
        });
    }
    read?;
    let status = status?;
    if too_large {
        return Err(DiffError::Git {
            message: format!("the diff is larger than {MAX_DIFF_BYTES} bytes"),
        });
    }
    if !status.success() {
        return Err(DiffError::Git {
            message: String::from_utf8_lossy(&stderr).trim().to_owned(),
        });
    }
    parse(&String::from_utf8_lossy(&stdout))
}

/// Parses `git diff -U0` output and assigns hunk ids in path order, then file order.
fn parse(text: &str) -> Result<Vec<FileDiff>, DiffError> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            files.push(FileDiff {
                path: header_path(rest).unwrap_or_default(),
                old_path: None,
                status: FileStatus::Modified,
                binary: false,
                hunks: Vec::new(),
            });
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if line.starts_with("new file mode ") {
            file.status = FileStatus::Added;
        } else if line.starts_with("deleted file mode ") {
            file.status = FileStatus::Deleted;
        } else if let Some(from) = line.strip_prefix("rename from ") {
            file.status = FileStatus::Renamed;
            file.old_path = Some(unquote(from));
        } else if let Some(to) = line.strip_prefix("rename to ") {
            file.path = unquote(to);
        } else if let Some(old) = line.strip_prefix("--- ") {
            if file.status == FileStatus::Deleted {
                file.path = strip_side_prefix(&unquote(file_header_name(old)), "a/");
            }
        } else if let Some(new) = line.strip_prefix("+++ ") {
            if file.status != FileStatus::Deleted {
                file.path = strip_side_prefix(&unquote(file_header_name(new)), "b/");
            }
        } else if line.starts_with("Binary files ") {
            file.binary = true;
        } else if let Some(header) = line.strip_prefix("@@ ") {
            let (base, head) = hunk_header(header).ok_or_else(|| DiffError::Git {
                message: format!("unexpected hunk header in the diff output: {line}"),
            })?;
            // Consume the hunk's lines: a removed line "-- x" reads "--- x" and is not a header.
            let (mut old, mut new) = (base.len, head.len);
            while old > 0 || new > 0 {
                match lines.next() {
                    Some(body) if body.starts_with('-') => old = old.saturating_sub(1),
                    Some(body) if body.starts_with('+') => new = new.saturating_sub(1),
                    Some(_) => {}
                    None => break,
                }
            }
            file.hunks.push(Hunk {
                id: HunkId(String::new()),
                base,
                head,
                added: (head.start..head.start + head.len).collect(),
                removed: (base.start..base.start + base.len).collect(),
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let hunks = files.iter_mut().flat_map(|file| file.hunks.iter_mut());
    for (index, hunk) in hunks.enumerate() {
        hunk.id = HunkId(format!("h{}", index + 1));
    }
    Ok(files)
}

/// The path of `a/<p> b/<p>` (quoted, or two equal halves); renames and deletions get theirs from later lines.
fn header_path(rest: &str) -> Option<String> {
    if rest.starts_with('"') {
        let end = closing_quote(rest)?;
        return Some(strip_side_prefix(&unquote(&rest[..=end]), "a/"));
    }
    let len = rest.len().checked_sub(5)? / 2;
    let old = rest.strip_prefix("a/")?.get(..len)?;
    let new = rest.get(2 + len..)?.strip_prefix(" b/")?;
    (old == new).then(|| old.to_owned())
}

/// The name of a `---`/`+++` line: git ends a name containing a space with a tab separator, after the closing
/// quote of a quoted name (a tab inside a name is always quoted).
fn file_header_name(text: &str) -> &str {
    if text.starts_with('"') {
        closing_quote(text).map_or(text, |end| &text[..=end])
    } else {
        text.strip_suffix('\t').unwrap_or(text)
    }
}

fn closing_quote(quoted: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, c) in quoted.char_indices().skip(1) {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return Some(index),
            _ => {}
        }
    }
    None
}

fn strip_side_prefix(path: &str, prefix: &str) -> String {
    path.strip_prefix(prefix).unwrap_or(path).to_owned()
}

/// Undoes git's C-style path quoting (`"a/x\tb\303\251"`); unquoted input is returned as is.
fn unquote(text: &str) -> String {
    let Some(inner) = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return text.to_owned();
    };
    let mut bytes = Vec::with_capacity(inner.len());
    let mut input = inner.bytes().peekable();
    while let Some(byte) = input.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        match input.next() {
            Some(b'a') => bytes.push(0x07),
            Some(b'b') => bytes.push(0x08),
            Some(b't') => bytes.push(b'\t'),
            Some(b'n') => bytes.push(b'\n'),
            Some(b'v') => bytes.push(0x0b),
            Some(b'f') => bytes.push(0x0c),
            Some(b'r') => bytes.push(b'\r'),
            Some(digit @ b'0'..=b'7') => {
                let mut value = u32::from(digit - b'0');
                for _ in 0..2 {
                    if let Some(next @ b'0'..=b'7') = input.peek().copied() {
                        value = value * 8 + u32::from(next - b'0');
                        input.next();
                    }
                }
                bytes.push(u8::try_from(value).unwrap_or(u8::MAX));
            }
            Some(other) => bytes.push(other),
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `-a[,b] +c[,d] @@ …` → base and head spans (an omitted length is 1).
fn hunk_header(header: &str) -> Option<(LineSpan, LineSpan)> {
    let mut parts = header.split(' ');
    let base = span_of(parts.next()?.strip_prefix('-')?)?;
    let head = span_of(parts.next()?.strip_prefix('+')?)?;
    (parts.next()? == "@@").then_some((base, head))
}

fn span_of(text: &str) -> Option<LineSpan> {
    let (start, len) = match text.split_once(',') {
        Some((start, len)) => (start.parse().ok()?, len.parse().ok()?),
        None => (text.parse().ok()?, 1),
    };
    Some(LineSpan { start, len })
}

#[cfg(test)]
mod tests {
    use contracts::common::LineSide;

    use super::*;

    fn span(start: u32, len: u32) -> LineSpan {
        LineSpan { start, len }
    }

    fn hunk(id: &str, base: LineSpan, head: LineSpan) -> Hunk {
        Hunk {
            id: HunkId(id.to_owned()),
            base,
            head,
            added: (head.start..head.start + head.len).collect(),
            removed: (base.start..base.start + base.len).collect(),
        }
    }

    fn file(path: &str, old_path: Option<&str>, status: FileStatus, hunks: Vec<Hunk>) -> FileDiff {
        FileDiff {
            path: path.to_owned(),
            old_path: old_path.map(str::to_owned),
            status,
            binary: false,
            hunks,
        }
    }

    fn files() -> Vec<FileDiff> {
        vec![
            file(
                "src/a.rs",
                None,
                FileStatus::Modified,
                vec![hunk("h1", span(10, 3), span(10, 3))],
            ),
            file(
                "src/b.rs",
                None,
                FileStatus::Modified,
                vec![hunk("h2", span(10, 3), span(10, 3))],
            ),
            file(
                "src/new.rs",
                Some("src/old.rs"),
                FileStatus::Renamed,
                vec![hunk("h3", span(5, 1), span(5, 2))],
            ),
            file(
                "src/gone.rs",
                None,
                FileStatus::Deleted,
                vec![hunk("h4", span(1, 40), span(0, 0))],
            ),
            file(
                "src/fresh.rs",
                None,
                FileStatus::Added,
                vec![hunk("h5", span(0, 0), span(1, 9))],
            ),
            file(
                "src/edges.rs",
                None,
                FileStatus::Modified,
                vec![
                    hunk("h6", span(1, 2), span(0, 0)),
                    hunk("h7", span(20, 3), span(18, 0)),
                ],
            ),
        ]
    }

    #[test]
    fn hunks_resolve_to_their_own_file_on_both_sides() {
        let files = files();
        let index = DiffIndex::new(&files).expect("unique hunk ids");
        let a = index.hunk(&HunkId("h1".to_owned())).expect("h1");
        let b = index.hunk(&HunkId("h2".to_owned())).expect("h2");
        assert_eq!(a.hunk.base, b.hunk.base, "same line numbers in two files");
        assert_eq!(a.path(LineSide::Head), Some("src/a.rs"));
        assert_eq!(b.path(LineSide::Head), Some("src/b.rs"));
        let renamed = index.hunk(&HunkId("h3".to_owned())).expect("h3");
        assert_eq!(renamed.path(LineSide::Base), Some("src/old.rs"));
        assert_eq!(renamed.path(LineSide::Head), Some("src/new.rs"));
        let deleted = index.hunk(&HunkId("h4".to_owned())).expect("h4");
        assert_eq!(deleted.path(LineSide::Base), Some("src/gone.rs"));
        assert_eq!(deleted.path(LineSide::Head), None);
        let added = index.hunk(&HunkId("h5".to_owned())).expect("h5");
        assert_eq!(added.path(LineSide::Base), None);
        assert!(index.hunk(&HunkId("h99".to_owned())).is_none());
    }

    #[test]
    fn duplicate_hunk_ids_are_rejected() {
        let mut files = files();
        files[1].hunks[0].id = HunkId("h1".to_owned());
        assert!(matches!(
            DiffIndex::new(&files),
            Err(DiffError::DuplicateHunk { .. })
        ));
    }

    #[test]
    fn removed_lines_map_to_head_deletion_markers() {
        let files = files();
        let index = DiffIndex::new(&files).expect("index");
        // Removed at the start of the file: marker before line 1.
        assert_eq!(index.deletion_marker("src/edges.rs", 1), Some(0));
        assert_eq!(index.deletion_marker("src/edges.rs", 2), Some(0));
        // Removed at the end: marker after the last kept head line.
        assert_eq!(index.deletion_marker("src/edges.rs", 21), Some(18));
        // Modified in place: removed lines sit before the replacing head lines.
        assert_eq!(index.deletion_marker("src/a.rs", 11), Some(9));
        // Renamed file: looked up by its base path.
        assert_eq!(index.deletion_marker("src/old.rs", 5), Some(4));
        // A deleted file has no head location.
        assert_eq!(index.deletion_marker("src/gone.rs", 3), None);
        // Not a removed line.
        assert_eq!(index.deletion_marker("src/edges.rs", 10), None);
    }

    fn fixture_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482/repo")
    }

    #[derive(serde::Deserialize)]
    struct FixtureHunk {
        id: String,
        path: String,
        old_path: Option<String>,
        status: String,
        base: FixtureSpan,
        head: FixtureSpan,
    }

    #[derive(serde::Deserialize)]
    struct FixtureSpan {
        start: u32,
        len: u32,
    }

    #[test]
    fn fixture_hunks_match_hunks_json() {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo = dir.path().join("repo");
        let built = Command::new(fixture_dir().join("build-repo.sh"))
            .arg(&repo)
            .output()
            .expect("run build-repo.sh");
        assert!(built.status.success(), "{built:?}");
        let commits: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(fixture_dir().join("commits.json")).expect("commits.json"),
        )
        .expect("commits JSON");
        let sha = |key: &str| CommitSha(commits[key].as_str().expect("sha").to_owned());

        let files = diff(&repo, &sha("base"), &sha("head")).expect("diff");

        let expected: Vec<FixtureHunk> = serde_json::from_str(
            &std::fs::read_to_string(fixture_dir().join("hunks.json")).expect("hunks.json"),
        )
        .expect("hunks JSON");
        let actual: Vec<(String, String, Option<String>, String, LineSpan, LineSpan)> = files
            .iter()
            .flat_map(|file| {
                file.hunks.iter().map(move |hunk| {
                    let status = match file.status {
                        FileStatus::Added => "added",
                        FileStatus::Modified => "modified",
                        FileStatus::Deleted => "deleted",
                        FileStatus::Renamed => "renamed",
                    };
                    (
                        hunk.id.0.clone(),
                        file.path.clone(),
                        file.old_path.clone(),
                        status.to_owned(),
                        hunk.base,
                        hunk.head,
                    )
                })
            })
            .collect();
        let expected: Vec<_> = expected
            .into_iter()
            .map(|h| {
                (
                    h.id,
                    h.path,
                    h.old_path,
                    h.status,
                    span(h.base.start, h.base.len),
                    span(h.head.start, h.head.len),
                )
            })
            .collect();
        assert_eq!(actual, expected);
        let deleted = files
            .iter()
            .find(|file| file.path == "tests/send_once.rs")
            .expect("deleted file");
        assert_eq!(deleted.path_on(LineSide::Head), None);
        assert_eq!(deleted.hunks[0].removed, (1..=10).collect::<Vec<_>>());
        assert!(deleted.hunks[0].added.is_empty());
    }

    #[test]
    fn parses_quoted_paths_binary_files_and_omitted_lengths() {
        let text = concat!(
            "diff --git \"a/dir/sp\\303\\251cial\\tname.rs\" \"b/dir/sp\\303\\251cial\\tname.rs\"\n",
            "index 1..2 100644\n",
            "--- \"a/dir/sp\\303\\251cial\\tname.rs\"\n",
            "+++ \"b/dir/sp\\303\\251cial\\tname.rs\"\n",
            "@@ -3 +3,2 @@ fn x()\n",
            "-old\n",
            "+new\n",
            "+more\n",
            "diff --git a/assets/logo.png b/assets/logo.png\n",
            "index 3..4 100644\n",
            "Binary files a/assets/logo.png and b/assets/logo.png differ\n",
            "diff --git a/with space.txt b/with space.txt\n",
            "old mode 100644\n",
            "new mode 100755\n",
        );
        let files = parse(text).expect("parse");
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "assets/logo.png",
                "dir/sp\u{e9}cial\tname.rs",
                "with space.txt"
            ]
        );
        assert!(files[0].binary && files[0].hunks.is_empty());
        let hunk = &files[1].hunks[0];
        assert_eq!(
            (hunk.id.0.as_str(), hunk.base, hunk.head),
            ("h1", span(3, 1), span(3, 2))
        );
        assert_eq!(
            (hunk.removed.clone(), hunk.added.clone()),
            (vec![3], vec![3, 4])
        );
        assert!(files[2].hunks.is_empty());
    }

    #[test]
    fn hunk_lines_that_look_like_headers_stay_content() {
        let text = concat!(
            "diff --git a/notes.md b/notes.md\n",
            "--- a/notes.md\n",
            "+++ b/notes.md\n",
            "@@ -1,2 +0,0 @@\n",
            "--- a/elsewhere.md\n",
            "-diff --git a/x b/x\n",
            "@@ -9 +7 @@\n",
            "-++ b/notes.md\n",
            "++++ b/other.md\n",
        );
        let files = parse(text).expect("parse");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "notes.md");
        let spans: Vec<_> = files[0].hunks.iter().map(|h| (h.base, h.head)).collect();
        assert_eq!(spans, [(span(1, 2), span(0, 0)), (span(9, 1), span(7, 1))]);
    }

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

    #[test]
    fn paths_with_spaces_from_real_git_output() {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        let write = |name: &str, text: &str| std::fs::write(repo.join(name), text).expect("write");
        write("mod file.txt", "a\nb\nc\nd\ne\nf\n");
        write("del file.txt", "x\n");
        write("old name.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
        // A space and a double quote: git quotes the name and still appends the tab separator.
        write("say \"hi\".txt", "a\n");
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-q", "-m", "base"]);
        let base = CommitSha(git(repo, &["rev-parse", "HEAD"]));
        write("mod file.txt", "a\nB\nc\nd\ne\nf\n");
        std::fs::remove_file(repo.join("del file.txt")).expect("remove");
        write("new file.txt", "n\n");
        write("say \"hi\".txt", "b\n");
        git(repo, &["mv", "old name.txt", "new name.txt"]);
        write("new name.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n");
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-q", "-m", "head"]);
        let head = CommitSha(git(repo, &["rev-parse", "HEAD"]));

        let files = diff(repo, &base, &head).expect("diff");
        let summary: Vec<(&str, Option<&str>, FileStatus, usize)> = files
            .iter()
            .map(|f| {
                (
                    f.path.as_str(),
                    f.old_path.as_deref(),
                    f.status,
                    f.hunks.len(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("del file.txt", None, FileStatus::Deleted, 1),
                ("mod file.txt", None, FileStatus::Modified, 1),
                ("new file.txt", None, FileStatus::Added, 1),
                ("new name.txt", Some("old name.txt"), FileStatus::Renamed, 1),
                ("say \"hi\".txt", None, FileStatus::Modified, 1),
            ]
        );
    }

    /// Runs `diff_with` against a stub `git` with the shell `script`, a 300 ms deadline.
    fn diff_with_stub(script: &str) -> (Result<Vec<FileDiff>, DiffError>, Duration) {
        let dir = tempfile::tempdir().expect("temp dir");
        let stub = dir.path().join("git");
        std::fs::write(&stub, script).expect("stub");
        let made_executable = Command::new("chmod")
            .arg("+x")
            .arg(&stub)
            .status()
            .expect("chmod");
        assert!(made_executable.success());
        let sha = CommitSha("0".repeat(40));
        let started = std::time::Instant::now();
        let result = diff_with(
            stub.as_os_str(),
            dir.path(),
            &sha,
            &sha,
            Duration::from_millis(300),
        );
        (result, started.elapsed())
    }

    #[test]
    fn hanging_git_is_killed_at_the_deadline() {
        let (result, elapsed) = diff_with_stub("#!/bin/sh\nexec sleep 30\n");
        assert!(
            matches!(result, Err(DiffError::Timeout { .. })),
            "{result:?}"
        );
        assert!(elapsed < Duration::from_secs(10));
    }

    #[test]
    fn git_that_closes_stdout_and_hangs_is_killed_at_the_deadline() {
        let (result, elapsed) = diff_with_stub("#!/bin/sh\nexec 1>&-\nexec sleep 30\n");
        assert!(
            matches!(result, Err(DiffError::Timeout { .. })),
            "{result:?}"
        );
        assert!(elapsed < Duration::from_secs(10));
    }

    #[test]
    fn git_failure_is_reported() {
        let dir = tempfile::tempdir().expect("temp dir");
        let sha = CommitSha("0".repeat(40));
        assert!(matches!(
            diff(dir.path(), &sha, &sha),
            Err(DiffError::Git { .. })
        ));
    }
}
