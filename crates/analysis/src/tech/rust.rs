//! Rust line rules of the generic plugin, read from `rustc_lexer` tokens, never from raw text: comment-only lines
//! and plain private imports (brief amendment 2026-11-18). Whatever the rules cannot read with certainty is not
//! covered.

use std::collections::{HashMap, HashSet};

use contracts::common::LineSide;
use ra_ap_rustc_lexer::{FrontmatterAllowed, LiteralKind, TokenKind, strip_shebang, tokenize};

use super::{FileSource, OtherSide};
use crate::diff::Hunk;

/// Deepest `{` nesting of a `use` tree the rules read; a deeper tree is unreadable.
const MAX_USE_DEPTH: usize = 32;

/// Names every module sees without an import: the std prelude (2024 edition), primitive types and the std macros. A
/// `use` that binds one of them changes what unchanged code means.
const PRELUDE: &[&str] = &[
    "Copy",
    "Send",
    "Sized",
    "Sync",
    "Unpin",
    "Drop",
    "Fn",
    "FnMut",
    "FnOnce",
    "AsyncFn",
    "AsyncFnMut",
    "AsyncFnOnce",
    "drop",
    "align_of",
    "align_of_val",
    "size_of",
    "size_of_val",
    "Box",
    "ToOwned",
    "Clone",
    "PartialEq",
    "PartialOrd",
    "Eq",
    "Ord",
    "AsRef",
    "AsMut",
    "Into",
    "From",
    "Default",
    "Iterator",
    "Extend",
    "IntoIterator",
    "DoubleEndedIterator",
    "ExactSizeIterator",
    "Option",
    "Some",
    "None",
    "Result",
    "Ok",
    "Err",
    "String",
    "ToString",
    "Vec",
    "TryFrom",
    "TryInto",
    "FromIterator",
    "Future",
    "IntoFuture",
    "bool",
    "char",
    "str",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "usize",
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "isize",
    "f16",
    "f32",
    "f64",
    "f128",
    "assert",
    "assert_eq",
    "assert_ne",
    "cfg",
    "column",
    "compile_error",
    "concat",
    "dbg",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "env",
    "eprint",
    "eprintln",
    "file",
    "format",
    "format_args",
    "include",
    "include_bytes",
    "include_str",
    "line",
    "matches",
    "module_path",
    "option_env",
    "panic",
    "print",
    "println",
    "stringify",
    "thread_local",
    "todo",
    "unimplemented",
    "unreachable",
    "vec",
    "write",
    "writeln",
];

/// Keywords followed by the name of the item they declare (`crate` for `extern crate name`).
const ITEM_KEYWORDS: &[&str] = &[
    "fn", "struct", "enum", "union", "type", "trait", "mod", "const", "static", "crate",
];

/// Covered changed lines of one Rust file side, per rule.
pub(super) struct Covered {
    /// Comment-only lines.
    pub comments: Vec<u32>,
    /// Plain private `use` declarations.
    pub imports: Vec<u32>,
}

/// The covered changed lines of `file`, whose text is `text`.
pub(super) fn covered<'a>(file: &FileSource<'a>, text: &'a str) -> Covered {
    let nothing = || Covered {
        comments: Vec::new(),
        imports: Vec::new(),
    };
    let Some(this) = lex(text) else {
        return nothing();
    };
    // An unread or unreadable other side covers nothing.
    let other = match file.other {
        OtherSide::Absent => Other::Absent,
        OtherSide::Text(text) => match lex(text) {
            Some(lexed) => Other::Lexed(lexed),
            None => return nothing(),
        },
        OtherSide::Unknown => return nothing(),
    };
    Covered {
        comments: comment_lines(file, &this, &other),
        imports: import_lines(file, &this, &other),
    }
}

/// The other side of the file, lexed.
enum Other<'a> {
    Absent,
    Lexed(Lexed<'a>),
}

/// A token other than whitespace.
struct Token<'a> {
    kind: TokenKind,
    text: &'a str,
    /// Byte offset after the BOM and shebang.
    start: usize,
    /// First and last line (0-based) the token touches.
    first: usize,
    last: usize,
    /// Delimiter depth before the token, counted over code tokens.
    depth: usize,
}

/// One side of a Rust file as tokens.
struct Lexed<'a> {
    /// Every token except whitespace, in order.
    tokens: Vec<Token<'a>>,
    /// Number of lines.
    lines: usize,
    /// Line 1 is a shebang.
    shebang: bool,
    /// Per line (index = line - 1): inside a macro token tree, the delimited group after `name!` or
    /// `macro_rules! name`.
    in_macro: Vec<bool>,
}

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

