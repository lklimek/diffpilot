//! Counters over material that is not analysed (lines removed from tests, changed dependencies), each with the
//! lines it was computed from.

pub(crate) mod cargo;

use std::collections::BTreeMap;
use std::path::Path;

use contracts::api::tour::Counters;
use contracts::common::{Evidence, FileRanges, LineSide, NotAnalysedCategory};
use contracts::ids::CommitSha;
use serde::{Deserialize, Serialize};

use crate::diff::FileDiff;
use crate::lines::ranges;
use crate::tech::{Classified, DependencyFile, dependency_file};

/// Counting failed.
#[derive(Debug, thiserror::Error)]
pub enum CountersError {
    /// `git` failed.
    #[error("git failed while counting: {message}")]
    Git {
        /// Plain git error output.
        message: String,
    },
    /// Running `git` failed.
    #[error("could not run git")]
    Io(#[from] std::io::Error),
    /// Declared but not implemented yet.
    #[error("the analysis operation {operation} is not implemented yet")]
    NotImplemented {
        /// The operation.
        operation: &'static str,
    },
}

/// Deterministic counters for the PR context.
pub fn count(
    repo: &Path,
    base: &CommitSha,
    head: &CommitSha,
    files: &[FileDiff],
    classified: &[Classified],
) -> Result<Counters, CountersError> {
    let _ = (repo, base, head, files, classified);
    Err(CountersError::NotImplemented {
        operation: "counters::count",
    })
}

/// The counters with their evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CounterFacts {
    /// The counters of the PR context.
    pub counters: Counters,
    /// Removed lines of test files (evidence of `lines_removed_from_tests`).
    pub removed_test_lines: Vec<FileRanges>,
    /// Changed dependencies with the changed lines naming them (evidence of `dependencies_changed`).
    pub dependencies: Vec<DependencyChange>,
    /// Changed dependency files of ecosystems the pre-pass does not read: their dependencies are not checked.
    pub dependency_files_not_checked: Vec<String>,
}

/// A changed dependency.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyChange {
    /// Dependency (package) name.
    pub name: String,
    /// Changed lines naming it.
    pub evidence: Vec<Evidence>,
}

/// Counters of `files` given the not-analysed material `classified`; `text` gives a side's file text (dependency
/// files without text are listed as not checked).
pub fn compute<'t>(
    files: &[FileDiff],
    classified: &[Classified],
    text: &dyn Fn(LineSide, &str) -> Option<&'t str>,
) -> CounterFacts {
    let mut removed_test_lines = Vec::new();
    let mut dependencies: BTreeMap<String, Vec<Evidence>> = BTreeMap::new();
    let mut not_checked = Vec::new();
    for file in files {
        let test_file = file.path_on(LineSide::Base).filter(|path| {
            classified.iter().any(|item| {
                item.side == LineSide::Base
                    && item.file == *path
                    && item.category == NotAnalysedCategory::Tests
            })
        });
        if let Some(path) = test_file {
            let mut removed: Vec<u32> = file
                .hunks
                .iter()
                .flat_map(|hunk| hunk.removed.iter().copied())
                .collect();
            removed.sort_unstable();
            if !removed.is_empty() {
                removed_test_lines.push(FileRanges {
                    file: path.to_owned(),
                    side: LineSide::Base,
                    ranges: ranges(&removed),
                });
            }
        }
        for side in [LineSide::Base, LineSide::Head] {
            let Some((path, kind)) = file
                .path_on(side)
                .and_then(|path| Some((path, dependency_file(path)?)))
            else {
                continue;
            };
            let changed: Vec<u32> = file
                .hunks
                .iter()
                .flat_map(|hunk| match side {
                    LineSide::Base => &hunk.removed,
                    LineSide::Head => &hunk.added,
                })
                .copied()
                .collect();
            if changed.is_empty() {
                continue;
            }
            let names = match (kind, text(side, path)) {
                (DependencyFile::CargoToml, Some(text)) => manifest_changes(text, &changed),
                (DependencyFile::CargoLock, Some(text)) => lock_changes(text, &changed),
                (DependencyFile::Other, _) | (_, None) => {
                    not_checked.push(path.to_owned());
                    continue;
                }
            };
            for (name, line) in names {
                dependencies.entry(name).or_default().push(Evidence {
                    file: path.to_owned(),
                    side,
                    line,
                });
            }
        }
    }
    not_checked.sort();
    not_checked.dedup();
    let lines_removed = removed_test_lines
        .iter()
        .flat_map(|file| &file.ranges)
        .map(|range| range.end_line - range.start_line + 1)
        .fold(0, u32::saturating_add);
    CounterFacts {
        counters: Counters {
            lines_removed_from_tests: lines_removed,
            dependencies_changed: dependencies.keys().cloned().collect(),
        },
        removed_test_lines,
        dependencies: dependencies
            .into_iter()
            .map(|(name, evidence)| DependencyChange { name, evidence })
            .collect(),
        dependency_files_not_checked: not_checked,
    }
}

/// Dependencies named by the changed lines of a `Cargo.toml`.
fn manifest_changes(text: &str, changed: &[u32]) -> Vec<(String, u32)> {
    cargo::toml_lines(text)
        .iter()
        .filter(|line| changed.contains(&line.number) && !line.text.trim().is_empty())
        .filter_map(|line| Some((cargo::dependency_name(line)?.to_owned(), line.number)))
        .collect()
}

