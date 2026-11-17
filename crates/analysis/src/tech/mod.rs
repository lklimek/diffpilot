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
        Ok(if comments.is_empty() {
            Vec::new()
        } else {
            vec![item(
                NotAnalysedCategory::DocsComments,
                ProvenanceBadge::ClassifiedByLineRule,
                LINE_COMMENT_RULE,
                Some(comments),
            )]
        })
    }

    fn call_hierarchy(&self, _symbol: &SymbolAt) -> Option<CallHierarchy> {
        // TODO: static call hierarchy (tree-sitter tags) belongs to the language plugins (brief §5, v2).
        None
    }
}

/// Line rule of the `generic` plugin: comment-only lines (a run of changed lines whose non-blank lines all match),
/// decided by the Rust compiler's lexer over the whole file; other languages stay with the agent.
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
            GENERATED_HEADER_RULE,
            NotAnalysedCategory::GeneratedMechanical,
        ),
        (LINE_COMMENT_RULE, NotAnalysedCategory::DocsComments),
    ]
    .into_iter()
    .map(|(rule, category)| NotAnalysedRule {
        kind: RuleKind::LineRegex,
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
    item.rule == LINE_COMMENT_RULE
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
    let Some(comment) = path
        .ends_with(".rs")
        .then(|| rust_comment_lines(text))
        .flatten()
    else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let at = |number: u32| -> Option<usize> { usize::try_from(number).ok()?.checked_sub(1) };
    let mut out = Vec::new();
    for run in ranges(changed) {
        let mut any = false;
        let all = (run.start_line..=run.end_line).all(|number| {
            let Some(index) = at(number) else {
                return false;
            };
            match lines.get(index) {
                Some(text) if text.trim().is_empty() => true,
                Some(_) => {
                    any = true;
                    comment.get(index).copied().unwrap_or(false)
                }
                None => false,
            }
        });
        if all && any {
            out.push(run);
        }
    }
    out
}

/// Per line (index = line - 1): every token on the line is a comment or whitespace, and one is a comment. Tokens
/// come from `rustc_lexer` after the compiler's BOM and shebang stripping, frontmatter allowed. `None` when the
/// file has an unterminated literal or comment, an unknown token or invalid frontmatter.
fn rust_comment_lines(text: &str) -> Option<Vec<bool>> {
    use ra_ap_rustc_lexer::{FrontmatterAllowed, LiteralKind, TokenKind, strip_shebang, tokenize};

    let lines = text.lines().count();
    let (mut comment, mut other) = (vec![false; lines], vec![false; lines]);
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let shebang = strip_shebang(body).unwrap_or(0);
    if shebang > 0 {
        *other.first_mut()? = true;
    }
    let mut rest = body.get(shebang..)?;
    let mut line = 0usize;
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
        let marks = match token.kind {
            TokenKind::Whitespace => continue,
            TokenKind::LineComment { .. } | TokenKind::BlockComment { .. } => &mut comment,
            _ => &mut other,
        };
        for flag in marks.iter_mut().take(touched + 1).skip(first) {
            *flag = true;
        }
    }
    Some(comment.iter().zip(&other).map(|(c, o)| *c && !*o).collect())
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
        for production in [
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
                classify(production, Some(three), &[1]).is_empty(),
                "{production}"
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
                Some(vec![range(2, 4)]),
                NotAnalysedCategory::DocsComments,
                ProvenanceBadge::ClassifiedByLineRule,
                LINE_COMMENT_RULE.to_owned()
            )],
            "2..4 is comments and a blank line; 6..7 is a doc comment with its code"
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
            classify("src/a.rs", Some(&text), &[2])[0].0,
            Some(line(2)),
            "inside a nested block comment"
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
            None,
            "a doc comment with its code stays with the agent"
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
                .any(|rule| rule.kind == RuleKind::LineRegex && rule.pattern == LINE_COMMENT_RULE)
        );
        assert_eq!(listed.len(), PATH_RULES.len() + 2);
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
