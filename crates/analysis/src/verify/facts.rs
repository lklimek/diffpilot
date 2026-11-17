//! Reads line facts of files at the pinned base and head commits (`git cat-file --batch`, read-only).

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;

use contracts::common::LineSide;
use contracts::ids::CommitSha;

use super::{FileFacts, LineFacts, VerifyError};

/// Most `git` error output kept.
const MAX_STDERR_BYTES: u64 = 64 * 1024;

/// A repository-relative path as git stores it in a tree: no absolute path, no `.`/`..`/empty component, no
/// line break or NUL. Anything else cannot name a file of the pinned trees.
pub(super) fn is_tree_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\n', '\r', '\0'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

pub(super) fn load(
    repo: &Path,
    base: &CommitSha,
    head: &CommitSha,
    files: &[(LineSide, String)],
) -> Result<LineFacts, VerifyError> {
    let mut wanted: Vec<&(LineSide, String)> = files
        .iter()
        .filter(|(_, path)| is_tree_path(path))
        .collect();
    wanted.sort();
    wanted.dedup();
    let mut facts = LineFacts::default();
    if wanted.is_empty() {
        return Ok(facts);
    }
    let requests: String = wanted
        .iter()
        .map(|(side, path)| {
            let commit = match side {
                LineSide::Head => head,
                LineSide::Base => base,
            };
            format!("{}:{path}\n", commit.0)
        })
        .collect();
    let mut child = Command::new("git")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // A writer thread: git answers while reading, so writing everything first could fill both pipes.
    let writer = child.stdin.take().map(|mut stdin| {
        thread::spawn(move || {
            let _ = stdin.write_all(requests.as_bytes());
        })
    });
    let stderr_reader = child.stderr.take().map(|pipe| {
        thread::spawn(move || {
            let mut text = Vec::new();
            let _ = pipe.take(MAX_STDERR_BYTES).read_to_end(&mut text);
            text
        })
    });
    let read = match child.stdout.take() {
        Some(stdout) => read_answers(&mut BufReader::new(stdout), &wanted, &mut facts),
        None => Ok(()),
    };
    if read.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    let stderr = stderr_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    if !status.success() {
        return Err(VerifyError::Git {
            message: String::from_utf8_lossy(&stderr).trim().to_owned(),
        });
    }
    read?;
    Ok(facts)
}

/// Reads one answer per request: `<oid> <type> <size>` and the content, or `<name> missing`.
fn read_answers(
    stdout: &mut impl BufRead,
    wanted: &[&(LineSide, String)],
    facts: &mut LineFacts,
) -> Result<(), VerifyError> {
    let mut header = String::new();
    for (side, path) in wanted {
        header.clear();
        if stdout.read_line(&mut header)? == 0 {
            return Err(VerifyError::Git {
                message: "git cat-file ended before answering every request".to_owned(),
            });
        }
        let mut fields = header.trim_end().rsplitn(3, ' ');
        let (size, blob) = (fields.next(), fields.next() == Some("blob"));
        let Some(size) = size.and_then(|size| size.parse::<u64>().ok()) else {
            continue; // missing or ambiguous: the path is not in that tree
        };
        let file = count_lines(&mut stdout.by_ref().take(size))?;
        stdout.read_line(&mut header)?; // the newline after the content
        if blob {
            facts.insert(*side, path, file);
        }
    }
    Ok(())
}

/// Line count and blank lines of a file, by the rules of `lines::line_window` (`\n` separated, one trailing
/// newline ignored, `\r` before it ignored).
fn count_lines(content: &mut impl BufRead) -> Result<FileFacts, std::io::Error> {
    let mut facts = FileFacts::default();
    let mut line = Vec::new();
    loop {
        line.clear();
        if content.read_until(b'\n', &mut line)? == 0 {
            return Ok(facts);
        }
        facts.line_count = facts.line_count.saturating_add(1);
        if line.iter().all(u8::is_ascii_whitespace) {
            facts.blank_lines.push(facts.line_count);
        }
    }
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

    fn facts(line_count: u32, blank_lines: &[u32]) -> FileFacts {
        FileFacts {
            line_count,
            blank_lines: blank_lines.to_vec(),
        }
    }

    #[test]
    fn line_facts_load_reads_both_sides_and_skips_paths_outside_the_tree() {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join("src")).expect("mkdir");
        std::fs::write(dir.path().join("outside.txt"), "secret\n").expect("outside");
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("src/a.rs"), "a\n\nb\n").expect("write");
        std::fs::write(repo.join("gone.rs"), "x\r\n  \r\ny\nz").expect("write");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        let base = CommitSha(git(&repo, &["rev-parse", "HEAD"]));
        std::fs::write(repo.join("src/a.rs"), "a\n\nb\n\nc\n").expect("write");
        std::fs::write(repo.join("empty.rs"), "").expect("write");
        std::fs::remove_file(repo.join("gone.rs")).expect("remove");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "head"]);
        let head = CommitSha(git(&repo, &["rev-parse", "HEAD"]));

        let request = |side, path: &str| (side, path.to_owned());
        let loaded = LineFacts::load(
            &repo,
            &base,
            &head,
            &[
                request(LineSide::Head, "src/a.rs"),
                request(LineSide::Base, "src/a.rs"),
                request(LineSide::Base, "gone.rs"),
                request(LineSide::Head, "gone.rs"),
                request(LineSide::Head, "empty.rs"),
                request(LineSide::Head, "src"),
                request(LineSide::Head, "../outside.txt"),
                request(LineSide::Head, "./src/a.rs"),
                request(LineSide::Head, "/etc/passwd"),
                request(LineSide::Head, "missing.rs"),
                request(LineSide::Head, "src/a.rs"),
            ],
        )
        .expect("load");

        assert_eq!(
            loaded.get(LineSide::Head, "src/a.rs"),
            Some(&facts(5, &[2, 4]))
        );
        assert_eq!(
            loaded.get(LineSide::Base, "src/a.rs"),
            Some(&facts(3, &[2]))
        );
        assert_eq!(loaded.get(LineSide::Base, "gone.rs"), Some(&facts(4, &[2])));
        assert_eq!(loaded.get(LineSide::Head, "empty.rs"), Some(&facts(0, &[])));
        for (side, path) in [
            (LineSide::Head, "gone.rs"),
            (LineSide::Head, "src"),
            (LineSide::Head, "../outside.txt"),
            (LineSide::Head, "./src/a.rs"),
            (LineSide::Head, "/etc/passwd"),
            (LineSide::Head, "missing.rs"),
        ] {
            assert!(loaded.get(side, path).is_none(), "{side:?} {path}");
        }
    }

    #[test]
    fn line_facts_load_reports_a_broken_repository() {
        let dir = tempfile::tempdir().expect("temp dir");
        let sha = CommitSha("0".repeat(40));
        let result = LineFacts::load(dir.path(), &sha, &sha, &[(LineSide::Head, "a".to_owned())]);
        assert!(matches!(result, Err(VerifyError::Git { .. })), "{result:?}");
    }

    #[test]
    fn tree_paths_reject_traversal_and_absolute_paths() {
        for path in ["src/a.rs", "a", ".github/x.yml", "dir/..x"] {
            assert!(is_tree_path(path), "{path}");
        }
        for path in [
            "", "/a", "a/../b", "./a", "a//b", "a/", "a\nb", "a\0b", "..",
        ] {
            assert!(!is_tree_path(path), "{path:?}");
        }
    }
}
