//! Tech plugins: path and line classification rules and call hierarchy (rule A6; `generic` in v1), plus the
//! other path facts of the pre-pass: agent instruction files, dependency manifests, the project profile.

use contracts::api::settings::{NotAnalysedRule, RuleKind};
use contracts::api::tour::CallHierarchy;
use contracts::common::{LineRange, LineSide, NotAnalysedCategory, ProvenanceBadge};

use crate::lines::ranges;

/// A language plugin.
pub trait TechPlugin: Send + Sync {
    /// Plugin name.
    fn name(&self) -> &'static str;
    /// Whether the plugin handles this file.
    fn detect(&self, path: &str) -> bool;
    /// Material of one side of a file that is not analysed (tests, docs and comments, generated), by path or
    /// line rule; called once per side with changed lines (removed lines on base, e.g. a deleted comment).
    fn classify(&self, file: &FileSource<'_>) -> Result<Vec<Classified>, TechError>;
    /// Callers and callees of a symbol; `None` when the plugin cannot tell.
    fn call_hierarchy(&self, symbol: &SymbolAt) -> Option<CallHierarchy>;
}

/// One side of a changed file as a plugin sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileSource<'a> {
    /// Repository-relative path on that side.
    pub path: &'a str,
    /// `head` (added lines) or `base` (removed lines).
    pub side: LineSide,
    /// Text on that side; `None` for binary files.
    pub text: Option<&'a str>,
    /// Text of the same file on the other side; `None` when absent, binary or unread.
    pub other_text: Option<&'a str>,
    /// Changed lines on that side: added head lines or removed base lines.
    pub changed_lines: &'a [u32],
}

/// Material recognised by a rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Classified {
    /// Repository-relative path on that side.
    pub file: String,
    /// Side of `ranges`; `base` for a deleted file or removed lines.
    pub side: LineSide,
    /// Matched changed lines; `None` for the whole file.
    pub ranges: Option<Vec<LineRange>>,
    /// Section.
    pub category: NotAnalysedCategory,
    /// How it was recognised.
    pub badge: ProvenanceBadge,
    /// The rule (path glob or line regex).
    pub rule: String,
}

/// A symbol at a head line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolAt {
    /// Repository-relative path.
    pub file: String,
    /// Head line.
    pub line: u32,
    /// Symbol name.
    pub symbol: String,
}

/// A plugin failed.
#[derive(Debug, thiserror::Error)]
pub enum TechError {
    /// Declared but not implemented yet.
    #[error("the analysis operation {operation} is not implemented yet")]
    NotImplemented {
        /// The operation.
        operation: &'static str,
    },
}

/// The built-in language-agnostic plugin: path globs and line regexes; no call hierarchy in v1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Generic;

impl TechPlugin for Generic {
    fn name(&self) -> &'static str {
        "generic"
    }

    fn detect(&self, _path: &str) -> bool {
        true
    }

    fn classify(&self, file: &FileSource<'_>) -> Result<Vec<Classified>, TechError> {
        let item = |category, badge, rule: &str, ranges| Classified {
            file: file.path.to_owned(),
            side: file.side,
            ranges,
            category,
            badge,
            rule: rule.to_owned(),
        };
        if let Some(&(rule, category)) = PATH_RULES.iter().find(|(rule, _)| glob(rule, file.path)) {
            return Ok(vec![item(
                category,
                ProvenanceBadge::ClassifiedByPath,
                rule,
                changed_ranges(file),
            )]);
        }
        let Some(text) = file.text else {
            return Ok(Vec::new());
        };
        if has_generated_header(text) {
            return Ok(vec![item(
                NotAnalysedCategory::GeneratedMechanical,
                ProvenanceBadge::ClassifiedByLineRule,
                GENERATED_HEADER_RULE,
                changed_ranges(file),
            )]);
        }
        let comments = comment_runs(file.path, text, file.changed_lines);
        let imports = import_runs(file.path, text, file.other_text, file.changed_lines);
        Ok([(LINE_COMMENT_RULE, comments), (IMPORT_RULE, imports)]
            .into_iter()
            .filter(|(_, ranges)| !ranges.is_empty())
            .map(|(rule, ranges)| {
                item(
                    NotAnalysedCategory::DocsComments,
                    ProvenanceBadge::ClassifiedByLineRule,
                    rule,
                    Some(ranges),
                )
            })
            .collect())
    }

    fn call_hierarchy(&self, _symbol: &SymbolAt) -> Option<CallHierarchy> {
        // TODO: static call hierarchy (tree-sitter tags) belongs to the language plugins (brief §5, v2).
        None
    }
}

/// Complete single-line Rust imports and module declarations.
pub const IMPORT_RULE: &str = "Rust use/mod/extern crate declarations (line regex)";
/// Comment-only lines, including block and doc comments.
pub const LINE_COMMENT_RULE: &str = "comment-only lines (Rust, by the compiler's lexer)";
/// Rule of a generated-file header in the first lines.
pub const GENERATED_HEADER_RULE: &str = "header: Code generated … DO NOT EDIT / @generated";
/// Lines searched for a generated-file header.
const HEADER_LINES: usize = 10;
/// The project profile on the base ref (brief §2 Project profile).
pub const PROFILE_PATH: &str = ".claudius/project-profile.md";