/// Tokens after the compiler's BOM and shebang stripping, frontmatter allowed; `None` when the file has an
/// unterminated literal or comment, an unknown token or invalid frontmatter. Linear in the text.
fn lex(text: &str) -> Option<Lexed<'_>> {
    let lines = text.lines().count();
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let shebang = strip_shebang(body).unwrap_or(0);
    let mut rest = body.get(shebang..)?;
    let (mut line, mut depth, mut offset) = (0usize, 0usize, 0usize);
    let mut tokens = Vec::new();
    let mut in_macro = vec![false; lines];
    let mut state = Macro::None;
    for token in tokenize(rest, FrontmatterAllowed::Yes) {
        let len = usize::try_from(token.len).ok()?;
        let (lexeme, tail) = (rest.get(..len)?, rest.get(len..)?);
        rest = tail;
        let start = offset;
        offset += len;
        if invalid(token.kind) {
            return None;
        }
        let first = line;
        line += lexeme.matches('\n').count();
        let last = if lexeme.ends_with('\n') {
            line.saturating_sub(1)
        } else {
            line
        };
        let (open, close) = (is_open(token.kind), is_close(token.kind));
        state = match (state, token.kind) {
            (
                _,
                TokenKind::Whitespace
                | TokenKind::LineComment { .. }
                | TokenKind::BlockComment { .. },
            ) => state,
            (Macro::Inside(start, depth), _) if open => Macro::Inside(start, depth + 1),
            (Macro::Inside(start, 1), _) if close => {
                mark(&mut in_macro, start, last);
                Macro::None
            }
            (Macro::Inside(start, depth), _) if close => Macro::Inside(start, depth - 1),
            (Macro::Inside(..), _) => state,
            (Macro::Bang(true), TokenKind::Ident | TokenKind::RawIdent) => Macro::Rules,
            (_, TokenKind::Ident | TokenKind::RawIdent) => {
                Macro::Name(name(lexeme) == "macro_rules")
            }
            (Macro::Name(rules), TokenKind::Bang) => Macro::Bang(rules),
            (Macro::Bang(false) | Macro::Rules, _) if open => Macro::Inside(first, 1),
            _ => Macro::None,
        };
        if token.kind == TokenKind::Whitespace {
            continue;
        }
        let at = depth;
        if !is_comment(token.kind) {
            if open {
                depth += 1;
            } else if close {
                depth = depth.saturating_sub(1);
            }
        }
        tokens.push(Token {
            kind: token.kind,
            text: lexeme,
            start,
            first,
            last,
            depth: at,
        });
    }
    if let Macro::Inside(start, _) = state {
        mark(&mut in_macro, start, lines);
    }
    Some(Lexed {
        tokens,
        lines,
        shebang: shebang > 0,
        in_macro,
    })
}

fn invalid(kind: TokenKind) -> bool {
    match kind {
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
    }
}

fn is_comment(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LineComment { .. } | TokenKind::BlockComment { .. }
    )
}

fn is_open(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::OpenParen | TokenKind::OpenBrace | TokenKind::OpenBracket
    )
}

fn is_close(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::CloseParen | TokenKind::CloseBrace | TokenKind::CloseBracket
    )
}

fn is_ident(kind: TokenKind) -> bool {
    matches!(kind, TokenKind::Ident | TokenKind::RawIdent)
}

/// An identifier without its `r#` prefix.
fn name(text: &str) -> &str {
    text.strip_prefix("r#").unwrap_or(text)
}

fn mark(flags: &mut [bool], from: usize, to: usize) {
    for flag in flags.iter_mut().take(to.saturating_add(1)).skip(from) {
        *flag = true;
    }
}

/// `flags` at 1-based `line`; `false` outside the file.
fn at(flags: &[bool], line: u32) -> bool {
    usize::try_from(line)
        .ok()
        .and_then(|line| line.checked_sub(1))
        .and_then(|index| flags.get(index))
        .copied()
        .unwrap_or(false)
}

/// Changed lines of `side` in `hunk`.
fn lines_of(hunk: &crate::diff::Hunk, side: LineSide) -> &[u32] {
    match side {
        LineSide::Head => &hunk.added,
        LineSide::Base => &hunk.removed,
    }
}

fn other_side(side: LineSide) -> LineSide {
    match side {
        LineSide::Head => LineSide::Base,
        LineSide::Base => LineSide::Head,
    }
}

/// Per-line comment facts of one side.
struct CommentFacts {
    /// Every token is a comment and one is; no block comment on the line spans an unchanged line; not in a macro
    /// token tree.
    only: Vec<bool>,
    /// Carries a block comment.
    block: Vec<bool>,
    /// No token touches the line.
    blank: Vec<bool>,
}

impl Lexed<'_> {
    /// Comment facts with `changed` (sorted) as the changed lines of this side.
    fn comment_facts(&self, changed: &[u32]) -> CommentFacts {
        let lines = self.lines;
        let (mut comment, mut code) = (vec![false; lines], vec![false; lines]);
        let (mut block, mut touched) = (vec![false; lines], vec![false; lines]);
        if self.shebang {
            mark(&mut code, 0, 0);
            mark(&mut touched, 0, 0);
        }
        for token in &self.tokens {
            mark(&mut touched, token.first, token.last);
            let flags = match token.kind {
                TokenKind::LineComment { .. } => &mut comment,
                TokenKind::BlockComment { .. } => {
                    let whole = (token.first..=token.last).all(|index| {
                        u32::try_from(index + 1).is_ok_and(|n| changed.binary_search(&n).is_ok())
                    });
                    if whole {
                        mark(&mut block, token.first, token.last);
                        &mut comment
                    } else {
                        &mut code
                    }
                }
                _ => &mut code,
            };
            mark(flags, token.first, token.last);
        }
        CommentFacts {
            only: (0..lines)
                .map(|i| comment[i] && !code[i] && !self.in_macro[i])
                .collect(),
            block,
            blank: touched.iter().map(|touched| !touched).collect(),
        }
    }
}

/// Changed comment-only lines. A line with a block comment in a hunk that changes both sides counts only when every
/// non-blank changed line of the hunk, on both sides, is comment-only: a delimiter moved on a changed line can switch
/// unchanged code on or off.
fn comment_lines(file: &FileSource<'_>, this: &Lexed<'_>, other: &Other<'_>) -> Vec<u32> {
    let facts = this.comment_facts(file.changed_lines);
    let other_side = other_side(file.side);
    let mut other_changed: Vec<u32> = file
        .hunks
        .iter()
        .flat_map(|hunk| lines_of(hunk, other_side).iter().copied())
        .collect();
    other_changed.sort_unstable();
    let other_facts = match other {
        Other::Lexed(lexed) => Some(lexed.comment_facts(&other_changed)),
        Other::Absent => None,
    };
    let comment_or_blank =
        |facts: &CommentFacts, line: u32| at(&facts.only, line) || at(&facts.blank, line);
    let mut out = Vec::new();
    for hunk in file.hunks {
        let (lines, others) = (lines_of(hunk, file.side), lines_of(hunk, other_side));
        // A pure addition or deletion cannot reinterpret unchanged code: its comments span only changed lines.
        let whole = others.is_empty()
            || (lines.iter().all(|&line| comment_or_blank(&facts, line))
                && other_facts
                    .as_ref()
                    .is_some_and(|other| others.iter().all(|&line| comment_or_blank(other, line))));
        out.extend(
            lines
                .iter()
                .copied()
                .filter(|&line| at(&facts.only, line) && (!at(&facts.block, line) || whole)),
        );
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// One token of a line as the whitespace rule compares it: kind, text (a line comment without trailing whitespace) and
/// whether it is punctuation touching the previous punctuation (`&&` is not `& &`).
type LineToken<'a> = (std::mem::Discriminant<TokenKind>, &'a str, bool);

/// A line in the whitespace pairing; an opaque line equals no other line.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum LineKey<'r, 'a> {
    Tokens(&'r [LineToken<'a>]),
    Opaque(LineSide, u32),
}

