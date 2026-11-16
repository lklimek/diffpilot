//! Hunks of a pull request from `git diff` between base and head. Body filled in by WP2-INGEST.

use std::collections::HashMap;
use std::path::Path;

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

/// The changed files between `base` and `head` in the repository at `repo` (read-only).
pub fn diff(repo: &Path, base: &CommitSha, head: &CommitSha) -> Result<Vec<FileDiff>, DiffError> {
    let _ = (repo, base, head);
    Err(DiffError::NotImplemented {
        operation: "diff::diff",
    })
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
}
