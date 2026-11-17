//! Line facts of files at the pinned base and head commits, read through the bounded [`crate::blob`] reader.

use std::path::Path;

use contracts::common::LineSide;
use contracts::ids::CommitSha;

use super::{FileFacts, LineFacts, VerifyError};
use crate::blob::{self, Blob, Limits};

/// Most bytes one load reads (a file named twice counts twice); with the per-file limit of
/// [`blob::MAX_BLOB_BYTES`] this bounds the memory of the facts (at most one blank-line entry per byte read).
pub const MAX_FACTS_BYTES: u64 = 2 * blob::MAX_BLOB_BYTES;

pub(super) fn load(
    repo: &Path,
    base: &CommitSha,
    head: &CommitSha,
    files: &[(LineSide, String)],
) -> Result<LineFacts, VerifyError> {
    let mut wanted: Vec<&(LineSide, String)> = files.iter().collect();
    wanted.sort();
    wanted.dedup();
    let requests: Vec<(&CommitSha, &str)> = wanted
        .iter()
        .map(|(side, path)| {
            let commit = match side {
                LineSide::Head => head,
                LineSide::Base => base,
            };
            (commit, path.as_str())
        })
        .collect();
    let limits = Limits {
        max_blob_bytes: blob::MAX_BLOB_BYTES,
        total_bytes: MAX_FACTS_BYTES,
        timeout: blob::READ_TIMEOUT,
    };
    let answers = blob::read_blobs(repo, &requests, limits)?;
    let mut facts = LineFacts::default();
    for ((side, path), answer) in wanted.into_iter().zip(answers) {
        match answer {
            Blob::Content(bytes) => facts.insert(*side, path, count_lines(&bytes)),
            Blob::TooLarge { size } | Blob::OverBudget { size } => {
                facts.insert_unread(*side, path, size);
            }
            Blob::Missing => {}
        }
    }
    Ok(facts)
}

/// Line count and blank lines of a file, by the rules of `lines::line_window` (`\n` separated, one trailing
/// newline ignored, `\r` before it ignored).
fn count_lines(bytes: &[u8]) -> FileFacts {
    let mut facts = FileFacts::default();
    if bytes.is_empty() {
        return facts;
    }
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    for line in body.split(|&byte| byte == b'\n') {
        facts.line_count = facts.line_count.saturating_add(1);
        if line.iter().all(u8::is_ascii_whitespace) {
            facts.blank_lines.push(facts.line_count);
        }
    }
    facts
}

#[cfg(test)]
mod tests {
    use std::process::Command;

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
        let huge = vec![b'\n'; usize::try_from(blob::MAX_BLOB_BYTES).expect("size") + 1];
        std::fs::write(repo.join("huge.txt"), huge).expect("write");
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
                request(LineSide::Head, "huge.txt"),
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
            (LineSide::Head, "huge.txt"),
        ] {
            assert!(loaded.get(side, path).is_none(), "{side:?} {path}");
        }
        assert_eq!(
            loaded.unread_size(LineSide::Head, "huge.txt"),
            Some(blob::MAX_BLOB_BYTES + 1),
            "too large to read: recorded, not counted"
        );
    }

    #[test]
    fn line_facts_load_reports_a_broken_repository() {
        let dir = tempfile::tempdir().expect("temp dir");
        let sha = CommitSha("0".repeat(40));
        let result = LineFacts::load(dir.path(), &sha, &sha, &[(LineSide::Head, "a".to_owned())]);
        assert!(matches!(result, Err(VerifyError::Read(_))), "{result:?}");
    }
}
