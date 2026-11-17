//! Line scanners for `Cargo.toml` and `Cargo.lock` (no TOML parser: facts are tied to the changed lines).

/// One line of a `Cargo.toml` with the table and entry it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TomlLine<'t> {
    /// 1-based line number.
    pub number: u32,
    /// Header of the enclosing table, without brackets (`dependencies`, `target.'cfg(unix)'.dependencies`).
    pub table: &'t str,
    /// Key of the entry the line belongs to; a continuation line of a multi-line value keeps it.
    pub key: Option<&'t str>,
    /// The line without its comment.
    pub text: &'t str,
}

/// The lines of a `Cargo.toml`.
pub(crate) fn toml_lines(text: &str) -> Vec<TomlLine<'_>> {
    let mut out = Vec::new();
    let (mut table, mut key, mut depth) = ("", None, 0usize);
    for (number, raw) in (1u32..).zip(text.lines()) {
        let line = strip_comment(raw);
        let trimmed = line.trim();
        if depth == 0 && trimmed.starts_with('[') {
            table = trimmed.trim_start_matches('[').trim_end_matches(']').trim();
            key = None;
        } else if depth == 0 {
            key = trimmed
                .split_once('=')
                .map(|(name, _)| name.trim())
                .filter(|name| !name.is_empty());
        }
        out.push(TomlLine {
            number,
            table,
            key,
            text: line,
        });
        depth = brackets(line, depth);
    }
    out
}

/// The dependency a line of a dependency table names, if the line is in one.
pub(crate) fn dependency_name<'t>(line: &TomlLine<'t>) -> Option<&'t str> {
    const KINDS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
    let table = line.table;
    let entries = table.starts_with("patch.")
        || table == "replace"
        || KINDS
            .iter()
            .any(|kind| table == *kind || table.ends_with(&format!(".{kind}")));
    if entries {
        let key = line.key?;
        let first = key.split('.').next().unwrap_or(key);
        return Some(unquote(first.trim()));
    }
    // `[dependencies.foo]`, `[target.'cfg(unix)'.dev-dependencies.foo]`: one dependency per table.
    KINDS.iter().find_map(|kind| {
        let marker = format!("{kind}.");
        let at = table.rfind(&marker)?;
        let starts_segment = at == 0 || table[..at].ends_with('.');
        starts_segment.then(|| unquote(table[at + marker.len()..].trim()))
    })
}

/// The quoted string value of `key` on the line (`git = "…"` alone or inside an inline table).
pub(crate) fn string_value<'t>(text: &'t str, key: &str) -> Option<&'t str> {
    let mut rest = text;
    while let Some(at) = rest.find(key) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + key.len()..].trim_start();
        let boundary = before.is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '-'));
        if boundary && let Some(value) = after.strip_prefix('=') {
            let value = value.trim_start().strip_prefix('"')?;
            return value.split_once('"').map(|(value, _)| value);
        }
        rest = &rest[at + key.len()..];
    }
    None
}

/// A `[[package]]` block of a `Cargo.lock`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LockPackage<'t> {
    /// Package name.
    pub name: Option<&'t str>,
    /// `source` value; `None` for packages of the workspace.
    pub source: Option<&'t str>,
    /// First and last line of the block.
    pub lines: (u32, u32),
    /// Lines of the `dependencies = [...]` list (the package's own dependencies, not the package itself).
    pub dependency_list: Vec<u32>,
}

/// The packages of a `Cargo.lock`.
pub(crate) fn lock_packages(text: &str) -> Vec<LockPackage<'_>> {
    let mut out: Vec<LockPackage<'_>> = Vec::new();
    let mut in_list = false;
    for (number, line) in (1u32..).zip(text.lines()) {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && !in_list {
            if trimmed == "[[package]]" {
                out.push(LockPackage {
                    lines: (number, number),
                    ..LockPackage::default()
                });
            }
            continue;
        }
        let Some(package) = out.last_mut() else {
            continue;
        };
        package.lines.1 = number;
        if in_list || trimmed.starts_with("dependencies") {
            package.dependency_list.push(number);
            in_list = !trimmed.ends_with(']');
        } else if let Some(name) = string_value(trimmed, "name") {
            package.name = Some(name);
        } else if let Some(source) = string_value(trimmed, "source") {
            package.source = Some(source);
        }
    }
    out
}

fn strip_comment(line: &str) -> &str {
    let mut quoted = None;
    for (at, c) in line.char_indices() {
        match (quoted, c) {
            (None, '"' | '\'') => quoted = Some(c),
            (Some(open), _) if c == open => quoted = None,
            (None, '#') => return &line[..at],
            _ => {}
        }
    }
    line
}

/// Bracket depth after `line` (multi-line arrays and inline tables), quotes skipped.
fn brackets(line: &str, mut depth: usize) -> usize {
    let mut quoted = None;
    for c in line.chars() {
        match (quoted, c) {
            (None, '"' | '\'') => quoted = Some(c),
            (Some(open), _) if c == open => quoted = None,
            (None, '[' | '{') => depth += 1,
            (None, ']' | '}') => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

fn unquote(text: &str) -> &str {
    text.trim_matches(|c| c == '"' || c == '\'')
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"[package]
name = "node"
version = "0.7.0" # not a dependency

[dependencies]
tokio = { version = "1", features = [
    "macros",
] }
serde.workspace = true
"quoted-name" = "1"

[target.'cfg(unix)'.dev-dependencies]
nix = "0.29"

[dependencies.url]
version = "2"
git = "https://github.com/acme/url"

[patch.crates-io]
rand = { git = "https://github.com/acme/rand", branch = "fix" }
"#;

    #[test]
    fn manifest_lines_name_their_dependency() {
        let names: Vec<(u32, Option<&str>)> = toml_lines(MANIFEST)
            .iter()
            .map(|line| (line.number, dependency_name(line)))
            .collect();
        let name = |number: u32| names[usize::try_from(number - 1).expect("index")].1;
        assert_eq!(name(3), None, "package version");
        assert_eq!(name(6), Some("tokio"));
        assert_eq!(name(7), Some("tokio"), "continuation of a multi-line value");
        assert_eq!(name(8), Some("tokio"));
        assert_eq!(name(9), Some("serde"));
        assert_eq!(name(10), Some("quoted-name"));
        assert_eq!(name(13), Some("nix"));
        assert_eq!(name(15), Some("url"), "table header of one dependency");
        assert_eq!(name(16), Some("url"));
        assert_eq!(name(20), Some("rand"));
        let lines = toml_lines(MANIFEST);
        assert_eq!(
            string_value(lines[16].text, "git"),
            Some("https://github.com/acme/url")
        );
        assert_eq!(
            string_value(lines[19].text, "git"),
            Some("https://github.com/acme/rand")
        );
        assert_eq!(string_value("digest = \"x\"", "git"), None);
    }

    #[test]
    fn lock_packages_keep_name_source_and_dependency_list() {
        let lock = "version = 4\n\n[[package]]\nname = \"node\"\nversion = \"0.7.0\"\ndependencies = [\n \"tokio\",\n]\n\n[[package]]\nname = \"tokio\"\nversion = \"1.47.1\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
        let packages = lock_packages(lock);
        assert_eq!(packages.len(), 2);
        assert_eq!((packages[0].name, packages[0].source), (Some("node"), None));
        assert_eq!(packages[0].dependency_list, [6, 7, 8]);
        assert_eq!(packages[1].name, Some("tokio"));
        assert!(packages[1].source.is_some());
        assert_eq!(packages[1].lines, (10, 13));
    }
}