/// Pairing work of one file, in `(removed + added lines)²` per hunk (an upper bound of Myers); hunks beyond it
/// are not paired.
const WHITESPACE_WORK_BUDGET: u64 = 64_000_000;

fn is_punct(kind: TokenKind) -> bool {
    !matches!(
        kind,
        TokenKind::Ident
            | TokenKind::RawIdent
            | TokenKind::Literal { .. }
            | TokenKind::Lifetime { .. }
            | TokenKind::LineComment { .. }
            | TokenKind::BlockComment { .. }
            | TokenKind::Whitespace
    )
}

impl<'a> Lexed<'a> {
    /// Per line (index = line - 1): its tokens; `None` for a line a multi-line token (string, block comment), the
    /// shebang or a macro token tree touches: whitespace there may be content.
    fn line_tokens(&self) -> Vec<Option<Vec<LineToken<'a>>>> {
        let mut out: Vec<Option<Vec<LineToken<'a>>>> = vec![Some(Vec::new()); self.lines];
        if self.shebang
            && let Some(first) = out.first_mut()
        {
            *first = None;
        }
        let mut previous: Option<&Token<'a>> = None;
        for token in &self.tokens {
            if token.first != token.last {
                for line in out.iter_mut().take(token.last + 1).skip(token.first) {
                    *line = None;
                }
            } else if let Some(Some(line)) = out.get_mut(token.first) {
                let joint = previous.is_some_and(|before| {
                    is_punct(before.kind)
                        && is_punct(token.kind)
                        && before.start + before.text.len() == token.start
                });
                let text = match token.kind {
                    TokenKind::LineComment { .. } => token.text.trim_end(),
                    _ => token.text,
                };
                line.push((std::mem::discriminant(&token.kind), text, joint));
            }
            previous = Some(token);
        }
        // Inside a macro token tree spacing is observable (`stringify!`, procedural macros).
        for (line, inside) in out.iter_mut().zip(&self.in_macro) {
            if *inside {
                *line = None;
            }
        }
        out
    }
}

/// The pairing keys of `lines` (blank lines left out: they pair with nothing and stay visible).
fn line_keys<'r, 'a>(
    side: LineSide,
    lines: &[u32],
    tokens: &'r [Option<Vec<LineToken<'a>>>],
) -> Vec<(u32, LineKey<'r, 'a>)> {
    let mut out = Vec::new();
    for &line in lines {
        let found = usize::try_from(line)
            .ok()
            .and_then(|line| line.checked_sub(1))
            .and_then(|index| tokens.get(index));
        match found {
            Some(Some(tokens)) if tokens.is_empty() => {}
            Some(Some(tokens)) => out.push((line, LineKey::Tokens(tokens))),
            _ => out.push((line, LineKey::Opaque(side, line))),
        }
    }
    out
}

/// Pairs (base line, head line) of changed lines of one hunk whose tokens are equal (comments included, literals as
/// written): only indentation, trailing whitespace, line endings or spacing between tokens differ. Lines are paired by
/// an exact diff of their token sequences, so re-indented code inside a new `if` pairs with its original while the
/// wrapper lines stay changed. Nothing pairs when either side is not readable by the lexer.
pub(super) fn whitespace_pairs(base: &str, head: &str, hunks: &[Hunk]) -> Vec<(u32, u32)> {
    let (Some(base), Some(head)) = (lex(base), lex(head)) else {
        return Vec::new();
    };
    let (base_tokens, head_tokens) = (base.line_tokens(), head.line_tokens());
    let mut budget = WHITESPACE_WORK_BUDGET;
    let mut out = Vec::new();
    for hunk in hunks {
        if hunk.added.is_empty() || hunk.removed.is_empty() {
            continue;
        }
        let size = u64::try_from(hunk.added.len() + hunk.removed.len()).unwrap_or(u64::MAX);
        let Some(left) = budget.checked_sub(size.saturating_mul(size)) else {
            continue;
        };
        budget = left;
        let base = line_keys(LineSide::Base, &hunk.removed, &base_tokens);
        let head = line_keys(LineSide::Head, &hunk.added, &head_tokens);
        let base_keys: Vec<LineKey<'_, '_>> = base.iter().map(|(_, key)| *key).collect();
        let head_keys: Vec<LineKey<'_, '_>> = head.iter().map(|(_, key)| *key).collect();
        for op in similar::capture_diff_slices(similar::Algorithm::Myers, &base_keys, &head_keys) {
            if let similar::DiffOp::Equal {
                old_index,
                new_index,
                len,
            } = op
            {
                let base_lines = base.iter().skip(old_index).take(len);
                let head_lines = head.iter().skip(new_index).take(len);
                out.extend(base_lines.zip(head_lines).map(|((b, _), (h, _))| (*b, *h)));
            }
        }
    }
    out
}

