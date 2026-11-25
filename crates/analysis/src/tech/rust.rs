//! Rust line rules of the generic plugin, read from `rustc_lexer` tokens, never from raw text: comment-only lines,
//! plain imports and re-exports (brief amendments 2026-11-18 and 2026-11-20) and items compiled only for tests.
//! Whatever the rules cannot read with certainty is not covered.

use std::collections::{HashMap, HashSet};

use contracts::api::tour::CallHierarchy;
use contracts::common::{LineRange, LineSide};
use ra_ap_rustc_lexer::{
    DocStyle, FrontmatterAllowed, LiteralKind, TokenKind, strip_shebang, tokenize,
};

use super::{Classified, FileSource, Generic, OtherSide, SymbolAt, TechError, TechPlugin};
use crate::diff::Hunk;

pub(super) mod conversions;

/// The built-in Rust plugin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rust;

impl TechPlugin for Rust {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn detect(&self, path: &str) -> bool {
        path.ends_with(".rs")
    }

    fn classify(&self, file: &FileSource<'_>) -> Result<Vec<Classified>, TechError> {
        Generic.classify(file)
    }

    fn call_hierarchy(&self, _symbol: &SymbolAt) -> Option<CallHierarchy> {
        None
    }

    fn leading_context(&self, file: &FileSource<'_>, first_line: u32) -> Option<LineRange> {
        let item_line = usize::try_from(first_line.checked_sub(1)?).ok()?;
        let text = file.text?;
        let previous = text.lines().nth(item_line.checked_sub(1)?)?.trim_end();
        // Most steps have no context to lex. Tokens still decide what the markers mean.
        if !(previous.ends_with(']')
            || previous.ends_with("*/")
            || previous.contains("///")
            || previous.contains("//!"))
        {
            return None;
        }
        let lexed = lex(text)?;
        let tokens = &lexed.tokens;
        let mut before = tokens.partition_point(|token| token.first < item_line);
        let item = tokens.get(before..)?;
        if item.first()?.first != item_line || !starts_item(item) {
            return None;
        }

        let nearest = item_line.saturating_sub(30);
        let mut first = item_line;
        let mut next = item_line;
        while before > 0 && first > nearest {
            let last = &tokens[before - 1];
            if last.last + 1 < next || last.last >= item_line {
                break;
            }
            let Some(start) = context_start(tokens, before - 1) else {
                break;
            };
            before = start;
            next = tokens[start].first;
            // Skip a boundary line still shared with the preceding token.
            let shared_line = before > 0 && tokens[before - 1].last == next;
            first = (next + usize::from(shared_line)).min(first).max(nearest);
        }
        (first < item_line).then_some(LineRange {
            start_line: u32::try_from(first + 1).ok()?,
            end_line: first_line - 1,
        })
    }
}