/// Path rules of the `generic` plugin, first match wins. Paths are chosen by the pull request's author: a production
/// file placed under a test path is hidden from the agent. Accepted residual risk: the reviewer sees every such file
/// listed under Not analysed.
const PATH_RULES: &[(&str, NotAnalysedCategory)] = &[
    ("tests/**", NotAnalysedCategory::Tests),
    ("benches/**", NotAnalysedCategory::Tests),
    ("test/**", NotAnalysedCategory::Tests),
    ("crates/*/tests/**", NotAnalysedCategory::Tests),
    ("crates/*/benches/**", NotAnalysedCategory::Tests),
    ("packages/*/tests/**", NotAnalysedCategory::Tests),
    ("packages/*/benches/**", NotAnalysedCategory::Tests),
    ("packages/*/__tests__/**", NotAnalysedCategory::Tests),
    ("**/__tests__/**", NotAnalysedCategory::Tests),
    ("*_test.go", NotAnalysedCategory::Tests),
    ("*_test.py", NotAnalysedCategory::Tests),
    ("test_*.py", NotAnalysedCategory::Tests),
    ("*.test.js", NotAnalysedCategory::Tests),
    ("*.test.jsx", NotAnalysedCategory::Tests),
    ("*.test.ts", NotAnalysedCategory::Tests),
    ("*.test.tsx", NotAnalysedCategory::Tests),
    ("*.spec.js", NotAnalysedCategory::Tests),
    ("*.spec.jsx", NotAnalysedCategory::Tests),
    ("*.spec.ts", NotAnalysedCategory::Tests),
    ("*.spec.tsx", NotAnalysedCategory::Tests),
    ("**/tests.rs", NotAnalysedCategory::Tests),
    ("**/test_helpers.rs", NotAnalysedCategory::Tests),
    ("*_test.rs", NotAnalysedCategory::Tests),
    ("**/fixtures/**", NotAnalysedCategory::Tests),
    ("**/mocks/**", NotAnalysedCategory::Tests),
    ("**/test_helpers/**", NotAnalysedCategory::Tests),
    ("**/tests/**", NotAnalysedCategory::Tests),
    ("**/test/**", NotAnalysedCategory::Tests),
    ("**/benches/**", NotAnalysedCategory::Tests),
    ("docs/**", NotAnalysedCategory::DocsComments),
    ("*.md", NotAnalysedCategory::DocsComments),
    ("*.mdx", NotAnalysedCategory::DocsComments),
    ("*.rst", NotAnalysedCategory::DocsComments),
    ("*.adoc", NotAnalysedCategory::DocsComments),
    ("Cargo.lock", NotAnalysedCategory::GeneratedMechanical),
    (
        "package-lock.json",
        NotAnalysedCategory::GeneratedMechanical,
    ),
    (
        "npm-shrinkwrap.json",
        NotAnalysedCategory::GeneratedMechanical,
    ),
    ("yarn.lock", NotAnalysedCategory::GeneratedMechanical),
    ("pnpm-lock.yaml", NotAnalysedCategory::GeneratedMechanical),
    ("bun.lock", NotAnalysedCategory::GeneratedMechanical),
    ("go.sum", NotAnalysedCategory::GeneratedMechanical),
    ("poetry.lock", NotAnalysedCategory::GeneratedMechanical),
    ("uv.lock", NotAnalysedCategory::GeneratedMechanical),
    ("Pipfile.lock", NotAnalysedCategory::GeneratedMechanical),
    ("Gemfile.lock", NotAnalysedCategory::GeneratedMechanical),
    ("composer.lock", NotAnalysedCategory::GeneratedMechanical),
    ("flake.lock", NotAnalysedCategory::GeneratedMechanical),
    ("*.pb.go", NotAnalysedCategory::GeneratedMechanical),
    ("*_pb2.py", NotAnalysedCategory::GeneratedMechanical),
    ("*_pb2_grpc.py", NotAnalysedCategory::GeneratedMechanical),
    ("*.min.js", NotAnalysedCategory::GeneratedMechanical),
    ("*.min.css", NotAnalysedCategory::GeneratedMechanical),
];

/// The built-in Not analysed rules of the `generic` plugin as Settings lists them: every path rule, then the line
/// rules (generated header, comment-only lines).
pub fn rules() -> Vec<NotAnalysedRule> {
    let paths = PATH_RULES.iter().map(|(glob, category)| NotAnalysedRule {
        kind: RuleKind::PathGlob,
        pattern: (*glob).to_owned(),
        category: *category,
    });
    let lines = [
        (
            RuleKind::LineRegex,
            IMPORT_RULE,
            NotAnalysedCategory::DocsComments,
        ),
        (
            RuleKind::Header,
            GENERATED_HEADER_RULE,
            NotAnalysedCategory::GeneratedMechanical,
        ),
        (
            RuleKind::Lexer,
            LINE_COMMENT_RULE,
            NotAnalysedCategory::DocsComments,
        ),
        (
            RuleKind::Binary,
            "binary content (git diff)",
            NotAnalysedCategory::GeneratedMechanical,
        ),
    ]
    .into_iter()
    .map(|(kind, rule, category)| NotAnalysedRule {
        kind,
        pattern: rule.to_owned(),
        category,
    });
    paths.chain(lines).collect()
}

/// Path rules of generated (not merely mechanical) files: never rendered, never given to the agent (brief §7).
const GENERATED_PATH_RULES: &[&str] = &[
    "*.pb.go",
    "*_pb2.py",
    "*_pb2_grpc.py",
    "*.min.js",
    "*.min.css",
];

/// Files that instruct coding agents (threat model T14); matched case-insensitively.
const AGENT_INSTRUCTION_RULES: &[&str] = &[
    "agents.md",
    "agent.md",
    "claude.md",
    "claude.local.md",
    "gemini.md",
    ".cursor*",
    "**/.cursor*/**",
    "**/.agents/**",
    "**/.codex/**",
    "**/.claude/**",
    "**/.claudius/**",
    "**/.gemini/**",
    ".github/copilot-instructions.md",
    ".github/instructions/**",
    ".windsurfrules",
    ".clinerules",
    "**/.clinerules/**",
];