/// Path ids: a path is its parent's id plus one segment; the root is 0.
#[derive(Default)]
struct Paths<'a> {
    ids: HashMap<(u32, &'a str), u32>,
}

impl<'a> Paths<'a> {
    fn id(&mut self, parent: u32, segment: &'a str) -> u32 {
        let next = u32::try_from(self.ids.len() + 1).unwrap_or(u32::MAX);
        *self.ids.entry((parent, segment)).or_insert(next)
    }
}

/// A `use` tree read from tokens.
struct UseTree<'a> {
    /// Bound names with their path ids (`_` left out).
    bindings: Vec<(&'a str, u32)>,
    /// Has a `*`.
    glob: bool,
    /// Has an `as`.
    alias: bool,
    /// Index of the closing `;` among the code tokens.
    end: usize,
}

/// The `use` tree after the `use` keyword at code token `start - 1`, iterative; `None` when it is not a tree the rules
/// read with certainty (macro variables, nesting deeper than [`MAX_USE_DEPTH`], anything unexpected).
fn parse_use<'a>(code: &[&Token<'a>], start: usize, paths: &mut Paths<'a>) -> Option<UseTree<'a>> {
    let mut tree = UseTree {
        bindings: Vec::new(),
        glob: false,
        alias: false,
        end: start,
    };
    // Segments of the current path and their ids (`ids[k]` = id after `k` segments); group starts as segment counts.
    let (mut segments, mut ids, mut marks): (Vec<&str>, Vec<u32>, Vec<usize>) =
        (Vec::new(), vec![0], Vec::new());
    let (mut want_segment, mut closed, mut expect_alias) = (true, false, false);
    let mut alias: Option<&str> = None;
    let mut index = start;
    loop {
        let token = code.get(index)?;
        let top = marks.last().copied().unwrap_or(0);
        let mut end_item = |segments: &mut Vec<&'a str>,
                            ids: &mut Vec<u32>,
                            alias: &mut Option<&'a str>| {
            if expect_alias {
                return None;
            }
            if !closed {
                if want_segment {
                    // An empty item (`{}`, a trailing comma) binds nothing; `a::` is incomplete.
                    if segments.len() > top {
                        return None;
                    }
                } else {
                    let mut len = segments.len();
                    if segments.last() == Some(&"self") {
                        len -= 1;
                    }
                    let bound = alias
                        .or_else(|| len.checked_sub(1).and_then(|i| segments.get(i).copied()))?;
                    if bound != "_" {
                        tree.bindings.push((bound, *ids.get(len)?));
                    }
                }
            }
            segments.truncate(top);
            ids.truncate(top + 1);
            *alias = None;
            Some(())
        };
        match token.kind {
            TokenKind::Ident | TokenKind::RawIdent => {
                let word = name(token.text);
                if token.kind == TokenKind::Ident
                    && word == "as"
                    && !want_segment
                    && !closed
                    && !expect_alias
                {
                    expect_alias = true;
                    tree.alias = true;
                } else if expect_alias {
                    alias = Some(word);
                    expect_alias = false;
                } else if want_segment && !closed {
                    let parent = *ids.last()?;
                    ids.push(paths.id(parent, word));
                    segments.push(word);
                    want_segment = false;
                } else {
                    return None;
                }
            }
            TokenKind::Colon => {
                if code.get(index + 1)?.kind != TokenKind::Colon
                    || expect_alias
                    || closed
                    || alias.is_some()
                {
                    return None;
                }
                index += 1;
                if want_segment {
                    // A leading `::` is a segment of its own; elsewhere `::` must follow a segment.
                    if segments.len() > top {
                        return None;
                    }
                    let parent = *ids.last()?;
                    ids.push(paths.id(parent, "::"));
                    segments.push("::");
                } else {
                    want_segment = true;
                }
            }
            TokenKind::Star => {
                if !want_segment || closed {
                    return None;
                }
                tree.glob = true;
                closed = true;
            }
            TokenKind::OpenBrace => {
                if !want_segment || closed || marks.len() >= MAX_USE_DEPTH {
                    return None;
                }
                marks.push(segments.len());
            }
            TokenKind::Comma => {
                end_item(&mut segments, &mut ids, &mut alias)?;
                (want_segment, closed) = (true, false);
            }
            TokenKind::CloseBrace => {
                end_item(&mut segments, &mut ids, &mut alias)?;
                marks.pop()?;
                let parent = marks.last().copied().unwrap_or(0);
                segments.truncate(parent);
                ids.truncate(parent + 1);
                (want_segment, closed) = (false, true);
            }
            TokenKind::Semi => {
                end_item(&mut segments, &mut ids, &mut alias)?;
                if !marks.is_empty() {
                    return None;
                }
                tree.end = index;
                return Some(tree);
            }
            _ => return None,
        }
        index += 1;
    }
}

/// What the `use` declarations and items of a file bind.
#[derive(Default)]
struct Scope<'a> {
    /// Name -> its one path id, `None` for several.
    bound: HashMap<&'a str, Option<u32>>,
    /// Names of items declared in the file, at any nesting.
    items: HashSet<&'a str>,
    /// A glob import or an unreadable `use` tree: no binding is known for certain.
    unsure: bool,
    /// Addresses of the bound-name tokens of every `use` tree (to tell a binding from a use of the name).
    binding_tokens: HashSet<usize>,
}

/// A changed-line candidate: a plain private `use` alone on its line at the top level of the file.
struct Plain<'a> {
    line: u32,
    bindings: Vec<(&'a str, u32)>,
}