/// The first token of a doc comment or outer attribute ending at `end`.
fn context_start(tokens: &[Token<'_>], end: usize) -> Option<usize> {
    let last = &tokens[end];
    match last.kind {
        TokenKind::LineComment { doc_style: Some(_) }
        | TokenKind::BlockComment {
            doc_style: Some(DocStyle::Outer),
            ..
        } => Some(end),
        TokenKind::CloseBracket => {
            let bracket = tokens[..end]
                .iter()
                .rposition(|token| token.depth < last.depth)?;
            let pound = bracket.checked_sub(1)?;
            (tokens[bracket].kind == TokenKind::OpenBracket
                && tokens[pound].kind == TokenKind::Pound)
                .then_some(pound)
        }
        _ => None,
    }
}

/// Recognises an item header at the start of the line, after optional visibility and qualifiers.
fn starts_item(mut tokens: &[Token<'_>]) -> bool {
    if tokens.first().is_some_and(|token| token.text == "pub") {
        tokens = &tokens[1..];
        if let Some(open) = tokens
            .first()
            .filter(|token| token.kind == TokenKind::OpenParen)
        {
            let Some(end) = tokens.iter().position(|token| {
                token.kind == TokenKind::CloseParen && token.depth == open.depth + 1
            }) else {
                return false;
            };
            tokens = &tokens[end + 1..];
        }
    }
    while let [token, next, ..] = tokens {
        if !(matches!(token.text, "async" | "unsafe")
            || token.text == "const" && matches!(next.text, "async" | "unsafe" | "fn"))
        {
            break;
        }
        tokens = &tokens[1..];
    }
    let [keyword, next, ..] = tokens else {
        return false;
    };
    match keyword.text {
        "fn" | "struct" | "enum" | "trait" | "mod" | "const" | "static" | "type" => {
            matches!(next.kind, TokenKind::Ident | TokenKind::RawIdent)
        }
        "impl" => matches!(
            next.kind,
            TokenKind::Ident | TokenKind::RawIdent | TokenKind::Lt | TokenKind::Bang
        ),
        "macro_rules" => next.kind == TokenKind::Bang,
        _ => false,
    }
}

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

/// Keywords of the items the test rule reads.
const TEST_ITEM_KEYWORDS: &[&str] = &[
    "fn",
    "mod",
    "impl",
    "trait",
    "use",
    "struct",
    "enum",
    "union",
    "macro_rules",
];

/// Covered changed lines of one Rust file side, per rule.
pub(super) struct Covered {
    /// Comment-only lines.
    pub comments: Vec<u32>,
    /// Plain `use` declarations and re-exports.
    pub imports: Vec<u32>,
    /// Lines of items compiled only for tests.
    pub tests: Vec<u32>,
}

/// The covered changed lines of `file`, whose text is `text`.
pub(super) fn covered<'a>(file: &FileSource<'a>, text: &'a str) -> Covered {
    let nothing = || Covered {
        comments: Vec::new(),
        imports: Vec::new(),
        tests: Vec::new(),
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
    let tests = TestItems::new(&this);
    Covered {
        comments: comment_lines(file, &this, &other),
        imports: import_lines(file, &this, &tests, &other),
        tests: tests.changed_lines(this.lines, file.changed_lines),
    }
}

/// The code tokens of `lexed` (comments left out), in order.
fn code_tokens<'l, 'a>(lexed: &'l Lexed<'a>) -> Vec<&'l Token<'a>> {
    lexed
        .tokens
        .iter()
        .filter(|t| !is_comment(t.kind))
        .collect()
}

/// For each code token that opens a delimited group, the index of the token closing it.
fn closing(code: &[&Token<'_>]) -> Vec<Option<usize>> {
    let mut out = vec![None; code.len()];
    let mut open = Vec::new();
    for (index, token) in code.iter().enumerate() {
        if is_open(token.kind) {
            open.push(index);
        } else if is_close(token.kind)
            && let Some(start) = open.pop()
        {
            out[start] = Some(index);
        }
    }
    out
}

/// An item compiled only for tests: lines are 0-based.
struct TestItem {
    first: usize,
    /// Line of the first token after the attributes.
    header: usize,
    last: usize,
    /// Lines of its body's `{` and `}`.
    body: Option<(usize, usize)>,
}

/// The test items of one side, with a mask of the code tokens inside them.
pub(super) struct TestItems {
    items: Vec<TestItem>,
    /// Per code token (see [`code_tokens`]): inside a test item, after its attributes.
    inside: Vec<bool>,
}

impl TestItems {
    /// No test items: every token counts.
    pub(super) fn none() -> Self {
        Self {
            items: Vec::new(),
            inside: Vec::new(),
        }
    }

    /// Items whose first outer attribute is `#[cfg(test)]` or `#[test]` / `#[<path>::test(..)]`, starting a line after
    /// `;`, `{`, `}` or the file start and ending a line; an item is `fn`, `mod`, `impl`, `trait`, `use`, `struct`,
    /// `enum`, `union` or `macro_rules` ending at its `;` or its body's `}`. Never inside a macro token tree; a header
    /// with `=`, `,` or an unbalanced close is not read. A path attribute counts only when no `use`, glob or item of
    /// the file can bind its first segment (an imported `test` macro is not the test harness). Linear in the tokens.
    fn new(lexed: &Lexed<'_>) -> Self {
        let mut paths = Paths::default();
        let mut scope = Scope::default();
        scan(lexed, &Self::none(), &mut paths, &mut scope, None);
        let free = |head: &str| {
            !scope.unsure && !scope.bound.contains_key(head) && !scope.items.contains(head)
        };
        let code = code_tokens(lexed);
        let close = closing(&code);
        let mut items = Vec::new();
        let mut marks = vec![0i64; code.len() + 1];
        for index in 0..code.len() {
            if let Some((item, start, end)) = test_item(lexed, &code, &close, &free, index) {
                marks[start] += 1;
                marks[end + 1] -= 1;
                items.push(item);
            }
        }
        let mut depth = 0;
        let inside = marks[..code.len()]
            .iter()
            .map(|mark| {
                depth += mark;
                depth > 0
            })
            .collect();
        Self { items, inside }
    }

    /// The `changed` lines (1-based, sorted) of a side with `lines` lines inside the body of a test item, or inside a
    /// test item whose attribute lines and first item line all changed (so no attribute moves onto an unchanged item;
    /// an unchanged closing line the diff pairs elsewhere does not matter).
    fn changed_lines(&self, lines: usize, changed: &[u32]) -> Vec<u32> {
        let mut is_changed = vec![false; lines];
        for &line in changed {
            if let Some(slot) = usize::try_from(line)
                .ok()
                .and_then(|line| line.checked_sub(1))
                .and_then(|index| is_changed.get_mut(index))
            {
                *slot = true;
            }
        }
        let mut count = vec![0usize; lines + 1];
        for (index, changed) in is_changed.iter().enumerate() {
            count[index + 1] = count[index] + usize::from(*changed);
        }
        let mut marks = vec![0i64; lines + 1];
        let mut cover = |from: usize, to: usize| {
            if from <= to && to < lines {
                marks[from] += 1;
                marks[to + 1] -= 1;
            }
        };
        for item in &self.items {
            if item.header < lines
                && count[item.header + 1] - count[item.first] == item.header - item.first + 1
            {
                cover(item.first, item.last);
            } else if let Some((open, close)) = item.body
                && open + 1 < close
            {
                cover(open + 1, close - 1);
            }
        }
        let mut depth = 0;
        let mut out = Vec::new();
        for (index, mark) in marks[..lines].iter().enumerate() {
            depth += mark;
            if depth > 0
                && is_changed[index]
                && let Ok(line) = u32::try_from(index + 1)
            {
                out.push(line);
            }
        }
        out
    }
}

/// The test item whose attributes start at code token `index`, with the indices of its first token after the
/// attributes and of its last token; `free(name)` tells whether a path attribute may start with `name`.
fn test_item(
    lexed: &Lexed<'_>,
    code: &[&Token<'_>],
    close: &[Option<usize>],
    free: &dyn Fn(&str) -> bool,
    index: usize,
) -> Option<(TestItem, usize, usize)> {
    let token = code[index];
    if token.kind != TokenKind::Pound || lexed.in_macro.get(token.first).copied().unwrap_or(true) {
        return None;
    }
    let starts_line = index.checked_sub(1).is_none_or(|before| {
        matches!(
            code[before].kind,
            TokenKind::Semi | TokenKind::OpenBrace | TokenKind::CloseBrace
        ) && code[before].last < token.first
    });
    if !starts_line {
        return None;
    }
    let (mut at, mut test) = (index, false);
    while code.get(at)?.kind == TokenKind::Pound {
        let open = at + 1;
        if code.get(open)?.kind != TokenKind::OpenBracket {
            return None;
        }
        let end = (*close.get(open)?)?;
        // Only the first attribute: one before it may be a macro that sees the item first.
        test |= at == index && is_test_attribute(code, close, free, open + 1, end);
        at = end + 1;
    }
    if !test {
        return None;
    }
    let start = at;
    let mut keyword = false;
    let (end, body) = loop {
        let token = code.get(at)?;
        match token.kind {
            TokenKind::Semi => break (at, None),
            TokenKind::OpenBrace => {
                let end = (*close.get(at)?)?;
                break (end, Some((token.first, code[end].last)));
            }
            TokenKind::OpenParen | TokenKind::OpenBracket => at = (*close.get(at)?)?,
            TokenKind::CloseBrace
            | TokenKind::CloseParen
            | TokenKind::CloseBracket
            | TokenKind::Comma
            | TokenKind::Eq => return None,
            TokenKind::Ident => keyword |= TEST_ITEM_KEYWORDS.contains(&token.text),
            _ => {}
        }
        at += 1;
    };
    let ends_line = code
        .get(end + 1)
        .is_none_or(|after| after.first > code[end].last);
    (keyword && ends_line).then(|| {
        let item = TestItem {
            first: token.first,
            header: code[start].first,
            last: code[end].last,
            body,
        };
        (item, start, end)
    })
}

/// Whether the attribute tokens `code[from..to]` are `cfg(test)`, or a path ending in `test` whose first segment is
/// `free`, with or without arguments in parentheses.
fn is_test_attribute(
    code: &[&Token<'_>],
    close: &[Option<usize>],
    free: &dyn Fn(&str) -> bool,
    from: usize,
    to: usize,
) -> bool {
    let kind = |at: usize| code[at].kind;
    if to.checked_sub(from) == Some(4)
        && kind(from) == TokenKind::Ident
        && kind(from + 1) == TokenKind::OpenParen
        && kind(from + 2) == TokenKind::Ident
        && kind(from + 3) == TokenKind::CloseParen
        && code[from].text == "cfg"
        && code[from + 2].text == "test"
    {
        return true;
    }
    // A path: identifiers joined by `::`, then optional arguments closing at `to - 1`.
    let mut at = from;
    let mut last = None;
    while at < to {
        match code[at].kind {
            TokenKind::Ident if last.is_none() || code[at - 1].kind == TokenKind::Colon => {
                last = Some(code[at].text);
                at += 1;
            }
            TokenKind::Colon
                if last.is_some()
                    && code.get(at + 1).is_some_and(|t| t.kind == TokenKind::Colon)
                    && code.get(at + 2).is_some_and(|t| t.kind == TokenKind::Ident) =>
            {
                at += 2;
            }
            TokenKind::OpenParen
                if last.is_some() && close.get(at).copied().flatten() == Some(to - 1) =>
            {
                at = to;
            }
            _ => return false,
        }
    }
    last == Some("test") && free(code[from].text)
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
fn parse_use<'a>(
    code: &[&Token<'a>],
    start: usize,
    paths: &mut Paths<'a>,
    reached: &mut usize,
) -> Option<UseTree<'a>> {
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
        *reached = index;
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
                } else if want_segment
                    && !closed
                    && !(token.kind == TokenKind::Ident && word == "use")
                {
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

/// A changed-line candidate: a plain `use` declaration alone on its lines (1-based, `first..=last`) at the top level
/// of the file.
struct Plain<'a> {
    first: u32,
    last: u32,
    /// Has a visibility: a re-export.
    public: bool,
    bindings: Vec<(&'a str, u32)>,
}

/// Index of the first token of the visibility before the `use` at code token `index` (`pub`, `pub(crate)`,
/// `pub(super)`, `pub(self)`, `pub(in path)`), or `index` without one.
fn visibility_start(code: &[&Token<'_>], index: usize) -> usize {
    let Some(before) = index.checked_sub(1) else {
        return index;
    };
    let is_pub = |at: usize| code[at].kind == TokenKind::Ident && code[at].text == "pub";
    if is_pub(before) {
        return before;
    }
    if code[before].kind != TokenKind::CloseParen {
        return index;
    }
    let mut at = before;
    while let Some(previous) = at.checked_sub(1) {
        at = previous;
        match code[at].kind {
            TokenKind::OpenParen => {
                return match at.checked_sub(1) {
                    Some(start) if is_pub(start) => start,
                    _ => index,
                };
            }
            TokenKind::Ident | TokenKind::Colon => {}
            _ => return index,
        }
    }
    index
}

/// Adds the bindings and items of `lexed` outside its test items to `scope`; collects plain `use` declarations into
/// `plain` when given.
fn scan<'a>(
    lexed: &Lexed<'a>,
    tests: &TestItems,
    paths: &mut Paths<'a>,
    scope: &mut Scope<'a>,
    mut plain: Option<&mut Vec<Plain<'a>>>,
) {
    let code = code_tokens(lexed);
    // Tokens before this index were read by an earlier `use` tree: a `use` among them is not a declaration.
    let mut consumed = 0;
    for (index, token) in code.iter().enumerate() {
        // A test item's imports and items are seen only by test code.
        if token.kind != TokenKind::Ident || tests.inside.get(index).copied().unwrap_or(false) {
            continue;
        }
        let next = |offset: usize| code.get(index + offset).copied();
        match token.text {
            "use" if index >= consumed => {
                let mut reached = index;
                let parsed = parse_use(&code, index + 1, paths, &mut reached);
                consumed = reached;
                let Some(tree) = parsed else {
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
                let start = visibility_start(&code, index);
                let last = code[tree.end].last;
                let alone = start.checked_sub(1).is_none_or(|before| {
                    matches!(code[before].kind, TokenKind::Semi | TokenKind::CloseBrace)
                        && code[before].last < line
                }) && code[start].first == line
                    && next(tree.end + 1 - index).is_none_or(|after| after.first > last);
                let top_level =
                    token.depth == 0 && !lexed.in_macro.get(line).copied().unwrap_or(true);
                if let Some(plain) = plain.as_deref_mut()
                    && alone
                    && top_level
                    && !tree.glob
                    && !tree.alias
                    && !tree.bindings.is_empty()
                    && let (Ok(first), Ok(last)) =
                        (u32::try_from(line + 1), u32::try_from(last + 1))
                {
                    plain.push(Plain {
                        first,
                        last,
                        public: start != index,
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

/// Changed lines of plain `use` declarations: alone on their lines at the top level of the file after `;`, `}` or the
/// start (nothing but a visibility before them on their line, no attribute), no `as` or `*`, and every bound name bound
/// to this one path on both sides, not declared as an item in the file, not a prelude name, and either bound by a plain
/// `use` of the same path on the other side (a moved import) or used nowhere there except as a `use` binding while a
/// private import's own side names it (an import named nowhere may be a trait imported for its methods). Test
/// items are left out on both sides (their names reach only test code). A glob or an unreadable tree outside them on
/// either side covers nothing.
fn import_lines<'a>(
    file: &FileSource<'a>,
    this: &Lexed<'a>,
    tests: &TestItems,
    other: &Other<'a>,
) -> Vec<u32> {
    let mut paths = Paths::default();
    let mut scope = Scope::default();
    let (mut plain, mut other_plain) = (Vec::new(), Vec::new());
    scan(this, tests, &mut paths, &mut scope, Some(&mut plain));
    let other_tests = match other {
        Other::Lexed(lexed) => {
            let other_tests = TestItems::new(lexed);
            scan(
                lexed,
                &other_tests,
                &mut paths,
                &mut scope,
                Some(&mut other_plain),
            );
            Some(other_tests)
        }
        Other::Absent => None,
    };
    // Bound by a plain import of the same path on the other side too: a moved import re-points nothing.
    let moved: HashSet<(&str, u32)> = other_plain
        .iter()
        .flat_map(|plain| plain.bindings.iter().copied())
        .collect();
    if scope.unsure {
        return Vec::new();
    }
    // A name the side without the import already uses resolved to something else there (extern prelude, macro
    // prelude, a derive, a constant in a pattern): the import re-points it. Its own binding there (a reordered
    // import, bound to the same path on both sides) is no use.
    // Names a side uses outside `use` bindings and test items.
    let used = |lexed: &Lexed<'a>, tests: &TestItems| -> HashSet<&'a str> {
        code_tokens(lexed)
            .into_iter()
            .zip(&tests.inside)
            .filter(|(token, inside)| is_ident(token.kind) && !**inside)
            .map(|(token, _)| name(token.text))
            .filter(|word| !scope.binding_tokens.contains(&word.as_ptr().addr()))
            .collect()
    };
    let used_without: HashSet<&str> = match (other, &other_tests) {
        (Other::Lexed(lexed), Some(other_tests)) => used(lexed, other_tests),
        _ => HashSet::new(),
    };
    let used_here = used(this, tests);
    let mut out: Vec<u32> = plain
        .into_iter()
        .filter(|plain| {
            plain.bindings.iter().all(|&(bound, id)| {
                // A private import its own side never names (a trait for its methods) can re-point an unchanged call.
                let named = plain.public || used_here.contains(bound);
                !scope.items.contains(bound)
                    && !PRELUDE.contains(&bound)
                    && (moved.contains(&(bound, id)) || (named && !used_without.contains(bound)))
                    && scope.bound.get(bound) == Some(&Some(id))
            })
        })
        .flat_map(|plain| {
            let from = file
                .changed_lines
                .partition_point(|&line| line < plain.first);
            let to = file
                .changed_lines
                .partition_point(|&line| line <= plain.last);
            file.changed_lines[from..to].to_vec()
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use contracts::common::LineSide;
    use contracts::ids::HunkId;
    use similar::{Algorithm, DiffOp, capture_diff_slices};

    use crate::diff::{Hunk, LineSpan};
    use crate::tech::{
        Classified, FileSource, Generic, IMPORT_RULE, OtherSide, TEST_ITEM_RULE, TechPlugin,
        is_line_rule,
    };

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
        lines_where(side, base, head, unknown, &is_line_rule)
    }

    /// Lines of `side` of `base` -> `head` that items accepted by `keep` cover.
    fn lines_where(
        side: LineSide,
        base: Option<&[&str]>,
        head: Option<&[&str]>,
        unknown: bool,
        keep: &dyn Fn(&Classified) -> bool,
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
            .filter(|item| keep(item))
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
            "fn f(_: fmt::Error, _: X, _: Y) { c() }",
            "/*",
            " * A block.",
            " */",
        ];
        assert_eq!(added(&file), [1, 2, 3, 4, 5, 7, 8, 9]);
        let base_file = ["use std::fmt;", "use std::sync::Mutex;", "fn f() {}"];
        let reordered = ["use std::sync::Mutex;", "use std::fmt;", "fn f() {}"];
        assert_eq!(head(&base_file, &reordered), [1], "a reordered import");
        let grouped = ["use std::fmt;", "use std::sync::{Arc, Mutex};", "fn f() {}"];
        assert_eq!(
            head(&base_file, &grouped),
            NONE,
            "a new name the file never names may be a trait for its methods"
        );
        // The same with the names in use: a moved import binds the same path on both sides, it re-points nothing.
        let used = [
            "use std::fmt;",
            "use std::sync::Mutex;",
            "fn f(m: Mutex<fmt::Error>) {}",
        ];
        let reordered = ["use std::sync::Mutex;", "use std::fmt;", used[2]];
        assert_eq!(
            head(&used, &reordered),
            [1],
            "a reordered import of a used name"
        );
        let grouped = [
            "use std::fmt;",
            "use std::sync::{Arc, Mutex};",
            "fn f(m: Mutex<fmt::Error>, a: Arc<u8>) {}",
        ];
        assert_eq!(
            head(&used, &grouped),
            [2],
            "a regrouped import of a used name"
        );
    }

    #[test]
    fn aliases_globs_attributes_mod_and_extern_crate_are_never_covered() {
        for line in [
            "pub use crate::a::X as Y;",
            "pub use crate::a::*;",
            "use crate::auth::verify_lenient as verify;",
            "use crate::a::{X as Y};",
            "use crate::prelude::*;",
            "use crate::a::{b::*, c};",
            "mod m;",
            "pub mod m;",
            "extern crate foo;",
            "extern crate foo as bar;",
            "#[cfg(feature = \"x\")] use crate::a::X;",
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
            "fn g(_: fmt::Error) {}",
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
        let head_file = [
            "use std::fmt;",
            "/* note */",
            "// line",
            "fn f() {}",
            "fn g(_: fmt::Error) {}",
        ];
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

    /// Head lines of `base` -> `head` covered by `rule`.
    fn head_by(rule: &str, base: &[&str], head: &[&str]) -> Vec<u32> {
        lines_where(LineSide::Head, Some(base), Some(head), false, &|item| {
            item.rule == rule
        })
    }

    /// Base lines of `base` -> `head` covered by `rule`.
    fn base_by(rule: &str, base: &[&str], head: &[&str]) -> Vec<u32> {
        lines_where(LineSide::Base, Some(base), Some(head), false, &|item| {
            item.rule == rule
        })
    }

    /// Lines of an added file covered by `rule`.
    fn added_by(rule: &str, head: &[&str]) -> Vec<u32> {
        lines_where(LineSide::Head, None, Some(head), false, &|item| {
            item.rule == rule
        })
    }

    #[test]
    fn re_exports_are_covered_like_plain_imports() {
        for line in [
            "pub use crate::a::X;",
            "pub(crate) use crate::a::X;",
            "pub(super) use crate::a::X;",
            "pub(in crate::outer) use crate::a::X;",
            "pub use crate::a::{X, Y};",
        ] {
            assert_eq!(added_by(IMPORT_RULE, &[line]), [1], "{line}");
        }
        // The owner's case: a removed public re-export of a type the file does not use.
        let base_file = ["pub use crate::reason::RejectionReason;", "pub fn f() {}"];
        assert_eq!(base_by(IMPORT_RULE, &base_file, &base_file[1..]), [1]);
        // A private import turned into a re-export binds the same path on both sides.
        let private = ["use crate::a::X;", "fn f(x: X) {}"];
        let public = ["pub use crate::a::X;", "fn f(x: X) {}"];
        assert_eq!(head_by(IMPORT_RULE, &private, &public), [1]);
        assert_eq!(base_by(IMPORT_RULE, &private, &public), [1]);
        // Still visible: a re-export that re-points a name the other side uses.
        let other = [
            "pub use crate::lenient::verify;",
            "fn f() -> bool { verify() }",
        ];
        let strict = ["pub use crate::strict::verify;", other[1]];
        assert_eq!(head_by(IMPORT_RULE, &other, &strict), NONE);
        // A visibility that is not `pub` before `use` on the same line.
        assert_eq!(added_by(IMPORT_RULE, &["crate(x) use a::b;"]), NONE);
    }

    #[test]
    fn multi_line_use_declarations_are_covered_whole() {
        let group = [
            "use std::sync::{",
            "    Arc,",
            "    Mutex,",
            "};",
            "fn f(a: Arc<Mutex<u8>>) {}",
        ];
        assert_eq!(added_by(IMPORT_RULE, &group), [1, 2, 3, 4]);
        let base_file = ["use std::sync::{", "    Arc,", "};", "fn f(a: Arc<u8>) {}"];
        let head_file = [
            "use std::sync::{",
            "    Arc,",
            "    Mutex,",
            "};",
            "fn f(a: Arc<u8>, m: Mutex<u8>) {}",
        ];
        assert_eq!(head_by(IMPORT_RULE, &base_file, &head_file), [3]);
        assert_eq!(base_by(IMPORT_RULE, &head_file, &base_file), [3]);
        let public = ["pub use crate::a::{", "    X,", "    Y,", "};"];
        assert_eq!(added_by(IMPORT_RULE, &public), [1, 2, 3, 4]);
        // Not covered: a glob or alias in the tree, code after the `;`, a name the other side uses.
        for tree in [
            ["use a::{", "    b::*,", "    c,", "};"],
            ["use a::{", "    b as d,", "    c,", "};"],
            ["use a::{", "    b,", "    c,", "}; fn f() {}"],
        ] {
            assert_eq!(added_by(IMPORT_RULE, &tree), NONE, "{tree:?}");
        }
        let uses = ["fn f(m: Mutex) {}"];
        let imports = ["use std::sync::{", "    Mutex,", "};", "fn f(m: Mutex) {}"];
        assert_eq!(head_by(IMPORT_RULE, &uses, &imports), NONE);
    }

    #[test]
    fn inline_test_items_are_covered_by_the_test_rule() {
        let production = ["fn f() -> u8 {", "    1", "}"];
        let with_tests = [
            "fn f() -> u8 {",
            "    1",
            "}",
            "",
            "#[cfg(test)]",
            "mod tests {",
            "    use super::*;",
            "",
            "    #[test]",
            "    fn one() {",
            "        assert_eq!(f(), 1);",
            "    }",
            "}",
        ];
        // A whole new test module, attribute included.
        assert_eq!(
            head_by(TEST_ITEM_RULE, &production, &with_tests),
            [5, 6, 7, 8, 9, 10, 11, 12, 13]
        );
        assert_eq!(
            base_by(TEST_ITEM_RULE, &with_tests, &production),
            [5, 6, 7, 8, 9, 10, 11, 12, 13]
        );
        // Lines inside the body of an unchanged test module.
        let mut edited = with_tests;
        edited[10] = "        assert_eq!(f(), 2);";
        assert_eq!(head_by(TEST_ITEM_RULE, &with_tests, &edited), [11]);
        assert_eq!(base_by(TEST_ITEM_RULE, &with_tests, &edited), [11]);
        // `#[test]` and `#[<path>::test(..)]` functions outside a test module, and a `#[cfg(test)] use`.
        let tokio = [
            "#[tokio::test(flavor = \"multi_thread\")]",
            "async fn two() {",
            "    run().await;",
            "}",
        ];
        assert_eq!(added_by(TEST_ITEM_RULE, &tokio), [1, 2, 3, 4]);
        assert_eq!(
            added_by(TEST_ITEM_RULE, &["#[cfg(test)] use crate::a::X;"]),
            [1]
        );
        let attributes = ["#[cfg(test)]", "#[expect(dead_code)]", "fn helper() {}"];
        assert_eq!(added_by(TEST_ITEM_RULE, &attributes), [1, 2, 3]);
    }

    #[test]
    fn test_rule_never_covers_what_can_reach_production() {
        let rule = TEST_ITEM_RULE;
        // Other conditions.
        for attribute in [
            "#[cfg(not(test))]",
            "#[cfg(all(test, unix))]",
            "#[cfg_attr(test, x)]",
        ] {
            assert_eq!(
                added_by(rule, &[attribute, "fn f() {}"]),
                NONE,
                "{attribute}"
            );
        }
        // An attribute added to or removed from an unchanged item.
        let plain = ["fn real() {}"];
        let gated = ["#[cfg(test)]", "fn real() {}"];
        assert_eq!(head_by(rule, &plain, &gated), NONE);
        assert_eq!(base_by(rule, &gated, &plain), NONE);
        // Deleting the item under the attribute moves the attribute onto the next item.
        let base_file = ["#[cfg(test)]", "use crate::helpers;", "fn real() {}"];
        assert_eq!(base_by(rule, &base_file, &gated), NONE);
        // The item's opening and closing lines of an unchanged module.
        let module = ["#[cfg(test)]", "mod tests {", "    fn a() {}", "}"];
        let renamed = ["#[cfg(test)]", "mod checks {", "    fn a() {}", "}"];
        assert_eq!(head_by(rule, &module, &renamed), NONE);
        // Production code sharing a line with the item.
        assert_eq!(
            added_by(rule, &["#[cfg(test)]", "mod tests {}  fn prod() {}"]),
            NONE
        );
        assert_eq!(added_by(rule, &["fn prod() {} #[test] fn t() {}"]), NONE);
        // A field, an expression or a constant initialiser is no item the rule reads.
        let field = ["struct S {", "    #[cfg(test)]", "    probe: u8,", "}"];
        assert_eq!(added_by(rule, &field), NONE);
        assert_eq!(
            added_by(
                rule,
                &["#[cfg(test)]", "const LIMIT: Limits = Limits { max: 1 };"]
            ),
            NONE
        );
        // Inside a macro token tree.
        let call = ["items! {", "    #[test]", "    fn t() {}", "}"];
        assert_eq!(added_by(rule, &call), NONE);
        // An imported, declared or glob-imported attribute macro is not the test harness.
        for (binding, attribute) in [
            ("use skipping::test;", "#[test]"),
            ("mod tokio {}", "#[tokio::test]"),
            ("use helpers::*;", "#[test]"),
        ] {
            let file = [binding, attribute, "fn t() {}"];
            assert_eq!(added_by(rule, &file), NONE, "{binding}");
        }
        // An attribute before the test attribute may be a macro that runs first.
        let wrapped = ["#[wrap]", "#[cfg(test)]", "fn t() {}"];
        assert_eq!(added_by(rule, &wrapped), NONE);
    }

    #[test]
    fn imports_stay_covered_beside_a_test_module_glob() {
        let tests = ["#[cfg(test)]", "mod tests {", "    use super::*;", "}"];
        let base_file = ["fn a() {}", tests[0], tests[1], tests[2], tests[3]];
        let head_file = [
            "use crate::audit::Sink;",
            "fn a() {}",
            "fn b(s: Sink) {}",
            tests[0],
            tests[1],
            tests[2],
            tests[3],
        ];
        assert_eq!(head_by(IMPORT_RULE, &base_file, &head_file), [1]);
        // A glob of a module that is not a test module still shows every changed use.
        let plain = ["mod t { use super::*; }"];
        assert_eq!(head_by(IMPORT_RULE, &plain, &["use a::b;", plain[0]]), NONE);
    }

    #[test]
    fn use_keywords_inside_a_use_tree_are_read_once() {
        let levels = 20_000;
        for hostile in [
            format!("use a{};\n", "::use".repeat(levels)),
            format!(
                "use a::{}x{};\n",
                "{use, ".repeat(levels),
                "}".repeat(levels)
            ),
        ] {
            let lines: Vec<&str> = hostile.lines().collect();
            let started = Instant::now();
            assert_eq!(added(&lines), NONE);
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "{:?}",
                started.elapsed()
            );
        }
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
        let nested = format!(
            "{}{}",
            "#[cfg(test)]\nmod t {\n".repeat(10_000),
            "}\n".repeat(10_000)
        );
        let lines: Vec<&str> = nested.lines().collect();
        let started = Instant::now();
        assert_eq!(added_by(TEST_ITEM_RULE, &lines).len(), 30_000);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        let many: Vec<String> = (0..20_000)
            .map(|i| format!("pub use m::n{i:06};"))
            .collect();
        let lines: Vec<&str> = many.iter().map(String::as_str).collect();
        let started = Instant::now();
        assert_eq!(added(&lines).len(), 20_000);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    /// Attributes nested inside a first attribute, each starting a line after `;`: every start reads its own
    /// attribute once, so the test rule must stay linear in the tokens (a 500 KB file, well under the blob limit).
    #[test]
    fn hostile_nested_attributes_do_not_take_quadratic_time() {
        let levels = 100_000;
        let nested = format!("{}{}", "#[\n;\n".repeat(levels), "]\n".repeat(levels));
        let lines: Vec<&str> = nested.lines().collect();
        let started = Instant::now();
        assert_eq!(added_by(TEST_ITEM_RULE, &lines), NONE);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    /// A trait import whose name the file never uses changes which method a call resolves to (a trait method on
    /// the receiver beats an inherent method of its `Deref` target); its hunk is no longer listed to the agent, so
    /// the rule must leave it visible.
    #[test]
    fn an_import_used_by_no_name_on_its_side_is_not_cut() {
        let base = [
            "use crate::wrap::Wrapper;",
            "fn show(w: &Wrapper) -> &str {",
            "    w.name()",
            "}",
        ];
        let head = ["use crate::ext::Named;", base[0], base[1], base[2], base[3]];
        assert_eq!(head_by(IMPORT_RULE, &base, &head), NONE);
    }
}