/// Dependency files of other ecosystems: the pre-pass does not read them (their dependencies are not checked).
const OTHER_DEPENDENCY_FILES: &[&str] = &[
    "package.json",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "go.mod",
    "go.sum",
    "requirements*.txt",
    "pyproject.toml",
    "poetry.lock",
    "uv.lock",
    "Pipfile",
    "Pipfile.lock",
    "Gemfile",
    "Gemfile.lock",
    "composer.json",
    "composer.lock",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "Package.swift",
    "Package.resolved",
    "flake.lock",
];

/// A dependency file the pre-pass recognises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DependencyFile {
    /// `Cargo.toml`.
    CargoToml,
    /// `Cargo.lock`.
    CargoLock,
    /// A manifest or lockfile of another ecosystem (not read).
    Other,
}

/// The dependency file kind of `path`, if it is one.
pub fn dependency_file(path: &str) -> Option<DependencyFile> {
    match basename(path) {
        "Cargo.toml" => Some(DependencyFile::CargoToml),
        "Cargo.lock" => Some(DependencyFile::CargoLock),
        _ => OTHER_DEPENDENCY_FILES
            .iter()
            .any(|rule| glob(rule, path))
            .then_some(DependencyFile::Other),
    }
}

/// Whether `path` looks like an agent instruction file (`AGENTS.md`, `CLAUDE.md`, `.codex/`, …).
pub fn is_agent_instruction_file(path: &str) -> bool {
    let path = path.to_lowercase();
    AGENT_INSTRUCTION_RULES.iter().any(|rule| glob(rule, &path))
}

/// Whether the item is recognised by the comment line rule (not a whole-file rule).
pub fn is_line_rule(item: &Classified) -> bool {
    item.rule == LINE_COMMENT_RULE || item.rule == IMPORT_RULE
}

/// Whether the item marks a generated file: never rendered, never given to the agent (brief §7).
pub fn is_generated(item: &Classified) -> bool {
    item.rule == GENERATED_HEADER_RULE || GENERATED_PATH_RULES.contains(&item.rule.as_str())
}

/// Changed lines as ranges; `None` when every line of the side changed (an added or deleted file) or the file is
/// binary.
fn changed_ranges(file: &FileSource<'_>) -> Option<Vec<LineRange>> {
    let whole = file
        .text
        .is_none_or(|text| text.lines().count() == file.changed_lines.len());
    (!whole).then(|| ranges(file.changed_lines))
}

/// Whether a comment line among the first lines says the file is generated.
pub fn has_generated_header(text: &str) -> bool {
    text.lines().take(HEADER_LINES).any(|line| {
        let line = line.trim_start();
        let comment = ["//", "/*", "*", "--", "<!--", ";"]
            .iter()
            .any(|marker| line.starts_with(marker))
            || (line.starts_with('#') && !line.starts_with("#["));
        let lower = line.to_lowercase();
        comment
            && ((lower.contains("code generated") && lower.contains("do not edit"))
                || lower.contains("@generated")
                || lower.contains("<auto-generated"))
    })
}

/// Runs of consecutive changed lines whose non-blank lines are all comment-only (at least one); Rust files only.
fn comment_runs(path: &str, text: &str, changed: &[u32]) -> Vec<LineRange> {
    let Some(RustLines { comment, .. }) = path
        .ends_with(".rs")
        .then(|| rust_lines(text, changed))
        .flatten()
    else {
        return Vec::new();
    };
    let selected: Vec<u32> = changed
        .iter()
        .copied()
        .filter(|&line| {
            line.checked_sub(1)
                .and_then(|i| usize::try_from(i).ok())
                .and_then(|i| comment.get(i))
                .copied()
                .unwrap_or(false)
        })
        .collect();
    ranges(&selected)
}