/// Adds the bindings and items of `lexed` to `scope`; collects plain `use` lines into `plain` when given.
fn scan<'a>(
    lexed: &Lexed<'a>,
    paths: &mut Paths<'a>,
    scope: &mut Scope<'a>,
    mut plain: Option<&mut Vec<Plain<'a>>>,
) {
    let code: Vec<&Token<'a>> = lexed
        .tokens
        .iter()
        .filter(|t| !is_comment(t.kind))
        .collect();
    for (index, token) in code.iter().enumerate() {
        if token.kind != TokenKind::Ident {
            continue;
        }
        let next = |offset: usize| code.get(index + offset).copied();
        match token.text {
            "use" => {
                let Some(tree) = parse_use(&code, index + 1, paths) else {
                    scope.unsure = true;
                    continue;
                };
                scope.unsure |= tree.glob;
                for &(bound, id) in &tree.bindings {
                    scope.binding_tokens.insert(bound.as_ptr().addr());
                    scope
                        .bound
                        .entry(bound)
                        .and_modify(|known| {
                            if *known != Some(id) {
                                *known = None;
                            }
                        })
                        .or_insert(Some(id));
                }
                let line = token.first;
                let alone = index.checked_sub(1).is_none_or(|before| {
                    matches!(code[before].kind, TokenKind::Semi | TokenKind::CloseBrace)
                        && code[before].last < line
                }) && code[tree.end].last == line
                    && token.last == line
                    && next(tree.end + 1 - index).is_none_or(|after| after.first > line);
                let top_level =
                    token.depth == 0 && !lexed.in_macro.get(line).copied().unwrap_or(true);
                if let Some(plain) = plain.as_deref_mut()
                    && alone
                    && top_level
                    && !tree.glob
                    && !tree.alias
                    && !tree.bindings.is_empty()
                    && let Ok(line) = u32::try_from(line + 1)
                {
                    plain.push(Plain {
                        line,
                        bindings: tree.bindings,
                    });
                }
            }
            "macro_rules" => {
                if let (Some(bang), Some(item)) = (next(1), next(2))
                    && bang.kind == TokenKind::Bang
                    && is_ident(item.kind)
                {
                    scope.items.insert(name(item.text));
                }
            }
            keyword if ITEM_KEYWORDS.contains(&keyword) => {
                if let Some(item) = next(1).filter(|item| is_ident(item.kind)) {
                    scope.items.insert(name(item.text));
                }
            }
            _ => {}
        }
    }
}