/// Packages from a registry or git source whose own lines (name, version, source, checksum) changed in a
/// `Cargo.lock`; workspace packages and changes of a package's dependency list do not count.
fn lock_changes(text: &str, changed: &[u32]) -> Vec<(String, u32)> {
    let packages = cargo::lock_packages(text);
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for &number in changed {
        let blank = usize::try_from(number)
            .ok()
            .and_then(|n| lines.get(n.checked_sub(1)?))
            .is_none_or(|line| line.trim().is_empty());
        let package = packages
            .iter()
            .find(|package| (package.lines.0..=package.lines.1).contains(&number));
        if let Some(package) = package.filter(|package| {
            !blank && package.source.is_some() && !package.dependency_list.contains(&number)
        }) && let Some(name) = package.name
        {
            out.push((name.to_owned(), number));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use contracts::common::{LineRange, ProvenanceBadge};
    use contracts::ids::HunkId;

    use super::*;
    use crate::diff::{FileStatus, Hunk, LineSpan};

    fn hunk(base: (u32, u32), head: (u32, u32)) -> Hunk {
        let base = LineSpan {
            start: base.0,
            len: base.1,
        };
        let head = LineSpan {
            start: head.0,
            len: head.1,
        };
        Hunk {
            id: HunkId("h".to_owned()),
            base,
            head,
            added: (head.start..head.start + head.len).collect(),
            removed: (base.start..base.start + base.len).collect(),
        }
    }

    fn file(path: &str, status: FileStatus, hunks: Vec<Hunk>) -> FileDiff {
        FileDiff {
            path: path.to_owned(),
            old_path: None,
            status,
            binary: false,
            hunks,
        }
    }

    fn tests_item(path: &str) -> Classified {
        Classified {
            file: path.to_owned(),
            side: LineSide::Base,
            ranges: None,
            category: NotAnalysedCategory::Tests,
            badge: ProvenanceBadge::ClassifiedByPath,
            rule: "tests/**".to_owned(),
        }
    }

    #[test]
    fn removed_test_lines_are_counted_with_their_ranges() {
        let files = [
            file(
                "tests/a.rs",
                FileStatus::Deleted,
                vec![hunk((1, 10), (0, 0))],
            ),
            file(
                "tests/b.rs",
                FileStatus::Modified,
                vec![hunk((4, 2), (4, 1)), hunk((9, 0), (9, 3))],
            ),
            file("src/c.rs", FileStatus::Modified, vec![hunk((1, 5), (1, 0))]),
        ];
        let classified = [tests_item("tests/a.rs"), tests_item("tests/b.rs")];
        let facts = compute(&files, &classified, &|_, _| None);
        assert_eq!(facts.counters.lines_removed_from_tests, 12);
        assert_eq!(
            facts.removed_test_lines[1],
            FileRanges {
                file: "tests/b.rs".to_owned(),
                side: LineSide::Base,
                ranges: vec![LineRange {
                    start_line: 4,
                    end_line: 5
                }]
            }
        );
        assert!(facts.counters.dependencies_changed.is_empty());
    }

    #[test]
    fn changed_dependencies_come_from_cargo_files_with_evidence() {
        let base_lock = "[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\n \"tokio\",\n]\n\n[[package]]\nname = \"tokio\"\nversion = \"1.0.0\"\nsource = \"registry+x\"\n";
        let head_lock = "[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\n \"serde\",\n \"tokio\",\n]\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\nsource = \"registry+x\"\n\n[[package]]\nname = \"tokio\"\nversion = \"1.0.1\"\nsource = \"registry+x\"\n";
        let base_toml =
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ntokio = \"1\"\n";
        let head_toml = "[package]\nname = \"app\"\nversion = \"0.2.0\"\n\n[dependencies]\nserde = \"1\"\ntokio = \"1\"\n";
        let files = [
            file(
                "Cargo.lock",
                FileStatus::Modified,
                vec![
                    hunk((4, 0), (5, 1)),
                    hunk((7, 0), (8, 6)),
                    hunk((10, 1), (16, 1)),
                ],
            ),
            file(
                "Cargo.toml",
                FileStatus::Modified,
                vec![hunk((3, 1), (3, 1)), hunk((5, 0), (6, 1))],
            ),
            file(
                "web/package.json",
                FileStatus::Modified,
                vec![hunk((3, 1), (3, 1))],
            ),
        ];
        let text = |side: LineSide, path: &str| match (side, path) {
            (LineSide::Base, "Cargo.lock") => Some(base_lock),
            (LineSide::Head, "Cargo.lock") => Some(head_lock),
            (LineSide::Base, "Cargo.toml") => Some(base_toml),
            (LineSide::Head, "Cargo.toml") => Some(head_toml),
            _ => None,
        };
        let facts = compute(&files, &[], &text);
        assert_eq!(facts.counters.dependencies_changed, ["serde", "tokio"]);
        assert_eq!(facts.dependency_files_not_checked, ["web/package.json"]);
        let evidence = |name: &str| -> Vec<(String, LineSide, u32)> {
            facts
                .dependencies
                .iter()
                .filter(|change| change.name == name)
                .flat_map(|change| &change.evidence)
                .map(|e| (e.file.clone(), e.side, e.line))
                .collect()
        };
        let lock = |side, line| ("Cargo.lock".to_owned(), side, line);
        assert_eq!(
            evidence("tokio"),
            [lock(LineSide::Base, 10), lock(LineSide::Head, 16)],
            "the version line; the app's dependency list is not the dependency"
        );
        assert_eq!(
            evidence("serde"),
            [
                lock(LineSide::Head, 9),
                lock(LineSide::Head, 10),
                lock(LineSide::Head, 11),
                lock(LineSide::Head, 12),
                ("Cargo.toml".to_owned(), LineSide::Head, 6)
            ]
        );
        assert!(
            evidence("app").is_empty(),
            "the package version is not a dependency"
        );
    }
}