/// Changed Rust comment/import lines; other languages have no rule. `other` is the file's text on the other side.
pub fn context_lines(path: &str, text: &str, other: Option<&str>, changed: &[u32]) -> Vec<u32> {
    let mut lines: Vec<u32> = comment_runs(path, text, changed)
        .into_iter()
        .chain(import_runs(path, text, other, changed))
        .flat_map(|r| r.start_line..=r.end_line)
        .collect();
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// File-path test rules shared by pre-pass and plan verification.
// TODO: delimit inline Rust #[cfg(test)] modules and #[test] functions with a syntax plugin.
pub fn is_test_path(path: &str) -> bool {
    PATH_RULES
        .iter()
        .any(|(rule, category)| *category == NotAnalysedCategory::Tests && glob(rule, path))
}

/// Changed plain private imports: a complete single-line `use` without `as` or `*`, `mod name;` or
/// `extern crate name;`, outside literals, block comments and macro token trees, binding no name that a `use` on
/// either side (`text`, `other`) binds to another path. Rust files only.
fn import_runs(path: &str, text: &str, other: Option<&str>, changed: &[u32]) -> Vec<LineRange> {
    // TODO: delimit multiline use groups before treating their lines as import-only context.
    if !path.ends_with(".rs") {
        return Vec::new();
    }
    let (Some(RustLines { opaque, .. }), Ok(plain), Ok(alias), Ok(any_use)) = (
        rust_lines(text, changed),
        regex::Regex::new(
            r"^\s*(?:use\s+([^;*]+)|mod\s+[A-Za-z_][A-Za-z_0-9]*|extern\s+crate\s+[A-Za-z_][A-Za-z_0-9]*)\s*;\s*$",
        ),
        regex::Regex::new(r"\bas\b"),
        regex::Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+([^;]+);"),
    ) else {
        return Vec::new();
    };
    // Every name any `use` of either side binds, with its paths (a glob binds no known name).
    let mut bound: Vec<(String, String)> = Vec::new();
    for found in [Some(text), other]
        .into_iter()
        .flatten()
        .flat_map(|text| any_use.captures_iter(text))
    {
        if let Some(tree) = found.get(1) {
            use_bindings("", &compact_use_tree(tree.as_str()), &mut bound);
        }
    }
    let mut selected = Vec::new();
    for (number, line) in (1u32..).zip(text.split_inclusive('\n')) {
        let Some(found) = changed
            .binary_search(&number)
            .is_ok()
            .then(|| plain.captures(line))
            .flatten()
        else {
            continue;
        };
        let rebinds = found.get(1).is_some_and(|tree| {
            let mut own = Vec::new();
            alias.is_match(tree.as_str())
                || use_bindings("", &compact_use_tree(tree.as_str()), &mut own).is_none()
                || own.iter().any(|(name, path)| {
                    bound
                        .iter()
                        .any(|(other, other_path)| other == name && other_path != path)
                })
        });
        let opaque = usize::try_from(number - 1)
            .ok()
            .and_then(|index| opaque.get(index))
            .copied()
            .unwrap_or(true);
        if !opaque && !rebinds {
            selected.push(number);
        }
    }
    ranges(&selected)
}

/// A `use` tree without whitespace, ` as ` written `@`.
fn compact_use_tree(tree: &str) -> String {
    tree.split_whitespace()
        .map(|word| if word == "as" { "@" } else { word })
        .collect()
}

/// Adds the names a compact `use` tree binds, each with its full path; `None` for a glob or an unreadable tree.
fn use_bindings(prefix: &str, tree: &str, out: &mut Vec<(String, String)>) -> Option<()> {
    if let Some(inner) = tree.strip_suffix('}') {
        let (head, list) = inner.split_once('{')?;
        let prefix = format!("{prefix}{head}");
        let mut depth = 0usize;
        let mut item = String::new();
        for c in list.chars().chain([',']) {
            match c {
                ',' if depth == 0 => {
                    if !item.is_empty() {
                        use_bindings(&prefix, &item, out)?;
                    }
                    item.clear();
                    continue;
                }
                '{' => depth += 1,
                '}' => depth = depth.checked_sub(1)?,
                _ => {}
            }
            item.push(c);
        }
        return Some(());
    }
    let (path, alias) = match tree.split_once('@') {
        Some((path, alias)) => (path, Some(alias)),
        None => (tree, None),
    };
    if path.is_empty() || path.contains(['{', '}', '*', '@']) {
        return None;
    }
    let full = format!("{prefix}{path}");
    let full = full.strip_suffix("::self").unwrap_or(&full);
    let name = alias.unwrap_or_else(|| full.rsplit("::").next().unwrap_or(full));
    if name != "_" {
        out.push((name.to_owned(), full.to_owned()));
    }
    Some(())
}

/// Per-line facts of a Rust file (index = line - 1).
struct RustLines {
    /// Every token is a comment or whitespace and one is a comment; no block comment on the line spans an unchanged
    /// line (delimiters around unchanged code switch it off or on); not inside a macro token tree.
    comment: Vec<bool>,
    /// Touched by a literal, a block comment or a macro token tree: never an import declaration.
    opaque: Vec<bool>,
}

/// Tokens come from `rustc_lexer` after the compiler's BOM and shebang stripping, frontmatter allowed. A macro token
/// tree is the delimited group after `name!` or `macro_rules! name`. `changed` holds the changed lines. `None` when
/// the file has an unterminated literal or comment, an unknown token or invalid frontmatter.
fn rust_lines(text: &str, changed: &[u32]) -> Option<RustLines> {
    use ra_ap_rustc_lexer::{FrontmatterAllowed, LiteralKind, TokenKind, strip_shebang, tokenize};

    /// Where the tokens stand towards a macro token tree.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Macro {
        None,
        /// An identifier; `true` for `macro_rules`.
        Name(bool),
        /// `name!`; `true` after `macro_rules!`.
        Bang(bool),
        /// `macro_rules! name`.
        Rules,
        /// Inside a tree opened at the line, at the depth.
        Inside(usize, usize),
    }

    let lines = text.lines().count();
    let (mut comment, mut other) = (vec![false; lines], vec![false; lines]);
    let (mut opaque, mut in_macro) = (vec![false; lines], vec![false; lines]);
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let shebang = strip_shebang(body).unwrap_or(0);
    if shebang > 0 {
        *other.first_mut()? = true;
    }
    let mut rest = body.get(shebang..)?;
    let mut line = 0usize;
    let mut state = Macro::None;
    let mark = |flags: &mut Vec<bool>, from: usize, to: usize| {
        for flag in flags.iter_mut().take(to + 1).skip(from) {
            *flag = true;
        }
    };
    for token in tokenize(rest, FrontmatterAllowed::Yes) {
        let len = usize::try_from(token.len).ok()?;
        let (lexeme, tail) = (rest.get(..len)?, rest.get(len..)?);
        rest = tail;
        let invalid = match token.kind {
            TokenKind::BlockComment { terminated, .. } => !terminated,
            TokenKind::Literal { kind, .. } => match kind {
                LiteralKind::Char { terminated }
                | LiteralKind::Byte { terminated }
                | LiteralKind::Str { terminated }
                | LiteralKind::ByteStr { terminated }
                | LiteralKind::CStr { terminated } => !terminated,
                LiteralKind::RawStr { n_hashes }
                | LiteralKind::RawByteStr { n_hashes }
                | LiteralKind::RawCStr { n_hashes } => n_hashes.is_none(),
                LiteralKind::Int { .. } | LiteralKind::Float { .. } => false,
            },
            TokenKind::Frontmatter {
                has_invalid_preceding_whitespace,
                invalid_infostring,
            } => has_invalid_preceding_whitespace || invalid_infostring,
            TokenKind::Unknown
            | TokenKind::InvalidIdent
            | TokenKind::UnknownPrefix
            | TokenKind::UnknownPrefixLifetime
            | TokenKind::GuardedStrPrefix => true,
            _ => false,
        };
        if invalid {
            return None;
        }
        let first = line;
        line += lexeme.matches('\n').count();
        let touched = if lexeme.ends_with('\n') {
            line.saturating_sub(1)
        } else {
            line
        };
        let open = matches!(
            token.kind,
            TokenKind::OpenParen | TokenKind::OpenBrace | TokenKind::OpenBracket
        );
        let close = matches!(
            token.kind,
            TokenKind::CloseParen | TokenKind::CloseBrace | TokenKind::CloseBracket
        );
        state = match (state, token.kind) {
            (
                _,
                TokenKind::Whitespace
                | TokenKind::LineComment { .. }
                | TokenKind::BlockComment { .. },
            ) => state,
            (Macro::Inside(start, depth), _) if open => Macro::Inside(start, depth + 1),
            (Macro::Inside(start, 1), _) if close => {
                mark(&mut in_macro, start, touched);
                Macro::None
            }
            (Macro::Inside(start, depth), _) if close => Macro::Inside(start, depth - 1),
            (Macro::Inside(..), _) => state,
            (Macro::Bang(true), TokenKind::Ident) => Macro::Rules,
            (_, TokenKind::Ident) => Macro::Name(lexeme == "macro_rules"),
            (Macro::Name(rules), TokenKind::Bang) => Macro::Bang(rules),
            (Macro::Bang(false) | Macro::Rules, _) if open => Macro::Inside(first, 1),
            _ => Macro::None,
        };
        let marks = match token.kind {
            TokenKind::Whitespace => continue,
            TokenKind::LineComment { .. } => &mut comment,
            TokenKind::BlockComment { .. } => {
                mark(&mut opaque, first, touched);
                let whole = (first..=touched).all(|index| {
                    u32::try_from(index + 1).is_ok_and(|n| changed.binary_search(&n).is_ok())
                });
                if whole { &mut comment } else { &mut other }
            }
            TokenKind::Literal { .. } => {
                mark(&mut opaque, first, touched);
                &mut other
            }
            _ => &mut other,
        };
        mark(marks, first, touched);
    }
    if let Macro::Inside(start, _) = state {
        mark(&mut in_macro, start, lines);
    }
    Some(RustLines {
        comment: (0..lines)
            .map(|i| comment[i] && !other[i] && !in_macro[i])
            .collect(),
        opaque: (0..lines).map(|i| opaque[i] || in_macro[i]).collect(),
    })
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Glob match: a pattern without `/` matches the file name; otherwise segment by segment, `**` = any number of
/// segments; `*` = any characters within a segment.
fn glob(pattern: &str, path: &str) -> bool {
    if !pattern.contains('/') {
        return wildcard(pattern.as_bytes(), basename(path).as_bytes());
    }
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    segments(&pattern, &path)
}

fn segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments(rest, &path[skip..])),
        Some((first, rest)) => path.split_first().is_some_and(|(segment, tail)| {
            wildcard(first.as_bytes(), segment.as_bytes()) && segments(rest, tail)
        }),
    }
}