/// Changed plain private `use` lines: alone on their line at the top level of the file after `;`, `}` or the start
/// (no visibility, attribute or other token before them), no `as` or `*`, and every bound name bound to this one path
/// on both sides, not declared as an item in the file, not a prelude name and used nowhere on the other side (the
/// side without the import) except as a `use` binding. A glob or an unreadable tree on either side covers nothing.
fn import_lines<'a>(file: &FileSource<'a>, this: &Lexed<'a>, other: &Other<'a>) -> Vec<u32> {
    let mut paths = Paths::default();
    let mut scope = Scope::default();
    let mut plain = Vec::new();
    scan(this, &mut paths, &mut scope, Some(&mut plain));
    if let Other::Lexed(lexed) = other {
        scan(lexed, &mut paths, &mut scope, None);
    }
    if scope.unsure {
        return Vec::new();
    }
    // A name the side without the import already uses resolved to something else there (extern prelude, macro
    // prelude, a derive, a constant in a pattern): the import re-points it. Its own binding there (a reordered
    // import, bound to the same path on both sides) is no use.
    let used_without: HashSet<&str> = match other {
        Other::Lexed(lexed) => lexed
            .tokens
            .iter()
            .filter(|token| is_ident(token.kind))
            .map(|token| name(token.text))
            .filter(|word| !scope.binding_tokens.contains(&word.as_ptr().addr()))
            .collect(),
        Other::Absent => HashSet::new(),
    };
    plain
        .into_iter()
        .filter(|plain| {
            file.changed_lines.binary_search(&plain.line).is_ok()
                && plain.bindings.iter().all(|&(bound, id)| {
                    !scope.items.contains(bound)
                        && !PRELUDE.contains(&bound)
                        && !used_without.contains(bound)
                        && scope.bound.get(bound) == Some(&Some(id))
                })
        })
        .map(|plain| plain.line)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use contracts::common::LineSide;
    use contracts::ids::HunkId;
    use similar::{Algorithm, DiffOp, capture_diff_slices};

    use crate::diff::{Hunk, LineSpan};
    use crate::tech::{FileSource, Generic, OtherSide, TechPlugin, is_line_rule};

    /// -U0 hunks between two line lists, as `git diff` reports them.
    fn hunks(base: &[&str], head: &[&str]) -> Vec<Hunk> {
        let to = |value: usize| u32::try_from(value).expect("small");
        capture_diff_slices(Algorithm::Myers, base, head)
            .into_iter()
            .filter_map(|op| {
                let (old, old_len, new, new_len) = match op {
                    DiffOp::Equal { .. } => return None,
                    DiffOp::Delete {
                        old_index,
                        old_len,
                        new_index,
                    } => (old_index, old_len, new_index, 0),
                    DiffOp::Insert {
                        old_index,
                        new_index,
                        new_len,
                    } => (old_index, 0, new_index, new_len),
                    DiffOp::Replace {
                        old_index,
                        old_len,
                        new_index,
                        new_len,
                    } => (old_index, old_len, new_index, new_len),
                };
                let start =
                    |index: usize, len: usize| if len == 0 { to(index) } else { to(index + 1) };
                let (base, head) = (
                    LineSpan {
                        start: start(old, old_len),
                        len: to(old_len),
                    },
                    LineSpan {
                        start: start(new, new_len),
                        len: to(new_len),
                    },
                );
                Some(Hunk {
                    id: HunkId(format!("h{old}-{new}")),
                    base,
                    head,
                    added: (head.start..head.start + head.len).collect(),
                    removed: (base.start..base.start + base.len).collect(),
                })
            })
            .collect()
    }

    fn text(lines: &[&str]) -> String {
        lines.iter().map(|line| format!("{line}\n")).collect()
    }

    /// Lines of `side` covered by a line rule for the file `base` -> `head` (`None`: the file is absent there);
    /// `unknown` makes the other side unread.
    fn covered_with(
        side: LineSide,
        base: Option<&[&str]>,
        head: Option<&[&str]>,
        unknown: bool,
    ) -> Vec<u32> {
        let hunks = hunks(base.unwrap_or_default(), head.unwrap_or_default());
        let (this, other) = match side {
            LineSide::Head => (head, base),
            LineSide::Base => (base, head),
        };
        let this = text(this.expect("this side"));
        let other_text = other.map(text);
        let mut changed: Vec<u32> = hunks
            .iter()
            .flat_map(|h| match side {
                LineSide::Head => h.added.clone(),
                LineSide::Base => h.removed.clone(),
            })
            .collect();
        changed.sort_unstable();
        let other = match (&other_text, unknown) {
            (_, true) => OtherSide::Unknown,
            (Some(text), false) => OtherSide::Text(text),
            (None, false) => OtherSide::Absent,
        };
        let file = FileSource {
            path: "src/a.rs",
            side,
            text: Some(&this),
            other,
            changed_lines: &changed,
            hunks: &hunks,
        };
        let mut lines: Vec<u32> = Generic
            .classify(&file)
            .expect("classify")
            .into_iter()
            .filter(is_line_rule)
            .flat_map(|item| item.ranges.unwrap_or_default())
            .flat_map(|r| r.start_line..=r.end_line)
            .collect();
        lines.sort_unstable();
        lines.dedup();
        lines
    }

    /// Covered head lines of `base` -> `head`.
    fn head(base: &[&str], head: &[&str]) -> Vec<u32> {
        covered_with(LineSide::Head, Some(base), Some(head), false)
    }

    /// Covered base lines of `base` -> `head`.
    fn base(base: &[&str], head: &[&str]) -> Vec<u32> {
        covered_with(LineSide::Base, Some(base), Some(head), false)
    }

    /// Covered lines of an added file.
    fn added(head: &[&str]) -> Vec<u32> {
        covered_with(LineSide::Head, None, Some(head), false)
    }

    const NONE: [u32; 0] = [];

    #[test]
    fn plain_comments_and_imports_are_covered() {
        let file = [
            "// A note.",
            "use std::fmt;",
            "use crate::a::{X, Y};",
            "use ::b::c;",
            "/// Doc of f.",
            "fn f() {}",
            "/*",
            " * A block.",
            " */",
        ];
        assert_eq!(added(&file), [1, 2, 3, 4, 5, 7, 8, 9]);
        let base_file = ["use std::fmt;", "use std::sync::Mutex;", "fn f() {}"];
        let reordered = ["use std::sync::Mutex;", "use std::fmt;", "fn f() {}"];
        assert_eq!(head(&base_file, &reordered), [1], "a reordered import");
        let grouped = ["use std::fmt;", "use std::sync::{Arc, Mutex};", "fn f() {}"];
        assert_eq!(head(&base_file, &grouped), [2], "same paths");
    }

    #[test]
    fn visibility_attributes_mod_and_extern_crate_are_never_covered() {
        for line in [
            "pub use crate::a::X;",
            "pub(crate) use crate::a::X;",
            "use crate::auth::verify_lenient as verify;",
            "use crate::a::{X as Y};",
            "use crate::prelude::*;",
            "use crate::a::{b::*, c};",
            "mod m;",
            "pub mod m;",
            "extern crate foo;",
            "extern crate foo as bar;",
            "#[cfg(test)] use crate::a::X;",
        ] {
            assert_eq!(added(&[line]), NONE, "{line}");
        }
        // Visibility or an attribute on the previous, unchanged line.
        let base_file = ["pub", "use crate::strict::check;"];
        let head_file = ["pub", "use crate::lenient::verify;"];
        assert_eq!(head(&base_file, &head_file), NONE);
        assert_eq!(base(&base_file, &head_file), NONE);
        for attribute in [
            "#[cfg(feature = \"audit\")]",
            "#[rustfmt::skip]",
            "#[macro_use]",
        ] {
            let base_file = [attribute, "fn audit() {}"];
            let head_file = [attribute, "use crate::sink::sink;", "fn audit() {}"];
            assert_eq!(head(&base_file, &head_file), NONE, "{attribute}");
        }
        // Deleting an import under an attribute moves the attribute onto the next item.
        let base_file = ["#[cfg(test)]", "use crate::helpers;", "fn real() {}"];
        let head_file = ["#[cfg(test)]", "fn real() {}"];
        assert_eq!(base(&base_file, &head_file), NONE);
    }

    #[test]
    fn a_use_inside_a_block_or_macro_is_never_covered() {
        let local = [
            "fn run() -> bool {",
            "    use crate::lenient::verify;",
            "    verify()",
            "}",
        ];
        assert_eq!(added(&local), NONE);
        let module = ["mod inner {", "    use crate::a::X;", "}"];
        assert_eq!(added(&module), NONE);
        let gate = [
            "fn allowed() -> bool {",
            "    gate! {",
            "        use allow;",
            "    }",
            "}",
            "use std::fmt;",
        ];
        assert_eq!(added(&gate), [6]);
        let raw = [
            "fn allowed() -> bool {",
            "    r#gate! {",
            "        use allow;",
            "        // pick",
            "    }",
            "}",
        ];
        assert_eq!(added(&raw), NONE, "a raw-identifier macro name");
        let rules = [
            "macro_rules! r#gate {",
            "    (use deny;) => { false };",
            "    // pick",
            "}",
        ];
        assert_eq!(added(&rules), NONE);
        let body = [
            "macro_rules! imports {",
            "    () => {",
            "        use crate::strict::verify;",
            "    };",
            "}",
        ];
        assert_eq!(added(&body), NONE);
        let call = ["let v = json!({", "    // a note", "    \"a\": 1,", "});"];
        assert_eq!(added(&call), NONE);
        let negation = [
            "fn f(done: bool) {",
            "    while !done {",
            "        // a note",
            "    }",
            "}",
        ];
        assert_eq!(added(&negation), [3], "a negation is no macro");
    }

    #[test]
    fn a_name_bound_elsewhere_makes_a_use_visible() {
        // Another path on the other side.
        let base_file = ["use std::sync::Mutex;", "fn f() {}"];
        assert_eq!(
            head(&base_file, &["use parking_lot::Mutex;", "fn f() {}"]),
            NONE
        );
        // A comment inside a use group does not hide a binding.
        let group = [
            "use std::sync::{",
            "    Arc, // shared; see docs",
            "    Mutex,",
            "};",
        ];
        let split = ["use parking_lot::Mutex;", "use std::sync::Arc;"];
        assert_eq!(head(&group, &split), [2]);
        // A block comment with prose starting with `use` does not swallow the next declaration.
        let side = |checker: &'static str| {
            vec![
                "/* We could".to_owned(),
                "   use a cheaper check here".to_owned(),
                "*/".to_owned(),
                format!("use {checker}::verify;"),
                "fn f() -> bool { verify() }".to_owned(),
            ]
        };
        let (strict, lenient) = (side("strict"), side("lenient"));
        let strict: Vec<&str> = strict.iter().map(String::as_str).collect();
        let lenient: Vec<&str> = lenient.iter().map(String::as_str).collect();
        assert_eq!(head(&strict, &lenient), NONE);
        assert_eq!(base(&strict, &lenient), NONE);
        // A use not at the start of its line, or inside an inline module, still binds.
        let attr = ["#[rustfmt::skip] use strict::verify;", "fn f() {}"];
        assert_eq!(
            head(&attr, &[attr[0], "use lenient::verify;", attr[1]]),
            NONE
        );
        let inline = ["mod checks { use strict::verify; }", "fn f() {}"];
        assert_eq!(
            head(&inline, &[inline[0], "use lenient::verify;", inline[1]]),
            NONE
        );
        // An item of the file, at any nesting.
        let item = [
            "fn verify() -> bool { false }",
            "mod lenient { pub fn verify() -> bool { true } }",
        ];
        assert_eq!(
            head(&item, &[item[0], item[1], "use lenient::verify;"]),
            NONE
        );
        // A prelude name.
        assert_eq!(added(&["use crate::shim::Some;"]), NONE);
        assert_eq!(added(&["use crate::shim::Vec;"]), NONE);
        // Any glob on either side shows every changed use of the file.
        assert_eq!(
            head(
                &["use strict::*;"],
                &["use strict::*;", "use lenient::verify;"]
            ),
            NONE
        );
        assert_eq!(
            head(
                &["mod t { use super::*; }"],
                &["use a::b;", "mod t { use super::*; }"]
            ),
            NONE
        );
        // An unreadable tree on either side.
        assert_eq!(
            head(&["use $crate::x;"], &["use $crate::x;", "use a::b;"]),
            NONE
        );
    }

    #[test]
    fn block_comments_are_covered_only_when_the_whole_change_is_comment() {
        let disabled = [
            "fn transfer() {",
            "    check_authorised()?;",
            "    debit()",
            "}",
        ];
        let wrapped = [
            "fn transfer() {",
            "    /*",
            "    check_authorised()?;",
            "    */",
            "    debit()",
            "}",
        ];
        assert_eq!(head(&disabled, &wrapped), NONE, "code switched off");
        assert_eq!(base(&wrapped, &disabled), NONE, "code switched on");
        let trailing = ["/*", "run();", "*/ // end of disabled code"];
        assert_eq!(head(&["run();"], &trailing), NONE);
        // Closed early on the changed line: `run();` becomes live; the change is code on both sides.
        let open = ["/* note", "run();", "// */"];
        let closed = ["/* note */", "run();", "// */"];
        assert_eq!(head(&open, &closed), NONE);
        assert_eq!(base(&open, &closed), NONE);
        assert_eq!(head(&closed, &open), NONE);
        // A whole added or edited block comment is covered; an edit inside one spanning unchanged lines is not.
        let note = ["/*", " * A note.", " */", "fn f() {}"];
        assert_eq!(head(&["fn f() {}"], &note), [1, 2, 3]);
        assert_eq!(
            head(&note, &["/*", " * Another note.", " */", "fn f() {}"]),
            NONE
        );
        assert_eq!(
            head(&["/* a */", "fn f() {}"], &["/* b */", "fn f() {}"]),
            [1]
        );
        assert_eq!(head(&["// a", "fn f() {}"], &["// b", "fn f() {}"]), [1]);
    }

    #[test]
    fn an_unread_other_side_covers_nothing() {
        let base_file = ["fn f() {}"];
        let head_file = ["use std::fmt;", "/* note */", "// line", "fn f() {}"];
        assert_eq!(head(&base_file, &head_file), [1, 2, 3]);
        assert_eq!(
            covered_with(LineSide::Head, Some(&base_file), Some(&head_file), true),
            NONE
        );
    }

    /// Whitespace-only pairs (base line, head line) of `base` -> `head` in a Rust file.
    fn pairs(base: &[&str], head: &[&str]) -> Vec<(u32, u32)> {
        crate::tech::whitespace_pairs("src/a.rs", &text(base), &text(head), &hunks(base, head))
    }

    #[test]
    fn whitespace_only_lines_pair_exactly() {
        // Wrapping code in `if`: the re-indented body pairs, the wrapper lines stay changed.
        let flat = ["fn f(ok: bool) {", "    a();", "    b(1, 2);", "}"];
        let wrapped = [
            "fn f(ok: bool) {",
            "    if ok {",
            "        a();",
            "        b(1, 2);",
            "    }",
            "}",
        ];
        assert_eq!(pairs(&flat, &wrapped), [(2, 3), (3, 4)]);
        // A swap inside a new `if`: one statement pairs, the other stays changed on both sides.
        let base = [
            "fn w() {",
            "    ensure_authorised()?;",
            "    debit()?;",
            "}",
        ];
        let head = [
            "fn w() {",
            "    if enabled {",
            "        debit()?;",
            "        ensure_authorised()?;",
            "    }",
            "}",
        ];
        assert_eq!(pairs(&base, &head).len(), 1);
        // Trailing whitespace, spacing between tokens, CRLF.
        assert_eq!(
            pairs(&["let x = 1;", "f();"], &["let x = 1;   ", "f();"]),
            [(1, 1)]
        );
        assert_eq!(pairs(&["g(a, b);"], &["g( a,b );"]), [(1, 1)]);
        assert_eq!(
            pairs(
                &["let x = 1;\r", "y(); // note\r"],
                &["let x = 1;", "y(); // note"]
            ),
            [(1, 1), (2, 2)]
        );
        // A literal is compared as written: its inner whitespace is content.
        assert_eq!(pairs(&["f(\"a  b\");"], &["    f(\"a  b\");"]), [(1, 1)]);
        assert_eq!(pairs(&["f(\"a b\");"], &["f(\"a  b\");"]), []);
        // A multi-line string whose indentation changed is a changed value.
        let string = ["let s = \"first", "    second\";"];
        assert_eq!(
            pairs(&string, &["let s = \"first", "        second\";"]),
            []
        );
        // Spacing between punctuation decides the tokens the parser sees (`&&` vs `& &`, `--` vs `- -`).
        assert_eq!(pairs(&["let c = a && b;"], &["let c = a & & b;"]), []);
        assert_eq!(pairs(&["let c = a - -b;"], &["let c = a--b;"]), []);
        assert_eq!(pairs(&["let c = a + b;"], &["let c = a + c;"]), []);
        // Not Rust: no pairs.
        assert_eq!(
            crate::tech::whitespace_pairs(
                "a.py",
                "x = 1\n",
                "x  = 1\n",
                &hunks(&["x = 1"], &["x  = 1"])
            ),
            []
        );
    }

    #[test]
    fn spacing_inside_a_macro_token_tree_is_never_whitespace_only() {
        // `stringify!` (and so `assert!` messages) and procedural macros see the spacing.
        for (base, head) in [
            (
                "    assert!(limit>=amount);",
                "    assert!(limit >= amount);",
            ),
            (
                "    let key = field!(user . name);",
                "    let key = field!(user.name);",
            ),
            (
                "    let page = html!(<a href = \"x\">go</a>);",
                "    let page = html!(<a href=\"x\">go</a>);",
            ),
            (
                "macro_rules! k { ($e:expr) => { stringify!($e . id) }; }",
                "macro_rules! k { ($e:expr) => { stringify!($e.id) }; }",
            ),
        ] {
            assert_eq!(pairs(&[base], &[head]), [], "{head}");
        }
        let body = ["m! {", "    a b", "}"];
        assert_eq!(
            pairs(&body, &["m! {", "        a b", "}"]),
            [],
            "re-indented macro body"
        );
    }

    #[test]
    fn a_use_is_visible_when_its_name_occurs_on_the_side_without_it() {
        // Names in scope without an import: extern prelude, macro prelude, derives, attributes.
        let cases: [(&[&str], &[&str]); 5] = [
            (
                &["fn pid() -> u32 {", "    std::process::id()", "}"],
                &[
                    "use crate::shim::std;",
                    "fn pid() -> u32 {",
                    "    std::process::id()",
                    "}",
                ],
            ),
            (
                &["async fn go() {", "    tokio::spawn(work());", "}"],
                &[
                    "use crate::shim::tokio;",
                    "async fn go() {",
                    "    tokio::spawn(work());",
                    "}",
                ],
            ),
            (
                &["fn f() {", "    info!(\"paid\");", "}"],
                &[
                    "use crate::quiet::info;",
                    "fn f() {",
                    "    info!(\"paid\");",
                    "}",
                ],
            ),
            (
                &["#[derive(Debug)]", "struct Secret(String);"],
                &[
                    "use redact::Debug;",
                    "#[derive(Debug)]",
                    "struct Secret(String);",
                ],
            ),
            (
                &["#[test]", "fn works() {}"],
                &["use skipping::test;", "#[test]", "fn works() {}"],
            ),
        ];
        for (base, head_file) in cases {
            assert_eq!(head(base, head_file), NONE, "{}", head_file[0]);
        }
        // A removed constant import turns the unchanged pattern into a catch-all binding.
        let with = [
            "use limits::ADMIN;",
            "fn is_admin(r: u8) -> bool {",
            "    match r { ADMIN => true, _ => false }",
            "}",
        ];
        assert_eq!(base(&with, &with[1..]), NONE);
        // Control: a name only new code uses stays covered.
        assert_eq!(
            head(
                &["fn a() {}"],
                &["use crate::audit::Sink;", "fn a() {}", "fn b(s: Sink) {}"]
            ),
            [1]
        );
    }

    #[test]
    fn hostile_use_trees_neither_overflow_nor_take_quadratic_time() {
        let deep = format!("use a::{}b{};\n", "{".repeat(10_000), "}".repeat(10_000));
        let found = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || {
                let lines = [deep.trim_end()];
                added(&lines)
            })
            .expect("spawn")
            .join()
            .expect("no stack overflow");
        assert_eq!(found, NONE, "deeper than the rules read: not covered");
        let many: Vec<String> = (0..20_000).map(|i| format!("use m::n{i:06};")).collect();
        let lines: Vec<&str> = many.iter().map(String::as_str).collect();
        let started = Instant::now();
        assert_eq!(added(&lines).len(), 20_000);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }
}
