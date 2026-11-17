//! `Cargo.toml` and `Cargo.lock` parsed with `toml_edit` (positions kept): dependency declarations and package
//! records with the lines they occupy. A file that is too large or does not parse gives `None`: not checked.

use std::ops::Range;

use toml_edit::{Document, Item, TableLike};

/// Largest Cargo file read; a larger one is not checked.
pub(crate) const MAX_CARGO_BYTES: usize = 4 * 1024 * 1024;

/// One dependency declaration of a `Cargo.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Declaration {
    /// Package name (`package = …` of a renamed dependency, else the key).
    pub name: String,
    /// `git = …`, if any.
    pub git: Option<String>,
    /// `path = …`, if any.
    pub path: Option<String>,
    /// Line ranges (1-based, inclusive) of its key and values.
    pub lines: Vec<(u32, u32)>,
}

/// One `[[package]]` record of a `Cargo.lock`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LockPackage {
    /// Package name.
    pub name: String,
    /// `source`; `None` for packages of the workspace or path dependencies.
    pub source: Option<String>,
    /// Line ranges of the header and of every field except the package's own `dependencies` list.
    pub lines: Vec<(u32, u32)>,
}

/// Dependency tables of a manifest or a `target.<cfg>` table (`_` spellings are Cargo's legacy aliases).
const KINDS: [&str; 5] = [
    "dependencies",
    "dev-dependencies",
    "dev_dependencies",
    "build-dependencies",
    "build_dependencies",
];

/// The dependency declarations of a `Cargo.toml`: the tables of `KINDS`, their `target.*` forms,
/// `[workspace.dependencies]`, and the source overrides `[patch.*]` and `[replace]`. `None` when the file is too
/// large or does not parse.
pub(crate) fn manifest(text: &str) -> Option<Vec<Declaration>> {
    let document = parse(text)?;
    let lines = LineIndex::new(text);
    let mut tables = kinds(document.as_table());
    if let Some(targets) = document.get("target").and_then(Item::as_table_like) {
        for (_, target) in targets.iter() {
            tables.extend(target.as_table_like().map(kinds).unwrap_or_default());
        }
    }
    let workspace = document.get("workspace").and_then(Item::as_table_like);
    tables.extend(workspace.and_then(|workspace| workspace.get("dependencies")));
    if let Some(patches) = document.get("patch").and_then(Item::as_table_like) {
        tables.extend(patches.iter().map(|(_, item)| item));
    }
    tables.extend(document.get("replace"));
    let mut out = Vec::new();
    for table in tables.into_iter().filter_map(Item::as_table_like) {
        for (key, item) in table.iter() {
            let mut spans = Vec::new();
            if let Some((key, _)) = table.get_key_value(key) {
                spans.extend(key.span());
            }
            collect_spans(item, &mut spans);
            let field = |name: &str| {
                item.as_table_like()
                    .and_then(|fields| fields.get(name))
                    .and_then(Item::as_str)
                    .map(str::to_owned)
            };
            // `[replace]` keys are package ids (`name:version`).
            let key = key.split_once(':').map_or(key, |(name, _)| name);
            out.push(Declaration {
                name: field("package").unwrap_or_else(|| key.to_owned()),
                git: field("git"),
                path: field("path"),
                lines: spans.into_iter().map(|span| lines.range(span)).collect(),
            });
        }
    }
    Some(out)
}

/// The package records of a `Cargo.lock`; `None` when it is too large or does not parse.
pub(crate) fn lock(text: &str) -> Option<Vec<LockPackage>> {
    let document = parse(text)?;
    let lines = LineIndex::new(text);
    let Some(packages) = document.get("package").and_then(Item::as_array_of_tables) else {
        return Some(Vec::new());
    };
    let mut out = Vec::new();
    for package in packages.iter() {
        let Some(name) = package.get("name").and_then(Item::as_str) else {
            continue;
        };
        // The record's span covers its body too: only its first line, the `[[package]]` header.
        let header = package.span().map(|span| span.start..span.start + 1);
        let mut spans: Vec<Range<usize>> = header.into_iter().collect();
        for (key, item) in package.iter().filter(|(key, _)| *key != "dependencies") {
            if let Some((key, _)) = package.get_key_value(key) {
                spans.extend(key.span());
            }
            collect_spans(item, &mut spans);
        }
        out.push(LockPackage {
            name: name.to_owned(),
            source: package
                .get("source")
                .and_then(Item::as_str)
                .map(str::to_owned),
            lines: spans.into_iter().map(|span| lines.range(span)).collect(),
        });
    }
    Some(out)
}

/// The dependency tables of a manifest or `target.<cfg>` table.
fn kinds(table: &dyn TableLike) -> Vec<&Item> {
    KINDS.iter().filter_map(|kind| table.get(kind)).collect()
}