fn wildcard(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => (0..=text.len()).any(|skip| wildcard(rest, &text[skip..])),
        Some((first, rest)) => text
            .split_first()
            .is_some_and(|(byte, tail)| byte == first && wildcard(rest, tail)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source<'a>(
        path: &'a str,
        side: LineSide,
        text: Option<&'a str>,
        changed: &'a [u32],
    ) -> FileSource<'a> {
        FileSource {
            path,
            side,
            text,
            other_text: None,
            changed_lines: changed,
        }
    }

    fn range(start_line: u32, end_line: u32) -> LineRange {
        LineRange {
            start_line,
            end_line,
        }
    }

    fn classify(
        path: &str,
        text: Option<&str>,
        changed: &[u32],
    ) -> Vec<(
        Option<Vec<LineRange>>,
        NotAnalysedCategory,
        ProvenanceBadge,
        String,
    )> {
        Generic
            .classify(&source(path, LineSide::Head, text, changed))
            .expect("classify")
            .into_iter()
            .map(|item| {
                assert_eq!((item.file.as_str(), item.side), (path, LineSide::Head));
                (item.ranges, item.category, item.badge, item.rule)
            })
            .collect()
    }

    #[test]
    fn path_rules_first_match_with_changed_ranges_or_whole_file() {
        let by_path = ProvenanceBadge::ClassifiedByPath;
        let three = "a\nb\nc\n";
        assert_eq!(
            classify("tests/common/mod.rs", Some(three), &[2, 3]),
            [(
                Some(vec![range(2, 3)]),
                NotAnalysedCategory::Tests,
                by_path,
                "tests/**".to_owned()
            )]
        );
        assert_eq!(
            classify("crates/x/tests/it.rs", Some(three), &[1, 2, 3]),
            [(
                None,
                NotAnalysedCategory::Tests,
                by_path,
                "crates/*/tests/**".to_owned()
            )],
            "every line changed: the whole file"
        );
        assert_eq!(
            classify("docs/rpc.md", Some(three), &[1])[0].3,
            "docs/**",
            "docs/** wins over *.md"
        );
        assert_eq!(classify("README.md", Some(three), &[1])[0].3, "*.md");
        assert_eq!(
            classify("web/src/app.test.tsx", Some(three), &[1])[0].1,
            NotAnalysedCategory::Tests
        );
        assert_eq!(
            classify("sub/Cargo.lock", Some(three), &[1])[0].1,
            NotAnalysedCategory::GeneratedMechanical
        );
        assert_eq!(
            classify("assets/logo.min.js", None, &[]),
            [(
                None,
                NotAnalysedCategory::GeneratedMechanical,
                by_path,
                "*.min.js".to_owned()
            )],
            "binary or unread: the whole file"
        );
        assert!(classify("src/testing.rs", Some(three), &[1]).is_empty());
        for test_path in [
            "src/tests/auth.rs",
            "src/routes/tests/create.ts",
            "crates/x/src/tests/mod.rs",
            "lib/test/x.js",
            "packages/app/src/benches/run.ts",
            "a/b/c/tests/x.rs",
            "server/tests/handler.go",
            "frontend/components/tests/x.ts",
            "sdk/tests/a.ts",
            "crates/x/y/tests/a.rs",
        ] {
            assert!(
                classify(test_path, Some(three), &[1])
                    .iter()
                    .all(|item| item.1 == NotAnalysedCategory::Tests)
                    && !classify(test_path, Some(three), &[1]).is_empty(),
                "{test_path}"
            );
        }
        for test in [
            "benches/speed.rs",
            "packages/sdk/tests/a.ts",
            "packages/sdk/benches/a.ts",
            "packages/sdk/__tests__/a.ts",
            "crates/x/tests/it.rs",
            "crates/x/benches/speed.rs",
            "web/src/__tests__/a.ts",
            "tests/it.rs",
            "test/handler_test.js",
        ] {
            assert_eq!(
                classify(test, Some(three), &[1])[0].1,
                NotAnalysedCategory::Tests,
                "{test}"
            );
        }
        assert!(
            classify("api/openapi.spec.yaml", Some(three), &[1]).is_empty(),
            "an API spec is no test"
        );
        assert!(classify("src/latest/mod.rs", Some(three), &[1]).is_empty());
    }

    #[test]
    fn generated_header_marks_the_file() {
        let go = "// Code generated by protoc-gen-go. DO NOT EDIT.\npackage x\nfunc A() {}\n";
        assert_eq!(
            classify("api/x.go", Some(go), &[3]),
            [(
                Some(vec![range(3, 3)]),
                NotAnalysedCategory::GeneratedMechanical,
                ProvenanceBadge::ClassifiedByLineRule,
                GENERATED_HEADER_RULE.to_owned()
            )]
        );
        let prost = "// This file is @generated by prost-build.\npub struct A;\n";
        assert_eq!(classify("src/pb.rs", Some(prost), &[1, 2])[0].0, None);
        let attribute = "#[doc = \"@generated\"]\nfn a() {}\n";
        assert!(classify("src/a.rs", Some(attribute), &[2]).is_empty());
        let late: String = (0..HEADER_LINES).map(|_| "x\n").collect::<String>() + "// @generated\n";
        assert!(classify("src/a.rs", Some(&late), &[1]).is_empty());
    }

    #[test]
    fn comment_line_rule_takes_only_runs_of_comment_lines() {
        let text = "fn a() {}\n/// Doc.\n\n// Note.\nlet x = 1;\n/// Doc of b.\nfn b() {}\n";
        assert_eq!(
            classify("src/a.rs", Some(text), &[2, 3, 4, 6, 7]),
            [(
                Some(vec![range(2, 2), range(4, 4), range(6, 6)]),
                NotAnalysedCategory::DocsComments,
                ProvenanceBadge::ClassifiedByLineRule,
                LINE_COMMENT_RULE.to_owned()
            )],
            "comment-only lines are covered individually, even in a mixed run"
        );
        assert!(
            classify("src/a.rs", Some(text), &[3]).is_empty(),
            "blank only"
        );
        assert!(
            classify("src/a.rs", Some(text), &[99]).is_empty(),
            "beyond the file"
        );
        assert!(classify("src/a.rs", None, &[1]).is_empty(), "binary");
    }

    /// Comment-rule ranges of `lines` when every line changed.
    fn comment_rule(path: &str, lines: &[&str]) -> Option<Vec<LineRange>> {
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        let changed: Vec<u32> = (1..=u32::try_from(lines.len()).expect("small")).collect();
        let found = classify(path, Some(&text), &changed);
        found
            .into_iter()
            .next()
            .map(|item| item.0.unwrap_or_default())
    }

    #[test]
    fn comment_rule_follows_rust_lexical_context() {
        let line = |n| vec![range(n, n)];
        // Inside a raw string a `//` line is data, and the closing line may run code.
        let raw = ["let url = r#\"", "//good.example", "\"#;"];
        assert_eq!(
            comment_rule("src/a.rs", &raw),
            None,
            "no line of a raw string is a comment"
        );
        let payload = [
            "let s = r#\"",
            "//\"#; launch_payload(); let _ = r#\"",
            "\"#;",
            "// plain",
        ];
        let text: String = payload.iter().map(|l| format!("{l}\n")).collect();
        assert!(
            classify("src/a.rs", Some(&text), &[2]).is_empty(),
            "closes the raw string and runs code"
        );
        assert_eq!(classify("src/a.rs", Some(&text), &[4])[0].0, Some(line(4)));
        let multi = [
            "let s = \"first",
            "// inside a string",
            "\";",
            "let t = br##\"x\"#",
            "// still raw",
            "\"##;",
        ];
        let text: String = multi.iter().map(|l| format!("{l}\n")).collect();
        assert!(classify("src/a.rs", Some(&text), &[2]).is_empty());
        assert!(classify("src/a.rs", Some(&text), &[5]).is_empty());
        let block = [
            "/* outer /* nested */",
            "still inside the outer comment",
            "*/ run();",
            "// after",
        ];
        let text: String = block.iter().map(|l| format!("{l}\n")).collect();
        assert_eq!(
            classify("src/a.rs", Some(&text), &[1, 2, 3])[0].0,
            Some(vec![range(1, 2)]),
            "inside a nested block comment"
        );
        assert!(
            classify("src/a.rs", Some(&text), &[2]).is_empty(),
            "the comment spans unchanged lines"
        );
        assert!(
            classify("src/a.rs", Some(&text), &[3]).is_empty(),
            "the comment closes and code runs"
        );
        assert_eq!(classify("src/a.rs", Some(&text), &[4])[0].0, Some(line(4)));
        let shebang = [
            "#!/unused \"",
            "fn main() {",
            "let _ = \"",
            "//\"; std::process::exit(1); let _ = \"",
            "\";",
            "}",
        ];
        let text: String = shebang.iter().map(|l| format!("{l}\n")).collect();
        assert!(
            classify("src/a.rs", Some(&text), &[1, 4]).is_empty(),
            "the shebang is stripped before lexing: line 4 closes a string and runs code"
        );
        let frontmatter = ["---", "// in frontmatter", "---", "fn main() {}"];
        let text: String = frontmatter.iter().map(|l| format!("{l}\n")).collect();
        assert!(
            classify("src/a.rs", Some(&text), &[2]).is_empty(),
            "frontmatter is not a comment"
        );
        let unterminated = ["// a note", "fn f() { let s = \"open; }"];
        let text: String = unterminated.iter().map(|l| format!("{l}\n")).collect();
        assert!(
            classify("src/a.rs", Some(&text), &[1]).is_empty(),
            "an unterminated literal leaves the whole file to the agent"
        );
        let open_comment = ["// a note", "/* never closed"];
        let text: String = open_comment.iter().map(|l| format!("{l}\n")).collect();
        assert!(classify("src/a.rs", Some(&text), &[1]).is_empty());
        let chars = [
            "let q = '\"';",
            "let e = '\\'';",
            "fn f<'a>(x: &'a str) {}",
            "// comment",
        ];
        let text: String = chars.iter().map(|l| format!("{l}\n")).collect();
        assert_eq!(
            classify("src/a.rs", Some(&text), &[4])[0].0,
            Some(line(4)),
            "char literals and lifetimes do not open strings"
        );
        let doc = ["/// Sends a request.", "pub fn send() {}"];
        assert_eq!(
            comment_rule("src/a.rs", &doc),
            Some(line(1)),
            "the doc line is covered by rule; the code still needs coverage"
        );
        assert_eq!(
            comment_rule("src/a.rs", &["/// Only a doc line."]),
            Some(line(1))
        );
        assert_eq!(
            comment_rule("web/a.ts", &["// a note"]),
            None,
            "no lexer: not confirmed"
        );
    }

    /// Line-rule ranges of `lines` (changed: `changed`) with `other` as the other side's text.
    fn line_rule(lines: &[&str], other: Option<&str>, changed: &[u32]) -> Vec<LineRange> {
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        let mut found: Vec<u32> = Generic
            .classify(&FileSource {
                other_text: other,
                ..source("src/a.rs", LineSide::Base, Some(&text), changed)
            })
            .expect("classify")
            .into_iter()
            .flat_map(|item| item.ranges.unwrap_or_default())
            .flat_map(|r| r.start_line..=r.end_line)
            .collect();
        found.sort_unstable();
        assert_eq!(
            context_lines("src/a.rs", &text, other, changed),
            found,
            "main view and pre-pass agree"
        );
        ranges(&found)
    }

    #[test]
    fn block_comment_delimiters_around_unchanged_code_stay_with_the_agent() {
        let disabled = [
            "fn transfer() {",
            "    /*",
            "    check_authorised()?;",
            "    */",
            "    debit()",
            "}",
        ];
        assert_eq!(
            line_rule(&disabled, None, &[2, 4]),
            [],
            "the delimiters switch unchanged code off (head) or on (base)"
        );
        let trailing = ["/*", "run();", "*/ // end of disabled code"];
        assert_eq!(line_rule(&trailing, None, &[1, 3]), []);
        let added = ["/*", " * A new note.", " */", "fn f() {}"];
        assert_eq!(
            line_rule(&added, None, &[1, 2, 3]),
            [range(1, 3)],
            "a whole added block comment is still hidden"
        );
        assert_eq!(
            line_rule(&added, None, &[2]),
            [],
            "an edit inside a block comment spanning unchanged lines is shown"
        );
        assert_eq!(
            line_rule(&["// a", "/* b */", "fn f() {}"], None, &[1, 2]),
            [range(1, 2)]
        );
    }

    #[test]
    fn import_rule_hides_only_plain_private_imports() {
        let plain = [
            "use std::fmt;",
            "use crate::a::{X, Y};",
            "mod m;",
            "extern crate foo;",
            "use ::b::c;",
        ];
        assert_eq!(line_rule(&plain, None, &[1, 2, 3, 4, 5]), [range(1, 5)]);
        for line in [
            "pub use crate::a::X;",
            "pub(crate) use crate::a::X;",
            "pub mod m;",
            "use crate::auth::verify_lenient as verify;",
            "use crate::a::{X as Y};",
            "use crate::prelude::*;",
            "use crate::a::{b::*, c};",
            "extern crate foo as bar;",
        ] {
            assert_eq!(line_rule(&[line], None, &[1]), [], "{line}");
        }
    }

    #[test]
    fn import_rule_shows_a_name_rebound_to_another_path() {
        let base = "use std::sync::Mutex;\nfn f() {}\n";
        assert_eq!(
            line_rule(&["use parking_lot::Mutex;", "fn f() {}"], Some(base), &[1]),
            [],
            "the same name now means another item"
        );
        let shadowed = [
            "use std::sync::Mutex;",
            "mod inner {",
            "    use parking_lot::Mutex;",
            "}",
        ];
        assert_eq!(line_rule(&shadowed, None, &[3]), []);
        assert_eq!(
            line_rule(&["use std::sync::{Arc, Mutex};"], Some(base), &[1]),
            [range(1, 1)],
            "same paths: still hidden"
        );
        assert_eq!(
            line_rule(
                &["use std::fmt;", "use std::sync::Mutex;"],
                Some(base),
                &[1, 2]
            ),
            [range(1, 2)],
            "a reordered import is still hidden"
        );
    }

    #[test]
    fn nothing_inside_a_macro_token_tree_is_hidden() {
        let gate = [
            "macro_rules! gate {",
            "    (use deny;) => { false };",
            "    (use allow;) => { true };",
            "}",
            "fn allowed() -> bool {",
            "    gate! {",
            "        use allow;",
            "    }",
            "}",
            "use std::fmt;",
        ];
        assert_eq!(line_rule(&gate, None, &[7, 10]), [range(10, 10)]);
        let body = [
            "macro_rules! imports {",
            "    () => {",
            "        use crate::strict::verify;",
            "        // pick the strict checker",
            "    };",
            "}",
        ];
        assert_eq!(line_rule(&body, None, &[3, 4]), []);
        let call = ["let v = json!({", "    // a note", "    \"a\": 1,", "});"];
        assert_eq!(line_rule(&call, None, &[2]), []);
        let not_macro = ["while !done {", "    // a note", "}"];
        assert_eq!(
            line_rule(&not_macro, None, &[2]),
            [range(2, 2)],
            "a negation is no macro"
        );
    }

    #[test]
    fn agent_instruction_file_rules() {
        for path in [
            "AGENTS.md",
            "sub/dir/CLAUDE.md",
            "claude.md",
            ".codex/config.toml",
            ".claude/settings.json",
            "pkg/.agents/skill.md",
            ".cursorrules",
            ".cursor/rules/a.mdc",
            ".github/copilot-instructions.md",
            ".github/instructions/rust.instructions.md",
            ".claudius/project-profile.md",
        ] {
            assert!(is_agent_instruction_file(path), "{path}");
        }
        for path in [
            "src/agents.rs",
            "docs/claude-notes.md",
            ".github/workflows/ci.yml",
            "README.md",
        ] {
            assert!(!is_agent_instruction_file(path), "{path}");
        }
    }

    #[test]
    fn dependency_files_are_recognised_by_name() {
        assert_eq!(
            dependency_file("Cargo.toml"),
            Some(DependencyFile::CargoToml)
        );
        assert_eq!(
            dependency_file("crates/a/Cargo.toml"),
            Some(DependencyFile::CargoToml)
        );
        assert_eq!(
            dependency_file("Cargo.lock"),
            Some(DependencyFile::CargoLock)
        );
        assert_eq!(
            dependency_file("web/package.json"),
            Some(DependencyFile::Other)
        );
        assert_eq!(
            dependency_file("requirements-dev.txt"),
            Some(DependencyFile::Other)
        );
        assert_eq!(dependency_file("go.mod"), Some(DependencyFile::Other));
        assert_eq!(dependency_file("src/main.rs"), None);
    }

    #[test]
    fn generated_and_line_rule_items_are_told_apart() {
        let item = |rule: &str| Classified {
            file: "x".to_owned(),
            side: LineSide::Head,
            ranges: None,
            category: NotAnalysedCategory::GeneratedMechanical,
            badge: ProvenanceBadge::ClassifiedByPath,
            rule: rule.to_owned(),
        };
        assert!(is_generated(&item(GENERATED_HEADER_RULE)));
        assert!(is_generated(&item("*.pb.go")));
        assert!(
            !is_generated(&item("Cargo.lock")),
            "lockfiles are mechanical, still rendered"
        );
        assert!(is_line_rule(&item(LINE_COMMENT_RULE)));
        let listed = rules();
        assert!(
            listed
                .iter()
                .any(|rule| rule.kind == RuleKind::PathGlob && rule.pattern == "tests/**")
        );
        assert!(
            listed
                .iter()
                .any(|rule| rule.kind == RuleKind::Lexer && rule.pattern == LINE_COMMENT_RULE)
        );
        assert_eq!(listed.len(), PATH_RULES.len() + 4);
        assert!(!is_line_rule(&item("*.md")));
    }

    #[test]
    fn generic_call_hierarchy_none() {
        let symbol = SymbolAt {
            file: "src/rpc/client.rs".to_owned(),
            line: 99,
            symbol: "RpcClient::send".to_owned(),
        };
        assert_eq!(Generic.call_hierarchy(&symbol), None);
        assert!(Generic.detect("any/file.ext"));
        assert_eq!(Generic.name(), "generic");
    }
}

#[cfg(test)]
mod rule_kind_regression {
    #[test]
    fn rule_kinds_name_the_actual_detector() {
        let rules = super::rules();
        let kind = |pattern: &str| {
            serde_json::to_value(rules.iter().find(|r| r.pattern == pattern).expect("rule"))
                .expect("wire")["kind"]
                .clone()
        };
        assert_eq!(kind(super::LINE_COMMENT_RULE), "lexer");
        assert_eq!(kind(super::GENERATED_HEADER_RULE), "header");
        assert!(
            rules
                .iter()
                .any(|r| serde_json::to_value(r).expect("wire")["kind"] == "binary")
        );
    }
}
