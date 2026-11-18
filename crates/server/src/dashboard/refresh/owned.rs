//! Ownership policies (Q3, Q4; skill `fetch_owned_paths`, `owned_change`): the paths whose areas name a scanned
//! login as owner or reviewer, and how much real code a pull request changes there.

use std::collections::BTreeSet;

use serde::Deserialize;

use crate::dashboard::score::{OwnedChange, ScoreRules};
use crate::github::work::PullFile;

const NON_CODE_SUFFIXES: [&str; 6] = [".md", ".txt", ".lock", ".snap", ".svg", ".png"];
const HASH_COMMENT_SUFFIXES: [&str; 5] = [".py", ".sh", ".toml", ".yml", ".yaml"];
/// A change is significant from this many distinct code lines …
const SIGNIFICANT_DISTINCT_LINES: usize = 12;
/// … with at least this many in one file.
const SIGNIFICANT_MAX_FILE_LINES: usize = 8;

#[derive(Deserialize)]
struct Policy {
    #[serde(default)]
    areas: Vec<Area>,
}

#[derive(Deserialize)]
struct Area {
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    owners: Vec<String>,
    #[serde(default)]
    reviewers: Vec<String>,
}

/// Path prefixes of the policy's areas a scanned login owns or reviews (format of `stale_prs_are_bad`).
pub(super) fn owned_paths(
    rules: &ScoreRules,
    policy: &str,
) -> Result<Vec<String>, serde_json::Error> {
    let policy: Policy = serde_json::from_str(policy)?;
    Ok(policy
        .areas
        .into_iter()
        .filter(|area| {
            area.owners
                .iter()
                .chain(&area.reviewers)
                .any(|login| rules.is_scanned(login))
        })
        .flat_map(|area| area.paths)
        .collect())
}

fn is_code_line(line: &str, path: &str) -> bool {
    let text = line.trim();
    if text.is_empty() {
        return false;
    }
    if HASH_COMMENT_SUFFIXES.iter().any(|s| path.ends_with(s)) {
        return !text.starts_with('#');
    }
    !(text.starts_with("//")
        || text.starts_with("/*")
        || text.starts_with("*/")
        || text.starts_with("* ")
        || text == "*")
}

/// How much real code `files` change under `owned` (skill heuristic): none = only comments or docs, minor =
/// small or thinly spread, significant = concentrated.
pub(super) fn verdict(files: &[PullFile], owned: &[String]) -> OwnedChange {
    let mut per_file: Vec<usize> = Vec::new();
    let mut distinct: BTreeSet<String> = BTreeSet::new();
    for file in files {
        let path = &file.filename;
        if !owned.iter().any(|prefix| path.starts_with(prefix.as_str()))
            || NON_CODE_SUFFIXES.iter().any(|s| path.ends_with(s))
        {
            continue;
        }
        let count = match &file.patch {
            // Too large or binary: no patch to inspect, the raw count stands.
            None => {
                let count = usize::try_from(file.additions + file.deletions).unwrap_or(usize::MAX);
                distinct.extend((0..count).map(|i| format!("{path}:{i}")));
                count
            }
            Some(patch) => {
                let lines: Vec<&str> = patch
                    .lines()
                    .filter_map(|line| line.strip_prefix(['+', '-']))
                    .filter(|line| is_code_line(line, path))
                    .map(str::trim)
                    .collect();
                distinct.extend(lines.iter().map(|l| (*l).to_owned()));
                lines.len()
            }
        };
        if count > 0 {
            per_file.push(count);
        }
    }
    let total: usize = per_file.iter().sum();
    let max_file = per_file.iter().copied().max().unwrap_or(0);
    if total == 0 {
        OwnedChange::None
    } else if distinct.len() >= SIGNIFICANT_DISTINCT_LINES && max_file >= SIGNIFICANT_MAX_FILE_LINES
    {
        OwnedChange::Significant
    } else {
        OwnedChange::Minor
    }
}

#[cfg(test)]
mod tests {
    use contracts::api::dashboard::DashboardSettings;

    use super::*;

    fn file(name: &str, patch: Option<&str>, changed: u64) -> PullFile {
        PullFile {
            filename: name.to_owned(),
            status: "modified".to_owned(),
            additions: changed,
            deletions: 0,
            patch: patch.map(str::to_owned),
        }
    }

    #[test]
    fn owned_paths_are_those_of_areas_naming_a_scanned_login() {
        let rules = ScoreRules::new("alice", &DashboardSettings::default());
        let policy = r#"{"areas":[
            {"id":"sync","paths":["src/sync/"],"owners":["Alice"]},
            {"id":"rpc","paths":["src/rpc/"],"reviewers":["bob"]}]}"#;
        assert_eq!(owned_paths(&rules, policy).expect("policy"), ["src/sync/"]);
        assert!(owned_paths(&rules, "<html>").is_err());
    }

    #[test]
    fn verdict_counts_distinct_code_lines_in_owned_paths_only() {
        let owned = vec!["src/sync/".to_owned()];
        let comments = "@@\n+// note\n+\n-/* old */";
        assert_eq!(
            verdict(&[file("src/sync/a.rs", Some(comments), 3)], &owned),
            OwnedChange::None
        );
        assert_eq!(
            verdict(&[file("src/rpc/a.rs", Some("+let a = 1;"), 1)], &owned),
            OwnedChange::None
        );
        assert_eq!(
            verdict(
                &[file("src/sync/a.rs", Some("+let a = 1;\n-let b = 2;"), 2)],
                &owned
            ),
            OwnedChange::Minor
        );
        let big: String = (0..12).map(|i| format!("+let v{i} = {i};\n")).collect();
        assert_eq!(
            verdict(&[file("src/sync/a.rs", Some(&big), 12)], &owned),
            OwnedChange::Significant
        );
        assert_eq!(
            verdict(&[file("src/sync/huge.rs", None, 40)], &owned),
            OwnedChange::Significant
        );
        assert_eq!(
            verdict(&[file("src/sync/README.md", None, 40)], &owned),
            OwnedChange::None
        );
    }
}
