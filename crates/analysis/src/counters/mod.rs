//! Counters over material that is not analysed (lines removed from tests, changed dependencies), each with the
//! lines it was computed from.

pub(crate) mod cargo;

use std::collections::BTreeMap;

use contracts::api::tour::Counters;
use contracts::common::{Evidence, FileRanges, LineSide, NotAnalysedCategory};
use serde::{Deserialize, Serialize};

use crate::diff::FileDiff;
use crate::lines::ranges;
use crate::tech::{Classified, DependencyFile, dependency_file};

/// The counters with their evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CounterFacts {
    /// The counters of the PR context.
    pub counters: Counters,
    /// Removed lines of test files (evidence of `lines_removed_from_tests`).
    pub removed_test_lines: Vec<FileRanges>,
    /// Changed dependencies with the changed lines naming them (evidence of `dependencies_changed`).
    pub dependencies: Vec<DependencyChange>,
    /// Changed dependency files whose dependencies are not checked: other ecosystems, files without text, Cargo
    /// files that are too large or do not parse (never counted as "0 changed").
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
            let mut changed: Vec<u32> = file
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
            changed.sort_unstable();
            let names = match (kind, text(side, path)) {
                (DependencyFile::CargoToml, Some(text)) => manifest_changes(text, &changed),
                (DependencyFile::CargoLock, Some(text)) => lock_changes(text, &changed),
                (DependencyFile::Other, _) | (_, None) => None,
            };
            let Some(names) = names else {
                not_checked.push(path.to_owned());
                continue;
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

/// Dependencies named by the sorted `changed` lines of a `Cargo.toml` (O((D + C) log C) for D declaration
/// spans); `None` when it is not checked.
fn manifest_changes(text: &str, changed: &[u32]) -> Option<Vec<(String, u32)>> {
    let declarations = cargo::manifest(text)?;
    Some(named_lines(
        text,
        changed,
        declarations
            .into_iter()
            .map(|declaration| (declaration.name, declaration.lines)),
    ))
}

/// Packages from a registry or git source whose own lines (name, version, source, checksum) are among the sorted
/// `changed` lines of a `Cargo.lock`; workspace packages and changes of a package's dependency list do not count.
/// `None` when it is not checked.
fn lock_changes(text: &str, changed: &[u32]) -> Option<Vec<(String, u32)>> {
    let packages = cargo::lock(text)?;
    Some(named_lines(
        text,
        changed,
        packages
            .into_iter()
            .filter(|package| package.source.is_some())
            .map(|package| (package.name, package.lines)),
    ))
}

/// The non-blank changed lines inside each named set of line ranges, once per name and line.
fn named_lines(
    text: &str,
    changed: &[u32],
    named: impl Iterator<Item = (String, Vec<(u32, u32)>)>,
) -> Vec<(String, u32)> {
    let lines: Vec<&str> = text.lines().collect();
    let blank = |number: u32| {
        usize::try_from(number)
            .ok()
            .and_then(|n| lines.get(n.checked_sub(1)?))
            .is_none_or(|line| line.trim().is_empty())
    };
    let mut out = Vec::new();
    for (name, ranges) in named {
        let mut hits: Vec<u32> = ranges
            .into_iter()
            .flat_map(|range| cargo::changed_within(changed, range).iter().copied())
            .filter(|&number| !blank(number))
            .collect();
        hits.sort_unstable();
        hits.dedup();
        out.extend(hits.into_iter().map(|number| (name.clone(), number)));
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

    #[test]
    fn unreadable_cargo_files_are_not_checked_never_zero() {
        let files = [
            file(
                "Cargo.toml",
                FileStatus::Modified,
                vec![hunk((2, 1), (2, 1))],
            ),
            file(
                "crates/a/Cargo.toml",
                FileStatus::Modified,
                vec![hunk((2, 1), (2, 1))],
            ),
            file(
                "Cargo.lock",
                FileStatus::Modified,
                vec![hunk((2, 1), (2, 1))],
            ),
        ];
        let broken = "[dependencies\ntokio = \"1\"\n";
        let text = |_: LineSide, path: &str| match path {
            "Cargo.toml" => Some(broken),
            _ => None,
        };
        let facts = compute(&files, &[], &text);
        assert!(facts.counters.dependencies_changed.is_empty());
        assert_eq!(
            facts.dependency_files_not_checked,
            ["Cargo.lock", "Cargo.toml", "crates/a/Cargo.toml"],
            "a parse failure and missing text are listed, not counted as 0"
        );
    }

    #[test]
    fn only_dependency_tables_name_dependencies() {
        let base = "[package]\nname = \"app\"\n\n[package.metadata.dependencies]\nfake = \"1\"\n\n[target.'cfg(unix)'.dependencies]\nnix = \"0.28\"\n\n[dependencies]\njson = { package = \"serde_json\", version = \"1.0\" }\n";
        let head = "[package]\nname = \"app\"\n\n[package.metadata.dependencies]\nfake = \"2\"\n\n[target.'cfg(unix)'.dependencies]\nnix = \"0.29\"\n\n[dependencies]\njson = { package = \"serde_json\", version = \"1.1\" }\n";
        let files = [file(
            "Cargo.toml",
            FileStatus::Modified,
            vec![
                hunk((5, 1), (5, 1)),
                hunk((8, 1), (8, 1)),
                hunk((11, 1), (11, 1)),
            ],
        )];
        let text = |side: LineSide, _: &str| Some(if side == LineSide::Base { base } else { head });
        let facts = compute(&files, &[], &text);
        assert_eq!(facts.counters.dependencies_changed, ["nix", "serde_json"]);
        assert!(facts.dependency_files_not_checked.is_empty());
    }

    #[test]
    fn large_manifest_is_counted_in_bounded_time() {
        let count: u32 = 100_000;
        let mut base = String::from("[dependencies]\n");
        let mut head = String::from("[dependencies]\n");
        for n in 0..count {
            base.push_str(&format!("crate_{n} = \"1.0\"\n"));
            head.push_str(&format!("crate_{n} = \"1.1\"\n"));
        }
        let files = [file(
            "Cargo.toml",
            FileStatus::Modified,
            vec![hunk((2, count), (2, count))],
        )];
        let text = |side: LineSide, _: &str| {
            Some(if side == LineSide::Base {
                base.as_str()
            } else {
                head.as_str()
            })
        };
        let started = std::time::Instant::now();
        let facts = compute(&files, &[], &text);
        let elapsed = started.elapsed();
        assert_eq!(facts.counters.dependencies_changed.len(), 100_000);
        assert!(elapsed < std::time::Duration::from_secs(30), "{elapsed:?}");
    }
}
