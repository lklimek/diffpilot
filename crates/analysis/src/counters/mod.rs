//! Counters over material that is not analysed (lines removed from tests, changed dependencies), each with the
//! lines it was computed from.

pub(crate) mod cargo;

use std::collections::{BTreeMap, BTreeSet};

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
    let mut added_test_lines = 0_u32;
    let mut dependencies: BTreeMap<String, Vec<Evidence>> = BTreeMap::new();
    let mut not_checked = Vec::new();
    for file in files {
        if file.path_on(LineSide::Head).is_some_and(|path| {
            classified.iter().any(|item| {
                item.side == LineSide::Head
                    && item.file == path
                    && item.category == NotAnalysedCategory::Tests
            })
        }) {
            for hunk in &file.hunks {
                added_test_lines = added_test_lines
                    .saturating_add(u32::try_from(hunk.added.len()).unwrap_or(u32::MAX));
            }
        }
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
        dependency_changes(file, text, &mut dependencies, &mut not_checked);
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
            lines_added_to_tests: Some(added_test_lines),
            dependencies_changed: dependencies.keys().cloned().collect(),
            dependency_files_not_checked: not_checked.clone(),
        },
        removed_test_lines,
        dependencies: dependencies
            .into_iter()
            .map(|(name, evidence)| DependencyChange { name, evidence })
            .collect(),
        dependency_files_not_checked: not_checked,
    }
}

/// A dependency declaration or lockfile record on one side of a dependency file.
struct Entry {
    name: String,
    identity: String,
    lines: Vec<(u32, u32)>,
    header: Option<u32>,
}

/// Dependencies changed by `file`: entries whose decor-free identity is on one side only (added, removed, moved
/// to another table, or with another value). Changed lines are only evidence. A side that cannot be read or
/// parsed makes the whole file not checked.
fn dependency_changes<'t>(
    file: &FileDiff,
    text: &dyn Fn(LineSide, &str) -> Option<&'t str>,
    dependencies: &mut BTreeMap<String, Vec<Evidence>>,
    not_checked: &mut Vec<String>,
) {
    let sides = [LineSide::Base, LineSide::Head].map(|side| {
        file.path_on(side)
            .and_then(|path| Some((side, path, dependency_file(path)?)))
    });
    if file.hunks.is_empty() || sides.iter().all(Option::is_none) {
        return;
    }
    let mut entries: [Vec<Entry>; 2] = [Vec::new(), Vec::new()];
    let mut unreadable = Vec::new();
    for (slot, &(side, path, kind)) in sides
        .iter()
        .enumerate()
        .filter_map(|(slot, side)| Some((slot, side.as_ref()?)))
    {
        let parsed = match (kind, text(side, path)) {
            (DependencyFile::CargoToml, Some(text)) => manifest_entries(text),
            (DependencyFile::CargoLock, Some(text)) => lock_entries(text),
            (DependencyFile::Other, _) | (_, None) => None,
        };
        match (parsed, entries.get_mut(slot)) {
            (Some(parsed), Some(slot)) => *slot = parsed,
            _ => unreadable.push(path.to_owned()),
        }
    }
    if !unreadable.is_empty() {
        not_checked.extend(unreadable);
        return;
    }
    for (slot, &(side, path, _)) in sides
        .iter()
        .enumerate()
        .filter_map(|(slot, side)| Some((slot, side.as_ref()?)))
    {
        let (Some(own), Some(other)) = (entries.get(slot), entries.get(1 - slot)) else {
            continue;
        };
        let other: BTreeSet<&str> = other.iter().map(|entry| entry.identity.as_str()).collect();
        let mut changed: Vec<u32> = file
            .hunks
            .iter()
            .flat_map(|hunk| match side {
                LineSide::Base => &hunk.removed,
                LineSide::Head => &hunk.added,
            })
            .copied()
            .collect();
        changed.sort_unstable();
        for entry in own
            .iter()
            .filter(|entry| !other.contains(entry.identity.as_str()))
        {
            let mut lines: Vec<u32> = entry
                .lines
                .iter()
                .flat_map(|&range| cargo::changed_within(&changed, range).iter().copied())
                .chain(
                    entry
                        .header
                        .filter(|line| changed.binary_search(line).is_ok()),
                )
                .collect();
            if lines.is_empty() {
                lines.extend(entry.lines.iter().map(|range| range.0).min());
            }
            lines.sort_unstable();
            lines.dedup();
            let evidence = dependencies.entry(entry.name.clone()).or_default();
            evidence.extend(lines.into_iter().map(|line| Evidence {
                file: path.to_owned(),
                side,
                line,
            }));
        }
    }
}