fn parse(text: &str) -> Option<Document<&str>> {
    (text.len() <= MAX_CARGO_BYTES)
        .then(|| Document::parse(text).ok())
        .flatten()
}

/// Spans of an item and, for tables, of every key and value inside.
fn collect_spans(item: &Item, out: &mut Vec<Range<usize>>) {
    out.extend(item.span());
    if let Some(table) = item.as_table_like() {
        for (key, child) in table.iter() {
            if let Some((key, _)) = table.get_key_value(key) {
                out.extend(key.span());
            }
            collect_spans(child, out);
        }
    }
}

/// Byte offset → 1-based line number.
struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(at, _)| at + 1))
            .collect();
        Self { starts }
    }

    fn line(&self, offset: usize) -> u32 {
        u32::try_from(self.starts.partition_point(|&start| start <= offset)).unwrap_or(u32::MAX)
    }

    /// First and last line of a byte span.
    fn range(&self, span: Range<usize>) -> (u32, u32) {
        let last = span.end.saturating_sub(1).max(span.start);
        (self.line(span.start), self.line(last))
    }
}

/// The sorted `changed` lines within `first..=last`.
pub(crate) fn changed_within(changed: &[u32], (first, last): (u32, u32)) -> &[u32] {
    let from = changed.partition_point(|&line| line < first);
    let to = changed.partition_point(|&line| line <= last).max(from);
    changed.get(from..to).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"[package]
name = "node"
version = "0.7.0"
description = """
[
dependencies = "not a table"
"""

[package.metadata.dependencies]
fake = "1"

[package.metadata.release]
git = "https://github.com/acme/marketing"

[dependencies]
tokio = { version = "1", features = [
    "macros",
] }
serde.workspace = true
json = { package = "serde_json", version = "1" } # renamed

[target.'cfg(unix)'.dev-dependencies]
nix = "0.29"

[dependencies.url]
version = "2"
git = "https://github.com/acme/url"

[workspace.dependencies]
shared = { path = "../shared" }

[patch.crates-io]
rand = { git = "https://github.com/acme/rand", branch = "fix" }
"#;

    /// First and last line of a declaration.
    fn extent(declaration: &Declaration) -> (u32, u32) {
        let first = declaration.lines.iter().map(|l| l.0).min().unwrap_or(0);
        let last = declaration.lines.iter().map(|l| l.1).max().unwrap_or(0);
        (first, last)
    }

    #[test]
    fn manifest_declarations_with_their_lines() {
        let declarations = manifest(MANIFEST).expect("parses");
        let names: Vec<&str> = declarations.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "tokio",
                "serde",
                "serde_json",
                "url",
                "nix",
                "shared",
                "rand"
            ],
            "no metadata tables, no text inside multi-line strings, renamed by `package`"
        );
        let get = |name: &str| {
            declarations
                .iter()
                .find(|d| d.name == name)
                .expect("declared")
        };
        assert_eq!(extent(get("tokio")), (16, 18), "a multi-line inline table");
        assert_eq!(extent(get("serde")), (19, 19));
        assert_eq!(
            extent(get("url")),
            (25, 27),
            "a table of one dependency with its header"
        );
        assert_eq!(
            get("url").git.as_deref(),
            Some("https://github.com/acme/url")
        );
        assert_eq!(get("shared").path.as_deref(), Some("../shared"));
        assert_eq!(manifest("[dependencies\nx = 1"), None, "does not parse");
        assert_eq!(
            manifest(&"#".repeat(MAX_CARGO_BYTES + 1)),
            None,
            "too large"
        );
    }

    #[test]
    fn lock_packages_without_their_dependency_list() {
        let lock_text = "version = 4\n\n# source = \"git+https://github.com/acme/comment\"\n[[package]]\nname = \"node\"\nversion = \"0.7.0\"\ndependencies = [\n \"tokio\",\n]\n\n[[package]]\nname = \"tokio\"\nversion = \"1.47.1\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
        let packages = lock(lock_text).expect("parses");
        assert_eq!(packages.len(), 2);
        assert_eq!(
            (packages[0].name.as_str(), packages[0].source.as_deref()),
            ("node", None)
        );
        let node: Vec<u32> = packages[0].lines.iter().flat_map(|&(a, b)| a..=b).collect();
        assert!([4, 5, 6].iter().all(|line| node.contains(line)), "{node:?}");
        assert!(!node.contains(&7) && !node.contains(&8), "{node:?}");
        assert!(packages[1].source.is_some());
        assert_eq!(lock("[[package]\n"), None);
    }

    #[test]
    fn changed_lines_are_found_by_range() {
        let changed = [2, 5, 6, 9];
        assert_eq!(changed_within(&changed, (5, 8)), [5, 6]);
        assert!(changed_within(&changed, (10, 12)).is_empty());
        assert_eq!(changed_within(&changed, (1, 2)), [2]);
    }
}