/// The dependency declarations of a `Cargo.toml`; `None` when it is not checked.
fn manifest_entries(text: &str) -> Option<Vec<Entry>> {
    let declarations = cargo::manifest(text)?;
    Some(
        declarations
            .into_iter()
            .map(|declaration| Entry {
                name: declaration.name,
                identity: declaration.identity,
                lines: declaration.lines,
                header: declaration.header,
            })
            .collect(),
    )
}

/// The registry and git packages of a `Cargo.lock` (workspace and path packages are not dependencies of their
/// own); a package's `dependencies` list is not part of it. `None` when it is not checked.
fn lock_entries(text: &str) -> Option<Vec<Entry>> {
    let packages = cargo::lock(text)?;
    Some(
        packages
            .into_iter()
            .filter(|package| package.source.is_some())
            .map(|package| Entry {
                name: package.name,
                identity: package.identity,
                lines: package.lines,
                header: None,
            })
            .collect(),
    )
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
    fn edited_test_lines_and_a_new_test_are_reported_as_churn() {
        let path = "internal/statesync/dispatcher_test.go";
        let files = [file(
            path,
            FileStatus::Modified,
            vec![
                hunk((85, 1), (85, 1)),
                hunk((130, 1), (130, 1)),
                hunk((167, 1), (167, 1)),
                hunk((189, 0), (189, 53)),
                hunk((407, 1), (460, 1)),
            ],
        )];
        let mut head = tests_item(path);
        head.side = LineSide::Head;
        let facts = compute(&files, &[tests_item(path), head], &|_, _| None);
        let wire = serde_json::to_value(facts.counters).expect("wire");
        assert_eq!(wire["lines_removed_from_tests"], 4);
        assert_eq!(wire["lines_added_to_tests"], 57);
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

    /// Dependency changes between two versions of `Cargo.toml`, with the hunks given.
    fn manifest_change(base: &str, head: &str, hunks: Vec<Hunk>) -> CounterFacts {
        let files = [file("Cargo.toml", FileStatus::Modified, hunks)];
        let text =
            move |side: LineSide, _: &str| Some(if side == LineSide::Base { base } else { head });
        compute(&files, &[], &text)
    }

    #[test]
    fn a_table_header_edit_changes_its_dependencies() {
        let facts = manifest_change(
            "[dependencies]\nserde = \"1\"\n",
            "[dev-dependencies]\nserde = \"1\"\n",
            vec![hunk((1, 1), (1, 1))],
        );
        assert_eq!(facts.counters.dependencies_changed, ["serde"]);
        let evidence: Vec<(LineSide, u32)> = facts.dependencies[0]
            .evidence
            .iter()
            .map(|evidence| (evidence.side, evidence.line))
            .collect();
        assert_eq!(
            evidence,
            [(LineSide::Base, 1), (LineSide::Head, 1)],
            "the changed header"
        );
        let facts = manifest_change(
            "[target.'cfg(unix)'.dependencies]\nnix = \"0.29\"\n",
            "[target.'cfg(windows)'.dependencies]\nnix = \"0.29\"\n",
            vec![hunk((1, 1), (1, 1))],
        );
        assert_eq!(facts.counters.dependencies_changed, ["nix"]);
    }

    #[test]
    fn a_removed_dependency_is_a_change() {
        let facts = manifest_change(
            "[dependencies]\nserde = \"1\"\ntokio = \"1\"\n",
            "[dependencies]\ntokio = \"1\"\n",
            vec![hunk((2, 1), (1, 0))],
        );
        assert_eq!(facts.counters.dependencies_changed, ["serde"]);
        assert_eq!(facts.dependencies[0].evidence[0].side, LineSide::Base);
        assert_eq!(facts.dependencies[0].evidence[0].line, 2);
    }

    #[test]
    fn a_comment_edit_changes_no_dependency() {
        let facts = manifest_change(
            "[dependencies.serde]\nversion = \"1\"\nfeatures = [\n  # old note\n  \"derive\",\n]\n",
            "[dependencies.serde]\nversion = \"1\"\nfeatures = [\n  # new note\n  \"derive\",\n]\n",
            vec![hunk((4, 1), (4, 1))],
        );
        assert!(facts.counters.dependencies_changed.is_empty(), "{facts:?}");
        assert!(facts.dependency_files_not_checked.is_empty());
        let facts = manifest_change(
            "[dependencies]\ntokio = { version = \"1\", features = [\"macros\"] }\n",
            "[dependencies]\ntokio = { features = [ \"macros\" ], version = \"1\" } # same\n",
            vec![hunk((2, 1), (2, 1))],
        );
        assert!(
            facts.counters.dependencies_changed.is_empty(),
            "formatting only"
        );
    }
}
